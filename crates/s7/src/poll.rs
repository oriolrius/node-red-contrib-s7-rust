//! Scan engine: scan groups (fixed, conditional and on-demand rates), a write
//! queue with write priority, optimistic writes, read-after-write, N parallel
//! connections, per-group metrics and update callbacks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use crate::addr::{parse_tag, Tag};
use crate::client::Client;
use crate::cpu::Family;
use crate::error::{local, Error, Result};
use crate::log::Log;
use crate::metrics::{ms, Snapshot, Stats};
use crate::wire::{DialOptions, Item};

/// Scan modes (connector "Rate mode").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Mode {
    /// fixed rate
    #[default]
    Fixed,
    /// the rate switches to `cond_rate` while `cond_tag` is true
    Conditional,
    /// fast rate only while a client watches the group (`Poller::watch`)
    OnDemand,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Fixed => "fixed",
            Mode::Conditional => "conditional",
            Mode::OnDemand => "ondemand",
        }
    }

    /// "fixed" (or ""), "conditional", "ondemand".
    pub fn parse(s: &str) -> Result<Mode> {
        match s.trim().to_lowercase().as_str() {
            "" | "fixed" => Ok(Mode::Fixed),
            "conditional" => Ok(Mode::Conditional),
            "ondemand" | "on-demand" => Ok(Mode::OnDemand),
            _ => Err(local(format!("unknown scan mode {s:?} (fixed, conditional, ondemand)"))),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of tags read together at one rate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanGroup {
    pub name: String,
    /// zero = never
    pub rate: Duration,
    pub tags: Vec<String>,
    pub mode: Mode,
    /// conditional: rate switches while true
    pub cond_tag: String,
    /// conditional: rate while true
    pub cond_rate: Duration,
    /// conditional: one immediate read when it turns true
    pub cond_one_shot: bool,
    /// on-demand: rate when nobody watches (zero = stop)
    pub idle_rate: Duration,
    /// gap spanning in bytes (-1 = no merge)
    pub max_gap: i64,
}

/// Tag value quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Quality {
    Good,
    Bad,
    #[default]
    Uncertain,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Good => "good",
            Quality::Bad => "bad",
            Quality::Uncertain => "uncertain",
        }
    }
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The current state of one tag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PollValue {
    pub tag: String,
    /// text form (`Tag::format`)
    pub value: String,
    pub quality: Quality,
    pub error: String,
    /// time of the read (None = never read)
    pub time: Option<SystemTime>,
    /// optimistic write not yet confirmed
    pub pending: bool,
    /// bytes shown (after optimistic overlay); `Tag::json_value(raw)` gives the typed value
    pub raw: Vec<u8>,
}

/// Update hook: called (outside locks) with every new tag value.
pub type OnUpdateFn = Arc<dyn Fn(&Tag, &PollValue) + Send + Sync>;

/// Configures a Poller.
#[derive(Clone, Default)]
pub struct PollConfig {
    pub dial: DialOptions,
    pub family: Option<Family>,
    /// parallel connections (in-flight requests); 0 -> 1
    pub concurrency: usize,
    /// writes executed per read when both are queued (0 -> 1)
    pub write_priority: usize,
    /// read the tag back after each write
    pub read_after_write: bool,
    /// show the written value at once
    pub optimistic_write: bool,
    /// revert if the device does not confirm (zero -> 5 s)
    pub optimistic_timeout: Duration,
    pub little_endian: bool,
    pub groups: Vec<ScanGroup>,
    /// attempts per request of each client (0 -> 3)
    pub attempts: usize,
    pub log: Log,
    pub on_update: Option<OnUpdateFn>,
}

impl fmt::Debug for PollConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollConfig")
            .field("dial", &self.dial)
            .field("family", &self.family)
            .field("concurrency", &self.concurrency)
            .field("write_priority", &self.write_priority)
            .field("read_after_write", &self.read_after_write)
            .field("optimistic_write", &self.optimistic_write)
            .field("optimistic_timeout", &self.optimistic_timeout)
            .field("little_endian", &self.little_endian)
            .field("groups", &self.groups)
            .finish()
    }
}

/// Per-scan-group diagnostics of the connector model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupMetrics {
    pub name: String,
    pub target_interval_ms: f64,
    pub achieved_interval_ms: f64,
    pub queue_wait_ms: f64,
    /// 100 × queue wait / achieved interval
    pub overload_pct: f64,
    /// requests needed per scan
    pub request_count: usize,
    pub monitored_points: usize,
    pub cycles: u64,
    pub response: Snapshot,
}

struct Group {
    cfg: ScanGroup,
    tags: Vec<Tag>,
    stats: Stats,
}

#[derive(Default)]
struct GroupRt {
    busy: bool,
    last_done: Option<Instant>,
    achieved: Duration,
    queue_wait: Duration,
    interest: Option<Instant>, // on-demand: last watch
    cond_on: bool,
    target: Duration,
    cycles: u64,
}

struct Optimistic {
    raw: Vec<u8>,
    until: Instant,
}

struct Shared {
    values: HashMap<String, PollValue>,
    tags: HashMap<String, Tag>,
    opt: HashMap<String, Optimistic>,
    rt: Vec<GroupRt>,
    on_update: Option<OnUpdateFn>,
}

enum Job {
    Read { g: usize, enqueued: Instant },
    Write { tag: Tag, val: Vec<u8>, done: mpsc::Sender<Result<()>> },
}

#[derive(Default)]
struct Queue {
    reads: VecDeque<Job>,
    writes: VecDeque<Job>,
    stopped: bool,
    running: bool,
}

/// Runs scan groups against one device. Create it, wrap it in `Arc`, `start`.
pub struct Poller {
    cfg: PollConfig,
    clients: Vec<Arc<Client>>,
    groups: Vec<Group>,
    shared: Mutex<Shared>,
    queue: Mutex<Queue>,
    work_cv: Condvar,
    stop_cv: Condvar,
    log: Log,
}

fn not_read(text: &str) -> PollValue {
    PollValue { tag: text.to_string(), quality: Quality::Uncertain, error: "not read yet".into(), ..Default::default() }
}

impl Poller {
    /// Parses all tags and prepares the clients.
    pub fn new(cfg: PollConfig) -> Result<Poller> {
        let mut cfg = cfg;
        if cfg.concurrency == 0 {
            cfg.concurrency = 1;
        }
        if cfg.write_priority == 0 {
            cfg.write_priority = 1;
        }
        if cfg.optimistic_timeout.is_zero() {
            cfg.optimistic_timeout = Duration::from_secs(5);
        }
        let mut values = HashMap::new();
        let mut tags = HashMap::new();
        let mut groups = Vec::new();
        for g in &cfg.groups {
            let mut gt = Vec::new();
            for a in &g.tags {
                let mut t = parse_tag(a)?;
                t.little_endian = t.little_endian || cfg.little_endian;
                if let Some(f) = cfg.family {
                    f.check(&t)?;
                }
                tags.insert(t.text.clone(), t.clone());
                values.insert(t.text.clone(), not_read(&t.text));
                gt.push(t);
            }
            groups.push(Group { cfg: g.clone(), tags: gt, stats: Stats::new() });
        }
        // A condition tag must be watched faster than its slow group: if no
        // fixed-rate group reads it, add an internal group at the fast rate.
        for g in &cfg.groups {
            if g.mode != Mode::Conditional || g.cond_tag.is_empty() {
                continue;
            }
            let watched = groups.iter().any(|o| o.cfg.mode != Mode::Conditional && o.tags.iter().any(|t| t.text == g.cond_tag));
            if watched {
                continue;
            }
            let t = parse_tag(&g.cond_tag).map_err(|e| e.context("cond_tag"))?;
            tags.insert(t.text.clone(), t.clone());
            values.insert(t.text.clone(), not_read(&t.text));
            let mut rate = g.cond_rate;
            if rate.is_zero() || (!g.rate.is_zero() && g.rate < rate) {
                rate = g.rate;
            }
            let cfg = ScanGroup { name: format!("{}.condition", g.name), rate, mode: Mode::Fixed, ..Default::default() };
            groups.push(Group { cfg, tags: vec![t], stats: Stats::new() });
        }
        let log = cfg.log.clone();
        let clients = (0..cfg.concurrency)
            .map(|i| Arc::new(Client::new(cfg.dial.clone()).with_attempts(cfg.attempts).with_log(log.with("conn", i))))
            .collect();
        let rt = groups.iter().map(|_| GroupRt::default()).collect();
        let on_update = cfg.on_update.clone();
        Ok(Poller {
            cfg,
            clients,
            groups,
            shared: Mutex::new(Shared { values, tags, opt: HashMap::new(), rt, on_update }),
            queue: Mutex::new(Queue::default()),
            work_cv: Condvar::new(),
            stop_cv: Condvar::new(),
            log,
        })
    }

    fn sh(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn q(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Starts one worker thread per connection and one scheduler thread per
    /// group. Polling runs until `PollerHandle::stop` (or the handle is dropped).
    pub fn start(self: &Arc<Self>) -> PollerHandle {
        {
            let mut q = self.q();
            q.stopped = false;
            q.running = true;
        }
        let mut threads = Vec::new();
        for (i, c) in self.clients.iter().enumerate() {
            let p = self.clone();
            let c = c.clone();
            threads.push(std::thread::Builder::new().name(format!("s7-poll-conn{i}")).spawn(move || p.worker(&c)).expect("spawn worker"));
        }
        for gi in 0..self.groups.len() {
            let p = self.clone();
            threads.push(
                std::thread::Builder::new().name(format!("s7-poll-group{gi}")).spawn(move || p.scheduler(gi)).expect("spawn scheduler"),
            );
        }
        PollerHandle { p: Some(self.clone()), threads }
    }

    fn shutdown(&self) {
        let pending: Vec<Job> = {
            let mut q = self.q();
            q.stopped = true;
            q.running = false;
            let mut v: Vec<Job> = q.writes.drain(..).collect();
            v.extend(q.reads.drain(..));
            v
        };
        self.work_cv.notify_all();
        self.stop_cv.notify_all();
        for j in pending {
            if let Job::Write { done, .. } = j {
                let _ = done.send(Err(local("poller stopped")));
            }
        }
        for c in &self.clients {
            c.close();
        }
        for rt in self.sh().rt.iter_mut() {
            rt.busy = false;
        }
    }

    /// The rate the group's mode gives now.
    fn effective_rate(&self, gi: usize) -> Duration {
        let g = &self.groups[gi].cfg;
        match g.mode {
            Mode::Conditional => {
                if !g.cond_tag.is_empty() && self.truthy(&g.cond_tag) {
                    return g.cond_rate;
                }
            }
            Mode::OnDemand => {
                let watched = self.sh().rt[gi].interest.is_some_and(|t| t.elapsed() < Duration::from_secs(10));
                if !watched {
                    return g.idle_rate;
                }
            }
            Mode::Fixed => {}
        }
        g.rate
    }

    fn truthy(&self, tag: &str) -> bool {
        let sh = self.sh();
        let Some(v) = sh.values.get(tag) else { return false };
        if v.quality != Quality::Good {
            return false;
        }
        let Some(first) = v.value.split_whitespace().next() else { return false };
        if first == "true" {
            return true;
        }
        crate::textfmt::parse_float(first, 64).is_ok_and(|f| f != 0.0)
    }

    fn scheduler(&self, gi: usize) {
        let g = &self.groups[gi].cfg;
        let mut last: Option<Instant> = None; // last time a scan was queued
        loop {
            let rate = self.effective_rate(gi);
            let one_shot = {
                let mut sh = self.sh();
                let rt = &mut sh.rt[gi];
                rt.target = rate;
                let on = g.mode == Mode::Conditional && rate == g.cond_rate && g.cond_rate != g.rate;
                let one = on && !rt.cond_on && g.cond_one_shot;
                rt.cond_on = on;
                one
            };
            let mut wait = Duration::from_millis(100); // re-evaluate idle/conditional groups
            if !rate.is_zero() {
                // The next scan follows from the last one and the CURRENT rate, so a
                // rate change (conditional, on-demand) takes effect at once.
                let now = Instant::now();
                let mut due = last.map(|l| l + rate);
                if one_shot || due.is_none_or(|d| now >= d) {
                    self.enqueue_read(gi);
                    let now = Instant::now();
                    last = Some(now);
                    due = Some(now + rate);
                }
                let d = due.unwrap().saturating_duration_since(Instant::now());
                if d < wait {
                    wait = d;
                }
            }
            let q = self.q();
            if q.stopped {
                return;
            }
            let (q, _) = self.stop_cv.wait_timeout(q, wait).unwrap_or_else(|p| p.into_inner());
            if q.stopped {
                return;
            }
        }
    }

    fn enqueue_read(&self, gi: usize) {
        {
            let mut sh = self.sh();
            if sh.rt[gi].busy {
                return; // previous scan still queued or running: skip (device overload)
            }
            sh.rt[gi].busy = true;
        }
        let mut q = self.q();
        if q.stopped {
            drop(q);
            self.sh().rt[gi].busy = false;
            return;
        }
        q.reads.push_back(Job::Read { g: gi, enqueued: Instant::now() });
        drop(q);
        self.work_cv.notify_one();
    }

    /// Executes jobs on one connection, giving writes priority: up to
    /// `write_priority` writes per read when both are queued.
    fn worker(&self, c: &Client) {
        let wp = self.cfg.write_priority;
        let mut writes_in_row = 0;
        loop {
            let job = {
                let mut q = self.q();
                loop {
                    if q.stopped {
                        return;
                    }
                    if writes_in_row < wp {
                        if let Some(j) = q.writes.pop_front() {
                            writes_in_row += 1;
                            break j;
                        }
                        if let Some(j) = q.reads.pop_front() {
                            writes_in_row = 0;
                            break j;
                        }
                    } else {
                        if let Some(j) = q.reads.pop_front() {
                            writes_in_row = 0;
                            break j;
                        }
                        if let Some(j) = q.writes.pop_front() {
                            writes_in_row = 1;
                            break j;
                        }
                    }
                    q = self.work_cv.wait(q).unwrap_or_else(|p| p.into_inner());
                }
            };
            match job {
                Job::Read { g, enqueued } => self.do_read(c, g, enqueued),
                Job::Write { tag, val, done } => {
                    let r = self.do_write(c, &tag, &val);
                    let _ = done.send(r);
                }
            }
        }
    }

    fn do_read(&self, c: &Client, gi: usize, enqueued: Instant) {
        let g = &self.groups[gi];
        let start = Instant::now();
        let items: Vec<Item> = g.tags.iter().map(|t| t.item).collect();
        let max_gap = g.cfg.max_gap;
        let res = c.run(|cn| if max_gap < 0 { cn.read(&items) } else { cn.read_optimized(&items, max_gap) });
        g.stats.observe(start.elapsed(), res.is_err());
        let now = Instant::now();
        let wall = SystemTime::now();
        let (updates, hook) = {
            let mut sh = self.sh();
            let rt = &mut sh.rt[gi];
            rt.busy = false;
            rt.queue_wait = start.saturating_duration_since(enqueued);
            if let Some(ld) = rt.last_done {
                rt.achieved = now.saturating_duration_since(ld);
            }
            rt.last_done = Some(now);
            rt.cycles += 1;
            let mut updates = Vec::with_capacity(g.tags.len());
            for (i, t) in g.tags.iter().enumerate() {
                let mut v = PollValue { tag: t.text.clone(), time: Some(wall), ..Default::default() };
                match &res {
                    Err(e) => {
                        v.quality = Quality::Bad;
                        v.error = e.to_string();
                    }
                    Ok(r) => match &r[i] {
                        Err(e) => {
                            v.quality = Quality::Bad;
                            v.error = e.to_string();
                        }
                        Ok(d) => self.apply_optimistic(&mut sh, t, d, &mut v, now),
                    },
                }
                sh.values.insert(t.text.clone(), v.clone());
                updates.push(v);
            }
            (updates, sh.on_update.clone())
        };
        if let Some(f) = hook {
            for (i, v) in updates.iter().enumerate() {
                f(&g.tags[i], v);
            }
        }
    }

    /// Decodes data and handles a pending optimistic write.
    fn apply_optimistic(&self, sh: &mut Shared, t: &Tag, data: &[u8], v: &mut PollValue, now: Instant) {
        let mut shown: &[u8] = data;
        let mut keep = None;
        if let Some(o) = sh.opt.get(&t.text) {
            if o.raw == data {
                sh.opt.remove(&t.text); // confirmed
            } else if now < o.until {
                keep = Some(o.raw.clone());
                v.pending = true;
            } else {
                sh.opt.remove(&t.text);
                self.log.warn("optimistic write not confirmed, value reverted", &[("tag", t.text.clone())]);
            }
        }
        if let Some(k) = &keep {
            shown = k;
        }
        match t.format(shown) {
            Err(e) => {
                v.quality = Quality::Bad;
                v.error = e.to_string();
            }
            Ok(s) => {
                v.value = s;
                v.quality = Quality::Good;
                v.raw = shown.to_vec();
            }
        }
    }

    fn notify(&self, tags: &[Tag], vals: &[PollValue]) {
        let hook = self.sh().on_update.clone();
        if let Some(f) = hook {
            for (t, v) in tags.iter().zip(vals) {
                f(t, v);
            }
        }
    }

    /// Replaces the update hook (safe while running).
    pub fn set_on_update(&self, f: Option<OnUpdateFn>) {
        self.sh().on_update = f;
    }

    /// All tags (scan groups' tags first, in order, without duplicates).
    pub fn tags(&self) -> Vec<Tag> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for g in &self.groups {
            for t in &g.tags {
                if seen.insert(t.text.clone()) {
                    out.push(t.clone());
                }
            }
        }
        out
    }

    /// The current value of one tag.
    pub fn value(&self, tag: &str) -> Option<PollValue> {
        self.sh().values.get(tag).cloned()
    }

    /// A copy of all tag values (group order; a tag in two groups appears twice).
    pub fn values(&self) -> Vec<PollValue> {
        let sh = self.sh();
        let mut out = Vec::new();
        for g in &self.groups {
            for t in &g.tags {
                if let Some(v) = sh.values.get(&t.text) {
                    out.push(v.clone());
                }
            }
        }
        out
    }

    /// Queues a write and waits for it (value in text form, like `Tag::value`).
    pub fn write(&self, tag: &str, value: &str) -> Result<()> {
        let known = self.sh().tags.get(tag).cloned();
        let t = match known {
            Some(t) => t,
            None => {
                let mut t = parse_tag(tag)?;
                t.little_endian = t.little_endian || self.cfg.little_endian;
                t
            }
        };
        let b = t.value(value)?;
        if self.cfg.optimistic_write {
            let v = {
                let mut sh = self.sh();
                sh.opt.insert(t.text.clone(), Optimistic { raw: b.clone(), until: Instant::now() + self.cfg.optimistic_timeout });
                let mut v = sh.values.get(&t.text).cloned().unwrap_or_default();
                if let Ok(s) = t.format(&b) {
                    v.tag = t.text.clone();
                    v.value = s;
                    v.pending = true;
                    v.time = Some(SystemTime::now());
                    v.raw = b.clone();
                    v.quality = Quality::Good;
                    v.error.clear();
                }
                sh.values.insert(t.text.clone(), v.clone());
                v
            };
            self.notify(std::slice::from_ref(&t), &[v]);
        }
        let (tx, rx) = mpsc::channel();
        {
            let mut q = self.q();
            if q.stopped {
                return Err(local("poller stopped"));
            }
            q.writes.push_back(Job::Write { tag: t, val: b, done: tx });
        }
        self.work_cv.notify_one();
        rx.recv().unwrap_or_else(|_| Err(local("poller stopped")))
    }

    /// Queues a write from a JSON-ish value.
    pub fn write_value(&self, tag: &str, value: &crate::value::Val) -> Result<()> {
        self.write(tag, &value.to_text()?)
    }

    fn do_write(&self, c: &Client, tag: &Tag, val: &[u8]) -> Result<()> {
        let r = c.run(|cn| {
            let errs = cn.write(&[tag.item], &[val])?;
            if let Some(e) = errs.into_iter().next().flatten() {
                return Err(e);
            }
            if !self.cfg.read_after_write {
                return Ok(());
            }
            let res = cn.read(&[tag.item])?;
            let now = Instant::now();
            let mut v = PollValue { tag: tag.text.clone(), time: Some(SystemTime::now()), ..Default::default() };
            let mut sh = self.sh();
            match &res[0] {
                Err(e) => {
                    v.quality = Quality::Bad;
                    v.error = e.to_string();
                }
                Ok(d) => self.apply_optimistic(&mut sh, tag, d, &mut v, now),
            }
            sh.values.insert(tag.text.clone(), v.clone());
            drop(sh);
            self.notify(std::slice::from_ref(tag), &[v]);
            Ok(())
        });
        if r.is_err() && self.cfg.optimistic_write {
            self.sh().opt.remove(&tag.text); // failed write: drop the optimistic value now
        }
        r
    }

    /// Marks interest in the group(s) holding `tag` (on-demand mode).
    pub fn watch(&self, tag: &str) {
        let mut sh = self.sh();
        let now = Instant::now();
        for (gi, g) in self.groups.iter().enumerate() {
            if g.tags.iter().any(|t| t.text == tag) {
                sh.rt[gi].interest = Some(now);
            }
        }
    }

    /// Per-group diagnostics.
    pub fn metrics(&self) -> Vec<GroupMetrics> {
        let mut pdu = 240u16;
        for c in &self.clients {
            let m = c.max_pdu();
            if m != 0 {
                pdu = m;
            }
        }
        let sh = self.sh();
        self.groups
            .iter()
            .enumerate()
            .map(|(gi, g)| {
                let rt = &sh.rt[gi];
                let mut m = GroupMetrics {
                    name: g.cfg.name.clone(),
                    target_interval_ms: ms(rt.target),
                    achieved_interval_ms: ms(rt.achieved),
                    queue_wait_ms: ms(rt.queue_wait),
                    monitored_points: g.tags.len(),
                    cycles: rt.cycles,
                    response: g.stats.snapshot(),
                    ..Default::default()
                };
                if !rt.achieved.is_zero() {
                    m.overload_pct = 100.0 * rt.queue_wait.as_secs_f64() / rt.achieved.as_secs_f64();
                }
                let items: Vec<Item> = g.tags.iter().map(|t| t.item).collect();
                m.request_count = plan_requests(&items, pdu);
                m
            })
            .collect()
    }

    /// The connections (state, stats).
    pub fn clients(&self) -> &[Arc<Client>] {
        &self.clients
    }

    /// True between `start` and `stop`.
    pub fn running(&self) -> bool {
        self.q().running
    }
}

/// Stops the poller when dropped.
pub struct PollerHandle {
    p: Option<Arc<Poller>>,
    threads: Vec<JoinHandle<()>>,
}

impl PollerHandle {
    pub fn poller(&self) -> &Arc<Poller> {
        self.p.as_ref().unwrap()
    }

    /// Stops all threads, fails queued writes, closes the connections.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(p) = self.p.take() {
            p.shutdown();
            for t in self.threads.drain(..) {
                let _ = t.join();
            }
            for c in &p.clients {
                c.close();
            }
        }
    }
}

impl Drop for PollerHandle {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// Counts the ReadVar requests a set of items needs for a PDU.
pub fn plan_requests(items: &[Item], pdu: u16) -> usize {
    if items.is_empty() {
        return 0;
    }
    let pdu = pdu as usize;
    let (mut req, mut rep, mut n) = (12usize, 14usize, 1);
    for it in items {
        let sz = it.size();
        if req + 12 > pdu || rep + 4 + sz + sz % 2 > pdu {
            n += 1;
            req = 12;
            rep = 14;
        }
        req += 12;
        rep += 4 + sz + sz % 2;
    }
    n
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn f<T: Send + Sync>() {}
    f::<Poller>();
    f::<Client>();
    let _: Option<Error> = None;
}
