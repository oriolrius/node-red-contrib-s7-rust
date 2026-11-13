//! S7 partner device (protocol "s7partner"): BSEND / BRCV over an S7
//! connection of the PLC (crate `s7`, module `partner`), and USEND / URCV
//! on the same connection. The `s7 brcv` and `s7 urcv` nodes poll what the
//! PLC sent like the alarm subscriptions (one queue per node, kind "bsend" or
//! "usend"); `s7 bsend` / `s7 usend` call the actions "bsend" / "usend".

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use serde_json::{json, Value as J};

use s7::partner::{Area, Message, Partner, MAX_AREAS, MAX_BLOCK};
use s7::wire::DialOptions;

use crate::device::{emit_log, AlarmPush, Device, Out};
use crate::{LogFn, OpenParams, OpenResult, StatusFn};

pub struct PartnerDev {
    id: String,
    opts: DialOptions,
    partner: Mutex<Option<Partner>>,
    state: Mutex<(&'static str, String)>,
    status: StatusFn,
    log: Option<LogFn>,
    last_try: Mutex<Option<Instant>>,
    reconnects: Mutex<u64>,
    /// Received messages (JSON) per `s7 brcv` / `s7 urcv` node (key of its
    /// poll loop), with the kind the node takes.
    queues: Mutex<HashMap<String, (&'static str, VecDeque<J>)>>,
    /// Messages received while no node of their kind listens (yet): the first
    /// node of that kind gets them (at most BACKLOG, the oldest are dropped).
    backlog: Mutex<HashMap<&'static str, VecDeque<J>>>,
    /// Received and dropped BSEND blocks, received and dropped USENDs.
    blocks: Mutex<(u64, u64, u64, u64)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("data must be hex with an even length".into());
    }
    let digit = |c: u8| -> Result<u8, String> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err("data is not hex".to_string()),
        }
    };
    s.as_bytes().chunks(2).map(|p| Ok(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

impl PartnerDev {
    pub fn open(p: OpenParams, status: StatusFn, log: Option<LogFn>) -> Result<(std::sync::Arc<dyn Device>, OpenResult), String> {
        let port = p.port.unwrap_or(102);
        let addr =
            if p.host.contains(':') && !p.host.starts_with('[') { format!("[{}]:{port}", p.host) } else { format!("{}:{port}", p.host) };
        let tsap = |s: &Option<String>, what: &str| -> Result<u16, String> {
            let s = s.as_deref().unwrap_or("").trim();
            if s.is_empty() {
                return Err(format!("S7 partner: {what} TSAP is required (from the S7 connection in TIA Portal, for example 10.01)"));
            }
            s7::parse_tsap(s).map_err(|e| format!("S7 partner: {what} TSAP {s:?}: {e}"))
        };
        let opts = DialOptions {
            addr: addr.clone(),
            local_tsap: tsap(&p.local_tsap, "local")?,
            remote_tsap: tsap(&p.remote_tsap, "remote")?,
            max_pdu: p.pdu.unwrap_or(0) as u16,
            timeout: Duration::from_millis(p.timeout_ms.filter(|x| *x > 0).unwrap_or(5000) as u64),
            local_addr: p.local_addr.clone().unwrap_or_default(),
            ..Default::default()
        };
        let dev = PartnerDev {
            id: p.conn.clone(),
            opts,
            partner: Mutex::new(None),
            state: Mutex::new(("connecting", String::new())),
            status,
            log,
            last_try: Mutex::new(None),
            reconnects: Mutex::new(0),
            queues: Mutex::new(HashMap::new()),
            backlog: Mutex::new(HashMap::new()),
            blocks: Mutex::new((0, 0, 0, 0)),
        };
        let res = OpenResult { protocol: "s7partner".into(), addr, rack: None, slot: None, link: None };
        Ok((std::sync::Arc::new(dev), res))
    }

    fn set_state(&self, st: &'static str, detail: String) {
        let mut s = self.state.lock().unwrap();
        if s.0 != st || s.1 != detail {
            *s = (st, detail.clone());
            drop(s);
            self.status.call((self.id.clone(), st.to_string(), detail), ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    /// The connection, connecting when needed.
    fn with<T>(&self, f: impl FnOnce(&mut Partner) -> s7::Result<T>) -> Result<T, String> {
        let mut g = self.partner.lock().unwrap();
        if g.is_none() {
            self.set_state("connecting", String::new());
            match Partner::connect(&self.opts) {
                Ok(p) => {
                    let pdu = p.max_pdu();
                    *g = Some(p);
                    self.set_state("connected", format!("S7 partner, PDU {pdu}"));
                }
                Err(e) => {
                    self.set_state("faulted", e.to_string());
                    return Err(e.to_string());
                }
            }
        }
        let p = g.as_mut().expect("partner");
        match f(p) {
            Ok(v) => Ok(v),
            Err(e) => {
                if e.retryable() {
                    p.close();
                    *g = None;
                    *self.reconnects.lock().unwrap() += 1;
                    emit_log(&self.log, &self.id, "warn", format!("connection lost: {e}"));
                    self.set_state("faulted", e.to_string());
                }
                Err(e.to_string())
            }
        }
    }

    fn deliver(&self, m: Message) {
        let (kind, j) = match &m {
            Message::Block(b) => ("bsend", json!({"kind": "bsend", "rid": b.r_id, "data": hex(&b.data)})),
            Message::Usend(u) => (
                "usend",
                json!({"kind": "usend", "rid": u.r_id,
                    "areas": u.areas.iter().map(|a| json!({"type": a.tsize, "data": hex(&a.data)})).collect::<Vec<_>>()}),
            ),
        };
        let mut q = self.queues.lock().unwrap();
        let mut c = self.blocks.lock().unwrap();
        if kind == "bsend" {
            c.0 += 1;
        } else {
            c.2 += 1;
        }
        let mut any = false;
        for (k, v) in q.values_mut() {
            if *k == kind {
                any = true;
                if v.len() < 1000 {
                    v.push_back(j.clone());
                }
            }
        }
        if !any {
            let mut bl = self.backlog.lock().unwrap();
            let bl = bl.entry(kind).or_default();
            if bl.len() >= BACKLOG {
                bl.pop_front();
                if kind == "bsend" {
                    c.1 += 1;
                } else {
                    c.3 += 1;
                }
                let node = if kind == "bsend" { "s7 brcv" } else { "s7 urcv" };
                emit_log(&self.log, &self.id, "warn", format!("{kind} dropped: no {node} node for {BACKLOG} messages"));
            }
            bl.push_back(j);
        }
    }
}

const BACKLOG: usize = 100;

impl Device for PartnerDev {
    fn read(&self, tags: &[String]) -> (Vec<Out>, Option<String>) {
        let e = "an S7 partner endpoint has no tags: use s7 brcv / s7 bsend".to_string();
        (tags.iter().map(|_| Out::bad(e.clone())).collect(), None)
    }

    fn write(&self, _items: &[(String, Val)]) -> Result<Vec<Option<String>>, String> {
        Err("an S7 partner endpoint has no tags: use s7 bsend".into())
    }

    fn check_tag(&self, _tag: &str) -> Option<String> {
        Some("an S7 partner endpoint has no tags: use s7 brcv / s7 bsend".into())
    }

    fn tick(&self, every: Duration, now: bool) {
        if self.partner.lock().unwrap().is_some() {
            return;
        }
        let mut last = self.last_try.lock().unwrap();
        if now || last.is_none_or(|t| t.elapsed() >= every) {
            *last = Some(Instant::now());
            drop(last);
            let _ = self.with(|_| Ok(()));
        }
    }

    /// Actions: status; bsend {rid, data (hex)}: sends a block to the PLC BRCV
    /// with this R_ID; usend {rid, areas: [{data (hex), type?}]}: sends a USEND
    /// to the PLC URCV with this R_ID (type: transport size, default 4 =
    /// bytes). Both write to the PLC.
    fn call(&self, action: &str, a: &J) -> Result<J, String> {
        Ok(match action {
            "status" => {
                let (st, det) = self.state.lock().unwrap().clone();
                let (n, dropped, un, udropped) = *self.blocks.lock().unwrap();
                let q = self.queues.lock().unwrap();
                let count = |kind: &str| q.values().filter(|(k, _)| *k == kind).count();
                let receivers = json!({"bsend": count("bsend"), "usend": count("usend")});
                drop(q);
                json!({
                    "conn": self.id, "protocol": "s7partner", "addr": self.opts.addr, "state": st, "detail": det,
                    "reconnects": *self.reconnects.lock().unwrap(), "blocksReceived": n, "blocksDropped": dropped,
                    "usendsReceived": un, "usendsDropped": udropped, "receivers": receivers,
                })
            }
            "bsend" => {
                let rid = a
                    .get("rid")
                    .and_then(|v| v.as_u64())
                    .filter(|r| *r <= u32::MAX as u64)
                    .ok_or("bsend: rid (0..4294967295) is required")?;
                let data = unhex(a.get("data").and_then(|v| v.as_str()).unwrap_or("")).map_err(|e| format!("bsend: {e}"))?;
                if data.is_empty() || data.len() > MAX_BLOCK {
                    return Err(format!("bsend: {} bytes; a block has 1..{MAX_BLOCK} bytes", data.len()));
                }
                let n = data.len();
                self.with(|p| p.send(rid as u32, &data))?;
                json!({"rid": rid, "bytes": n})
            }
            "usend" => {
                let rid = a
                    .get("rid")
                    .and_then(|v| v.as_u64())
                    .filter(|r| *r <= u32::MAX as u64)
                    .ok_or("usend: rid (0..4294967295) is required")?;
                let list = a.get("areas").and_then(|v| v.as_array()).ok_or("usend: areas (1 to 4) are required")?;
                if list.is_empty() || list.len() > MAX_AREAS {
                    return Err(format!("usend: {} areas; a USEND has 1..{MAX_AREAS}", list.len()));
                }
                let mut areas = Vec::with_capacity(list.len());
                for (i, x) in list.iter().enumerate() {
                    let data =
                        unhex(x.get("data").and_then(|v| v.as_str()).unwrap_or("")).map_err(|e| format!("usend: area {}: {e}", i + 1))?;
                    let tsize = match x.get("type") {
                        None | Some(J::Null) => 0x04,
                        Some(t) => {
                            t.as_u64().filter(|t| *t <= 0xff).ok_or(format!("usend: area {}: type is a transport size (0..255)", i + 1))?
                                as u8
                        }
                    };
                    if data.is_empty() {
                        return Err(format!("usend: area {} is empty", i + 1));
                    }
                    areas.push(Area { tsize, data });
                }
                let bytes: Vec<usize> = areas.iter().map(|a| a.data.len()).collect();
                self.with(|p| p.usend(rid as u32, &areas))?;
                json!({"rid": rid, "bytes": bytes})
            }
            x => return Err(format!("unknown action {x:?} for s7partner (status, bsend, usend)")),
        })
    }

    fn shutdown(&self) {
        if let Some(p) = self.partner.lock().unwrap().take() {
            p.close();
        }
    }

    /// What the PLC sent for the node `key`: BSEND blocks for an `s7 brcv`
    /// node (cfg.kind "bsend", the default), USENDs for an `s7 urcv` node
    /// (cfg.kind "usend").
    fn alarm_poll(&self, key: &str, cfg: &J, wait: Duration) -> Result<AlarmPush, String> {
        let kind: &'static str = if cfg.get("kind").and_then(|v| v.as_str()) == Some("usend") { "usend" } else { "bsend" };
        {
            let mut q = self.queues.lock().unwrap();
            if !q.contains_key(key) {
                // the first node of a kind gets what came before it
                let first = !q.values().any(|(k, _)| *k == kind);
                let mut v = VecDeque::new();
                if first {
                    if let Some(bl) = self.backlog.lock().unwrap().get_mut(kind) {
                        v.extend(bl.drain(..));
                    }
                }
                q.insert(key.to_string(), (kind, v));
            }
        }
        let pending =
            |me: &Self| -> Vec<J> { me.queues.lock().unwrap().get_mut(key).map(|(_, q)| q.drain(..).collect()).unwrap_or_default() };
        let out = pending(self);
        if !out.is_empty() {
            return Ok(AlarmPush::Alarms(out, false));
        }
        if self.partner.lock().unwrap().is_none() {
            return Ok(AlarmPush::Down);
        }
        match self.with(|p| p.recv(wait)) {
            Ok(Some(m)) => self.deliver(m),
            Ok(None) => {}
            Err(_) => return Ok(AlarmPush::Down),
        }
        let out = pending(self);
        Ok(if out.is_empty() { AlarmPush::Idle } else { AlarmPush::Alarms(out, false) })
    }

    fn alarm_close(&self, key: &str) {
        self.queues.lock().unwrap().remove(key);
    }
}

use s7::Val;

#[cfg(test)]
mod tests {
    use super::unhex;

    #[test]
    fn unhex_rejects_non_hex_without_panic() {
        assert_eq!(unhex("00ff10").unwrap(), vec![0, 255, 16]);
        assert_eq!(unhex("ABcd").unwrap(), vec![0xab, 0xcd]);
        for bad in ["0", "zz", "é0", "0é", "€€", "0x00"] {
            assert!(unhex(bad).is_err(), "{bad:?}");
        }
    }
}
