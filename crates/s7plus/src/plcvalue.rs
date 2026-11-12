//! Conversion between wire values and PLC values by softdatatype, text
//! parsing for writes, and symbolic read/write.

use std::fmt::Write as _;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::ids::TI_DTL;
use crate::msg::ItemAddress;
use crate::softdt::*;
use crate::textfmt::{self, format_float, hex, quote, DateTime, MILLISECOND, SECOND};
use crate::value::*;

/// Interface timestamp of the DTL system type used when a DTL is written
/// before it was read (value seen in the reference).
pub const DEFAULT_DTL_TIMESTAMP: u64 = 0x10ff_4ad6_dfd5_774c;

/// 1990-01-01 (DATE epoch).
pub fn epoch1990() -> DateTime {
    DateTime::from_civil(1990, 1, 1, 0, 0, 0, 0)
}

/// A resolved symbolic variable.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tag {
    pub name: String,
    pub addr: ItemAddress,
    pub softdatatype: u8,
    /// Whole one-dimensional array.
    pub array: bool,
    /// Element count of a whole array.
    pub count: u32,
    /// String/WString max length.
    pub max_len: u16,
    /// DTL interface timestamp (learned on read).
    pub packed_ts: u64,
}

impl Tag {
    /// A tag with only a softdatatype (for conversions).
    pub fn of_type(sdt: u8) -> Tag {
        Tag { name: softdatatype_name(sdt), softdatatype: sdt, ..Default::default() }
    }
}

/// A decoded PLC value, easy to map to JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum PlcValue {
    Null,
    Bool(bool),
    /// SInt, Int, DInt, LInt.
    Int(i64),
    /// USInt, UInt, UDInt, ULInt, Byte, Word, DWord, LWord.
    UInt(u64),
    Real(f32),
    LReal(f64),
    /// Char, WChar, String, WString.
    String(String),
    /// Pointer, Any, Remote (raw bytes).
    Bytes(Vec<u8>),
    /// Date, Date_And_Time, LDT, DTL (PLC local time shown as UTC).
    DateTime(DateTime),
    /// Time, LTime, S5Time, Time_Of_Day, LTOD in nanoseconds.
    Duration(i64),
    Array(Vec<PlcValue>),
    /// Values without a PLC meaning (structs, sparse arrays).
    Raw(Value),
}

impl PlcValue {
    /// JSON text: numbers, strings, arrays; date-times as RFC 3339 strings
    /// with nanoseconds (UTC "Z"), durations as milliseconds (fractional),
    /// bytes as arrays of numbers, raw values as their dump string.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.json_into(&mut s);
        s
    }

    fn json_into(&self, s: &mut String) {
        match self {
            PlcValue::Null => s.push_str("null"),
            PlcValue::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            PlcValue::Int(i) => {
                let _ = write!(s, "{i}");
            }
            PlcValue::UInt(u) => {
                let _ = write!(s, "{u}");
            }
            PlcValue::Real(f) => json_float(s, *f as f64, true),
            PlcValue::LReal(f) => json_float(s, *f, false),
            PlcValue::String(t) => json_string(s, t),
            PlcValue::Bytes(b) => {
                s.push('[');
                for (i, x) in b.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    let _ = write!(s, "{x}");
                }
                s.push(']');
            }
            PlcValue::DateTime(t) => json_string(s, &t.rfc3339_nano()),
            PlcValue::Duration(d) => {
                if d % MILLISECOND == 0 {
                    let _ = write!(s, "{}", d / MILLISECOND);
                } else {
                    let _ = write!(s, "{}", *d as f64 / MILLISECOND as f64);
                }
            }
            PlcValue::Array(a) => {
                s.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        s.push(',');
                    }
                    x.json_into(s);
                }
                s.push(']');
            }
            PlcValue::Raw(v) => json_string(s, &v.to_string()),
        }
    }
}

fn json_float(s: &mut String, f: f64, bits32: bool) {
    if !f.is_finite() {
        s.push_str("null");
        return;
    }
    let t = format_float(f, bits32);
    // the %g exponent form ("1e+300") is valid JSON
    s.push_str(&t);
}

fn json_string(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

/// The S7CommPlus datatype used on the wire for a softdatatype.
pub fn wire_type(sdt: u8) -> Option<u8> {
    Some(match sdt {
        SDT_BOOL => T_BOOL,
        SDT_BYTE => T_BYTE,
        SDT_CHAR | SDT_USINT => T_USINT,
        SDT_WORD | SDT_S5TIME => T_WORD,
        SDT_INT => T_INT,
        SDT_DWORD => T_DWORD,
        SDT_DINT | SDT_TIME => T_DINT,
        SDT_REAL => T_REAL,
        SDT_DATE | SDT_UINT | SDT_WCHAR => T_UINT,
        SDT_TIME_OF_DAY | SDT_UDINT => T_UDINT,
        SDT_LREAL => T_LREAL,
        SDT_ULINT | SDT_LTOD => T_ULINT,
        SDT_LINT => T_LINT,
        SDT_LWORD => T_LWORD,
        SDT_SINT => T_SINT,
        SDT_LTIME => T_TIMESPAN,
        SDT_LDT => T_TIMESTAMP,
        _ => return None,
    })
}

/// Converts a read value according to the tag type. Returns the value and,
/// for a DTL, the interface timestamp to remember.
pub fn decode_plc(t: &Tag, v: &Value) -> Result<PlcValue> {
    decode_plc_ts(t, v).map(|(x, _)| x)
}

pub fn decode_plc_ts(t: &Tag, v: &Value) -> Result<(PlcValue, Option<u64>)> {
    if t.array && v.flags & FLAG_ARRAY != 0 {
        let empty = Vec::new();
        let a = v.v.as_array().map(|a| a as &[V]).unwrap_or(&empty);
        let mut out = Vec::with_capacity(a.len());
        let mut ts = None;
        for e in a {
            let (x, t2) = decode_scalar_plc(t, &Value::new(v.typ, e.clone()))?;
            ts = ts.or(t2);
            out.push(x);
        }
        return Ok((PlcValue::Array(out), ts));
    }
    decode_scalar_plc(t, v)
}

pub(crate) fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    match &v.v {
        V::Bytes(b) => Some(b.clone()),
        V::Array(a) => a.iter().map(|e| e.as_u8()).collect(),
        _ => None,
    }
}

fn u16s_of(v: &Value) -> Option<Vec<u16>> {
    match &v.v {
        V::Array(a) => a.iter().map(|e| e.as_u16()).collect(),
        _ => None,
    }
}

fn bcd(b: u8) -> i64 {
    (b >> 4) as i64 * 10 + (b & 0x0f) as i64
}

fn tobcd(v: i64) -> u8 {
    (((v / 10 % 10) as u8) << 4) | (v % 10) as u8
}

fn plain(v: &V) -> PlcValue {
    match v {
        V::Null => PlcValue::Null,
        V::Bool(b) => PlcValue::Bool(*b),
        V::U8(x) => PlcValue::UInt(*x as u64),
        V::U16(x) => PlcValue::UInt(*x as u64),
        V::U32(x) => PlcValue::UInt(*x as u64),
        V::U64(x) => PlcValue::UInt(*x),
        V::I8(x) => PlcValue::Int(*x as i64),
        V::I16(x) => PlcValue::Int(*x as i64),
        V::I32(x) => PlcValue::Int(*x as i64),
        V::I64(x) => PlcValue::Int(*x),
        V::F32(x) => PlcValue::Real(*x),
        V::F64(x) => PlcValue::LReal(*x),
        V::Str(s) => PlcValue::String(s.clone()),
        V::Bytes(b) => PlcValue::Bytes(b.clone()),
        V::Array(a) => PlcValue::Array(a.iter().map(plain).collect()),
        V::Struct(_) | V::Sparse(_) => PlcValue::Raw(Value::new(0, v.clone())),
    }
}

fn decode_scalar_plc(t: &Tag, v: &Value) -> Result<(PlcValue, Option<u64>)> {
    let bad = || Err(Error::proto(format!("{}: unexpected {} for {}", t.name, v, softdatatype_name(t.softdatatype))));
    let ok = |x: PlcValue| Ok((x, None));
    match t.softdatatype {
        SDT_CHAR => match v.v {
            V::U8(u) => ok(PlcValue::String(char::from(u).to_string())), // Latin-1
            _ => bad(),
        },
        SDT_WCHAR => match v.v {
            V::U16(u) => ok(PlcValue::String(String::from_utf16_lossy(&[u]))),
            _ => bad(),
        },
        SDT_DATE => match v.v {
            V::U16(u) => ok(PlcValue::DateTime(epoch1990().add_nanos(u as i64 * 86400 * SECOND))),
            _ => bad(),
        },
        SDT_TIME_OF_DAY => match v.v {
            V::U32(u) => ok(PlcValue::Duration(u as i64 * MILLISECOND)),
            _ => bad(),
        },
        SDT_TIME => match v.v {
            V::I32(i) => ok(PlcValue::Duration(i as i64 * MILLISECOND)),
            _ => bad(),
        },
        SDT_S5TIME => match v.v {
            V::U16(u) => ok(PlcValue::Duration(s5time_to_duration(u))),
            _ => bad(),
        },
        SDT_LTIME => match v.v {
            V::I64(i) => ok(PlcValue::Duration(i)),
            _ => bad(),
        },
        SDT_LTOD => match v.v {
            V::U64(u) => ok(PlcValue::Duration(u as i64)),
            _ => bad(),
        },
        SDT_LDT => match v.v {
            V::U64(u) => ok(PlcValue::DateTime(DateTime::from_unix_nanos(u as i64))),
            _ => bad(),
        },
        SDT_DATE_AND_TIME => match bytes_of(v) {
            Some(b) if b.len() >= 8 => ok(PlcValue::DateTime(dt_to_time(&b))),
            _ => bad(),
        },
        SDT_DTL => match &v.v {
            V::Struct(s) if s.packed.as_ref().is_some_and(|p| p.len() >= 12) => {
                Ok((PlcValue::DateTime(dtl_to_time(s.packed.as_ref().unwrap())), Some(s.packed_ts)))
            }
            _ => bad(),
        },
        SDT_STRING => match bytes_of(v) {
            Some(b) if b.len() >= 2 => {
                let n = (b[1] as usize).min(b.len() - 2);
                ok(PlcValue::String(b[2..2 + n].iter().map(|c| char::from(*c)).collect()))
                // ISO-8859-1
            }
            _ => bad(),
        },
        SDT_WSTRING => {
            if let V::Str(s) = &v.v {
                return ok(PlcValue::String(s.clone()));
            }
            match u16s_of(v) {
                Some(u) if u.len() >= 2 => {
                    let n = (u[1] as usize).min(u.len() - 2);
                    ok(PlcValue::String(String::from_utf16_lossy(&u[2..2 + n])))
                }
                _ => bad(),
            }
        }
        SDT_POINTER | SDT_ANY | SDT_REMOTE => match bytes_of(v) {
            Some(b) => ok(PlcValue::Bytes(b)),
            None => bad(),
        },
        sdt => {
            if let Some(wt) = wire_type(sdt) {
                if wt != v.typ {
                    return bad();
                }
            }
            ok(plain(&v.v))
        }
    }
}

/// Decodes an S5TIME word (bits 12-13 time base, 3 BCD digits) to nanoseconds.
pub fn s5time_to_duration(w: u16) -> i64 {
    let val = ((w >> 8) & 0xf) as i64 * 100 + ((w >> 4) & 0xf) as i64 * 10 + (w & 0xf) as i64;
    let base = [10 * MILLISECOND, 100 * MILLISECOND, SECOND, 10 * SECOND][((w >> 12) & 3) as usize];
    val * base
}

/// Encodes a duration (ns) with the finest time base that fits.
pub fn duration_to_s5time(d: i64) -> Result<u16> {
    for (tb, base) in [10 * MILLISECOND, 100 * MILLISECOND, SECOND, 10 * SECOND].into_iter().enumerate() {
        if d % base == 0 || tb == 3 {
            let n = d / base;
            if !(0..=999).contains(&n) {
                continue;
            }
            return Ok(((tb as u16) << 12) | ((n / 100) as u16) << 8 | ((n / 10 % 10) as u16) << 4 | (n % 10) as u16);
        }
    }
    Err(Error::proto(format!("S5TIME out of range: {}", textfmt::duration_string(d))))
}

/// Decodes a DATE_AND_TIME (8 BCD bytes YY MM DD hh mm ss ms ms|wd).
pub fn dt_to_time(b: &[u8]) -> DateTime {
    let mut y = bcd(b[0]);
    if y >= 90 {
        y += 1900;
    } else {
        y += 2000;
    }
    let ms = bcd(b[6]) * 10 + (b[7] >> 4) as i64;
    DateTime::from_civil(y, bcd(b[1]), bcd(b[2]), bcd(b[3]), bcd(b[4]), bcd(b[5]), ms * MILLISECOND)
}

/// Encodes a DATE_AND_TIME (1990..2089), weekday set.
pub fn time_to_dt(t: &DateTime) -> Result<Vec<u8>> {
    let c = t.civil();
    if c.year < 1990 || c.year > 2089 {
        return Err(Error::proto("DATE_AND_TIME must be 1990..2089"));
    }
    let ms = (c.nanosecond / 1_000_000) as i64;
    Ok(vec![
        tobcd(c.year % 100),
        tobcd(c.month as i64),
        tobcd(c.day as i64),
        tobcd(c.hour as i64),
        tobcd(c.minute as i64),
        tobcd(c.second as i64),
        tobcd(ms / 10),
        ((ms % 10) as u8) << 4 | (c.weekday + 1) as u8,
    ])
}

/// Decodes the 12-byte DTL image (year u16, month, day, weekday, h, m, s, ns u32).
pub fn dtl_to_time(b: &[u8]) -> DateTime {
    let y = (b[0] as i64) << 8 | b[1] as i64;
    let ns = u32::from_be_bytes([b[8], b[9], b[10], b[11]]) as i64;
    DateTime::from_civil(y, b[2] as i64, b[3] as i64, b[5] as i64, b[6] as i64, b[7] as i64, ns)
}

/// Encodes the 12-byte DTL image.
pub fn time_to_dtl(t: &DateTime) -> Vec<u8> {
    let c = t.civil();
    let ns = c.nanosecond;
    vec![
        (c.year >> 8) as u8,
        c.year as u8,
        c.month as u8,
        c.day as u8,
        (c.weekday + 1) as u8,
        c.hour as u8,
        c.minute as u8,
        c.second as u8,
        (ns >> 24) as u8,
        (ns >> 16) as u8,
        (ns >> 8) as u8,
        ns as u8,
    ]
}

/// Renders a decoded value like the s7p CLI.
pub fn format_plc(t: &Tag, x: &PlcValue) -> String {
    match x {
        PlcValue::Array(a) => format!("[{}]", a.iter().map(|e| format_plc(t, e)).collect::<Vec<_>>().join(", ")),
        PlcValue::DateTime(d) => {
            if t.softdatatype == SDT_DATE {
                d.format_date()
            } else {
                d.format_plc()
            }
        }
        PlcValue::Duration(d) => {
            if t.softdatatype == SDT_TIME_OF_DAY || t.softdatatype == SDT_LTOD {
                format_tod(*d)
            } else {
                textfmt::duration_string(*d)
            }
        }
        PlcValue::String(s) => quote(s),
        PlcValue::Bytes(b) => hex(b),
        PlcValue::Real(f) => format_float(*f as f64, true),
        PlcValue::LReal(f) => format_float(*f, false),
        PlcValue::UInt(u) => match t.softdatatype {
            SDT_BYTE | SDT_WORD | SDT_DWORD | SDT_LWORD => format!("{u} (16#{u:X})"),
            _ => u.to_string(),
        },
        PlcValue::Int(i) => i.to_string(),
        PlcValue::Bool(b) => b.to_string(),
        PlcValue::Null => "<nil>".into(),
        PlcValue::Raw(v) => fmt_v(&v.v),
    }
}

/// "hh:mm:ss.fffffffff" of a time of day in ns.
fn format_tod(d: i64) -> String {
    let h = d / textfmt::HOUR;
    let m = d % textfmt::HOUR / textfmt::MINUTE;
    let s = d % textfmt::MINUTE;
    let mut ss = format!("{:02}.{:09}", s / SECOND, s % SECOND);
    if let Some(x) = ss.strip_suffix(".000000000") {
        ss = x.to_string();
    }
    format!("{h:02}:{m:02}:{ss}")
}

fn parse_time_local(s: &str) -> Result<DateTime> {
    DateTime::parse(s).ok_or_else(|| Error::proto(format!("bad date/time {s:?} (want YYYY-MM-DDThh:mm:ss.fff)")))
}

fn parse_dur(s: &str) -> Result<i64> {
    let up = s.to_uppercase();
    let up = up.strip_prefix("T#").unwrap_or(&up);
    let up = up.strip_prefix("LT#").unwrap_or(up);
    if let Ok(n) = up.parse::<i64>() {
        return Ok(n.saturating_mul(MILLISECOND));
    }
    let low = up.replace('_', "").to_lowercase();
    textfmt::parse_duration(&low).ok_or_else(|| Error::proto(format!("invalid duration {s:?}")))
}

fn parse_tod(s: &str) -> Result<i64> {
    if !s.contains(':') {
        return parse_dur(s);
    }
    let (h, m, sec, ns) = textfmt::parse_clock(s, true).ok_or_else(|| Error::proto(format!("bad time of day {s:?}")))?;
    Ok(h * textfmt::HOUR + m * textfmt::MINUTE + sec * SECOND + ns)
}

/// Parses a text value into the wire value for the tag. Arrays take
/// comma-separated elements ("1,2,3" or "[1,2,3]"); strings are verbatim.
pub fn encode_plc(t: &Tag, s: &str) -> Result<Value> {
    if t.array {
        let inner = s.strip_prefix('[').unwrap_or(s);
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        let parts: Vec<&str> = inner.split(',').collect();
        if t.count > 0 && parts.len() as u32 != t.count {
            return Err(Error::proto(format!("{} needs {} elements, got {}", t.name, t.count, parts.len())));
        }
        let mut et = t.clone();
        et.array = false;
        let mut wt = 0u8;
        let mut a = Vec::with_capacity(parts.len());
        for p in parts {
            let v = encode_plc(&et, p.trim())?;
            if v.flags != 0 {
                return Err(Error::proto(format!("arrays of {} are not supported", softdatatype_name(t.softdatatype))));
            }
            wt = v.typ;
            a.push(v.v);
        }
        return Ok(Value { flags: FLAG_ARRAY, typ: wt, v: V::Array(a) });
    }
    let syn = |what: &str| Error::proto(format!("parsing {s:?}: invalid {what} for {}", softdatatype_name(t.softdatatype)));
    let pi = |bits| textfmt::parse_int(s, bits).ok_or_else(|| syn("integer"));
    let pu = |bits| textfmt::parse_uint(s, bits).ok_or_else(|| syn("unsigned integer"));
    Ok(match t.softdatatype {
        // BBool: a Bool member of an optimized DB (S7-1500); same wire type as Bool
        SDT_BOOL | SDT_BBOOL => Value::bool(textfmt::parse_bool(s).ok_or_else(|| syn("bool"))?),
        SDT_BYTE => Value::byte(pu(8)? as u8),
        SDT_USINT => Value::usint(pu(8)? as u8),
        SDT_SINT => Value::new(T_SINT, V::I8(pi(8)? as i8)),
        SDT_WORD => Value::word(pu(16)? as u16),
        SDT_UINT => Value::uint(pu(16)? as u16),
        SDT_INT => Value::int(pi(16)? as i16),
        SDT_DWORD => Value::dword(pu(32)? as u32),
        SDT_UDINT => Value::udint(pu(32)? as u32),
        SDT_DINT => Value::dint(pi(32)? as i32),
        SDT_LWORD => Value::new(T_LWORD, V::U64(pu(64)?)),
        SDT_ULINT => Value::ulint(pu(64)?),
        SDT_LINT => Value::lint(pi(64)?),
        SDT_REAL => Value::real(s.parse::<f32>().map_err(|_| syn("float"))?),
        SDT_LREAL => Value::lreal(s.parse::<f64>().map_err(|_| syn("float"))?),
        SDT_CHAR => {
            let r: Vec<char> = s.chars().collect();
            if r.len() != 1 || r[0] as u32 > 255 {
                return Err(Error::proto("CHAR needs one Latin-1 character"));
            }
            Value::usint(r[0] as u32 as u8)
        }
        SDT_WCHAR => {
            let u: Vec<u16> = s.encode_utf16().collect();
            if u.len() != 1 {
                return Err(Error::proto("WCHAR needs one character"));
            }
            Value::uint(u[0])
        }
        SDT_DATE => {
            let d = parse_time_local(s)?;
            let days = d.sub(&epoch1990()) as f64 / SECOND as f64 / 3600.0 / 24.0;
            if days < 0.0 || days > u16::MAX as f64 {
                return Err(Error::proto("DATE out of range"));
            }
            Value::uint(days as u16)
        }
        SDT_TIME_OF_DAY => {
            let d = parse_tod(s).ok().filter(|d| (0..24 * textfmt::HOUR).contains(d));
            let d = d.ok_or_else(|| Error::proto(format!("bad TIME_OF_DAY {s:?}")))?;
            Value::udint((d / MILLISECOND) as u32)
        }
        SDT_LTOD => {
            let d = parse_tod(s).ok().filter(|d| (0..24 * textfmt::HOUR).contains(d));
            let d = d.ok_or_else(|| Error::proto(format!("bad LTOD {s:?}")))?;
            Value::ulint(d as u64)
        }
        SDT_TIME => {
            let ms = parse_dur(s)? / MILLISECOND;
            if ms > i32::MAX as i64 || ms < i32::MIN as i64 {
                return Err(Error::proto("TIME out of range"));
            }
            Value::dint(ms as i32)
        }
        SDT_LTIME => Value::new(T_TIMESPAN, V::I64(parse_dur(s)?)),
        SDT_S5TIME => Value::word(duration_to_s5time(parse_dur(s)?)?),
        SDT_LDT => {
            let tm = parse_time_local(s)?;
            Value::timestamp(tm.sub(&DateTime::default()) as u64)
        }
        SDT_DATE_AND_TIME => Value::usint_array(&time_to_dt(&parse_time_local(s)?)?),
        SDT_DTL => {
            let tm = parse_time_local(s)?;
            let ts = if t.packed_ts == 0 { DEFAULT_DTL_TIMESTAMP } else { t.packed_ts };
            Value::structv(Struct { id: TI_DTL, packed_ts: ts, packed_tf: 2, packed: Some(time_to_dtl(&tm)), elems: Vec::new() })
        }
        SDT_STRING => {
            let max = if t.max_len == 0 { 254 } else { t.max_len as usize };
            let r: Vec<char> = s.chars().collect();
            if r.len() > max {
                return Err(Error::proto(format!("string longer than {max}")));
            }
            let mut b = vec![0u8; max + 2];
            b[0] = max as u8;
            b[1] = r.len() as u8;
            for (i, c) in r.iter().enumerate() {
                if *c as u32 > 255 {
                    return Err(Error::proto("STRING is Latin-1, use WSTRING"));
                }
                b[2 + i] = *c as u32 as u8;
            }
            Value::usint_array(&b)
        }
        SDT_WSTRING => {
            let max = if t.max_len == 0 { 254 } else { t.max_len as usize };
            let u: Vec<u16> = s.encode_utf16().collect();
            if u.len() > max {
                return Err(Error::proto(format!("wstring longer than {max}")));
            }
            let mut a = Vec::with_capacity(u.len() + 2);
            a.push(V::U16(max as u16));
            a.push(V::U16(u.len() as u16));
            a.extend(u.into_iter().map(V::U16));
            Value { flags: FLAG_ARRAY, typ: T_UINT, v: V::Array(a) }
        }
        sdt => {
            return Err(Error::proto(format!("writing {} is not supported", softdatatype_name(sdt))));
        }
    })
}

/// The result of a symbolic read.
#[derive(Debug)]
pub struct TagResult {
    /// The converted value (None on error).
    pub value: Option<PlcValue>,
    pub raw: Value,
    pub err: Option<Error>,
}

impl Client {
    /// Reads resolved tags and converts the values. A DTL tag learns its
    /// interface timestamp (needed to write it back).
    pub fn read_tags(&self, tags: &mut [Tag]) -> Result<Vec<TagResult>> {
        let addrs: Vec<ItemAddress> = tags.iter().map(|t| t.addr.clone()).collect();
        let (vals, errs) = self.read(&addrs)?;
        let mut out = Vec::with_capacity(tags.len());
        for (i, (raw, e)) in vals.into_iter().zip(errs).enumerate() {
            if e != 0 {
                out.push(TagResult { value: None, raw, err: Some(Error::Item(e)) });
                continue;
            }
            match decode_plc_ts(&tags[i], &raw) {
                Ok((x, ts)) => {
                    if let Some(ts) = ts {
                        tags[i].packed_ts = ts;
                    }
                    out.push(TagResult { value: Some(x), raw, err: None });
                }
                Err(err) => out.push(TagResult { value: None, raw, err: Some(err) }),
            }
        }
        Ok(out)
    }

    /// Writes text values to resolved tags (per-item errors in the result).
    pub fn write_tags(&self, tags: &[Tag], text: &[&str]) -> Result<Vec<Option<Error>>> {
        let mut out: Vec<Option<Error>> = (0..tags.len()).map(|_| None).collect();
        let mut addrs = Vec::new();
        let mut vals = Vec::new();
        let mut idx = Vec::new();
        for (i, t) in tags.iter().enumerate() {
            match encode_plc(t, text.get(i).copied().unwrap_or("")) {
                Ok(v) => {
                    addrs.push(t.addr.clone());
                    vals.push(v);
                    idx.push(i);
                }
                Err(e) => out[i] = Some(e),
            }
        }
        self.write_values_into(&addrs, &vals, &idx, &mut out)?;
        Ok(out)
    }

    /// Writes already encoded values to resolved tags.
    pub fn write_tag_values(&self, tags: &[Tag], vals: &[Value]) -> Result<Vec<Option<Error>>> {
        let mut out: Vec<Option<Error>> = (0..tags.len()).map(|_| None).collect();
        let addrs: Vec<ItemAddress> = tags.iter().map(|t| t.addr.clone()).collect();
        let idx: Vec<usize> = (0..tags.len()).collect();
        self.write_values_into(&addrs, vals, &idx, &mut out)?;
        Ok(out)
    }

    fn write_values_into(&self, addrs: &[ItemAddress], vals: &[Value], idx: &[usize], out: &mut [Option<Error>]) -> Result<()> {
        if addrs.is_empty() {
            return Ok(());
        }
        let errs = self.write(addrs, vals)?;
        for (k, i) in idx.iter().enumerate() {
            if errs[k] != 0 {
                out[*i] = Some(Error::Item(errs[k]));
            }
        }
        Ok(())
    }
}
