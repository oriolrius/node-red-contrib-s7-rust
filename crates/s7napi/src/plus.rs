//! S7CommPlus device (crate `s7plus`): S7-1200 / S7-1500.
//!
//! Tags:
//! - classic absolute syntax (`MW2:int`, `DB1.DBD4:real`, `M10.1`) → byte-area
//!   access (LID [3, offset, length]), decoded with the classic types;
//! - symbols (`"DB_Opt".mat[1,2]`, `Test_Int`, or any text with the `sym:`
//!   prefix) → resolved once per session to LIDs and read with their PLC type.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use serde_json::{json, Value as J};

use s7::{Stats, Val};
use s7plus::{AreaItem, Client, Error as PErr, ItemAddress, Options, PlcValue, SubOptions, Subscription, TlsOptions, Value as PValue};

use crate::device::{emit_log, Device, Out, Push};
use crate::{LogFn, OpenParams, OpenResult, StatusFn};

#[derive(Clone)]
enum PTag {
    Abs(s7::Tag, AreaItem),
    Sym(s7plus::Tag),
}

struct Cfg {
    addr: String,
    tsap: String,
    timeout: Duration,
    tls: bool,
    password: String,
    username: String,
    attempts: usize,
    level: s7plus::Level,
}

pub struct PlusDev {
    id: String,
    cfg: Cfg,
    client: Mutex<Option<Arc<Client>>>,
    state: Mutex<(&'static str, String)>,
    tags: Mutex<HashMap<String, PTag>>,
    /// Symbols the PLC does not know (per session: not asked again on each read).
    missing: Mutex<HashMap<String, String>>,
    status: StatusFn,
    log: Option<LogFn>,
    last_try: Mutex<Option<Instant>>,
    reconnects: AtomicU64,
    stats: Stats,
    /// PLC subscriptions of the `s7 watch` nodes in push mode, by node id.
    push: Mutex<HashMap<String, PushSub>>,
}

/// A PLC subscription. It belongs to one session: after a reconnect it is
/// created again.
struct PushSub {
    client: Arc<Client>,
    sub: Subscription,
    /// Reference id -> (index in the tag list, tag).
    refs: HashMap<u32, (usize, PTag)>,
    /// Tags that do not resolve: sent with the first poll.
    pending: Vec<(usize, Out)>,
}

fn transport(e: &PErr) -> bool {
    matches!(e, PErr::Io(_) | PErr::Timeout | PErr::Short | PErr::Tls(_))
}

fn plus_level(l: s7::Level) -> s7plus::Level {
    match l {
        s7::Level::Error => s7plus::Level::Error,
        s7::Level::Warn => s7plus::Level::Warn,
        s7::Level::Info => s7plus::Level::Info,
        s7::Level::Debug => s7plus::Level::Debug,
        s7::Level::Trace => s7plus::Level::Trace,
    }
}

const SAFE_INT: i64 = 1 << 53;

/// PLC value → the JSON-friendly form of the classic crate: ints beyond ±2^53
/// as strings, REAL with its shortest float32 text, date-times as RFC 3339
/// nano strings, durations in milliseconds.
pub fn plc_to_val(v: &PlcValue) -> Val {
    match v {
        PlcValue::Null => Val::Null,
        PlcValue::Bool(b) => Val::Bool(*b),
        PlcValue::Int(i) => {
            if *i > SAFE_INT || *i < -SAFE_INT {
                Val::Str(i.to_string())
            } else {
                Val::Int(*i)
            }
        }
        PlcValue::UInt(u) => {
            if *u > SAFE_INT as u64 {
                Val::Str(u.to_string())
            } else {
                Val::Int(*u as i64)
            }
        }
        PlcValue::Real(f) => {
            if f.is_finite() {
                Val::Float(format!("{f}").parse().unwrap_or(*f as f64))
            } else {
                Val::Str(f.to_string())
            }
        }
        PlcValue::LReal(f) => {
            if f.is_finite() {
                Val::Float(*f)
            } else {
                Val::Str(f.to_string())
            }
        }
        PlcValue::String(s) => Val::Str(s.clone()),
        PlcValue::Bytes(b) => Val::List(b.iter().map(|x| Val::Int(*x as i64)).collect()),
        PlcValue::DateTime(t) => Val::Str(t.rfc3339_nano()),
        PlcValue::Duration(d) => {
            if d % 1_000_000 == 0 {
                Val::Int(d / 1_000_000)
            } else {
                Val::Float(*d as f64 / 1e6)
            }
        }
        PlcValue::Array(a) => Val::List(a.iter().map(plc_to_val).collect()),
        PlcValue::Raw(x) => Val::Str(x.to_string()),
    }
}

impl PlusDev {
    pub fn open(p: OpenParams, status: StatusFn, log: Option<LogFn>) -> Result<(Arc<dyn Device>, OpenResult), String> {
        let port = p.port.unwrap_or(102);
        let addr =
            if p.host.contains(':') && !p.host.starts_with('[') { format!("[{}]:{port}", p.host) } else { format!("{}:{port}", p.host) };
        let cfg = Cfg {
            addr: addr.clone(),
            tsap: p.tsap.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "SIMATIC-ROOT-HMI".into()),
            timeout: Duration::from_millis(p.timeout_ms.filter(|x| *x > 0).unwrap_or(5000) as u64),
            tls: p.tls.unwrap_or(false),
            password: p.password.clone().unwrap_or_default(),
            username: p.username.clone().unwrap_or_default(),
            attempts: p.attempts.filter(|x| *x > 0).unwrap_or(2) as usize,
            level: plus_level(crate::device::level_of(p.log_level.as_deref())),
        };
        let dev = PlusDev {
            id: p.conn.clone(),
            cfg,
            client: Mutex::new(None),
            state: Mutex::new(("connecting", String::new())),
            tags: Mutex::new(HashMap::new()),
            missing: Mutex::new(HashMap::new()),
            status,
            log,
            last_try: Mutex::new(None),
            reconnects: AtomicU64::new(0),
            stats: Stats::new(),
            push: Mutex::new(HashMap::new()),
        };
        Ok((Arc::new(dev), OpenResult { protocol: "s7plus".into(), addr, rack: None, slot: None, link: None }))
    }

    fn set_state(&self, st: &'static str, detail: String) {
        let mut s = self.state.lock().unwrap();
        if s.0 != st || s.1 != detail {
            *s = (st, detail.clone());
            drop(s);
            self.status.call((self.id.clone(), st.to_string(), detail), ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    fn options(&self) -> Options {
        let mut o = Options::new(self.cfg.addr.clone());
        o.tsap = self.cfg.tsap.clone();
        o.timeout = self.cfg.timeout;
        o.password = self.cfg.password.clone();
        o.username = self.cfg.username.clone();
        if self.cfg.tls {
            o.tls = Some(TlsOptions { config: None, key_log: None, server_name: String::new() });
        }
        if let Some(f) = &self.log {
            let (f, id, max) = (f.clone(), self.id.clone(), self.cfg.level);
            let lg = s7plus::LevelFilter {
                max,
                inner: move |lvl: s7plus::Level, msg: &str| {
                    let l = format!("{lvl:?}").to_lowercase();
                    f.call((id.clone(), l, msg.to_string()), ThreadsafeFunctionCallMode::NonBlocking);
                },
            };
            o.logger = Some(Arc::new(lg));
        }
        o
    }

    /// The live session, connecting when needed.
    fn client(&self) -> Result<Arc<Client>, PErr> {
        if let Some(c) = self.client.lock().unwrap().as_ref() {
            return Ok(c.clone());
        }
        self.set_state("connecting", String::new());
        match Client::connect(self.options()) {
            Ok(c) => {
                let i = c.info();
                let detail = format!("S7CommPlus V{}{} {}", i.protocol_ver, if i.tls { " TLS" } else { "" }, i.paom().trim());
                let c = Arc::new(c);
                *self.client.lock().unwrap() = Some(c.clone());
                self.tags.lock().unwrap().clear(); // LIDs may change after a download
                self.missing.lock().unwrap().clear();
                self.set_state("connected", detail);
                Ok(c)
            }
            Err(e) => {
                self.set_state("faulted", e.to_string());
                Err(e)
            }
        }
    }

    fn drop_client(&self, e: &PErr) {
        if let Some(c) = self.client.lock().unwrap().take() {
            let _ = c.close();
        }
        self.reconnects.fetch_add(1, Ordering::Relaxed);
        emit_log(&self.log, &self.id, "warn", format!("connection lost: {e}"));
        self.set_state("faulted", e.to_string());
    }

    /// Runs f with a session; reconnects and retries on transport errors.
    fn run<T>(&self, mut f: impl FnMut(&Client) -> Result<T, PErr>) -> Result<T, PErr> {
        let mut last = PErr::Timeout;
        for i in 0..self.cfg.attempts.max(1) {
            let c = match self.client() {
                Ok(c) => c,
                Err(e) => {
                    last = e;
                    if i + 1 < self.cfg.attempts {
                        std::thread::sleep(Duration::from_millis(200 * (i as u64 + 1)));
                    }
                    continue;
                }
            };
            let t0 = Instant::now();
            let r = f(&c);
            self.stats.observe(t0.elapsed(), r.is_err());
            match r {
                Ok(v) => return Ok(v),
                Err(e) if !transport(&e) => return Err(e),
                Err(e) => {
                    self.drop_client(&e);
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Resolves a tag (cached per session). Err(String) = bad tag; transport
    /// errors while resolving a symbol come back as Err too (the tag is bad).
    fn resolve(&self, name: &str) -> Result<PTag, String> {
        if let Some(t) = self.tags.lock().unwrap().get(name) {
            return Ok(t.clone());
        }
        if let Some(e) = self.missing.lock().unwrap().get(name) {
            return Err(e.clone());
        }
        let sym = name.strip_prefix("sym:");
        let t = match sym {
            None if !name.starts_with('"') => match s7::parse_tag(name) {
                Ok(t) => {
                    let it = t.item;
                    if it.is_timer_counter() {
                        return Err(format!("{name}: S5 timers/counters are not reachable over S7CommPlus"));
                    }
                    Some(PTag::Abs(
                        t,
                        AreaItem { area: it.area, db: it.db, start: it.start, bit: it.bit, count: it.count, is_bit: it.is_bit },
                    ))
                }
                Err(_) => None,
            },
            _ => None,
        };
        let t = match t {
            Some(t) => t,
            None => {
                let s = sym.unwrap_or(name);
                match self.run(|c| c.resolve_tag(s)) {
                    Ok(t) => PTag::Sym(t),
                    Err(e) => {
                        if e.is_not_found() {
                            self.missing.lock().unwrap().insert(name.to_string(), e.to_string());
                        }
                        return Err(e.to_string());
                    }
                }
            }
        };
        self.tags.lock().unwrap().insert(name.to_string(), t.clone());
        Ok(t)
    }
}

/// The address of a resolved tag.
fn tag_address(name: &str, pt: &PTag) -> Result<ItemAddress, String> {
    match pt {
        PTag::Abs(_, it) => s7plus::area_address(it).map_err(|e| format!("{name}: {e}")),
        PTag::Sym(s) => Ok(s.addr.clone()),
    }
}

/// A PLC value (read or notification) -> Out. A DTL tag can learn its
/// interface timestamp: the second value is then the updated tag.
fn decode_out(name: &str, pt: &PTag, v: &PValue) -> (Out, Option<PTag>) {
    match pt {
        PTag::Abs(tag, it) => match v.v.as_bytes() {
            None => (Out::bad(format!("{name}: no data (out of range?)")), None),
            Some(b) => {
                let data = if it.is_bit { vec![(b.first().copied().unwrap_or(0) >> (it.bit & 7)) & 1] } else { b.to_vec() };
                match tag.json_value(&data) {
                    Ok(v) => (Out { val: Some(v), err: None, raw: data }, None),
                    Err(e) => (Out::bad(e.to_string()), None),
                }
            }
        },
        PTag::Sym(st) => match s7plus::decode_plc_ts(st, v) {
            Ok((v, ts)) => {
                // DTL tags learn their interface timestamp (needed to write them)
                let learned = match ts {
                    Some(ts) if ts != st.packed_ts => {
                        let mut st = st.clone();
                        st.packed_ts = ts;
                        Some(PTag::Sym(st))
                    }
                    _ => None,
                };
                let raw = v.to_json().into_bytes();
                (Out { val: Some(plc_to_val(&v)), err: None, raw }, learned)
            }
            Err(e) => (Out::bad(format!("{name}: {e}")), None),
        },
    }
}

impl PlusDev {
    /// Creates the PLC subscription of `sub` on the current session.
    fn push_create(&self, c: &Arc<Client>, tags: &[String], cycle_ms: u32) -> Result<PushSub, PErr> {
        let mut addrs = Vec::new();
        let mut refs = HashMap::new();
        let mut pending = Vec::new();
        for (i, t) in tags.iter().enumerate() {
            match self.resolve(t).and_then(|pt| tag_address(t, &pt).map(|a| (pt, a))) {
                Ok((pt, a)) => {
                    addrs.push(a);
                    refs.insert(addrs.len() as u32, (i, pt)); // reference ids are 1..n in address order
                }
                Err(e) => pending.push((i, Out::bad(e))),
            }
        }
        if addrs.is_empty() {
            return Err(PErr::Proto("no tag to subscribe".into()));
        }
        let sub = c.subscribe(&addrs, SubOptions { cycle_ms, name: "node-red-s7rs".into(), ..Default::default() })?;
        Ok(PushSub { client: c.clone(), sub, refs, pending })
    }
}

impl Device for PlusDev {
    fn read(&self, tags: &[String]) -> (Vec<Out>, Option<String>) {
        let mut out: Vec<Out> = tags.iter().map(|_| Out::default()).collect();
        // absolute and symbolic tags go in the same GetMultiVariables request(s)
        let mut addrs = Vec::new();
        let mut kinds: Vec<(usize, PTag)> = Vec::new();
        for (i, t) in tags.iter().enumerate() {
            match self.resolve(t) {
                Ok(pt) => {
                    let a = match &pt {
                        PTag::Abs(_, it) => s7plus::area_address(it),
                        PTag::Sym(s) => Ok(s.addr.clone()),
                    };
                    match a {
                        Ok(a) => {
                            addrs.push(a);
                            kinds.push((i, pt));
                        }
                        Err(e) => out[i] = Out::bad(format!("{t}: {e}")),
                    }
                }
                Err(e) => out[i] = Out::bad(e),
            }
        }
        if addrs.is_empty() {
            return (out, None);
        }
        let (vals, errs) = match self.run(|c| c.read(&addrs)) {
            Ok(r) => r,
            Err(e) => {
                for (i, _) in &kinds {
                    out[*i] = Out::bad(e.to_string());
                }
                return (out, Some(e.to_string()));
            }
        };
        let mut learned = Vec::new();
        for (k, (i, pt)) in kinds.iter().enumerate() {
            let i = *i;
            if errs[k] != 0 {
                out[i] = Out::bad(format!("{}: {}", tags[i], PErr::Item(errs[k])));
                continue;
            }
            let (o, l) = decode_out(&tags[i], pt, &vals[k]);
            out[i] = o;
            if let Some(l) = l {
                learned.push((tags[i].clone(), l));
            }
        }
        if !learned.is_empty() {
            self.tags.lock().unwrap().extend(learned);
        }
        (out, None)
    }

    fn write(&self, items: &[(String, Val)]) -> Result<Vec<Option<String>>, String> {
        let mut out: Vec<Option<String>> = vec![None; items.len()];
        let (mut abs, mut abs_d, mut abs_i) = (Vec::new(), Vec::new(), Vec::new());
        let (mut sym, mut sym_t, mut sym_i) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (name, v)) in items.iter().enumerate() {
            match self.resolve(name) {
                Ok(PTag::Abs(tag, it)) => match tag.value_from(v) {
                    Ok(b) => {
                        abs.push(it);
                        abs_d.push(b);
                        abs_i.push(i);
                    }
                    Err(e) => out[i] = Some(format!("{name}: {e}")),
                },
                Ok(PTag::Sym(s)) => match v.to_text() {
                    Ok(t) => {
                        sym.push(s);
                        sym_t.push(t);
                        sym_i.push(i);
                    }
                    Err(e) => out[i] = Some(format!("{name}: {e}")),
                },
                Err(e) => out[i] = Some(e),
            }
        }
        if !abs.is_empty() {
            let errs = self.run(|c| c.write_items(&abs, &abs_d)).map_err(|e| e.to_string())?;
            for (k, e) in errs.into_iter().enumerate() {
                if let Some(e) = e {
                    out[abs_i[k]] = Some(format!("{}: {e}", items[abs_i[k]].0));
                }
            }
        }
        if !sym.is_empty() {
            let texts: Vec<&str> = sym_t.iter().map(|s| s.as_str()).collect();
            let errs = self.run(|c| c.write_tags(&sym, &texts)).map_err(|e| e.to_string())?;
            for (k, e) in errs.into_iter().enumerate() {
                if let Some(e) = e {
                    out[sym_i[k]] = Some(format!("{}: {e}", items[sym_i[k]].0));
                }
            }
        }
        Ok(out)
    }

    fn check_tag(&self, tag: &str) -> Option<String> {
        self.resolve(tag).err()
    }

    fn tick(&self, every: Duration, now: bool) {
        if self.client.lock().unwrap().is_some() {
            return;
        }
        let mut last = self.last_try.lock().unwrap();
        if now || last.is_none_or(|t| t.elapsed() >= every) {
            *last = Some(Instant::now());
            drop(last);
            let _ = self.client();
        }
    }

    /// Actions: status, info, cpuState, limits, dbs, browse, alarms {all}.
    /// RUN/STOP is not exposed.
    fn call(&self, action: &str, a: &J) -> Result<J, String> {
        let e = |e: PErr| e.to_string();
        Ok(match action {
            "status" => {
                let (st, det) = self.state.lock().unwrap().clone();
                let s = self.stats.snapshot();
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                json!({
                    "conn": self.id, "protocol": "s7plus", "addr": self.cfg.addr, "state": st, "detail": det,
                    "reconnects": self.reconnects.load(Ordering::Relaxed), "requests": s.requests, "errors": s.errors,
                    "p50ms": ms(s.p50), "p95ms": ms(s.p95), "p99ms": ms(s.p99), "rate1m": s.rate_1m,
                })
            }
            "info" => {
                let c = self.client().map_err(e)?;
                let i = c.info();
                let l = c.limits();
                json!({
                    "paom": i.paom(), "protocolVersion": i.protocol_ver, "tls": i.tls, "initSslRefused": i.init_ssl_refused,
                    "session": i.session, "limits": limits_json(&l),
                })
            }
            "cpuState" => {
                let s = self.run(|c| c.operating_state()).map_err(e)?;
                json!({"state": s7plus::operating_state_name(s), "code": s})
            }
            "limits" => limits_json(&self.run(|c| c.read_limits()).map_err(e)?),
            "dbs" => {
                let l = self.run(|c| c.list_datablocks(true)).map_err(e)?;
                J::Array(
                    l.iter()
                        .map(|d| json!({"name": d.name, "number": d.number, "rid": format!("0x{:08x}", d.rid), "optimized": d.optimized, "vars": d.vars}))
                        .collect(),
                )
            }
            "browse" => {
                let l = self.run(|c| c.browse()).map_err(e)?;
                J::Array(
                    l.iter()
                        .map(|v| {
                            json!({
                                "name": v.name, "type": s7plus::softdt::softdatatype_name(v.softdatatype), "softdatatype": v.softdatatype,
                                "address": v.addr.to_string(), "maxLen": v.max_len,
                                "optOffset": v.opt_offset, "optBit": v.opt_bit, "stdOffset": v.non_opt_offset, "stdBit": v.non_opt_bit,
                            })
                        })
                        .collect(),
                )
            }
            "alarms" => {
                let all = a.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
                let l = self.run(|c| c.active_alarms(all)).map_err(e)?;
                J::Array(
                    l.iter()
                        .map(|x| {
                            let cgs = |c: &Option<s7plus::AlarmCgs>| {
                                c.as_ref().map(|c| json!({"state": c.all_states, "time": c.timestamp.map(|t| t.rfc3339_nano()), "ackTime": c.ack_time.map(|t| t.rfc3339_nano())}))
                            };
                            let mut texts: Vec<(u32, String)> = x.texts.iter().map(|(k, v)| (*k, v.clone())).collect();
                            texts.sort();
                            json!({
                                "name": x.name, "id": format!("0x{:016x}", x.cpu_alarm_id), "allStates": x.all_states,
                                "messageType": x.message_type, "seq": x.seq_counter, "coming": cgs(&x.coming), "going": cgs(&x.going),
                                "texts": texts.iter().map(|(k, v)| json!({"lang": k >> 16, "id": k & 0xffff, "text": v})).collect::<Vec<_>>(),
                            })
                        })
                        .collect(),
                )
            }
            x => return Err(format!("unknown action {x:?} for s7plus (status, info, cpuState, limits, dbs, browse, alarms)")),
        })
    }

    fn push_poll(&self, sub: &str, tags: &[String], cycle_ms: u32, wait: Duration) -> Result<Push, String> {
        let Some(c) = self.client.lock().unwrap().clone() else { return Ok(Push::Down) };
        let mut map = self.push.lock().unwrap();
        if !map.get(sub).is_some_and(|p| Arc::ptr_eq(&p.client, &c)) {
            map.remove(sub); // the subscription of a lost session is gone with it
            match self.push_create(&c, tags, cycle_ms) {
                Ok(p) => {
                    map.insert(sub.to_string(), p);
                }
                Err(e) if transport(&e) => {
                    drop(map);
                    self.drop_client(&e);
                    return Ok(Push::Down);
                }
                Err(e) => {
                    emit_log(&self.log, &self.id, "warn", format!("PLC subscription refused ({e}); the tags are read cyclically"));
                    return Err(e.to_string());
                }
            }
        }
        let p = map.get_mut(sub).expect("subscription");
        let mut out = std::mem::take(&mut p.pending);
        let mut ts = None;
        let mut learned = Vec::new();
        match p.sub.next(&c, wait) {
            Ok(n) => {
                ts = n.timestamp.map(|t| t.rfc3339_nano());
                for (r, v) in &n.values {
                    if let Some((i, pt)) = p.refs.get_mut(r) {
                        let (o, l) = decode_out(&tags[*i], pt, v);
                        if let Some(l) = l {
                            learned.push((tags[*i].clone(), l.clone()));
                            *pt = l;
                        }
                        out.push((*i, o));
                    }
                }
                for (r, e) in &n.item_errors {
                    if let Some((i, _)) = p.refs.get(r) {
                        out.push((*i, Out::bad(format!("{}: {}", tags[*i], PErr::Item(*e as u64)))));
                    }
                }
            }
            Err(e) if e.is_timeout() => {}
            Err(e) if transport(&e) => {
                map.remove(sub);
                drop(map);
                self.drop_client(&e);
                return Ok(Push::Down);
            }
            Err(e) => emit_log(&self.log, &self.id, "warn", format!("notification: {e}")),
        }
        drop(map);
        if !learned.is_empty() {
            self.tags.lock().unwrap().extend(learned);
        }
        out.sort_by_key(|(i, _)| *i); // the PLC sends the values in its own order
        Ok(if out.is_empty() { Push::Idle } else { Push::Values(out, ts) })
    }

    fn push_close(&self, sub: &str) {
        let p = self.push.lock().unwrap().remove(sub);
        if let Some(p) = p {
            let cur = self.client.lock().unwrap().clone();
            if cur.is_some_and(|c| Arc::ptr_eq(&c, &p.client)) {
                let _ = p.sub.close(&p.client);
            }
        }
    }

    fn shutdown(&self) {
        if let Some(c) = self.client.lock().unwrap().take() {
            let _ = c.close();
        }
    }
}

fn limits_json(l: &s7plus::Limits) -> J {
    json!({
        "tagsPerReadMax": l.tags_per_read_max, "tagsPerWriteMax": l.tags_per_write_max,
        "subscriptionsMax": l.subscriptions_max, "attributesMax": l.attributes_max, "subMemoryMax": l.sub_memory_max,
        "subscriptionsFree": l.subscriptions_free, "attributesFree": l.attributes_free, "subMemoryFree": l.sub_memory_free,
    })
}
