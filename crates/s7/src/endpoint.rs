//! Endpoint: one device per
//! endpoint with a tag cache, reads with gap spanning that return JSON-friendly
//! values, writes from text / JSON-ish values, a connection supervisor with
//! state callbacks, and subscriptions (a thread per subscription that reads
//! every interval and reports the values that changed). This is the API the
//! Node-API addon uses.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::addr::{parse_tag, Tag};
use crate::client::{Client, State};
use crate::cpu::{parse_family, parse_tsap, Family};
use crate::error::{local, Error, Result};
use crate::log::Log;
use crate::metrics::ms;
use crate::poll::Quality;
use crate::value::Val;
use crate::wire::{DialOptions, Item};

/// Parameters of `Endpoint::open`.
#[derive(Debug, Clone, Default)]
pub struct EndpointConfig {
    pub host: String,
    /// 0 -> 102
    pub port: u16,
    /// CPU family ("1200", "s7-300", "logo"…); empty = none (link type 0x03 unless given)
    pub cpu: String,
    pub rack: Option<u8>,
    pub slot: Option<u8>,
    /// 1 PG, 2 OP, 3 S7-Basic/other
    pub link: Option<u8>,
    /// hex TSAP ("4D57", "10.00"); empty = default
    pub local_tsap: String,
    pub remote_tsap: String,
    /// proposed PDU (0 -> 960)
    pub pdu: u16,
    /// zero -> 5 s
    pub timeout: Duration,
    /// tries per request (0 -> 3)
    pub attempts: usize,
    /// gap spanning in bytes; None -> 5; -1 = no merge
    pub gap: Option<i64>,
    pub little_endian: bool,
    /// local IP to bind (NIC selection)
    pub local_addr: String,
}

/// Endpoint parameters after the family presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInfo {
    pub addr: String,
    pub rack: u8,
    pub slot: u8,
    pub link: u8,
}

/// One tag of a read.
#[derive(Debug, Clone, PartialEq)]
pub struct TagValue {
    pub tag: String,
    pub value: Option<Val>,
    pub quality: Quality,
    pub error: Option<String>,
    pub raw: Vec<u8>,
}

/// Connection summary.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusInfo {
    pub conn: String,
    pub addr: String,
    pub state: State,
    pub detail: String,
    pub pdu: u16,
    pub reconnects: u64,
    pub requests: u64,
    pub errors: u64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub rate_1m: f64,
}

/// State-change hook: (endpoint id, state, detail).
pub type OnStatusFn = Arc<dyn Fn(&str, State, &str) + Send + Sync>;

/// A device endpoint. `Send + Sync`; share it with `Arc`.
pub struct Endpoint {
    pub id: String,
    pub client: Client,
    pub family: Option<Family>,
    pub little: bool,
    pub gap: i64,
    pub addr: String,
    tags: Mutex<HashMap<String, Arc<Tag>>>,
    last: Mutex<Option<(State, String)>>,
    on_status: Mutex<Option<OnStatusFn>>,
}

/// A stop flag threads can sleep on.
#[derive(Default)]
struct Stop {
    stopped: Mutex<bool>,
    cv: Condvar,
}

impl Stop {
    fn stop(&self) {
        *self.stopped.lock().unwrap() = true;
        self.cv.notify_all();
    }
    /// Sleeps up to `d`; true if stopped.
    fn sleep(&self, d: Duration) -> bool {
        let g = self.stopped.lock().unwrap();
        if *g {
            return true;
        }
        let (g, _) = self.cv.wait_timeout(g, d).unwrap();
        *g
    }
    fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap()
    }
}

/// Builds the dial options of an endpoint config (family presets applied).
pub fn dial_options(cfg: &EndpointConfig) -> Result<(DialOptions, Option<Family>)> {
    if cfg.host.is_empty() {
        return Err(local("open: host is required"));
    }
    let port = if cfg.port == 0 { 102 } else { cfg.port };
    let addr = if cfg.host.contains(':') && !cfg.host.starts_with('[') {
        format!("[{}]:{port}", cfg.host)
    } else {
        format!("{}:{port}", cfg.host)
    };
    let mut o = DialOptions { addr, max_pdu: cfg.pdu, timeout: cfg.timeout, local_addr: cfg.local_addr.clone(), ..Default::default() };
    if let Some(r) = cfg.rack {
        o.rack = r;
    }
    if let Some(s) = cfg.slot {
        o.slot = s;
    }
    if let Some(l) = cfg.link {
        o.link_type = l;
    }
    o.local_tsap = parse_tsap(&cfg.local_tsap).map_err(|e| e.context("localTsap"))?;
    o.remote_tsap = parse_tsap(&cfg.remote_tsap).map_err(|e| e.context("remoteTsap"))?;
    let mut fam = None;
    if !cfg.cpu.is_empty() {
        let f = parse_family(&cfg.cpu)?;
        f.apply(&mut o, cfg.rack.is_some(), cfg.slot.is_some(), cfg.link.is_some());
        fam = Some(f);
    } else if cfg.link.is_none() {
        o.link_type = 0x03;
    }
    Ok((o, fam))
}

impl Endpoint {
    /// Creates an endpoint (does not connect: the first request or `supervise` does).
    pub fn open(id: &str, cfg: &EndpointConfig, log: &Log) -> Result<(Endpoint, OpenInfo)> {
        let (o, fam) = dial_options(cfg)?;
        let info = OpenInfo { addr: o.addr.clone(), rack: o.rack, slot: o.slot, link: o.link_type };
        let client = Client::new(o).with_attempts(cfg.attempts).with_log(log.with("conn", id));
        Ok((Endpoint::new(id, client, fam, cfg.gap.unwrap_or(5), cfg.little_endian), info))
    }

    /// Wraps an existing client.
    pub fn new(id: &str, client: Client, family: Option<Family>, gap: i64, little: bool) -> Endpoint {
        let addr = client.opts().addr.clone();
        Endpoint {
            id: id.to_string(),
            client,
            family,
            little,
            gap,
            addr,
            tags: Mutex::new(HashMap::new()),
            last: Mutex::new(None),
            on_status: Mutex::new(None),
        }
    }

    /// Sets the state-change hook.
    pub fn set_on_status(&self, f: Option<OnStatusFn>) {
        *self.on_status.lock().unwrap() = f;
    }

    /// Parses an address once (cached) and checks it against the CPU family.
    pub fn tag(&self, addr: &str) -> Result<Tag> {
        let mut tags = self.tags.lock().unwrap();
        self.lookup(&mut tags, addr).map(|t| (*t).clone())
    }

    fn lookup(&self, cache: &mut HashMap<String, Arc<Tag>>, addr: &str) -> Result<Arc<Tag>> {
        if let Some(t) = cache.get(addr) {
            return Ok(t.clone());
        }
        let mut t = parse_tag(addr.trim())?;
        t.little_endian = t.little_endian || self.little;
        if let Some(f) = self.family {
            f.check(&t)?;
        }
        let t = Arc::new(t);
        cache.insert(addr.to_string(), t.clone());
        Ok(t)
    }

    /// Looks up many addresses under one lock (hot path: no Tag copies).
    fn tags_of<'a>(&self, addrs: impl Iterator<Item = &'a str>) -> Vec<Result<Arc<Tag>>> {
        let mut cache = self.tags.lock().unwrap();
        addrs.map(|a| self.lookup(&mut cache, a)).collect()
    }

    /// Reports a state change to the hook, if any.
    pub fn publish_state(&self) {
        let (st, det) = self.client.status();
        let changed = {
            let mut last = self.last.lock().unwrap();
            let ch = last.as_ref().is_none_or(|(s, d)| *s != st || *d != det);
            *last = Some((st, det.clone()));
            ch
        };
        if changed {
            let hook = self.on_status.lock().unwrap().clone();
            if let Some(f) = hook {
                f(&self.id, st, &det);
            }
        }
    }

    /// Reads all tags in as few requests as the PDU allows. Bad addresses get a
    /// bad value and do not stop the others. On a transport error every tag is
    /// bad and the error is returned too.
    pub fn read_tags_detailed(&self, addrs: &[String]) -> (Vec<TagValue>, Option<Error>) {
        self.read_with(
            addrs,
            |a| TagValue { tag: a.clone(), value: None, quality: Quality::Uncertain, error: None, raw: Vec::new() },
            |o, r| match r {
                Ok((v, raw)) => {
                    o.value = Some(v);
                    o.quality = Quality::Good;
                    o.raw = raw;
                }
                Err(e) => {
                    o.quality = Quality::Bad;
                    o.error = Some(e);
                }
            },
        )
    }

    /// The read core with a caller-chosen result type (the Node-API layer
    /// uses it to skip the per-tag copies of TagValue): init(addr) builds each
    /// result, set(result, Ok((value, raw)) | Err(text)) fills it.
    pub fn read_with<T>(
        &self,
        addrs: &[String],
        init: impl FnMut(&String) -> T,
        mut set: impl FnMut(&mut T, std::result::Result<(Val, Vec<u8>), String>),
    ) -> (Vec<T>, Option<Error>) {
        let mut out: Vec<T> = addrs.iter().map(init).collect();
        let mut items = Vec::with_capacity(addrs.len());
        let mut idx = Vec::with_capacity(addrs.len());
        let mut tags = Vec::with_capacity(addrs.len());
        for (i, r) in self.tags_of(addrs.iter().map(|a| a.as_str())).into_iter().enumerate() {
            match r {
                Ok(t) => {
                    items.push(t.item);
                    idx.push(i);
                    tags.push(t);
                }
                Err(e) => set(&mut out[i], Err(e.to_string())),
            }
        }
        if items.is_empty() {
            return (out, None);
        }
        let gap = self.gap;
        let res = self.client.run(|cn| if gap < 0 { cn.read(&items) } else { cn.read_optimized(&items, gap) });
        self.publish_state();
        let res = match res {
            Err(e) => {
                let txt = e.to_string();
                for &i in &idx {
                    set(&mut out[i], Err(txt.clone()));
                }
                return (out, Some(e));
            }
            Ok(r) => r,
        };
        for (k, r) in res.into_iter().enumerate() {
            let o = &mut out[idx[k]];
            match r {
                Err(e) => set(o, Err(e.to_string())),
                Ok(d) => match tags[k].json_value(&d) {
                    Err(e) => set(o, Err(e.to_string())),
                    Ok(v) => set(o, Ok((v, d))),
                },
            }
        }
        (out, None)
    }

    /// `read_tags_detailed` with the transport error as `Err`.
    pub fn read_tags(&self, addrs: &[String]) -> Result<Vec<TagValue>> {
        match self.read_tags_detailed(addrs) {
            (_, Some(e)) => Err(e),
            (v, None) => Ok(v),
        }
    }

    /// Writes all items in as few WriteVar requests as possible. Each item is
    /// (address, value text or a conversion error). Per-item errors as text;
    /// `Err` = transport failure.
    pub fn write(&self, items: &[(String, std::result::Result<String, String>)]) -> Result<Vec<Option<String>>> {
        self.write_with(
            items.len(),
            |i| &items[i].0,
            |i, t| match &items[i].1 {
                Ok(text) => t.value(text).map_err(|e| e.to_string()),
                Err(e) => Err(format!("{}: {e}", items[i].0)),
            },
        )
    }

    /// Shared write core: addr(i) and encode(i, tag) -> bytes or error text.
    fn write_with<'a>(
        &self,
        n: usize,
        addr: impl Fn(usize) -> &'a String,
        encode: impl Fn(usize, &Tag) -> std::result::Result<Vec<u8>, String>,
    ) -> Result<Vec<Option<String>>> {
        let mut out = vec![None; n];
        let (mut wi, mut wd, mut idx): (Vec<Item>, Vec<Vec<u8>>, Vec<usize>) =
            (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        let tags = self.tags_of((0..n).map(|i| addr(i).as_str()));
        for (i, t) in tags.into_iter().enumerate() {
            let t = match t {
                Ok(t) => t,
                Err(e) => {
                    out[i] = Some(e.to_string());
                    continue;
                }
            };
            match encode(i, &t) {
                Ok(b) => {
                    wi.push(t.item);
                    wd.push(b);
                    idx.push(i);
                }
                Err(e) => out[i] = Some(e),
            }
        }
        if !wi.is_empty() {
            let r = self.client.run(|cn| cn.write(&wi, &wd));
            self.publish_state();
            for (k, e) in r?.into_iter().enumerate() {
                if let Some(e) = e {
                    out[idx[k]] = Some(e.to_string());
                }
            }
        }
        Ok(out)
    }

    /// `write` from JSON-ish values.
    pub fn write_values(&self, items: &[(String, Val)]) -> Result<Vec<Option<String>>> {
        self.write_with(items.len(), |i| &items[i].0, |i, t| t.value_from(&items[i].1).map_err(|e| e.to_string()))
    }

    /// Connection summary.
    pub fn status(&self) -> StatusInfo {
        let (st, det) = self.client.status();
        let s = self.client.stats().snapshot();
        StatusInfo {
            conn: self.id.clone(),
            addr: self.addr.clone(),
            state: st,
            detail: det,
            pdu: self.client.max_pdu(),
            reconnects: self.client.reconnects(),
            requests: s.requests,
            errors: s.errors,
            p50_ms: ms(s.p50),
            p95_ms: ms(s.p95),
            p99_ms: ms(s.p99),
            rate_1m: s.rate_1m,
        }
    }

    /// Connects at once, reconnects every `every` while the link is down, and
    /// reports each state change (checked every 250 ms). Stops with the handle.
    pub fn supervise(self: &Arc<Self>, every: Duration) -> Supervisor {
        let every = if every.is_zero() { Duration::from_secs(5) } else { every };
        let stop = Arc::new(Stop::default());
        let ep = self.clone();
        let st = stop.clone();
        let th = std::thread::Builder::new()
            .name(format!("s7-supervise-{}", self.id))
            .spawn(move || {
                let connect = || {
                    let _ = ep.client.run(|_| Ok(()));
                    ep.publish_state();
                };
                connect();
                let mut last_try = Instant::now();
                while !st.sleep(Duration::from_millis(250)) {
                    ep.publish_state();
                    if ep.client.status().0 != State::Connected && last_try.elapsed() >= every {
                        last_try = Instant::now();
                        connect();
                    }
                }
            })
            .expect("spawn supervisor");
        Supervisor { stop, th: Some(th) }
    }

    /// Closes the connection and reports "disconnected".
    pub fn close(&self) {
        self.client.close();
        self.publish_state();
    }
}

/// Stops the supervisor thread when dropped.
pub struct Supervisor {
    stop: Arc<Stop>,
    th: Option<JoinHandle<()>>,
}

impl Supervisor {
    pub fn stop(mut self) {
        self.stop_inner();
    }
    fn stop_inner(&mut self) {
        self.stop.stop();
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// One cycle of a subscription.
#[derive(Debug, Clone)]
pub struct DataEvent {
    pub sub: String,
    pub conn: String,
    pub values: Vec<TagValue>,
    /// start of the cycle
    pub ts: SystemTime,
    /// cycle duration in ms
    pub ms: f64,
}

/// Subscription sink.
pub type SinkFn = Arc<dyn Fn(DataEvent) + Send + Sync>;

/// A running subscription: reads the tags every interval (the next cycle starts
/// one interval after the start of the last one; a slow PLC delays it, it never
/// overlaps) and sends the values that changed, or all of them when
/// `on_change` is false. Stops when dropped.
pub struct Subscription {
    pub id: String,
    stop: Arc<Stop>,
    th: Option<JoinHandle<()>>,
}

impl Subscription {
    /// Starts a subscription; returns it and the tags that do not parse (they
    /// are read as bad values). `interval` under 10 ms -> 1 s.
    pub fn start(
        ep: Arc<Endpoint>,
        id: &str,
        tags: Vec<String>,
        interval: Duration,
        on_change: bool,
        sink: SinkFn,
    ) -> Result<(Subscription, Vec<(String, String)>)> {
        if id.is_empty() || tags.is_empty() {
            return Err(local("subscribe: sub and tags are required"));
        }
        let bad: Vec<(String, String)> = tags.iter().filter_map(|a| ep.tag(a).err().map(|e| (a.clone(), e.to_string()))).collect();
        let iv = if interval < Duration::from_millis(10) { Duration::from_secs(1) } else { interval };
        let stop = Arc::new(Stop::default());
        let st = stop.clone();
        let sid = id.to_string();
        let th = std::thread::Builder::new()
            .name(format!("s7-sub-{id}"))
            .spawn(move || {
                let mut prev: HashMap<String, TagValue> = HashMap::new();
                let mut next = Instant::now();
                loop {
                    let start = Instant::now();
                    let ts = SystemTime::now();
                    let (vals, _) = ep.read_tags_detailed(&tags);
                    let mut send = Vec::new();
                    for v in vals {
                        let changed = match prev.get(&v.tag) {
                            None => true,
                            Some(old) => old.quality != v.quality || old.error != v.error || old.raw != v.raw,
                        };
                        if !on_change || changed {
                            send.push(v.clone());
                        }
                        prev.insert(v.tag.clone(), v);
                    }
                    if !send.is_empty() && !st.is_stopped() {
                        sink(DataEvent { sub: sid.clone(), conn: ep.id.clone(), values: send, ts, ms: ms(start.elapsed()) });
                    }
                    // ticker semantics: next tick at a multiple of the interval
                    next += iv;
                    let now = Instant::now();
                    while next <= now {
                        next += iv;
                    }
                    if st.sleep(next - now) {
                        return;
                    }
                }
            })
            .expect("spawn subscription");
        Ok((Subscription { id: id.to_string(), stop, th: Some(th) }, bad))
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.stop();
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop_inner();
    }
}
