//! Data types and conversion. (the extended IEC and
//! S7-1200/1500 types are in `types_ext`). All S7 values are big-endian (driver
//! default byte order).

use std::fmt;

use chrono::{DateTime, FixedOffset};

use crate::error::{local, Error, Result};
use crate::log::hex_spaced;
use crate::textfmt::{self, Zone};
use crate::types_ext;

/// The interpretation of an item's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    Byte,
    Char,
    Word,
    Int,
    DWord,
    DInt,
    Real,
    LWord,
    LInt,
    LReal,
    /// S7 STRING: max(1) len(1) chars
    String,
    /// DATE_AND_TIME, 8 bytes BCD, local time
    DT,
    /// timer value
    S5Time,
    Counter,
    /// 16-bit packed BCD
    BCD,
    /// 32-bit packed BCD
    LBCD,
    Raw,
    // ---- types_ext ----
    /// DATE_AND_TIME read as UTC (nodes7 DTZ)
    DTZ,
    /// DTL, 12 bytes, local time (S7-1200/1500)
    DTL,
    /// DTL read as UTC (nodes7 DTLZ)
    DTLZ,
    /// DATE: days since 1990-01-01, uint16
    Date,
    /// TIME_OF_DAY: ms since midnight, uint32
    TOD,
    /// TIME: signed ms, int32
    Time,
    /// LTIME: signed ns, int64 (S7-1500)
    LTime,
    /// LTIME_OF_DAY: ns since midnight, uint64 (S7-1500)
    LTOD,
    /// LDT: ns since 1970-01-01, uint64 (S7-1500)
    LDT,
    /// WCHAR: one UTF-16 code unit
    WChar,
    /// WSTRING[n]: max(2) len(2) + n UTF-16 code units
    WString,
}

pub const ALL_TYPES: [Type; 29] = [
    Type::Bool,
    Type::Byte,
    Type::Char,
    Type::Word,
    Type::Int,
    Type::DWord,
    Type::DInt,
    Type::Real,
    Type::LWord,
    Type::LInt,
    Type::LReal,
    Type::String,
    Type::DT,
    Type::S5Time,
    Type::Counter,
    Type::BCD,
    Type::LBCD,
    Type::Raw,
    Type::DTZ,
    Type::DTL,
    Type::DTLZ,
    Type::Date,
    Type::TOD,
    Type::Time,
    Type::LTime,
    Type::LTOD,
    Type::LDT,
    Type::WChar,
    Type::WString,
];

impl Type {
    /// The type name ("int", "dtl", "wstring"…).
    pub fn as_str(self) -> &'static str {
        match self {
            Type::Bool => "bool",
            Type::Byte => "byte",
            Type::Char => "char",
            Type::Word => "word",
            Type::Int => "int",
            Type::DWord => "dword",
            Type::DInt => "dint",
            Type::Real => "real",
            Type::LWord => "lword",
            Type::LInt => "lint",
            Type::LReal => "lreal",
            Type::String => "string",
            Type::DT => "dt",
            Type::S5Time => "s5time",
            Type::Counter => "counter",
            Type::BCD => "bcd",
            Type::LBCD => "lbcd",
            Type::Raw => "raw",
            Type::DTZ => "dtz",
            Type::DTL => "dtl",
            Type::DTLZ => "dtlz",
            Type::Date => "date",
            Type::TOD => "tod",
            Type::Time => "time",
            Type::LTime => "ltime",
            Type::LTOD => "ltod",
            Type::LDT => "ldt",
            Type::WChar => "wchar",
            Type::WString => "wstring",
        }
    }

    /// Parses a type name (lower case, as after `:` in an address).
    pub fn from_name(s: &str) -> Option<Type> {
        ALL_TYPES.iter().copied().find(|t| t.as_str() == s)
    }

    /// Byte size of one element of t (`str_len` for STRING / WSTRING).
    pub fn size(self, str_len: usize) -> usize {
        let n = types_ext::ext_size(self, str_len);
        if n > 0 {
            return n;
        }
        match self {
            Type::Bool | Type::Byte | Type::Char => 1,
            Type::Word | Type::Int | Type::S5Time | Type::Counter | Type::BCD => 2,
            Type::DWord | Type::DInt | Type::Real | Type::LBCD => 4,
            Type::LWord | Type::LInt | Type::LReal | Type::DT => 8,
            Type::String => str_len + 2,
            _ => 1,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A decoded element as a typed value (BOOL→bool, BYTE→uint8,
/// CHAR→string, WORD→uint16, INT→int16, DWORD→uint32, DINT→int32,
/// REAL→float32, LWORD→uint64, LINT→int64, LREAL→float64, STRING→string,
/// DT→time, S5TIME→int32 ms, counter/BCD→uint16, LBCD→uint32,
/// DTL/DTLZ/DTZ/DATE/LDT→time, TIME→int32 ms, TOD→uint32 ms, LTIME→int64 ns,
/// LTOD→uint64 ns, WCHAR/WSTRING→string).
#[derive(Debug, Clone, PartialEq)]
pub enum Native {
    Bool(bool),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(String),
    Time(DateTime<FixedOffset>),
}

pub(crate) fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
pub(crate) fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
pub(crate) fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

pub(crate) fn bcd_byte(b: u8) -> i64 {
    (b >> 4) as i64 * 10 + (b & 0x0f) as i64
}

pub(crate) fn to_bcd(v: i64) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}

pub(crate) fn bcd16(v: u16) -> u32 {
    (v >> 12 & 0xf) as u32 * 1000 + (v >> 8 & 0xf) as u32 * 100 + (v >> 4 & 0xf) as u32 * 10 + (v & 0xf) as u32
}

/// Decodes an S5TIME word: bits 12-13 = time base (10 ms, 100 ms, 1 s, 10 s),
/// bits 0-11 = 3 BCD digits.
pub fn s5time_to_ms(b: &[u8]) -> Result<i64> {
    if b.len() < 2 {
        return Err(local("s5time needs 2 bytes"));
    }
    let v = be16(b);
    let base = [10, 100, 1000, 10000][(v >> 12 & 0x3) as usize];
    Ok(((v >> 8 & 0xf) as i64 * 100 + (v >> 4 & 0xf) as i64 * 10 + (v & 0xf) as i64) * base)
}

/// DATE_AND_TIME (8 bytes BCD) as a time in `zone`.
pub(crate) fn dt_time(b: &[u8], zone: Zone) -> DateTime<FixedOffset> {
    let mut y = bcd_byte(b[0]);
    y += if y < 90 { 2000 } else { 1900 };
    let ms = bcd_byte(b[6]) * 10 + (b[7] >> 4) as i64;
    textfmt::make_date(y, bcd_byte(b[1]), bcd_byte(b[2]), bcd_byte(b[3]), bcd_byte(b[4]), bcd_byte(b[5]), ms * 1_000_000, zone)
}

fn short(t: Type, need: usize, have: usize) -> Error {
    local(format!("{t} needs {need} bytes, have {have}"))
}

/// Turns raw bytes into a printable value.
pub fn decode(t: Type, b: &[u8]) -> Result<String> {
    if types_ext::is_ext(t) {
        return types_ext::decode_ext(t, b);
    }
    let need = t.size(0);
    if t != Type::String && t != Type::Raw && b.len() < need {
        return Err(short(t, need, b.len()));
    }
    Ok(match t {
        Type::Bool => (b[0] & 1 == 1).to_string(),
        Type::Byte => format!("{} (0x{:02x})", b[0], b[0]),
        Type::Char => textfmt::quote(&b[..1]),
        Type::Word => {
            let v = be16(b);
            format!("{v} (0x{v:04x})")
        }
        Type::Int => (be16(b) as i16).to_string(),
        Type::DWord => {
            let v = be32(b);
            format!("{v} (0x{v:08x})")
        }
        Type::DInt => (be32(b) as i32).to_string(),
        Type::Real => textfmt::format_float(f32::from_bits(be32(b)) as f64, 32),
        Type::LWord => be64(b).to_string(),
        Type::LInt => (be64(b) as i64).to_string(),
        Type::LReal => textfmt::format_float(f64::from_bits(be64(b)), 64),
        Type::String => {
            if b.len() < 2 {
                return Err(local("string too short"));
            }
            let n = (b[1] as usize).min(b.len() - 2);
            textfmt::quote(&b[2..2 + n])
        }
        Type::DT => textfmt::fmt_ms_layout(&dt_time(b, Zone::Local)),
        Type::S5Time => format!("{} ms", s5time_to_ms(b)?),
        Type::Counter | Type::BCD => bcd16(be16(b)).to_string(),
        Type::LBCD => {
            let v = be32(b);
            (bcd16((v >> 16) as u16) * 10000 + bcd16(v as u16)).to_string()
        }
        _ => hex_spaced(b),
    })
}

/// Turns a text value into bytes for writing.
pub fn encode(t: Type, s: &str, str_len: usize) -> Result<Vec<u8>> {
    let s = s.trim();
    if types_ext::is_ext(t) {
        return types_ext::encode_ext(t, s, str_len);
    }
    let mut b = vec![0u8; t.size(str_len)];
    match t {
        Type::Bool => {
            if textfmt::parse_bool(s)? {
                b[0] = 1;
            }
        }
        Type::Byte | Type::Char => {
            if t == Type::Char && s.len() == 1 {
                b[0] = s.as_bytes()[0];
            } else {
                b[0] = textfmt::parse_uint(s, 8)? as u8;
            }
        }
        Type::Word => b.copy_from_slice(&(textfmt::parse_uint(s, 16)? as u16).to_be_bytes()),
        Type::Int => b.copy_from_slice(&(textfmt::parse_int(s, 16)? as i16).to_be_bytes()),
        Type::DWord => b.copy_from_slice(&(textfmt::parse_uint(s, 32)? as u32).to_be_bytes()),
        Type::DInt => b.copy_from_slice(&(textfmt::parse_int(s, 32)? as i32).to_be_bytes()),
        Type::Real => b.copy_from_slice(&(textfmt::parse_float(s, 32)? as f32).to_bits().to_be_bytes()),
        Type::LWord => b.copy_from_slice(&textfmt::parse_uint(s, 64)?.to_be_bytes()),
        Type::LInt => b.copy_from_slice(&textfmt::parse_int(s, 64)?.to_be_bytes()),
        Type::LReal => b.copy_from_slice(&textfmt::parse_float(s, 64)?.to_bits().to_be_bytes()),
        Type::String => {
            if s.len() > str_len {
                return Err(local(format!("string longer than {str_len}")));
            }
            b[0] = str_len as u8;
            b[1] = s.len() as u8;
            b[2..2 + s.len()].copy_from_slice(s.as_bytes());
        }
        Type::DT => {
            let tm = textfmt::parse_time_text(s, Zone::Local)?;
            put_dt(&mut b, &Zone::Local.convert(&tm));
        }
        Type::Counter | Type::BCD => {
            let v = textfmt::parse_dec_i64(s).filter(|v| (0..=9999).contains(v)).ok_or_else(|| local("BCD value must be 0-9999"))?;
            let x = ((v / 1000) << 12 | (v / 100 % 10) << 8 | (v / 10 % 10) << 4 | (v % 10)) as u16;
            b.copy_from_slice(&x.to_be_bytes());
        }
        Type::LBCD => {
            let mut v =
                textfmt::parse_dec_i64(s).filter(|v| (0..=99_999_999).contains(v)).ok_or_else(|| local("LBCD value must be 0-99999999"))?;
            let mut x: u32 = 0;
            for i in 0..8 {
                x |= ((v % 10) as u32) << (4 * i);
                v /= 10;
            }
            b.copy_from_slice(&x.to_be_bytes());
        }
        _ => return Err(local(format!("writing type {t} is not supported"))),
    }
    Ok(b)
}

/// Writes a DATE_AND_TIME (8 bytes BCD; byte 7 low nibble = weekday, 1 = Sunday).
pub(crate) fn put_dt(b: &mut [u8], tm: &DateTime<FixedOffset>) {
    use chrono::{Datelike, Timelike};
    let ms = (tm.nanosecond() % 1_000_000_000 / 1_000_000) as i64;
    b[0] = to_bcd(tm.year() as i64 % 100);
    b[1] = to_bcd(tm.month() as i64);
    b[2] = to_bcd(tm.day() as i64);
    b[3] = to_bcd(tm.hour() as i64);
    b[4] = to_bcd(tm.minute() as i64);
    b[5] = to_bcd(tm.second() as i64);
    b[6] = to_bcd(ms / 10);
    b[7] = ((ms % 10) as u8) << 4 | (textfmt::weekday(tm) as u8 + 1);
}

/// Converts one element to a typed value.
pub fn native(t: Type, b: &[u8]) -> Result<Native> {
    if types_ext::is_ext(t) {
        return types_ext::native_ext(t, b);
    }
    if t != Type::String && b.len() < t.size(0) {
        return Err(local(format!("{t} needs {} bytes", t.size(0))));
    }
    Ok(match t {
        Type::Bool => Native::Bool(b[0] & 1 == 1),
        Type::Byte => Native::U8(b[0]),
        Type::Char => Native::Str(String::from_utf8_lossy(&b[..1]).into_owned()),
        Type::Word => Native::U16(be16(b)),
        Type::Int => Native::I16(be16(b) as i16),
        Type::DWord => Native::U32(be32(b)),
        Type::DInt => Native::I32(be32(b) as i32),
        Type::Real => Native::F32(f32::from_bits(be32(b))),
        Type::LWord => Native::U64(be64(b)),
        Type::LInt => Native::I64(be64(b) as i64),
        Type::LReal => Native::F64(f64::from_bits(be64(b))),
        Type::S5Time => Native::I32(s5time_to_ms(b)? as i32),
        Type::Counter | Type::BCD => Native::U16(bcd16(be16(b)) as u16),
        Type::LBCD => {
            let v = be32(b);
            Native::U32(bcd16((v >> 16) as u16) * 10000 + bcd16(v as u16))
        }
        Type::String => {
            if b.len() < 2 {
                return Err(local("string too short"));
            }
            let n = (b[1] as usize).min(b.len() - 2);
            Native::Str(String::from_utf8_lossy(&b[2..2 + n]).into_owned())
        }
        Type::DT => {
            // formatted with ms precision and parsed back: same wall clock, ms resolution
            Native::Time(dt_time(b, Zone::Local))
        }
        _ => return Err(local(format!("no native type for {t}"))),
    })
}
