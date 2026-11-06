//! Logging without a framework: the caller passes a sink (with a Trace level
//! below Debug for frame hex dumps). The default `Log`
//! discards everything (the crate never prints).

use std::fmt;
use std::sync::Arc;

/// Log levels (Trace is below Debug, for frame hex dumps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Receives log records. Any `Fn(Level, &str, &[(String, String)])` closure is a sink.
pub trait LogSink: Send + Sync {
    fn log(&self, level: Level, msg: &str, fields: &[(String, String)]);
}

impl<F> LogSink for F
where
    F: Fn(Level, &str, &[(String, String)]) + Send + Sync,
{
    fn log(&self, level: Level, msg: &str, fields: &[(String, String)]) {
        self(level, msg, fields)
    }
}

/// A cheap, clonable logger handle: optional sink, minimum level and fixed
/// fields.
#[derive(Clone)]
pub struct Log {
    sink: Option<Arc<dyn LogSink>>,
    min: Level,
    fields: Vec<(String, String)>,
}

impl Default for Log {
    fn default() -> Self {
        Log::discard()
    }
}

impl fmt::Debug for Log {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Log").field("enabled", &self.sink.is_some()).field("min", &self.min).field("fields", &self.fields).finish()
    }
}

impl Log {
    /// A logger that drops every record.
    pub fn discard() -> Log {
        Log { sink: None, min: Level::Error, fields: Vec::new() }
    }

    /// A logger that sends records at `min` level and above to `sink`.
    pub fn new(sink: Arc<dyn LogSink>, min: Level) -> Log {
        Log { sink: Some(sink), min, fields: Vec::new() }
    }

    /// Convenience: build from a closure.
    pub fn from_fn(min: Level, f: impl Fn(Level, &str, &[(String, String)]) + Send + Sync + 'static) -> Log {
        Log::new(Arc::new(f), min)
    }

    /// A child logger with one more fixed field.
    pub fn with(&self, key: &str, value: impl fmt::Display) -> Log {
        let mut l = self.clone();
        l.fields.push((key.to_string(), value.to_string()));
        l
    }

    pub fn enabled(&self, level: Level) -> bool {
        self.sink.is_some() && level >= self.min
    }

    /// Logs one record; `kv` are extra key/value fields.
    pub fn log(&self, level: Level, msg: &str, kv: &[(&str, String)]) {
        if !self.enabled(level) {
            return;
        }
        let mut f = self.fields.clone();
        f.extend(kv.iter().map(|(k, v)| (k.to_string(), v.clone())));
        if let Some(s) = &self.sink {
            s.log(level, msg, &f);
        }
    }

    pub fn trace(&self, msg: &str, kv: &[(&str, String)]) {
        self.log(Level::Trace, msg, kv)
    }
    pub fn debug(&self, msg: &str, kv: &[(&str, String)]) {
        self.log(Level::Debug, msg, kv)
    }
    pub fn info(&self, msg: &str, kv: &[(&str, String)]) {
        self.log(Level::Info, msg, kv)
    }
    pub fn warn(&self, msg: &str, kv: &[(&str, String)]) {
        self.log(Level::Warn, msg, kv)
    }
    pub fn error(&self, msg: &str, kv: &[(&str, String)]) {
        self.log(Level::Error, msg, kv)
    }
}

/// Hex dump with spaces.
pub fn hex_spaced(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{x:02x}"));
    }
    s
}
