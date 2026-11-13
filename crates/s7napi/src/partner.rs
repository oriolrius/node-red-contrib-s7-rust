//! S7 partner device (protocol "s7partner"): BSEND / BRCV over an S7
//! connection of the PLC (crate `s7`, module `partner`). The `s7 brcv`
//! nodes poll the received blocks like the alarm subscriptions (one queue per
//! node); `s7 bsend` calls the action "bsend".

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use serde_json::{json, Value as J};

use s7::partner::{Block, Partner, MAX_BLOCK};
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
    /// Received blocks per `s7 brcv` node (key of its poll loop).
    queues: Mutex<HashMap<String, VecDeque<Block>>>,
    /// Blocks received while no node listens (yet): the first node gets them
    /// (at most BACKLOG, the oldest are dropped).
    backlog: Mutex<VecDeque<Block>>,
    /// Received and dropped blocks.
    blocks: Mutex<(u64, u64)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("bsend: data must be hex with an even length".into());
    }
    let digit = |c: u8| -> Result<u8, String> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err("bsend: data is not hex".to_string()),
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
            backlog: Mutex::new(VecDeque::new()),
            blocks: Mutex::new((0, 0)),
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

    fn deliver(&self, b: Block) {
        let mut q = self.queues.lock().unwrap();
        let mut c = self.blocks.lock().unwrap();
        c.0 += 1;
        if q.is_empty() {
            let mut bl = self.backlog.lock().unwrap();
            if bl.len() >= BACKLOG {
                bl.pop_front();
                c.1 += 1;
                emit_log(&self.log, &self.id, "warn", format!("BSEND block dropped: no s7 brcv node for {BACKLOG} blocks"));
            }
            bl.push_back(b);
            return;
        }
        for v in q.values_mut() {
            if v.len() < 1000 {
                v.push_back(b.clone());
            }
        }
    }
}

const BACKLOG: usize = 100;

fn block_json(b: &Block) -> J {
    json!({"rid": b.r_id, "data": hex(&b.data)})
}

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
    /// with this R_ID (a write to the PLC).
    fn call(&self, action: &str, a: &J) -> Result<J, String> {
        Ok(match action {
            "status" => {
                let (st, det) = self.state.lock().unwrap().clone();
                let (n, dropped) = *self.blocks.lock().unwrap();
                json!({
                    "conn": self.id, "protocol": "s7partner", "addr": self.opts.addr, "state": st, "detail": det,
                    "reconnects": *self.reconnects.lock().unwrap(), "blocksReceived": n, "blocksDropped": dropped,
                })
            }
            "bsend" => {
                let rid = a
                    .get("rid")
                    .and_then(|v| v.as_u64())
                    .filter(|r| *r <= u32::MAX as u64)
                    .ok_or("bsend: rid (0..4294967295) is required")?;
                let data = unhex(a.get("data").and_then(|v| v.as_str()).unwrap_or(""))?;
                if data.is_empty() || data.len() > MAX_BLOCK {
                    return Err(format!("bsend: {} bytes; a block has 1..{MAX_BLOCK} bytes", data.len()));
                }
                let n = data.len();
                self.with(|p| p.send(rid as u32, &data))?;
                json!({"rid": rid, "bytes": n})
            }
            x => return Err(format!("unknown action {x:?} for s7partner (status, bsend)")),
        })
    }

    fn shutdown(&self) {
        if let Some(p) = self.partner.lock().unwrap().take() {
            p.close();
        }
    }

    /// The blocks the PLC sent (BSEND) for the `s7 brcv` node `key`.
    fn alarm_poll(&self, key: &str, _cfg: &J, wait: Duration) -> Result<AlarmPush, String> {
        {
            let mut q = self.queues.lock().unwrap();
            if !q.contains_key(key) {
                // the first node gets the blocks that came before it
                let first = q.is_empty();
                let v = q.entry(key.to_string()).or_default();
                if first {
                    v.extend(self.backlog.lock().unwrap().drain(..));
                }
            }
        }
        let pending = |me: &Self| -> Vec<J> {
            me.queues.lock().unwrap().get_mut(key).map(|q| q.drain(..).map(|b| block_json(&b)).collect()).unwrap_or_default()
        };
        let out = pending(self);
        if !out.is_empty() {
            return Ok(AlarmPush::Alarms(out, false));
        }
        if self.partner.lock().unwrap().is_none() {
            return Ok(AlarmPush::Down);
        }
        match self.with(|p| p.recv(wait)) {
            Ok(Some(b)) => self.deliver(b),
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
