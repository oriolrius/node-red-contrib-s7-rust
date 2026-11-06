//! Request statistics like the connector diagnostics expected in projects:
//! request count, errors, response-time percentiles and rates. Optional
//! Prometheus text renderer.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const STATS_WINDOW: usize = 2048;

#[derive(Default)]
struct Inner {
    requests: u64,
    errors: u64,
    samples: Vec<Duration>, // ring of recent response times
    stamps: Vec<Instant>,   // ring of recent request times (for rates)
    next: usize,
}

/// Thread-safe request statistics.
#[derive(Default)]
pub struct Stats {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.snapshot().fmt(f)
    }
}

/// A consistent copy of the statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub requests: u64,
    pub errors: u64,
    pub min: Duration,
    pub max: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    /// requests per second over the last minute
    pub rate_1m: f64,
    /// requests per second over the last 5 minutes
    pub rate_5m: f64,
}

/// Duration in milliseconds with µs resolution.
pub fn ms(d: Duration) -> f64 {
    d.as_micros() as f64 / 1000.0
}

impl Stats {
    pub fn new() -> Stats {
        Stats::default()
    }

    /// Records one request.
    pub fn observe(&self, d: Duration, err: bool) {
        let mut s = self.inner.lock().unwrap();
        s.requests += 1;
        if err {
            s.errors += 1;
        }
        let now = Instant::now();
        if s.samples.len() < STATS_WINDOW {
            s.samples.push(d);
            s.stamps.push(now);
        } else {
            let i = s.next;
            s.samples[i] = d;
            s.stamps[i] = now;
        }
        s.next = (s.next + 1) % STATS_WINDOW;
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.inner.lock().unwrap();
        let mut snap = Snapshot { requests: s.requests, errors: s.errors, ..Default::default() };
        if s.samples.is_empty() {
            return snap;
        }
        let mut v = s.samples.clone();
        v.sort();
        let sum: Duration = v.iter().sum();
        let pct = |p: f64| v[(p * (v.len() - 1) as f64) as usize];
        snap.min = v[0];
        snap.max = v[v.len() - 1];
        snap.mean = sum / v.len() as u32;
        snap.p50 = pct(0.50);
        snap.p95 = pct(0.95);
        snap.p99 = pct(0.99);
        let now = Instant::now();
        let (mut n1, mut n5) = (0, 0);
        for t in &s.stamps {
            let age = now.saturating_duration_since(*t);
            if age <= Duration::from_secs(60) {
                n1 += 1;
            }
            if age <= Duration::from_secs(300) {
                n5 += 1;
            }
        }
        snap.rate_1m = n1 as f64 / 60.0;
        snap.rate_5m = n5 as f64 / 300.0;
        snap
    }
}

impl Snapshot {
    /// Prometheus text exposition of the snapshot with `prefix` (e.g.
    /// "s7_request") and constant labels (`device="…"`).
    pub fn prometheus(&self, prefix: &str, labels: &str) -> String {
        let l = if labels.is_empty() { String::new() } else { format!("{{{labels}}}") };
        let q = |quant: &str| {
            if labels.is_empty() {
                format!("{{quantile=\"{quant}\"}}")
            } else {
                format!("{{{labels},quantile=\"{quant}\"}}")
            }
        };
        let mut s = String::new();
        s.push_str(&format!("# TYPE {prefix}_count counter\n{prefix}_count{l} {}\n", self.requests));
        s.push_str(&format!("# TYPE {prefix}_errors counter\n{prefix}_errors{l} {}\n", self.errors));
        s.push_str(&format!("# TYPE {prefix}_response_ms summary\n"));
        for (name, d) in [("0.5", self.p50), ("0.95", self.p95), ("0.99", self.p99)] {
            s.push_str(&format!("{prefix}_response_ms{} {}\n", q(name), ms(d)));
        }
        s.push_str(&format!("# TYPE {prefix}_rate_1m gauge\n{prefix}_rate_1m{l} {}\n", self.rate_1m));
        s.push_str(&format!("# TYPE {prefix}_rate_5m gauge\n{prefix}_rate_5m{l} {}\n", self.rate_5m));
        s
    }
}
