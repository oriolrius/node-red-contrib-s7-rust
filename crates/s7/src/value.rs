//! JSON-friendly values: the form reads return and writes
//! accept. The napi layer maps `Val` 1:1 to JavaScript values.

use std::fmt;

use crate::error::{local, Result};
use crate::textfmt;
use crate::types::Native;

/// A decoded PLC value in the shape JavaScript gets: integers that fit ±2^53
/// are `Int`, 64-bit integers outside that range are `Str` (no digit lost),
/// REAL keeps its shortest float32 text (3.1, not 3.0999999046325684), NaN/Inf
/// are strings, times are RFC 3339 nano strings, `[n]` tags are `List`.
/// `Null` only appears as a write input (it is an error there).
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Val>),
}

const SAFE_INT: i64 = 1 << 53;

impl Native {
    /// Converts a decoded element to a JSON-friendly transport value.
    pub fn to_val(&self) -> Val {
        match self {
            Native::Bool(b) => Val::Bool(*b),
            Native::U8(x) => Val::Int(*x as i64),
            Native::I16(x) => Val::Int(*x as i64),
            Native::U16(x) => Val::Int(*x as i64),
            Native::I32(x) => Val::Int(*x as i64),
            Native::U32(x) => Val::Int(*x as i64),
            Native::I64(x) => {
                if *x > SAFE_INT || *x < -SAFE_INT {
                    Val::Str(x.to_string())
                } else {
                    Val::Int(*x)
                }
            }
            Native::U64(x) => {
                if *x > SAFE_INT as u64 {
                    Val::Str(x.to_string())
                } else {
                    Val::Int(*x as i64)
                }
            }
            Native::F32(x) => {
                let s = textfmt::format_float(*x as f64, 32);
                if x.is_nan() || x.is_infinite() {
                    Val::Str(s)
                } else {
                    Val::Float(s.parse().unwrap_or(*x as f64))
                }
            }
            Native::F64(x) => {
                if x.is_nan() || x.is_infinite() {
                    Val::Str(textfmt::format_float(*x, 64))
                } else {
                    Val::Float(*x)
                }
            }
            Native::Str(s) => Val::Str(s.clone()),
            Native::Time(t) => Val::Str(textfmt::fmt_rfc3339_nano(t)),
        }
    }
}

impl Val {
    /// The text form the S7 encoders take (strings as
    /// they are, numbers with all their digits, arrays joined by ",").
    pub fn to_text(&self) -> Result<String> {
        Ok(match self {
            Val::Null => return Err(local("value is missing")),
            Val::Bool(b) => b.to_string(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => {
                if f.is_finite() {
                    format!("{f}")
                } else {
                    textfmt::format_float(*f, 64)
                }
            }
            Val::Str(s) => s.clone(),
            Val::List(l) => l.iter().map(|v| v.to_text()).collect::<Result<Vec<_>>>()?.join(","),
        })
    }

    /// Serialises as JSON text.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write_json(&mut s);
        s
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Val::Null => out.push_str("null"),
            Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Val::Int(i) => out.push_str(&i.to_string()),
            Val::Float(f) if f.is_finite() => out.push_str(&format!("{f}")),
            Val::Float(_) => out.push_str("null"),
            Val::Str(s) => json_string(s, out),
            Val::List(l) => {
                out.push('[');
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_json(out);
                }
                out.push(']');
            }
        }
    }
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json())
    }
}

/// Appends `s` as a JSON string literal.
pub fn json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Text form of a JSON-ish value for the S7 encoders; convenience alias of `Val::to_text`.
pub fn value_text(v: &Val) -> Result<String> {
    v.to_text()
}
