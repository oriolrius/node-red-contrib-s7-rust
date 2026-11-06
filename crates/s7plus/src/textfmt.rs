//! Small text formatting and parsing helpers: shortest `%g` floats, quoted
//! strings, durations ("1h2m3.5s") in both directions, integers with base
//! prefixes, and a minimal UTC date-time type (no external time crate).

use std::fmt::Write as _;

/// Shortest `%g` form of a float (`bits32`: shortest form that round-trips as f32).
pub fn format_float(v: f64, bits32: bool) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // shortest round-trip digits from Rust's {:e}
    let s = if bits32 { format!("{:e}", v as f32) } else { format!("{:e}", v) };
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, s),
    };
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let nd = digits.len() as i32;
    let dp = exp + 1; // decimal point position
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    let x = dp - 1;
    if !(-4..6).contains(&x) {
        // %e
        out.push_str(&digits[..1]);
        if nd > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        if x < 0 {
            out.push('-');
        } else {
            out.push('+');
        }
        let ax = x.abs();
        if ax < 10 {
            out.push('0');
        }
        let _ = write!(out, "{ax}");
        return out;
    }
    // %f
    if dp <= 0 {
        out.push_str("0.");
        for _ in 0..(-dp) {
            out.push('0');
        }
        out.push_str(digits);
    } else if dp >= nd {
        out.push_str(digits);
        for _ in 0..(dp - nd) {
            out.push('0');
        }
    } else {
        out.push_str(&digits[..dp as usize]);
        out.push('.');
        out.push_str(&digits[dp as usize..]);
    }
    out
}

/// Double-quoted string with backslash escapes.
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\x07' => o.push_str("\\a"),
            '\x08' => o.push_str("\\b"),
            '\x0c' => o.push_str("\\f"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\x0b' => o.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(o, "\\x{:02x}", c as u32);
            }
            c if c.is_control() => {
                if (c as u32) < 0x10000 {
                    let _ = write!(o, "\\u{:04x}", c as u32);
                } else {
                    let _ = write!(o, "\\U{:08x}", c as u32);
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Lower-case hex.
pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// Decodes a hex string (test and fake helper).
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Signed integer of `bits` bits with optional base prefix (0x, 0o, 0b, leading 0 = octal).
pub fn parse_int(s: &str, bits: u32) -> Option<i64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let u = parse_uint_body(body, 64)? as u128;
    let lim: u128 = 1u128 << (bits - 1);
    if neg {
        if u > lim {
            return None;
        }
        Some((-(u as i128)) as i64)
    } else {
        if u >= lim {
            return None;
        }
        Some(u as i64)
    }
}

/// Unsigned integer of `bits` bits with optional base prefix (0x, 0o, 0b, leading 0 = octal).
pub fn parse_uint(s: &str, bits: u32) -> Option<u64> {
    parse_uint_body(s, bits)
}

fn parse_uint_body(s: &str, bits: u32) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let (base, digits, prefixed) = if let Some(r) = lower.strip_prefix("0x") {
        (16, r.to_string(), true)
    } else if let Some(r) = lower.strip_prefix("0b") {
        (2, r.to_string(), true)
    } else if let Some(r) = lower.strip_prefix("0o") {
        (8, r.to_string(), true)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (8, lower[1..].to_string(), true)
    } else {
        (10, lower.clone(), false)
    };
    // underscores are allowed with base 0 between digits
    if (digits.starts_with('_') && !prefixed) || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let digits: String = digits.chars().filter(|c| *c != '_').collect();
    if digits.is_empty() {
        return None;
    }
    let v = u64::from_str_radix(&digits, base).ok()?;
    if bits < 64 && v >= 1u64 << bits {
        return None;
    }
    Some(v)
}

/// Boolean: 1/t/T/TRUE/true/True or 0/f/F/FALSE/false/False.
pub fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

pub const NANOSECOND: i64 = 1;
pub const MICROSECOND: i64 = 1_000;
pub const MILLISECOND: i64 = 1_000_000;
pub const SECOND: i64 = 1_000_000_000;
pub const MINUTE: i64 = 60 * SECOND;
pub const HOUR: i64 = 60 * MINUTE;

fn frac(v: u64, prec: u32) -> (u64, String) {
    let p = 10u64.pow(prec);
    let f = v % p;
    let i = v / p;
    if f == 0 {
        return (i, String::new());
    }
    let s = format!("{:0width$}", f, width = prec as usize);
    (i, format!(".{}", s.trim_end_matches('0')))
}

/// Text form ("1h2m3.5s", "1.5ms", "0s") of a duration in nanoseconds.
pub fn duration_string(d: i64) -> String {
    let neg = d < 0;
    let u = d.unsigned_abs();
    let mut out = String::new();
    if u < SECOND as u64 {
        if u == 0 {
            return "0s".into();
        }
        let (prec, unit) = if u < MICROSECOND as u64 {
            (0, "ns")
        } else if u < MILLISECOND as u64 {
            (3, "µs")
        } else {
            (6, "ms")
        };
        let (i, f) = frac(u, prec);
        let _ = write!(out, "{i}{f}{unit}");
    } else {
        let (secs, f) = frac(u, 9);
        let s = secs % 60;
        let m = secs / 60 % 60;
        let h = secs / 3600;
        if h > 0 {
            let _ = write!(out, "{h}h{m}m{s}{f}s");
        } else if secs / 60 > 0 {
            let _ = write!(out, "{m}m{s}{f}s");
        } else {
            let _ = write!(out, "{s}{f}s");
        }
    }
    if neg {
        out.insert(0, '-');
    }
    out
}

/// Parses a duration ("1h2m3.5s", "-500ms", "1.5us"...) into nanoseconds.
pub fn parse_duration(s0: &str) -> Option<i64> {
    let mut s = s0;
    let mut neg = false;
    if let Some(r) = s.strip_prefix('-') {
        neg = true;
        s = r;
    } else if let Some(r) = s.strip_prefix('+') {
        s = r;
    }
    if s == "0" {
        return Some(0);
    }
    if s.is_empty() {
        return None;
    }
    let mut total: u128 = 0;
    while !s.is_empty() {
        let b = s.as_bytes();
        if !(b[0] == b'.' || b[0].is_ascii_digit()) {
            return None;
        }
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let int_part = &s[..i];
        let mut frac_part = "";
        let mut j = i;
        if j < b.len() && b[j] == b'.' {
            j += 1;
            let k = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            frac_part = &s[k..j];
        }
        if int_part.is_empty() && frac_part.is_empty() {
            return None;
        }
        let rest = &s[j..];
        let ulen = rest.find(|c: char| c == '.' || c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = match &rest[..ulen] {
            "ns" => NANOSECOND,
            "us" | "µs" | "μs" => MICROSECOND,
            "ms" => MILLISECOND,
            "s" => SECOND,
            "m" => MINUTE,
            "h" => HOUR,
            _ => return None,
        } as u128;
        let iv: u128 = if int_part.is_empty() { 0 } else { int_part.parse().ok()? };
        let mut v = iv.checked_mul(unit)?;
        if !frac_part.is_empty() {
            let mut scale: u128 = 1;
            let mut fv: u128 = 0;
            for c in frac_part.bytes().take(30) {
                fv = fv * 10 + (c - b'0') as u128;
                scale *= 10;
            }
            v += fv * unit / scale;
        }
        total = total.checked_add(v)?;
        if total > (1u128 << 63) {
            return None;
        }
        s = &rest[ulen..];
    }
    if neg {
        Some((-(total as i128)) as i64)
    } else {
        if total > i64::MAX as u128 {
            return None;
        }
        Some(total as i64)
    }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A UTC date and time with nanoseconds (PLC local times are shown as
/// UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DateTime {
    /// Seconds since 1970-01-01T00:00:00Z.
    pub unix_secs: i64,
    /// 0..1e9.
    pub nanos: u32,
}

/// Broken-down date and time of a [`DateTime`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
    /// 0 = Sunday … 6 = Saturday.
    pub weekday: u32,
}

impl DateTime {
    /// From calendar fields in UTC: out-of-range fields are normalized.
    pub fn from_civil(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64, nsec: i64) -> DateTime {
        let y = year + (month - 1).div_euclid(12);
        let m = (month - 1).rem_euclid(12) + 1;
        let days = days_from_civil(y, m, 1) + day - 1;
        let secs = days * 86400 + hour * 3600 + min * 60 + sec + nsec.div_euclid(SECOND);
        DateTime { unix_secs: secs, nanos: nsec.rem_euclid(SECOND) as u32 }
    }

    /// From nanoseconds since 1970.
    pub fn from_unix_nanos(ns: i64) -> DateTime {
        DateTime { unix_secs: ns.div_euclid(SECOND), nanos: ns.rem_euclid(SECOND) as u32 }
    }

    /// Nanoseconds since 1970 (saturating).
    pub fn unix_nanos(&self) -> i64 {
        let v = self.unix_secs as i128 * SECOND as i128 + self.nanos as i128;
        v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Adds a duration in nanoseconds.
    pub fn add_nanos(&self, d: i64) -> DateTime {
        let t = self.unix_secs as i128 * SECOND as i128 + self.nanos as i128 + d as i128;
        DateTime { unix_secs: t.div_euclid(SECOND as i128) as i64, nanos: t.rem_euclid(SECOND as i128) as u32 }
    }

    /// Difference self - other in nanoseconds (saturating).
    pub fn sub(&self, other: &DateTime) -> i64 {
        let a = self.unix_secs as i128 * SECOND as i128 + self.nanos as i128;
        let b = other.unix_secs as i128 * SECOND as i128 + other.nanos as i128;
        (a - b).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    pub fn civil(&self) -> Civil {
        let days = self.unix_secs.div_euclid(86400);
        let sod = self.unix_secs.rem_euclid(86400) as u32;
        let (year, month, day) = civil_from_days(days);
        Civil {
            year,
            month,
            day,
            hour: sod / 3600,
            minute: sod / 60 % 60,
            second: sod % 60,
            nanosecond: self.nanos,
            weekday: (days + 4).rem_euclid(7) as u32,
        }
    }

    fn frac9(&self) -> String {
        if self.nanos == 0 {
            return String::new();
        }
        let s = format!("{:09}", self.nanos);
        format!(".{}", s.trim_end_matches('0'))
    }

    /// "YYYY-MM-DD".
    pub fn format_date(&self) -> String {
        let c = self.civil();
        format!("{:04}-{:02}-{:02}", c.year, c.month, c.day)
    }

    /// "YYYY-MM-DDThh:mm:ss.fffffffff" (trailing zeros trimmed).
    pub fn format_plc(&self) -> String {
        let c = self.civil();
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}", c.year, c.month, c.day, c.hour, c.minute, c.second, self.frac9())
    }

    /// RFC 3339 with nanoseconds, UTC ("YYYY-MM-DDThh:mm:ss.fffffffffZ").
    pub fn rfc3339_nano(&self) -> String {
        format!("{}Z", self.format_plc())
    }

    /// Parses "YYYY-MM-DDThh:mm:ss.fffffffff", "YYYY-MM-DD hh:mm:ss.fff",
    /// "YYYY-MM-DDThh:mm:ss" or "YYYY-MM-DD".
    pub fn parse(s: &str) -> Option<DateTime> {
        let b = s.as_bytes();
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |r: &str| -> Option<i64> {
            if r.bytes().all(|c| c.is_ascii_digit()) && !r.is_empty() {
                r.parse().ok()
            } else {
                None
            }
        };
        let y = num(&s[0..4])?;
        let mo = num(&s[5..7])?;
        let d = num(&s[8..10])?;
        if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) {
            return None;
        }
        if b.len() == 10 {
            return Some(DateTime::from_civil(y, mo, d, 0, 0, 0, 0));
        }
        if b[10] != b'T' && b[10] != b' ' {
            return None;
        }
        let (h, mi, sec, ns) = parse_clock(&s[11..], false)?;
        Some(DateTime::from_civil(y, mo, d, h, mi, sec, ns))
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// Parses "hh:mm:ss[.fffffffff]". With `loose_hour` the hour may have one digit.
pub(crate) fn parse_clock(s: &str, loose_hour: bool) -> Option<(i64, i64, i64, i64)> {
    let mut parts = s.splitn(3, ':');
    let hs = parts.next()?;
    let ms = parts.next()?;
    let rest = parts.next()?;
    let ok_h = if loose_hour { (1..=2).contains(&hs.len()) } else { hs.len() == 2 };
    if !ok_h || ms.len() != 2 || !hs.bytes().all(|c| c.is_ascii_digit()) || !ms.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (ss, fs) = match rest.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    if ss.len() != 2 || !ss.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = hs.parse().ok()?;
    let mi: i64 = ms.parse().ok()?;
    let sec: i64 = ss.parse().ok()?;
    if h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let mut ns = 0i64;
    if let Some(f) = fs {
        if f.is_empty() || f.len() > 9 || !f.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        ns = f.parse::<i64>().ok()? * 10i64.pow(9 - f.len() as u32);
    }
    Some((h, mi, sec, ns))
}
