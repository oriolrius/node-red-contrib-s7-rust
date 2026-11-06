//! Node-API addon: the S7 clients run inside the Node.js process (no IPC).
//!
//! Two protocols behind one JS API: classic S7comm (crate `s7`) and S7CommPlus
//! (crate `s7plus`, S7-1200/1500: absolute and symbolic access, TLS,
//! legitimation). One OS thread per PLC connection (requests are serialised
//! per connection), one thread per subscription. Hot-path results are
//! columnar (`vals`, `errs`, `idx`): the JS wrapper (lib/napi-bridge.js) builds
//! the {tag, value, quality} objects, which is much cheaper in V8 than one
//! Node-API call per property. Diagnostics (`call`) return JSON text.

mod device;
mod plus;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use napi::threadsafe_function::{ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Env, JsDeferred, JsFunction, JsObject, JsUnknown, ValueType};
use napi_derive::napi;
use s7::Val;

use device::{Device, Out};

type Resolver = Box<dyn FnOnce(Env) -> napi::Result<JsObject> + Send>;
type Deferred = JsDeferred<JsObject, Resolver>;
type UnknownDeferred = JsDeferred<JsUnknown, Box<dyn FnOnce(Env) -> napi::Result<JsUnknown> + Send>>;
pub(crate) type StatusFn = ThreadsafeFunction<(String, String, String), ErrorStrategy::Fatal>;
type DataFn = ThreadsafeFunction<DataEv, ErrorStrategy::Fatal>;
pub(crate) type LogFn = ThreadsafeFunction<(String, String, String), ErrorStrategy::Fatal>;

/// A unit of work run on the connection thread.
type Job = Box<dyn FnOnce(&dyn Device) + Send>;

enum Msg {
    Job(Job),
    Stop,
}

struct DataEv {
    sub: String,
    conn: String,
    total: usize,
    vals: Vec<(usize, Out)>,
    ts: String,
    ms: f64,
}

#[napi(object)]
pub struct OpenParams {
    pub conn: String,
    /// "s7" (default) or "s7plus"
    pub protocol: Option<String>,
    pub host: String,
    pub port: Option<u32>,
    pub timeout_ms: Option<u32>,
    pub attempts: Option<u32>,
    pub reconnect_ms: Option<u32>,
    /// "error" | "warn" | "info" | "debug" | "trace" (trace = frames in hex); default "warn"
    pub log_level: Option<String>,
    // classic S7
    pub cpu: Option<String>,
    pub rack: Option<u32>,
    pub slot: Option<u32>,
    pub link: Option<u32>,
    pub local_tsap: Option<String>,
    pub remote_tsap: Option<String>,
    pub pdu: Option<u32>,
    pub gap: Option<i32>,
    pub little_endian: Option<bool>,
    pub local_addr: Option<String>,
    // S7CommPlus
    /// "SIMATIC-ROOT-HMI" (default) or "SIMATIC-ROOT-ES"
    pub tsap: Option<String>,
    pub tls: Option<bool>,
    pub password: Option<String>,
    pub username: Option<String>,
}

#[napi(object)]
pub struct OpenResult {
    pub protocol: String,
    pub addr: String,
    pub rack: Option<u32>,
    pub slot: Option<u32>,
    pub link: Option<u32>,
}

#[napi(object)]
pub struct SubParams {
    pub conn: String,
    pub sub: String,
    pub tags: Vec<String>,
    pub interval_ms: Option<u32>,
    pub on_change: Option<bool>,
    /// Conditional rate: while this tag is true (or non-zero), read every condIntervalMs.
    pub cond_tag: Option<String>,
    pub cond_interval_ms: Option<u32>,
}

#[napi(object)]
pub struct TagError {
    pub tag: String,
    pub error: String,
}

struct ConnHandle {
    tx: Sender<Msg>,
}

struct SubHandle {
    conn: String,
    stop: Arc<AtomicBool>,
}

#[napi]
pub struct S7Native {
    conns: Mutex<HashMap<String, ConnHandle>>,
    subs: Mutex<HashMap<String, SubHandle>>,
    on_status: StatusFn,
    on_data: DataFn,
    on_log: Option<LogFn>,
}

pub(crate) fn napi_err(e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

fn str_tsfn(env: &Env, f: JsFunction) -> napi::Result<ThreadsafeFunction<(String, String, String), ErrorStrategy::Fatal>> {
    let mut t = f.create_threadsafe_function(0, |ctx: ThreadSafeCallContext<(String, String, String)>| {
        let (a, b, c) = ctx.value;
        Ok(vec![ctx.env.create_string(&a)?, ctx.env.create_string(&b)?, ctx.env.create_string(&c)?])
    })?;
    t.unref(env)?;
    Ok(t)
}

#[napi]
impl S7Native {
    /// onStatus(conn, state, detail); onData(event); onLog(conn, level, message) optional.
    /// All are unref'd: they do not keep Node alive on their own.
    #[napi(constructor)]
    pub fn new(env: Env, on_status: JsFunction, on_data: JsFunction, on_log: Option<JsFunction>) -> napi::Result<Self> {
        let st = str_tsfn(&env, on_status)?;
        let lg = match on_log {
            Some(f) => Some(str_tsfn(&env, f)?),
            None => None,
        };
        // A bounded queue: a slow JS side blocks the subscription thread (back-pressure)
        // instead of growing memory.
        let mut dt: DataFn = on_data.create_threadsafe_function(16, |ctx: ThreadSafeCallContext<DataEv>| {
            let ev = ctx.value;
            let env = ctx.env;
            let mut o = env.create_object()?;
            o.set_named_property("sub", env.create_string(&ev.sub)?)?;
            o.set_named_property("conn", env.create_string(&ev.conn)?)?;
            let (vals, errs) = columns(&env, ev.vals.iter().map(|(_, r)| r))?;
            o.set_named_property("vals", vals)?;
            o.set_named_property("errs", errs)?;
            if ev.vals.len() != ev.total {
                let mut idx = env.create_array_with_length(ev.vals.len())?;
                for (k, (i, _)) in ev.vals.iter().enumerate() {
                    idx.set_element(k as u32, env.create_uint32(*i as u32)?)?;
                }
                o.set_named_property("idx", idx)?;
            }
            o.set_named_property("ts", env.create_string(&ev.ts)?)?;
            o.set_named_property("ms", env.create_double(ev.ms)?)?;
            Ok(vec![o])
        })?;
        dt.unref(&env)?;
        Ok(S7Native { conns: Mutex::new(HashMap::new()), subs: Mutex::new(HashMap::new()), on_status: st, on_data: dt, on_log: lg })
    }

    #[napi]
    pub fn open(&self, p: OpenParams) -> napi::Result<OpenResult> {
        if p.conn.is_empty() || p.host.is_empty() {
            return Err(napi_err("open: conn and host are required"));
        }
        let every = Duration::from_millis(p.reconnect_ms.filter(|x| *x > 0).unwrap_or(5000) as u64);
        let id = p.conn.clone();
        let (dev, res): (Arc<dyn Device>, OpenResult) = device::open(p, self.on_status.clone(), self.on_log.clone()).map_err(napi_err)?;
        self.close(id.clone()); // a redeploy reopens the same id
        let (tx, rx) = mpsc::channel();
        let status = self.on_status.clone();
        let id2 = id.clone();
        std::thread::Builder::new().name(format!("s7-{id}")).spawn(move || worker(id2, dev, rx, status, every)).map_err(napi_err)?;
        self.conns.lock().unwrap().insert(id, ConnHandle { tx });
        Ok(res)
    }

    #[napi]
    pub fn close(&self, conn: String) {
        let subs: Vec<String> = self.subs.lock().unwrap().iter().filter(|(_, s)| s.conn == conn).map(|(k, _)| k.clone()).collect();
        for s in subs {
            self.unsubscribe(s);
        }
        if let Some(h) = self.conns.lock().unwrap().remove(&conn) {
            let _ = h.tx.send(Msg::Stop);
        }
    }

    fn sender(&self, conn: &str) -> napi::Result<Sender<Msg>> {
        self.conns.lock().unwrap().get(conn).map(|h| h.tx.clone()).ok_or_else(|| napi_err(format!("connection {conn:?} is not open")))
    }

    /// read(conn, tags) -> Promise<{vals, errs: [[i, message]], nativeMs}>
    #[napi(ts_return_type = "Promise<any>")]
    pub fn read(&self, env: Env, conn: String, tags: Vec<String>) -> napi::Result<JsObject> {
        let tx = self.sender(&conn)?;
        let (d, promise): (Deferred, JsObject) = env.create_deferred()?;
        let job: Job = Box::new(move |dev| {
            let t0 = Instant::now();
            let (outs, err) = dev.read(&tags);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match err {
                // a transport error with no per-tag data rejects; per-tag errors resolve
                Some(e) if outs.iter().all(|o| o.val.is_none()) => d.reject(napi_err(e)),
                _ => d.resolve(Box::new(move |env: Env| {
                    let (vals, errs) = columns(&env, outs.iter())?;
                    let mut o = env.create_object()?;
                    o.set_named_property("vals", vals)?;
                    o.set_named_property("errs", errs)?;
                    o.set_named_property("nativeMs", env.create_double(ms)?)?;
                    Ok(o)
                })),
            }
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// write(conn, tags, values) -> Promise<{errs: [[i, message]], nativeMs}>
    #[napi(ts_return_type = "Promise<any>")]
    pub fn write(&self, env: Env, conn: String, tags: Vec<String>, values: JsObject) -> napi::Result<JsObject> {
        let tx = self.sender(&conn)?;
        let mut list = Vec::with_capacity(tags.len());
        let mut pre: Vec<Option<String>> = vec![None; tags.len()];
        for (i, tag) in tags.into_iter().enumerate() {
            match js_to_val(values.get_element::<JsUnknown>(i as u32)?) {
                Ok(v) => list.push((tag, v)),
                Err(e) => {
                    pre[i] = Some(format!("{tag}: {e}"));
                    list.push((tag, Val::Null));
                }
            }
        }
        let (d, promise): (Deferred, JsObject) = env.create_deferred()?;
        let job: Job = Box::new(move |dev| {
            let t0 = Instant::now();
            // values that did not convert are not sent
            let send: Vec<(usize, (String, Val))> = list.into_iter().enumerate().filter(|(i, _)| pre[*i].is_none()).collect();
            let items: Vec<(String, Val)> = send.iter().map(|(_, x)| x.clone()).collect();
            let r = if items.is_empty() { Ok(Vec::new()) } else { dev.write(&items) };
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match r {
                Err(e) => d.reject(napi_err(e)),
                Ok(errs) => {
                    let mut all = pre;
                    for (k, (i, _)) in send.iter().enumerate() {
                        all[*i] = errs.get(k).cloned().flatten();
                    }
                    d.resolve(Box::new(move |env: Env| {
                        let mut arr = env.create_array_with_length(0)?;
                        let mut ne = 0u32;
                        for (i, e) in all.iter().enumerate() {
                            if let Some(e) = e {
                                let mut pair = env.create_array_with_length(2)?;
                                pair.set_element(0, env.create_uint32(i as u32)?)?;
                                pair.set_element(1, env.create_string(e)?)?;
                                arr.set_element(ne, pair)?;
                                ne += 1;
                            }
                        }
                        let mut o = env.create_object()?;
                        o.set_named_property("errs", arr)?;
                        o.set_named_property("nativeMs", env.create_double(ms)?)?;
                        Ok(o)
                    }))
                }
            }
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// subscribe({conn, sub, tags, intervalMs, onChange, condTag, condIntervalMs})
    /// -> Promise<[{tag, error}]> (tags that do not parse; they are read as bad values)
    #[napi(ts_return_type = "Promise<any>")]
    pub fn subscribe(&self, env: Env, p: SubParams) -> napi::Result<JsObject> {
        if p.sub.is_empty() || p.tags.is_empty() {
            return Err(napi_err("subscribe: sub and tags are required"));
        }
        let tx = self.sender(&p.conn)?;
        self.unsubscribe(p.sub.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let ms = |x: Option<u32>, dflt: u64| {
            let d = Duration::from_millis(x.map(|v| v as u64).unwrap_or(dflt));
            if d < Duration::from_millis(10) {
                Duration::from_secs(1)
            } else {
                d
            }
        };
        let cfg = SubCfg {
            sub: p.sub.clone(),
            conn: p.conn.clone(),
            tags: Arc::new(p.tags.clone()),
            interval: ms(p.interval_ms, 1000),
            on_change: p.on_change.unwrap_or(true),
            cond: p.cond_tag.clone().filter(|s| !s.is_empty()),
            cond_interval: ms(p.cond_interval_ms, 100),
        };
        let (data, s2, tx2) = (self.on_data.clone(), stop.clone(), tx.clone());
        std::thread::Builder::new().name(format!("s7sub-{}", p.sub)).spawn(move || sub_loop(cfg, tx2, s2, data)).map_err(napi_err)?;
        self.subs.lock().unwrap().insert(p.sub, SubHandle { conn: p.conn, stop });
        // check the tags on the connection thread (S7CommPlus symbols need the PLC)
        let (d, promise): (Deferred, JsObject) = env.create_deferred()?;
        let mut tags = p.tags;
        if let Some(c) = p.cond_tag.filter(|s| !s.is_empty()) {
            tags.push(c);
        }
        let job: Job = Box::new(move |dev| {
            let bad: Vec<(String, String)> = tags.iter().filter_map(|t| dev.check_tag(t).map(|e| (t.clone(), e))).collect();
            d.resolve(Box::new(move |env: Env| {
                let mut arr = env.create_array_with_length(bad.len())?;
                for (i, (t, e)) in bad.iter().enumerate() {
                    let mut o = env.create_object()?;
                    o.set_named_property("tag", env.create_string(t)?)?;
                    o.set_named_property("error", env.create_string(e)?)?;
                    arr.set_element(i as u32, o)?;
                }
                Ok(arr)
            }))
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    #[napi]
    pub fn unsubscribe(&self, sub: String) {
        // Never join here: the subscription thread may wait for this (JS) thread.
        if let Some(s) = self.subs.lock().unwrap().remove(&sub) {
            s.stop.store(true, Ordering::SeqCst);
        }
    }

    /// status(conn) -> Promise<{conn, protocol, addr, state, detail, pdu, reconnects, requests, errors, p50ms, p95ms, p99ms, rate1m}>
    #[napi(ts_return_type = "Promise<any>")]
    pub fn status(&self, env: Env, conn: String) -> napi::Result<JsObject> {
        self.call_json(env, conn, "status".into(), None)
    }

    /// call(conn, action, argsJson?) -> Promise<string (JSON)>. Diagnostics and
    /// browsing: see device::Device::call for the actions of each protocol.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn call(&self, env: Env, conn: String, action: String, args: Option<String>) -> napi::Result<JsObject> {
        self.call_json(env, conn, action, args)
    }

    fn call_json(&self, env: Env, conn: String, action: String, args: Option<String>) -> napi::Result<JsObject> {
        let tx = self.sender(&conn)?;
        let args: serde_json::Value = match args.as_deref() {
            None | Some("") => serde_json::Value::Null,
            Some(s) => serde_json::from_str(s).map_err(|e| napi_err(format!("{action}: bad args: {e}")))?,
        };
        let (d, promise): (UnknownDeferred, JsObject) = env.create_deferred()?;
        let job: Job = Box::new(move |dev| match dev.call(&action, &args) {
            Err(e) => d.reject(napi_err(format!("{action}: {e}"))),
            Ok(v) => {
                let text = v.to_string();
                let is_status = action == "status";
                d.resolve(Box::new(move |env: Env| {
                    if is_status {
                        // status is used by the editor: hand an object, not text
                        let json: JsObject = env.get_global()?.get_named_property("JSON")?;
                        let parse: JsFunction = json.get_named_property("parse")?;
                        parse.call(Some(&json), &[env.create_string(&text)?])
                    } else {
                        Ok(env.create_string(&text)?.into_unknown())
                    }
                }))
            }
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// Stops every subscription and connection (threads end on their own).
    #[napi]
    pub fn stop(&self) {
        let ids: Vec<String> = self.conns.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.close(id);
        }
    }
}

/// Columnar result: vals[i] (null when bad) and errs = [[i, message], ...].
fn columns<'a>(env: &Env, outs: impl ExactSizeIterator<Item = &'a Out>) -> napi::Result<(JsObject, JsObject)> {
    let mut vals = env.create_array_with_length(outs.len())?;
    let mut errs = env.create_array_with_length(0)?;
    let mut ne = 0u32;
    for (i, r) in outs.enumerate() {
        match (&r.val, &r.err) {
            (Some(v), None) => vals.set_element(i as u32, val_to_js(env, v)?)?,
            (_, e) => {
                vals.set_element(i as u32, env.get_null()?)?;
                let mut pair = env.create_array_with_length(2)?;
                pair.set_element(0, env.create_uint32(i as u32)?)?;
                pair.set_element(1, env.create_string(e.as_deref().unwrap_or("bad"))?)?;
                errs.set_element(ne, pair)?;
                ne += 1;
            }
        }
    }
    Ok((vals, errs))
}

fn val_to_js(env: &Env, v: &Val) -> napi::Result<JsUnknown> {
    Ok(match v {
        Val::Null => env.get_null()?.into_unknown(),
        Val::Bool(b) => env.get_boolean(*b)?.into_unknown(),
        Val::Int(i) => env.create_int64(*i)?.into_unknown(),
        Val::Float(f) => env.create_double(*f)?.into_unknown(),
        Val::Str(s) => env.create_string(s)?.into_unknown(),
        Val::List(l) => {
            let mut a = env.create_array_with_length(l.len())?;
            for (i, x) in l.iter().enumerate() {
                a.set_element(i as u32, val_to_js(env, x)?)?;
            }
            a.into_unknown()
        }
    })
}

fn js_to_val(v: JsUnknown) -> Result<Val, String> {
    let e = |e: napi::Error| e.reason;
    match v.get_type().map_err(e)? {
        ValueType::Boolean => Ok(Val::Bool(v.coerce_to_bool().map_err(e)?.get_value().map_err(e)?)),
        ValueType::Number => {
            let f = v.coerce_to_number().map_err(e)?.get_double().map_err(e)?;
            if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                Ok(Val::Int(f as i64))
            } else {
                Ok(Val::Float(f))
            }
        }
        ValueType::String | ValueType::BigInt => {
            Ok(Val::Str(v.coerce_to_string().map_err(e)?.into_utf8().map_err(e)?.into_owned().map_err(e)?))
        }
        ValueType::Object => {
            if v.is_array().map_err(e)? || v.is_buffer().map_err(e)? || v.is_typedarray().map_err(e)? {
                // SAFETY-free cast: napi's JsUnknown -> JsObject for arrays is checked above
                let o: JsObject = unsafe { v.cast() };
                let n = o.get_named_property::<JsUnknown>("length").map_err(e)?.coerce_to_number().map_err(e)?.get_uint32().map_err(e)?;
                let mut l = Vec::with_capacity(n as usize);
                for i in 0..n {
                    l.push(js_to_val(o.get_element::<JsUnknown>(i).map_err(e)?)?);
                }
                Ok(Val::List(l))
            } else if v.is_date().map_err(e)? {
                let o: JsObject = unsafe { v.cast() };
                let f: JsFunction = o.get_named_property("toISOString").map_err(e)?;
                let s = f.call_without_args(Some(&o)).map_err(e)?;
                Ok(Val::Str(s.coerce_to_string().map_err(e)?.into_utf8().map_err(e)?.into_owned().map_err(e)?))
            } else {
                Err("objects are not S7 values".into())
            }
        }
        _ => Err("value is missing".into()),
    }
}

fn worker(id: String, dev: Arc<dyn Device>, rx: Receiver<Msg>, status: StatusFn, every: Duration) {
    status.call((id.clone(), "connecting".into(), String::new()), ThreadsafeFunctionCallMode::NonBlocking);
    dev.tick(every, true);
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Msg::Job(job)) => job(dev.as_ref()),
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        dev.tick(every, false);
    }
    dev.shutdown();
    status.call((id, "disconnected".into(), "closed".into()), ThreadsafeFunctionCallMode::NonBlocking);
}

struct SubCfg {
    sub: String,
    conn: String,
    tags: Arc<Vec<String>>,
    interval: Duration,
    on_change: bool,
    cond: Option<String>,
    cond_interval: Duration,
}

fn truthy(v: &Option<Val>) -> bool {
    match v {
        Some(Val::Bool(b)) => *b,
        Some(Val::Int(i)) => *i != 0,
        Some(Val::Float(f)) => *f != 0.0,
        Some(Val::Str(s)) => s == "true" || s == "1",
        _ => false,
    }
}

fn sub_loop(c: SubCfg, tx: Sender<Msg>, stop: Arc<AtomicBool>, data: DataFn) {
    let n = c.tags.len();
    let mut read_list: Vec<String> = c.tags.as_ref().clone();
    if let Some(ct) = &c.cond {
        read_list.push(ct.clone());
    }
    let read_list = Arc::new(read_list);
    let mut prev: Vec<Option<(Option<String>, Vec<u8>)>> = vec![None; n];
    let mut next = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let start = Instant::now();
        let (rtx, rrx) = mpsc::sync_channel(1);
        let list = read_list.clone();
        let job: Job = Box::new(move |dev| {
            let _ = rtx.send(dev.read(&list).0);
        });
        if tx.send(Msg::Job(job)).is_err() {
            break;
        }
        let Ok(mut outs) = rrx.recv() else { break };
        let fast = c.cond.is_some() && outs.len() > n && truthy(&outs[n].val);
        outs.truncate(n);
        let mut send = Vec::new();
        for (i, o) in outs.into_iter().enumerate() {
            let changed = match &prev[i] {
                None => true,
                Some((e, raw)) => *e != o.err || *raw != o.raw,
            };
            if changed {
                prev[i] = Some((o.err.clone(), o.raw.clone()));
            }
            if changed || !c.on_change {
                send.push((i, o));
            }
        }
        if !send.is_empty() && !stop.load(Ordering::SeqCst) {
            let ev = DataEv {
                sub: c.sub.clone(),
                conn: c.conn.clone(),
                total: n,
                vals: send,
                ts: device::rfc3339_now(),
                ms: start.elapsed().as_secs_f64() * 1000.0,
            };
            if data.call(ev, ThreadsafeFunctionCallMode::Blocking) != napi::Status::Ok {
                break;
            }
        }
        // ticker: the next cycle starts one interval after the start of this one;
        // a slow PLC delays it, cycles never overlap
        let iv = if fast { c.cond_interval } else { c.interval };
        next += iv;
        let now = Instant::now();
        if next <= now {
            next = now;
        }
        while !stop.load(Ordering::SeqCst) {
            let left = next.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(20)));
        }
    }
}

/// Test support (cargo feature `fake`): in-process fake PLCs on 127.0.0.1.
#[cfg(feature = "fake")]
#[allow(dead_code)] // used from JavaScript only
mod fakes {
    use napi_derive::napi;
    use std::sync::Mutex;

    static KEEP: Mutex<Vec<Box<dyn std::any::Any + Send>>> = Mutex::new(Vec::new());

    /// Starts a fake classic CPU (M 4096 B, I 1024 B, DB1 2000 B + the given DBs);
    /// returns "127.0.0.1:port". Lives until the process exits.
    #[napi]
    pub fn start_fake_s7(pdu: u32, dbs: Option<Vec<u32>>) -> String {
        let f = s7::fakecpu::FakeCpu::new(pdu as u16);
        for n in dbs.unwrap_or_default() {
            f.add_db(n as u16, 65536);
        }
        let a = f.start();
        KEEP.lock().unwrap().push(Box::new(f));
        a
    }

    /// Starts a fake S7-1200 speaking S7CommPlus (DB1 "Data_block_1", M tags
    /// Motor_On/Speed…); returns "127.0.0.1:port".
    #[napi]
    pub fn start_fake_s7plus(tls: Option<bool>) -> napi::Result<String> {
        let f = s7plus::fake::FakePlc::start().map_err(crate::napi_err)?;
        if tls == Some(true) {
            f.enable_tls().map_err(crate::napi_err)?;
        }
        let a = f.addr();
        KEEP.lock().unwrap().push(Box::new(f));
        Ok(a)
    }
}
