//! Optional logging hook (no global logger, nothing printed).

use std::sync::Arc;

/// Log levels; `Trace` carries frame dumps in hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

/// A log sink. Implemented for closures `Fn(Level, &str)`.
pub trait Logger: Send + Sync {
    /// Cheap filter: messages above the level are not even formatted.
    fn enabled(&self, _level: Level) -> bool {
        true
    }
    fn log(&self, level: Level, msg: &str);
}

impl<F: Fn(Level, &str) + Send + Sync> Logger for F {
    fn log(&self, level: Level, msg: &str) {
        self(level, msg)
    }
}

/// A logger with a maximum level around another logger.
pub struct LevelFilter<L: Logger> {
    pub max: Level,
    pub inner: L,
}

impl<L: Logger> Logger for LevelFilter<L> {
    fn enabled(&self, level: Level) -> bool {
        level <= self.max && self.inner.enabled(level)
    }
    fn log(&self, level: Level, msg: &str) {
        if level <= self.max {
            self.inner.log(level, msg)
        }
    }
}

pub(crate) fn log_with(l: &Option<Arc<dyn Logger>>, level: Level, f: impl FnOnce() -> String) {
    if let Some(l) = l {
        if l.enabled(level) {
            l.log(level, &f());
        }
    }
}
