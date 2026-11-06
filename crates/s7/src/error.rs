//! Error type shared by the whole crate: item errors, protocol errors, userdata
//! errors, local errors and transport errors in one enum. `Context` wraps an
//! error with context text ("SZL 0x001c/0: userdata error 0xd401: …") and
//! `root()` returns the innermost error.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Transport failure or unexpected framing (socket, timeout, bad reply): retryable.
    Io(String),
    /// Per-item return code from the CPU (not 0xFF).
    Item(u8),
    /// Ack-Data header error class/code.
    Proto(u8, u8),
    /// Error word of a Userdata reply parameter.
    Userdata(u16),
    /// Local error (address, value, argument): nothing was sent to the PLC.
    Local(String),
    /// A wrapped error with context text.
    Context(String, Box<Error>),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Texts of the item return codes.
pub fn item_err_text(code: u8) -> Option<&'static str> {
    Some(match code {
        0x01 => "hardware fault",
        0x03 => "illegal object access",
        0x04 => "protection error",
        0x05 => "invalid address",
        0x06 => "data type not supported",
        0x07 => "invalid data size / too much data",
        0x0a => "object does not exist or length error",
        _ => return None,
    })
}

/// Texts of the Ack-Data error classes.
pub fn class_text(class: u8) -> &'static str {
    match class {
        0x81 => "application relationship",
        0x82 => "object definition",
        0x83 => "no resources",
        0x84 => "service structure",
        0x85 => "communication equipment",
        0x87 => "access error",
        0xd2 => "OVS error",
        0xd4 => "diagnostic error",
        0xd6 => "protection system",
        0xd8 => "BuB error",
        0xef => "layer 2 specific",
        _ => "",
    }
}

/// Texts of the 16-bit Userdata error codes (also used for Ack-Data class/code).
pub fn userdata_err_text(code: u16) -> Option<&'static str> {
    Some(match code {
        0x8104 => "function not supported by this CPU",
        0xd401 => "SZL ID or index not available on this CPU",
        0xd402 => "SZL index not available",
        0xd403 => "SZL ID not available",
        0x8304 => "service not allowed in this CPU state",
        _ => return None,
    })
}

impl Error {
    /// The innermost error.
    pub fn root(&self) -> &Error {
        let mut e = self;
        while let Error::Context(_, inner) = e {
            e = inner;
        }
        e
    }

    /// Wraps the error with a context prefix.
    pub fn context(self, ctx: impl Into<String>) -> Error {
        Error::Context(ctx.into(), Box::new(self))
    }

    /// A transport failure, not a PLC answer. Local errors are not retried
    /// either (they would fail the same way).
    pub fn retryable(&self) -> bool {
        matches!(self.root(), Error::Io(_))
    }

    /// True when the CPU answered (item, Ack-Data header or Userdata error).
    pub fn is_device_error(&self) -> bool {
        matches!(self.root(), Error::Item(_) | Error::Proto(..) | Error::Userdata(_))
    }

    /// Item return code, if the root error is an item error.
    pub fn item_code(&self) -> Option<u8> {
        match self.root() {
            Error::Item(c) => Some(*c),
            _ => None,
        }
    }

    /// Ack-Data class/code, if the root error is a protocol error.
    pub fn protocol(&self) -> Option<(u8, u8)> {
        match self.root() {
            Error::Proto(a, b) => Some((*a, *b)),
            _ => None,
        }
    }

    /// Userdata error word, if the root error is a Userdata error.
    pub fn userdata_code(&self) -> Option<u16> {
        match self.root() {
            Error::Userdata(c) => Some(*c),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(s) | Error::Local(s) => f.write_str(s),
            Error::Item(c) => match item_err_text(*c) {
                Some(t) => write!(f, "item return code 0x{c:02x}: {t}"),
                None => write!(f, "item return code 0x{c:02x}"),
            },
            Error::Proto(cl, co) => {
                write!(f, "S7 error class 0x{cl:02x} ({}) code 0x{co:02x}", class_text(*cl))?;
                if let Some(t) = userdata_err_text((*cl as u16) << 8 | *co as u16) {
                    write!(f, ": {t}")?;
                }
                Ok(())
            }
            Error::Userdata(c) => match userdata_err_text(*c) {
                Some(t) => write!(f, "userdata error 0x{c:04x}: {t}"),
                None => write!(f, "userdata error 0x{c:04x}"),
            },
            Error::Context(ctx, inner) => write!(f, "{ctx}: {inner}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Io("i/o timeout".into()),
            std::io::ErrorKind::UnexpectedEof => Error::Io("EOF".into()),
            _ => Error::Io(e.to_string()),
        }
    }
}

pub(crate) fn local(s: impl Into<String>) -> Error {
    Error::Local(s.into())
}

pub(crate) fn io(s: impl Into<String>) -> Error {
    Error::Io(s.into())
}
