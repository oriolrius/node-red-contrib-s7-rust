//! One PLC connection behind a protocol-neutral interface, and the classic
//! S7comm implementation (crate `s7`).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use serde_json::{json, Value as J};

use s7::{parse_block_type, szl_name, Endpoint, EndpointConfig, State, Val, Zone};

use crate::{LogFn, OpenParams, OpenResult, StatusFn};

/// One tag of a read: value or error, plus the raw bytes (change detection).
#[derive(Debug, Clone, Default)]
pub struct Out {
    pub val: Option<Val>,
    pub err: Option<String>,
    pub raw: Vec<u8>,
}

impl Out {
    pub fn bad(e: impl Into<String>) -> Out {
        Out { val: None, err: Some(e.into()), raw: Vec::new() }
    }
}

/// A PLC connection. All methods run on the connection thread.
pub trait Device: Send + Sync {
    /// Reads all tags; per-tag errors in `Out`. The second value is the
    /// transport error, if any (then every tag is bad).
    fn read(&self, tags: &[String]) -> (Vec<Out>, Option<String>);
    /// Writes; per-item errors (None = ok). Err = transport failure.
    fn write(&self, items: &[(String, Val)]) -> Result<Vec<Option<String>>, String>;
    /// Parse / resolve error of a tag, None if it is valid.
    fn check_tag(&self, tag: &str) -> Option<String>;
    /// Connects when down (at once when `now`, else every `every`).
    fn tick(&self, every: Duration, now: bool);
    /// Diagnostics: "status" and the protocol's actions.
    fn call(&self, action: &str, args: &J) -> Result<J, String>;
    fn shutdown(&self);
}

/// Builds the device of an `open` call.
pub fn open(p: OpenParams, status: StatusFn, log: Option<LogFn>) -> Result<(Arc<dyn Device>, OpenResult), String> {
    match p.protocol.as_deref().unwrap_or("s7") {
        "" | "s7" => S7Dev::open(p, status, log),
        "s7plus" => crate::plus::PlusDev::open(p, status, log),
        x => Err(format!("unknown protocol {x:?} (s7 or s7plus)")),
    }
}

pub fn level_of(s: Option<&str>) -> s7::Level {
    match s.unwrap_or("warn") {
        "error" => s7::Level::Error,
        "info" => s7::Level::Info,
        "debug" => s7::Level::Debug,
        "trace" => s7::Level::Trace,
        _ => s7::Level::Warn,
    }
}

pub fn emit_log(log: &Option<LogFn>, conn: &str, level: &str, msg: String) {
    if let Some(f) = log {
        f.call((conn.to_string(), level.to_string(), msg), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

// ---------------------------------------------------------------- classic S7

pub struct S7Dev {
    ep: Arc<Endpoint>,
    last_try: Mutex<Option<Instant>>,
}

impl S7Dev {
    fn open(p: OpenParams, status: StatusFn, log: Option<LogFn>) -> Result<(Arc<dyn Device>, OpenResult), String> {
        let cfg = EndpointConfig {
            host: p.host.clone(),
            port: p.port.unwrap_or(102) as u16,
            cpu: p.cpu.clone().unwrap_or_default(),
            rack: p.rack.map(|x| x as u8),
            slot: p.slot.map(|x| x as u8),
            link: p.link.map(|x| x as u8),
            local_tsap: p.local_tsap.clone().unwrap_or_default(),
            remote_tsap: p.remote_tsap.clone().unwrap_or_default(),
            pdu: p.pdu.unwrap_or(0) as u16,
            timeout: Duration::from_millis(p.timeout_ms.unwrap_or(0) as u64),
            attempts: p.attempts.unwrap_or(0) as usize,
            gap: p.gap.map(|g| g as i64),
            little_endian: p.little_endian.unwrap_or(false),
            local_addr: p.local_addr.clone().unwrap_or_default(),
        };
        let lg = match &log {
            Some(f) => {
                let (f, id) = (f.clone(), p.conn.clone());
                s7::Log::from_fn(level_of(p.log_level.as_deref()), move |lvl, msg, kv| {
                    let mut m = msg.to_string();
                    for (k, v) in kv {
                        m.push_str(&format!(" {k}={v}"));
                    }
                    f.call((id.clone(), lvl.as_str().to_string(), m), ThreadsafeFunctionCallMode::NonBlocking);
                })
            }
            None => s7::Log::discard(),
        };
        let (ep, info) = Endpoint::open(&p.conn, &cfg, &lg).map_err(|e| e.to_string())?;
        ep.set_on_status(Some(Arc::new(move |id: &str, st: State, det: &str| {
            status.call((id.to_string(), st.as_str().to_string(), det.to_string()), ThreadsafeFunctionCallMode::NonBlocking);
        })));
        let res = OpenResult {
            protocol: "s7".into(),
            addr: info.addr,
            rack: Some(info.rack as u32),
            slot: Some(info.slot as u32),
            link: Some(info.link as u32),
        };
        Ok((Arc::new(S7Dev { ep: Arc::new(ep), last_try: Mutex::new(None) }), res))
    }

    fn run<T>(&self, f: impl FnMut(&mut s7::Conn) -> s7::Result<T>) -> Result<T, String> {
        let r = self.ep.client.run(f);
        self.ep.publish_state();
        r.map_err(|e| e.to_string())
    }
}

fn arg_u64(a: &J, k: &str) -> Option<u64> {
    match a.get(k)? {
        J::Number(n) => n.as_u64(),
        J::String(s) => {
            let s = s.trim();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(h) => u64::from_str_radix(h, 16).ok(),
                None => s.parse().ok(),
            }
        }
        _ => None,
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Device for S7Dev {
    fn read(&self, tags: &[String]) -> (Vec<Out>, Option<String>) {
        let (outs, err) = self.ep.read_with(
            tags,
            |_| Out::default(),
            |o, r| match r {
                Ok((v, raw)) => {
                    o.val = Some(v);
                    o.raw = raw;
                }
                Err(e) => o.err = Some(e),
            },
        );
        (outs, err.map(|e| e.to_string()))
    }

    fn write(&self, items: &[(String, Val)]) -> Result<Vec<Option<String>>, String> {
        self.ep.write_values(items).map_err(|e| e.to_string())
    }

    fn check_tag(&self, tag: &str) -> Option<String> {
        self.ep.tag(tag).err().map(|e| e.to_string())
    }

    fn tick(&self, every: Duration, now: bool) {
        let (st, _) = self.ep.client.status();
        if st == State::Connected {
            return;
        }
        let mut last = self.last_try.lock().unwrap();
        if now || last.is_none_or(|t| t.elapsed() >= every) {
            *last = Some(Instant::now());
            drop(last);
            let _ = self.ep.client.run(|_| Ok(()));
            self.ep.publish_state();
        }
    }

    /// Actions: status, identify, cpuState, szl {id, index}, szlList, clock,
    /// setClock {time?}, syncClock {thresholdMs}, blocks, listBlocks {type},
    /// blockInfo {type, number}. STOP/START and block upload are not exposed.
    fn call(&self, action: &str, a: &J) -> Result<J, String> {
        Ok(match action {
            "status" => {
                let s = self.ep.status();
                json!({
                    "conn": s.conn, "protocol": "s7", "addr": s.addr, "state": s.state.as_str(), "detail": s.detail,
                    "pdu": s.pdu, "reconnects": s.reconnects, "requests": s.requests, "errors": s.errors,
                    "p50ms": s.p50_ms, "p95ms": s.p95_ms, "p99ms": s.p99_ms, "rate1m": s.rate_1m,
                })
            }
            "identify" => {
                let order = self.run(|cn| cn.module_identification())?;
                let comps = self.run(|cn| cn.component_info()).unwrap_or_default();
                let (state, code) = self.run(|cn| cn.cpu_state())?;
                json!({
                    "modules": order.iter().map(|o| json!({"index": o.index, "orderNo": o.order_no, "version": o.version})).collect::<Vec<_>>(),
                    "components": comps.iter().map(|(k, v)| json!({"index": k, "name": s7::component_name(*k), "value": v})).collect::<Vec<_>>(),
                    "cpuState": state, "cpuStateCode": code,
                })
            }
            "cpuState" => {
                let (state, code) = self.run(|cn| cn.cpu_state())?;
                json!({"state": state, "code": code})
            }
            "szl" => {
                let id = arg_u64(a, "id").ok_or("szl: id is required")? as u16;
                let index = arg_u64(a, "index").unwrap_or(0) as u16;
                let (len, data) = self.run(|cn| cn.read_szl(id, index))?;
                json!({"id": id, "index": index, "name": szl_name(id), "recordLength": len, "data": hex(&data)})
            }
            "szlList" => {
                let ids = self.run(|cn| cn.szl_ids())?;
                J::Array(ids.iter().map(|i| json!({"id": format!("0x{i:04X}"), "name": szl_name(*i)})).collect())
            }
            "clock" => {
                let t = self.run(|cn| cn.read_clock(Zone::Local))?;
                json!({"time": t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)})
            }
            "setClock" => {
                let t = match a.get("time").and_then(|v| v.as_str()) {
                    Some(s) => chrono::DateTime::parse_from_rfc3339(s).map_err(|e| format!("time: {e}"))?,
                    None => Zone::Utc.now(),
                };
                self.run(|cn| cn.set_clock(&t, Zone::Local))?;
                json!({"time": t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)})
            }
            "syncClock" => {
                let th = chrono::TimeDelta::milliseconds(arg_u64(a, "thresholdMs").unwrap_or(0) as i64);
                let (off, synced) = self.run(|cn| cn.sync_clock(&Zone::Utc.now(), th, Zone::Local))?;
                json!({"offsetMs": off.num_milliseconds(), "synced": synced})
            }
            "blocks" => {
                let c = self.run(|cn| cn.block_counts())?;
                J::Array(c.iter().map(|b| json!({"type": b.typ.name().unwrap_or("?"), "count": b.count})).collect())
            }
            "listBlocks" => {
                let t = parse_block_type(a.get("type").and_then(|v| v.as_str()).unwrap_or("DB")).map_err(|e| e.to_string())?;
                let l = self.run(|cn| cn.list_blocks(t))?;
                J::Array(
                    l.iter()
                        .map(|b| json!({"number": b.number, "flags": b.flags, "language": s7::language_name(b.language)}))
                        .collect(),
                )
            }
            "blockInfo" => {
                let t = parse_block_type(a.get("type").and_then(|v| v.as_str()).unwrap_or("DB")).map_err(|e| e.to_string())?;
                let n = arg_u64(a, "number").ok_or("blockInfo: number is required")? as u16;
                let b = self.run(|cn| cn.get_block_info(t, n))?;
                json!({
                    "type": b.typ.name(), "number": b.number, "language": s7::language_name(b.language),
                    "flags": b.flags, "loadSize": b.load_size, "localData": b.local_data, "mc7Size": b.mc7_size,
                    "author": b.author, "family": b.family, "name": b.name, "version": b.version_string(),
                    "checksum": b.checksum, "codeDate": b.code_date.to_rfc3339(), "interfaceDate": b.interface_date.to_rfc3339(),
                })
            }
            x => return Err(format!("unknown action {x:?} for s7 (status, identify, cpuState, szl, szlList, clock, setClock, syncClock, blocks, listBlocks, blockInfo)")),
        })
    }

    fn shutdown(&self) {
        self.ep.close();
    }
}

/// UTC time as RFC 3339 with nanoseconds.
pub fn rfc3339_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    s7plus::DateTime::from_unix_nanos(d.as_nanos() as i64).rfc3339_nano()
}
