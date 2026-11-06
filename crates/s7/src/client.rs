//! Client with the driver's "attempts before timeout" behaviour (Timing:
//! Connect Timeout, Attempts Before Timeout): on a transport error it
//! reconnects and retries. It tracks state and request statistics.

use std::fmt;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::log::{hex_spaced, Level, Log};
use crate::metrics::Stats;
use crate::wire::{Conn, DialOptions};

/// Device connection state (connector model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
    Faulted,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Disconnected => "disconnected",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::Faulted => "faulted",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

struct Inner {
    conn: Option<Conn>,
    state: State,
    detail: String,
    reconnects: u64,
}

/// A device client: one S7 connection, serialised requests (AMQ 1), reconnect
/// on transport errors. `Send + Sync`: share it with `Arc`.
pub struct Client {
    opts: DialOptions,
    attempts: usize,
    log: Log,
    stats: Arc<Stats>,
    max_pdu: AtomicU16,
    inner: Mutex<Inner>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (st, det) = self.status();
        f.debug_struct("Client").field("addr", &self.opts.addr).field("state", &st).field("detail", &det).finish()
    }
}

impl Client {
    /// A client; the first request connects.
    pub fn new(opts: DialOptions) -> Client {
        Client {
            opts,
            attempts: 0,
            log: Log::discard(),
            stats: Arc::new(Stats::new()),
            max_pdu: AtomicU16::new(0),
            inner: Mutex::new(Inner { conn: None, state: State::Disconnected, detail: String::new(), reconnects: 0 }),
        }
    }

    /// Tries per request; 0 -> 3 (driver default).
    pub fn with_attempts(mut self, n: usize) -> Client {
        self.attempts = n;
        self
    }

    /// Logger (default: discard).
    pub fn with_log(mut self, log: Log) -> Client {
        self.log = log;
        self
    }

    pub fn opts(&self) -> &DialOptions {
        &self.opts
    }

    pub fn attempts(&self) -> usize {
        if self.attempts == 0 {
            3
        } else {
            self.attempts
        }
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    /// Request statistics of this client (all connections it made).
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    fn set_state(&self, inner: &mut Inner, s: State, detail: String) {
        if inner.state != s {
            self.log.info("connection state", &[("device", self.opts.addr.clone()), ("state", s.to_string()), ("detail", detail.clone())]);
        }
        inner.state = s;
        inner.detail = detail;
    }

    /// State and detail text (last error).
    pub fn status(&self) -> (State, String) {
        let i = self.inner.lock().unwrap();
        (i.state, i.detail.clone())
    }

    /// How many times the client reconnected after a failure.
    pub fn reconnects(&self) -> u64 {
        self.inner.lock().unwrap().reconnects
    }

    /// Negotiated PDU (0 before the first connection or after a drop). Lock-free.
    pub fn max_pdu(&self) -> u16 {
        self.max_pdu.load(Ordering::Relaxed)
    }

    fn ensure<'a>(&self, inner: &'a mut Inner) -> Result<&'a mut Conn> {
        if inner.conn.is_none() {
            self.set_state(inner, State::Connecting, String::new());
            let mut o = self.opts.clone();
            let lg = self.log.clone();
            let user_trace = o.trace.take();
            o.trace = Some(Arc::new(move |dir: &str, f: &[u8]| {
                if lg.enabled(Level::Trace) {
                    lg.trace("frame", &[("dir", dir.to_string()), ("hex", hex_spaced(f))]);
                }
                if let Some(t) = &user_trace {
                    t(dir, f);
                }
            }));
            let stats = self.stats.clone();
            let lg = self.log.clone();
            let addr = self.opts.addr.clone();
            let user_req = o.on_request.take();
            o.on_request = Some(Arc::new(move |d: Duration, err: Option<&Error>| {
                stats.observe(d, err.is_some());
                if lg.enabled(Level::Debug) {
                    lg.debug(
                        "request",
                        &[
                            ("device", addr.clone()),
                            ("ms", (d.as_micros() as f64 / 1000.0).to_string()),
                            ("err", err.map(|e| e.to_string()).unwrap_or_else(|| "<nil>".into())),
                        ],
                    );
                }
                if let Some(f) = &user_req {
                    f(d, err);
                }
            }));
            match Conn::dial(&o) {
                Err(e) => {
                    self.set_state(inner, State::Faulted, e.to_string());
                    return Err(e);
                }
                Ok(cn) => {
                    self.max_pdu.store(cn.max_pdu, Ordering::Relaxed);
                    let pdu = cn.max_pdu;
                    inner.conn = Some(cn);
                    self.set_state(inner, State::Connected, format!("PDU {pdu}"));
                }
            }
        }
        Ok(inner.conn.as_mut().unwrap())
    }

    fn drop_locked(&self, inner: &mut Inner, err: Option<&Error>) {
        if let Some(c) = inner.conn.take() {
            c.close();
        }
        self.max_pdu.store(0, Ordering::Relaxed);
        match err {
            Some(e) => self.set_state(inner, State::Faulted, e.to_string()),
            None => self.set_state(inner, State::Disconnected, String::new()),
        }
    }

    /// Runs `f` with a live connection, reconnecting on transport errors.
    /// Calls are serialised: one S7 connection has one request in
    /// flight. A PLC answer (item/protocol/userdata error) is never retried.
    pub fn run<T>(&self, mut f: impl FnMut(&mut Conn) -> Result<T>) -> Result<T> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let n = self.attempts();
        let mut last = Error::Io("no attempt".into());
        for i in 0..n {
            match self.ensure(&mut inner) {
                Ok(cn) => match f(cn) {
                    Ok(v) => return Ok(v),
                    Err(e) if !e.retryable() => return Err(e),
                    Err(e) => {
                        self.log.warn(
                            "request failed, reconnecting",
                            &[("device", self.opts.addr.clone()), ("attempt", (i + 1).to_string()), ("err", e.to_string())],
                        );
                        inner.reconnects += 1;
                        last = e;
                    }
                },
                Err(e) => last = e,
            }
            self.drop_locked(&mut inner, Some(&last));
            if i + 1 < n {
                std::thread::sleep(Duration::from_millis(200 * (i as u64 + 1)));
            }
        }
        Err(last)
    }

    /// Drops the connection (the next request reconnects).
    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        self.drop_locked(&mut inner, None);
    }
}
