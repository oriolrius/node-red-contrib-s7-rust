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

use napi::bindgen_prelude::{Array, Either, FnArgs, Function, JsObjectValue, JsValue, Null, Object, ToNapiValue, Unknown};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{sys, Env, JsDeferred, Status, ValueType};
use napi_derive::napi;
use s7::Val;

use device::{Device, Out};

/// A promise settled from the connection thread; the value is built on the JS thread.
type Deferred<T> = JsDeferred<T, Box<dyn FnOnce(Env) -> napi::Result<T> + Send>>;
type StrArgs = (String, String, String);
/// fn(a, b, c): no error argument, weak (does not keep Node alive), unbounded queue.
type StrFn = ThreadsafeFunction<StrArgs, Unknown<'static>, FnArgs<StrArgs>, Status, false, true, 0>;
pub(crate) type StatusFn = Arc<StrFn>;
pub(crate) type LogFn = Arc<StrFn>;
/// fn(event): weak, and a bounded queue: a slow JS side blocks the subscription
/// thread (back-pressure) instead of growing memory.
type DataFn = Arc<ThreadsafeFunction<DataEv, Unknown<'static>, DataEv, Status, false, true, 16>>;
type JsStrFn<'a> = Function<'a, FnArgs<StrArgs>, Unknown<'static>>;

/// A unit of work run on the connection thread.
type Job = Box<dyn FnOnce(&dyn Device) + Send>;

enum Msg {
    Job(Job),
    Stop,
}

pub struct DataEv {
    sub: String,
    conn: String,
    total: usize,
    vals: Vec<(usize, Out)>,
    ts: String,
    ms: f64,
    /// Alarm events of an alarm subscription, JSON text {snapshot, alarms}; no values then.
    alarms: Option<String>,
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
    /// Check of the PLC certificate (TLS): "none" (default), "fingerprint"
    /// (certFingerprint: SHA-256, one or more) or "trusted" (certTrusted:
    /// PEM certificates, the PLC certificate or the project CA).
    pub cert_verify: Option<String>,
    pub cert_fingerprint: Option<String>,
    pub cert_trusted: Option<String>,
    /// Name for the check of "trusted" (and SNI); empty = the host.
    pub cert_name: Option<String>,
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
    /// S7CommPlus: a PLC subscription with cycle time intervalMs; the PLC sends
    /// the changes. Falls back to cyclic reads when the PLC refuses it.
    /// onChange and the conditional rate do not apply.
    pub push: Option<bool>,
}

#[napi(object)]
pub struct AlarmParams {
    pub conn: String,
    pub sub: String,
    /// Send the active alarms first (default true; again after a reconnect).
    pub initial: Option<bool>,
    /// "auto" (default), "tia" or "reference": the subscription layout.
    pub layout: Option<String>,
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

fn str_tsfn(f: JsStrFn<'_>) -> napi::Result<Arc<StrFn>> {
    let t = f
        .build_threadsafe_function::<StrArgs>()
        .callee_handled::<false>()
        .weak::<true>()
        .build_callback(|ctx| Ok(FnArgs::from(ctx.value)))?;
    Ok(Arc::new(t))
}

/// The data event: {sub, conn, vals, errs, idx?, ts, ms}.
impl ToNapiValue for DataEv {
    unsafe fn to_napi_value(raw: sys::napi_env, ev: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let mut o = Object::new(&env)?;
        o.set_named_property("sub", ev.sub)?;
        o.set_named_property("conn", ev.conn)?;
        let (vals, errs) = columns(&env, ev.vals.iter().map(|(_, r)| r))?;
        o.set_named_property("vals", vals)?;
        o.set_named_property("errs", errs)?;
        // without idx the values are the tags 0..n in order
        if ev.vals.len() != ev.total || ev.vals.iter().enumerate().any(|(k, (i, _))| k != *i) {
            o.set_named_property("idx", ev.vals.iter().map(|(i, _)| *i as u32).collect::<Vec<u32>>())?;
        }
        o.set_named_property("ts", ev.ts)?;
        o.set_named_property("ms", ev.ms)?;
        if let Some(a) = ev.alarms {
            o.set_named_property("alarms", a)?;
        }
        Ok(o.raw())
    }
}

/// read() result: {vals, errs, nativeMs}.
struct ReadRes {
    outs: Vec<Out>,
    ms: f64,
}

impl ToNapiValue for ReadRes {
    unsafe fn to_napi_value(raw: sys::napi_env, r: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let (vals, errs) = columns(&env, r.outs.iter())?;
        let mut o = Object::new(&env)?;
        o.set_named_property("vals", vals)?;
        o.set_named_property("errs", errs)?;
        o.set_named_property("nativeMs", r.ms)?;
        Ok(o.raw())
    }
}

/// write() result: {errs: [[i, message]], nativeMs}.
struct WriteRes {
    errs: Vec<Option<String>>,
    ms: f64,
}

impl ToNapiValue for WriteRes {
    unsafe fn to_napi_value(raw: sys::napi_env, r: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let mut arr = env.create_array(0)?;
        for (i, e) in r.errs.into_iter().enumerate() {
            if let Some(e) = e {
                arr.insert(err_pair(&env, i, &e)?)?;
            }
        }
        let mut o = Object::new(&env)?;
        o.set_named_property("errs", arr)?;
        o.set_named_property("nativeMs", r.ms)?;
        Ok(o.raw())
    }
}

/// subscribe() result: [{tag, error}].
struct BadTags(Vec<(String, String)>);

impl ToNapiValue for BadTags {
    unsafe fn to_napi_value(raw: sys::napi_env, b: Self) -> napi::Result<sys::napi_value> {
        let env = Env::from_raw(raw);
        let mut arr = env.create_array(b.0.len() as u32)?;
        for (i, (t, e)) in b.0.into_iter().enumerate() {
            let mut o = Object::new(&env)?;
            o.set_named_property("tag", t)?;
            o.set_named_property("error", e)?;
            arr.set(i as u32, o)?;
        }
        Ok(arr.raw())
    }
}

#[napi]
impl S7Native {
    /// onStatus(conn, state, detail); onData(event); onLog(conn, level, message) optional.
    /// All are unref'd: they do not keep Node alive on their own.
    #[napi(constructor)]
    pub fn new(
        on_status: JsStrFn<'_>,
        #[napi(ts_arg_type = "(event: any) => void")] on_data: Function<'_, DataEv, Unknown<'static>>,
        on_log: Option<JsStrFn<'_>>,
    ) -> napi::Result<Self> {
        let st = str_tsfn(on_status)?;
        let lg = match on_log {
            Some(f) => Some(str_tsfn(f)?),
            None => None,
        };
        let dt: DataFn = Arc::new(
            on_data.build_threadsafe_function::<DataEv>().callee_handled::<false>().weak::<true>().max_queue_size::<16>().build()?,
        );
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
    pub fn read<'e>(&self, env: &'e Env, conn: String, tags: Vec<String>) -> napi::Result<Object<'e>> {
        let tx = self.sender(&conn)?;
        let (d, promise): (Deferred<ReadRes>, Object) = env.create_deferred()?;
        let job: Job = Box::new(move |dev| {
            let t0 = Instant::now();
            let (outs, err) = dev.read(&tags);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match err {
                // a transport error with no per-tag data rejects; per-tag errors resolve
                Some(e) if outs.iter().all(|o| o.val.is_none()) => d.reject(napi_err(e)),
                _ => d.resolve(Box::new(move |_| Ok(ReadRes { outs, ms }))),
            }
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// write(conn, tags, values) -> Promise<{errs: [[i, message]], nativeMs}>
    #[napi(ts_return_type = "Promise<any>")]
    pub fn write<'e>(&self, env: &'e Env, conn: String, tags: Vec<String>, values: Vec<Unknown<'e>>) -> napi::Result<Object<'e>> {
        let tx = self.sender(&conn)?;
        let mut list = Vec::with_capacity(tags.len());
        let mut pre: Vec<Option<String>> = vec![None; tags.len()];
        for (i, tag) in tags.into_iter().enumerate() {
            match values.get(i).map_or(Err("value is missing".to_string()), js_to_val) {
                Ok(v) => list.push((tag, v)),
                Err(e) => {
                    pre[i] = Some(format!("{tag}: {e}"));
                    list.push((tag, Val::Null));
                }
            }
        }
        let (d, promise): (Deferred<WriteRes>, Object) = env.create_deferred()?;
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
                    d.resolve(Box::new(move |_| Ok(WriteRes { errs: all, ms })))
                }
            }
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// subscribe({conn, sub, tags, intervalMs, onChange, condTag, condIntervalMs, push})
    /// -> Promise<[{tag, error}]> (tags that do not parse; they are read as bad values)
    #[napi(ts_return_type = "Promise<any>")]
    pub fn subscribe<'e>(&self, env: &'e Env, p: SubParams) -> napi::Result<Object<'e>> {
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
        let generation = SUB_GEN.fetch_add(1, Ordering::Relaxed);
        let cfg = SubCfg {
            key: format!("{}#{generation}", p.sub),
            sub: p.sub.clone(),
            conn: p.conn.clone(),
            tags: Arc::new(p.tags.clone()),
            interval: ms(p.interval_ms, 1000),
            on_change: p.on_change.unwrap_or(true),
            cond: p.cond_tag.clone().filter(|s| !s.is_empty()),
            cond_interval: ms(p.cond_interval_ms, 100),
            push: p.push.unwrap_or(false),
        };
        let (data, s2, tx2) = (self.on_data.clone(), stop.clone(), tx.clone());
        std::thread::Builder::new().name(format!("s7sub-{}", p.sub)).spawn(move || sub_loop(cfg, tx2, s2, data)).map_err(napi_err)?;
        self.subs.lock().unwrap().insert(p.sub, SubHandle { conn: p.conn, stop });
        // check the tags on the connection thread (S7CommPlus symbols need the PLC)
        let (d, promise): (Deferred<BadTags>, Object) = env.create_deferred()?;
        let mut tags = p.tags;
        if let Some(c) = p.cond_tag.filter(|s| !s.is_empty()) {
            tags.push(c);
        }
        let job: Job = Box::new(move |dev| {
            let bad: Vec<(String, String)> = tags.iter().filter_map(|t| dev.check_tag(t).map(|e| (t.clone(), e))).collect();
            d.resolve(Box::new(move |_| Ok(BadTags(bad))))
        });
        tx.send(Msg::Job(job)).map_err(|_| napi_err("connection closed"))?;
        Ok(promise)
    }

    /// subscribeAlarms({conn, sub, initial, layout}): S7CommPlus alarm events
    /// as data events with `alarms` (JSON text {snapshot, alarms}; snapshot =
    /// a new subscription, first or after a reconnect, with the active alarms
    /// when `initial`). Stop with unsubscribe(sub).
    #[napi]
    pub fn subscribe_alarms(&self, p: AlarmParams) -> napi::Result<()> {
        if p.sub.is_empty() {
            return Err(napi_err("subscribeAlarms: sub is required"));
        }
        let tx = self.sender(&p.conn)?;
        self.unsubscribe(p.sub.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let generation = SUB_GEN.fetch_add(1, Ordering::Relaxed);
        let cfg = serde_json::json!({"initial": p.initial.unwrap_or(true), "layout": p.layout.clone().unwrap_or_else(|| "auto".into())});
        let (data, s2, sub, conn) = (self.on_data.clone(), stop.clone(), p.sub.clone(), p.conn.clone());
        let key = format!("{}#{generation}", p.sub);
        std::thread::Builder::new()
            .name(format!("s7alarm-{}", p.sub))
            .spawn(move || alarm_loop(sub, conn, key, cfg, tx, s2, data))
            .map_err(napi_err)?;
        self.subs.lock().unwrap().insert(p.sub, SubHandle { conn: p.conn, stop });
        Ok(())
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
    pub fn status<'e>(&self, env: &'e Env, conn: String) -> napi::Result<Object<'e>> {
        self.call_json(env, conn, "status".into(), None)
    }

    /// call(conn, action, argsJson?) -> Promise<string (JSON)>. Diagnostics and
    /// browsing: see device::Device::call for the actions of each protocol.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn call<'e>(&self, env: &'e Env, conn: String, action: String, args: Option<String>) -> napi::Result<Object<'e>> {
        self.call_json(env, conn, action, args)
    }

    fn call_json<'e>(&self, env: &'e Env, conn: String, action: String, args: Option<String>) -> napi::Result<Object<'e>> {
        let tx = self.sender(&conn)?;
        let args: serde_json::Value = match args.as_deref() {
            None | Some("") => serde_json::Value::Null,
            Some(s) => serde_json::from_str(s).map_err(|e| napi_err(format!("{action}: bad args: {e}")))?,
        };
        let (d, promise): (Deferred<Either<serde_json::Value, String>>, Object) = env.create_deferred()?;
        let job: Job = Box::new(move |dev| match dev.call(&action, &args) {
            Err(e) => d.reject(napi_err(format!("{action}: {e}"))),
            // status is used by the editor: hand an object; the other actions return JSON text
            Ok(v) if action == "status" => d.resolve(Box::new(move |_| Ok(Either::A(v)))),
            Ok(v) => d.resolve(Box::new(move |_| Ok(Either::B(v.to_string())))),
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
fn columns<'e, 'a>(env: &'e Env, outs: impl ExactSizeIterator<Item = &'a Out>) -> napi::Result<(Array<'e>, Array<'e>)> {
    let mut vals = env.create_array(outs.len() as u32)?;
    let mut errs = env.create_array(0)?;
    for (i, r) in outs.enumerate() {
        match (&r.val, &r.err) {
            (Some(v), None) => vals.set(i as u32, JsVal(v))?,
            (_, e) => {
                vals.set(i as u32, Null)?;
                errs.insert(err_pair(env, i, e.as_deref().unwrap_or("bad"))?)?;
            }
        }
    }
    Ok((vals, errs))
}

/// [i, message]
fn err_pair<'e>(env: &'e Env, i: usize, e: &str) -> napi::Result<Array<'e>> {
    let mut pair = env.create_array(2)?;
    pair.set(0, i as u32)?;
    pair.set(1, e)?;
    Ok(pair)
}

/// An S7 value as a JS value (lists become arrays).
struct JsVal<'a>(&'a Val);

impl ToNapiValue for JsVal<'_> {
    unsafe fn to_napi_value(env: sys::napi_env, v: Self) -> napi::Result<sys::napi_value> {
        unsafe {
            match v.0 {
                Val::Null => Null::to_napi_value(env, Null),
                Val::Bool(b) => bool::to_napi_value(env, *b),
                Val::Int(i) => i64::to_napi_value(env, *i),
                Val::Float(f) => f64::to_napi_value(env, *f),
                Val::Str(s) => <&str>::to_napi_value(env, s.as_str()),
                Val::List(l) => Vec::to_napi_value(env, l.iter().map(JsVal).collect::<Vec<_>>()),
            }
        }
    }
}

fn js_to_val(v: &Unknown<'_>) -> Result<Val, String> {
    let e = |e: napi::Error| e.reason;
    let text = |v: Unknown<'_>| -> Result<String, String> {
        Ok(v.coerce_to_string().map_err(e)?.into_utf8().map_err(e)?.as_str().map_err(e)?.to_owned())
    };
    match v.get_type().map_err(e)? {
        ValueType::Boolean => Ok(Val::Bool(v.coerce_to_bool().map_err(e)?)),
        ValueType::Number => {
            let f = v.coerce_to_number().map_err(e)?.get_double().map_err(e)?;
            if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                Ok(Val::Int(f as i64))
            } else {
                Ok(Val::Float(f))
            }
        }
        ValueType::String | ValueType::BigInt => Ok(Val::Str(text(*v)?)),
        ValueType::Object => {
            if v.is_array().map_err(e)? || v.is_buffer().map_err(e)? || v.is_typedarray().map_err(e)? {
                // checked above: an array-like object
                let o: Object<'_> = unsafe { v.cast() }.map_err(e)?;
                let n: u32 =
                    o.get_named_property::<Unknown>("length").map_err(e)?.coerce_to_number().map_err(e)?.get_uint32().map_err(e)?;
                let mut l = Vec::with_capacity(n as usize);
                for i in 0..n {
                    l.push(js_to_val(&o.get_element::<Unknown>(i).map_err(e)?)?);
                }
                Ok(Val::List(l))
            } else if v.is_date().map_err(e)? {
                let o: Object<'_> = unsafe { v.cast() }.map_err(e)?;
                let f: Function<'_, (), Unknown> = o.get_named_property("toISOString").map_err(e)?;
                Ok(Val::Str(text(f.apply(o, ()).map_err(e)?)?))
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

/// Generation counter of the subscriptions: a re-subscribe with the same id
/// gets a new PLC subscription key, so the cleanup of the old loop (which can
/// still run after the new one starts) never touches the new one.
static SUB_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct SubCfg {
    sub: String,
    /// Key of the PLC subscription in the device: the id and the generation.
    key: String,
    conn: String,
    tags: Arc<Vec<String>>,
    interval: Duration,
    on_change: bool,
    cond: Option<String>,
    cond_interval: Duration,
    push: bool,
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

/// How long one push poll holds the connection thread.
const PUSH_WAIT: Duration = Duration::from_millis(50);

/// Push mode: polls the PLC subscription on the connection thread. Returns
/// false when the PLC refuses the subscription (the caller then reads the
/// tags cyclically).
fn push_loop(c: &SubCfg, tx: &Sender<Msg>, stop: &AtomicBool, data: &DataFn) -> bool {
    let cycle_ms = c.interval.as_millis().min(u32::MAX as u128) as u32;
    while !stop.load(Ordering::SeqCst) {
        let start = Instant::now();
        let (rtx, rrx) = mpsc::sync_channel(1);
        let (key, tags) = (c.key.clone(), c.tags.clone());
        let job: Job = Box::new(move |dev| {
            let _ = rtx.send(dev.push_poll(&key, &tags, cycle_ms, PUSH_WAIT));
        });
        if tx.send(Msg::Job(job)).is_err() {
            return true;
        }
        let Ok(r) = rrx.recv() else { return true };
        match r {
            Ok(device::Push::Values(vals, ts)) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let ev = DataEv {
                    sub: c.sub.clone(),
                    conn: c.conn.clone(),
                    total: c.tags.len(),
                    vals,
                    ts: ts.unwrap_or_else(device::rfc3339_now),
                    ms: start.elapsed().as_secs_f64() * 1000.0,
                    alarms: None,
                };
                if data.call(ev, ThreadsafeFunctionCallMode::Blocking) != napi::Status::Ok {
                    break;
                }
            }
            Ok(device::Push::Idle) => {}
            Ok(device::Push::Down) => {
                // the connection thread reconnects (tick); wait here, not there
                let until = Instant::now() + Duration::from_millis(500);
                while !stop.load(Ordering::SeqCst) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            Err(_) => return false,
        }
    }
    let key = c.key.clone();
    let _ = tx.send(Msg::Job(Box::new(move |dev| dev.push_close(&key))));
    true
}

/// Polls a PLC alarm subscription on the connection thread and sends the
/// alarm events. When the PLC refuses the subscription it tries again every
/// 30 s (the device logs the reason).
fn alarm_loop(sub: String, conn: String, key: String, cfg: serde_json::Value, tx: Sender<Msg>, stop: Arc<AtomicBool>, data: DataFn) {
    let pause = |ms: u64| {
        let until = Instant::now() + Duration::from_millis(ms);
        while !stop.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    while !stop.load(Ordering::SeqCst) {
        let start = Instant::now();
        let (rtx, rrx) = mpsc::sync_channel(1);
        let (k, c) = (key.clone(), cfg.clone());
        if tx.send(Msg::Job(Box::new(move |dev| drop(rtx.send(dev.alarm_poll(&k, &c, PUSH_WAIT)))))).is_err() {
            return;
        }
        let Ok(r) = rrx.recv() else { return };
        match r {
            Ok(device::AlarmPush::Alarms(list, snapshot)) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let ev = DataEv {
                    sub: sub.clone(),
                    conn: conn.clone(),
                    total: 0,
                    vals: Vec::new(),
                    ts: device::rfc3339_now(),
                    ms: start.elapsed().as_secs_f64() * 1000.0,
                    alarms: Some(serde_json::json!({"snapshot": snapshot, "alarms": list}).to_string()),
                };
                if data.call(ev, ThreadsafeFunctionCallMode::Blocking) != napi::Status::Ok {
                    break;
                }
            }
            Ok(device::AlarmPush::Idle) => {}
            Ok(device::AlarmPush::Down) => pause(500),
            Err(_) => pause(30_000),
        }
    }
    let _ = tx.send(Msg::Job(Box::new(move |dev| dev.alarm_close(&key))));
}

fn sub_loop(c: SubCfg, tx: Sender<Msg>, stop: Arc<AtomicBool>, data: DataFn) {
    if c.push && push_loop(&c, &tx, &stop, &data) {
        return;
    }
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
                alarms: None,
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
    /// Motor_On/Speed…); returns "127.0.0.1:port". `fw4`: behave like an
    /// S7-1500 FW V4.x (subscriptions need a cycle time, FW V4 vartype flags).
    #[napi]
    pub fn start_fake_s7plus(tls: Option<bool>, fw4: Option<bool>) -> napi::Result<String> {
        let f = s7plus::fake::FakePlc::start().map_err(crate::napi_err)?;
        if tls == Some(true) {
            f.enable_tls().map_err(crate::napi_err)?;
        }
        if fw4 == Some(true) {
            let mut st = f.lock();
            st.model.use_fw4_flags();
            st.model.sub_needs_cycle = true;
            st.model.alarm_needs_reference = true;
            st.paom = "1;6ES7 SIM-01500-APLC;S4.1".into();
        }
        let a = f.addr();
        KEEP.lock().unwrap().push(Box::new(f));
        Ok(a)
    }

    /// The TLS certificate (PEM) of the fake S7CommPlus PLC at `addr`.
    #[napi]
    pub fn fake_s7plus_certificate(addr: String) -> Option<String> {
        let keep = KEEP.lock().unwrap();
        let f = keep.iter().filter_map(|b| b.downcast_ref::<s7plus::fake::FakePlc>()).find(|f| f.addr() == addr)?;
        f.tls_cert().map(|d| s7plus::x509::pem(&d))
    }

    /// Closes every client connection of the fake S7CommPlus PLC at `addr`.
    #[napi]
    pub fn fake_s7plus_drop_connections(addr: String) {
        let keep = KEEP.lock().unwrap();
        if let Some(f) = keep.iter().filter_map(|b| b.downcast_ref::<s7plus::fake::FakePlc>()).find(|f| f.addr() == addr) {
            f.drop_connections();
        }
    }

    /// Raises (coming) or clears (going) an alarm on the fake S7CommPlus PLC at
    /// `addr`: id as hex text, associated values as byte arrays; `ack_required`:
    /// an alarm class with acknowledgement.
    #[napi]
    pub fn fake_s7plus_raise_alarm(addr: String, id: String, coming: bool, values: Option<Vec<Vec<u8>>>, ack_required: Option<bool>) {
        let id = u64::from_str_radix(id.trim_start_matches("0x"), 16).unwrap_or(0);
        let keep = KEEP.lock().unwrap();
        if let Some(f) = keep.iter().filter_map(|b| b.downcast_ref::<s7plus::fake::FakePlc>()).find(|f| f.addr() == addr) {
            f.raise_alarm_ack(id, coming, values.unwrap_or_default(), ack_required.unwrap_or(false));
        }
    }

    /// Number of live subscriptions of the fake S7CommPlus PLC at `addr`.
    #[napi]
    pub fn fake_s7plus_subscriptions(addr: String) -> u32 {
        let keep = KEEP.lock().unwrap();
        keep.iter()
            .filter_map(|b| b.downcast_ref::<s7plus::fake::FakePlc>())
            .find(|f| f.addr() == addr)
            .map_or(0, |f| f.subscriptions() as u32)
    }
}
