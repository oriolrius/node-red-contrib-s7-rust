//! Error type of the crate.

use std::fmt;
use std::io;

use crate::ids::fn_name;

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Every error of the crate.
#[derive(Debug)]
pub enum Error {
    /// A buffer ended before a field was complete.
    Short,
    /// Socket error.
    Io(io::Error),
    /// The request or the wait for a notification timed out.
    Timeout,
    /// Protocol or usage error (the text starts with "s7plus: ").
    Proto(String),
    /// A negative S7CommPlus return value.
    Return { function: u16, ret: u64 },
    /// A per-item error return value.
    Item(u64),
    /// The PLC rejected the password, with the return value.
    AccessDenied(u64),
    /// A symbol does not exist; the text names the symbol.
    NotFound(String),
    /// Single-bit write through byte-area access.
    BitWrite,
    /// TLS error.
    Tls(String),
}

impl Error {
    pub(crate) fn proto(s: impl Into<String>) -> Error {
        let s = s.into();
        if s.starts_with("s7plus: ") {
            Error::Proto(s)
        } else {
            Error::Proto(format!("s7plus: {s}"))
        }
    }

    /// True for [`Error::NotFound`].
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::NotFound(_))
    }

    /// True for timeouts (also socket timeouts).
    pub fn is_timeout(&self) -> bool {
        match self {
            Error::Timeout => true,
            Error::Io(e) => matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock),
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Short => write!(f, "s7plus: short buffer"),
            Error::Io(e) => write!(f, "s7plus: {e}"),
            Error::Timeout => write!(f, "s7plus: timeout"),
            Error::Proto(s) => write!(f, "{s}"),
            Error::Return { function, ret } => write!(f, "s7plus: {} returned {:#x}", fn_name(*function), ret),
            Error::Item(e) => write!(f, "s7plus: item error {e:#x}"),
            Error::AccessDenied(rv) => write!(f, "s7plus: legitimation refused (access denied) ({rv:#x})"),
            Error::NotFound(s) => write!(f, "s7plus: symbol not found: {s}"),
            Error::BitWrite => write!(f, "s7plus: bit writes are not supported by byte-area access"),
            Error::Tls(s) => write!(f, "s7plus: TLS: {s}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Error::Timeout,
            _ => Error::Io(e),
        }
    }
}

impl From<rustls::Error> for Error {
    fn from(e: rustls::Error) -> Self {
        Error::Tls(e.to_string())
    }
}
