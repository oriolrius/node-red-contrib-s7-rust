//! Text formatting and parsing of values: integers with base prefixes,
//! booleans, floats (shortest `%g` form), quoted strings, durations
//! ("1h2m3.5s") and date/time layouts (calendar normalisation, the
//! "YYYY-MM-DD hh:mm:ss.fffffffff" layout, RFC 3339 nano).

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, NaiveDateTime, Offset, TimeDelta, TimeZone, Timelike, Utc};

use crate::error::{local, Error, Result};

// ---- numbers and booleans --------------------------------------------------

/// `what` names the expected kind of value ("integer", "boolean"...).
fn syntax(what: &str, s: &str) -> Error {
    local(format!("invalid {what} {}", quote(s.as_bytes())))
}

fn range(what: &str, s: &str) -> Error {
    local(format!("{what} {} out of range", quote(s.as_bytes())))
}

/// Digits of an unsigned number with base prefixes (0x, 0o, 0b, leading 0 = octal)
/// and underscores (only allowed with base 0 prefixes, between digits).
fn parse_magnitude(what: &str, orig: &str, s: &str) -> Result<u128> {
    if s.is_empty() {
        return Err(syntax(what, orig));
    }
    let b = s.as_bytes();
    let (base, mut digits, prefixed) = if b.len() >= 2 && b[0] == b'0' {
        match b[1] {
            b'x' | b'X' => (16u32, &s[2..], true),
            b'o' | b'O' => (8, &s[2..], true),
            b'b' | b'B' => (2, &s[2..], true),
            _ => (8, &s[1..], true),
        }
    } else {
        (10, s, false)
    };
    if digits.is_empty() && base != 8 {
        return Err(syntax(what, orig));
    }
    if digits.is_empty() {
        digits = "0";
    }
    // underscores: allowed only with a prefix (base 0), never doubled or trailing
    // "0_1" (octal with leading 0) is fine, "0x_1" too
    if digits.contains('_') && (!prefixed || digits.ends_with('_') || digits.contains("__") || base == 10) {
        return Err(syntax(what, orig));
    }
    let mut v: u128 = 0;
    let mut any = false;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(base).ok_or_else(|| syntax(what, orig))?;
        any = true;
        v = v.saturating_mul(base as u128).saturating_add(d as u128);
    }
    if !any {
        return Err(syntax(what, orig));
    }
    Ok(v)
}

/// Signed integer of `bits` bits with optional base prefix.
pub fn parse_int(s: &str, bits: u32) -> Result<i64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let m = parse_magnitude("integer", s, body)?;
    let cut = 1u128 << (bits - 1);
    if (!neg && m >= cut) || (neg && m > cut) {
        return Err(range("integer", s));
    }
    Ok(if neg { (m as i128).wrapping_neg() as i64 } else { m as i64 })
}

/// Unsigned integer of `bits` bits with optional base prefix.
pub fn parse_uint(s: &str, bits: u32) -> Result<u64> {
    let m = parse_magnitude("unsigned integer", s, s)?;
    if m >= 1u128 << bits {
        return Err(range("unsigned integer", s));
    }
    Ok(m as u64)
}

/// Decimal i64, optional sign.
pub fn parse_dec_i64(s: &str) -> Option<i64> {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if body.is_empty() || !body.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Decimal u64: digits only.
pub fn parse_dec_u64(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Boolean: 1/t/T/TRUE/true/True or 0/f/F/FALSE/false/False.
pub fn parse_bool(s: &str) -> Result<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(syntax("boolean", s)),
    }
}

fn is_float_syntax(s: &str) -> bool {
    let t = s.strip_prefix(['+', '-']).unwrap_or(s).to_ascii_lowercase();
    if t == "inf" || t == "infinity" || t == "nan" {
        return true;
    }
    // [digits][.digits][e[+-]digits] with at least one digit in the mantissa
    let b = t.as_bytes();
    let mut i = 0;
    let mut mant = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        mant += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            mant += 1;
        }
    }
    if mant == 0 {
        return false;
    }
    if i < b.len() && b[i] == b'e' {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st {
            return false;
        }
    }
    i == b.len()
}

/// Float of `bits` bits (decimal forms, inf, nan).
pub fn parse_float(s: &str, bits: u32) -> Result<f64> {
    if !is_float_syntax(s) {
        return Err(syntax("number", s));
    }
    let lower = s.to_ascii_lowercase();
    let special = lower.contains("inf") || lower.contains("nan");
    let v = if bits == 32 { s.parse::<f32>().map(|x| x as f64) } else { s.parse::<f64>() }.map_err(|_| syntax("number", s))?;
    if v.is_infinite() && !special {
        return Err(range("number", s));
    }
    Ok(v)
}

/// Shortest `%g` representation, %e form
/// when the exponent is < -4 or >= 6 (1e+06, 123456.7, 1.234567e+06).
pub fn format_float(v: f64, bits: u32) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    // shortest digits via Rust's {:e} (also shortest round-trip)
    let e = if bits == 32 { format!("{:e}", v as f32) } else { format!("{:e}", v) };
    let (neg, e) = match e.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, e.as_str()),
    };
    let (mant, exp) = e.split_once('e').unwrap_or((e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if digits == "0" {
        out.push('0');
        return out;
    }
    let nd = digits.len() as i32;
    let dp = exp + 1; // decimal point position
    if !(-4..6).contains(&exp) {
        // %e: d.ddde±XX
        out.push_str(&digits[..1]);
        if nd > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let a = exp.unsigned_abs();
        if a < 10 {
            out.push('0');
        }
        out.push_str(&a.to_string());
        return out;
    }
    // %f with max(nd - dp, 0) decimals
    if dp <= 0 {
        out.push_str("0.");
        for _ in 0..(-dp) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if dp >= nd {
        out.push_str(&digits);
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

fn is_print(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(c as u32, 0xAD | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x2064 | 0xFEFF | 0xFFF9..=0xFFFB)
}

/// Double-quoted, escaped form of a byte string (invalid UTF-8 bytes become `\xNN`).
pub fn quote(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() + 2);
    out.push('"');
    let mut rest = b;
    while !rest.is_empty() {
        let (c, w) = match std::str::from_utf8(rest) {
            Ok(s) => {
                let c = s.chars().next().unwrap();
                (Some(c), c.len_utf8())
            }
            Err(e) if e.valid_up_to() > 0 => {
                let s = std::str::from_utf8(&rest[..e.valid_up_to()]).unwrap();
                let c = s.chars().next().unwrap();
                (Some(c), c.len_utf8())
            }
            Err(_) => (None, 1),
        };
        match c {
            None => out.push_str(&format!("\\x{:02x}", rest[0])),
            Some('"') => out.push_str("\\\""),
            Some('\\') => out.push_str("\\\\"),
            Some(c) if is_print(c) => out.push(c),
            Some('\x07') => out.push_str("\\a"),
            Some('\x08') => out.push_str("\\b"),
            Some('\x0c') => out.push_str("\\f"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some('\t') => out.push_str("\\t"),
            Some('\x0b') => out.push_str("\\v"),
            Some(c) if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            Some(c) if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            Some(c) => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
        rest = &rest[w..];
    }
    out.push('"');
    out
}

// ---- time.Duration ----------------------------------------------------------

pub const NANOSECOND: i64 = 1;
pub const MICROSECOND: i64 = 1_000;
pub const MILLISECOND: i64 = 1_000_000;
pub const SECOND: i64 = 1_000_000_000;
pub const MINUTE: i64 = 60 * SECOND;
pub const HOUR: i64 = 60 * MINUTE;

fn fmt_frac(v: u64, prec: u32) -> (String, u64) {
    // fraction of v / 10^prec, trailing zeros omitted, "." omitted if zero
    let mut digits = Vec::new();
    let mut print = false;
    let mut v = v;
    for _ in 0..prec {
        let d = v % 10;
        print = print || d != 0;
        if print {
            digits.push(b'0' + d as u8);
        }
        v /= 10;
    }
    if print {
        digits.push(b'.');
    }
    digits.reverse();
    (String::from_utf8(digits).unwrap(), v)
}

/// Text form of a duration in nanoseconds ("1h2m3.5s", "1.5ms", "0s").
pub fn duration_string(ns: i64) -> String {
    let neg = ns < 0;
    let u = ns.unsigned_abs();
    let mut s;
    if u < SECOND as u64 {
        let (prec, unit) = match u {
            0 => return "0s".into(),
            x if x < MICROSECOND as u64 => (0, "ns"),
            x if x < MILLISECOND as u64 => (3, "µs"),
            _ => (6, "ms"),
        };
        let (frac, int) = fmt_frac(u, prec);
        s = format!("{int}{frac}{unit}");
    } else {
        let (frac, mut u) = fmt_frac(u, 9);
        s = format!("{}{frac}s", u % 60);
        u /= 60;
        if u > 0 {
            s = format!("{}m{s}", u % 60);
            u /= 60;
            if u > 0 {
                s = format!("{u}h{s}");
            }
        }
    }
    if neg {
        s.insert(0, '-');
    }
    s
}

/// Parses a duration ("1h2m3.5s", "-500ms", "1.5µs"); returns nanoseconds.
pub fn parse_duration(orig: &str) -> Result<i64> {
    let bad = || local(format!("invalid duration {}", quote(orig.as_bytes())));
    let mut s = orig;
    let mut neg = false;
    if let Some(c) = s.chars().next() {
        if c == '-' || c == '+' {
            neg = c == '-';
            s = &s[1..];
        }
    }
    if s == "0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(bad());
    }
    let mut d: u64 = 0;
    while !s.is_empty() {
        let b = s.as_bytes();
        if !(b[0] == b'.' || b[0].is_ascii_digit()) {
            return Err(bad());
        }
        // leading int
        let mut i = 0;
        let mut v: u64 = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            if v > (1u64 << 63) / 10 {
                return Err(bad());
            }
            v = v * 10 + (b[i] - b'0') as u64;
            if v > 1u64 << 63 {
                return Err(bad());
            }
            i += 1;
        }
        let pre = i > 0;
        s = &s[i..];
        let (mut f, mut scale, mut post) = (0u64, 1f64, false);
        if s.starts_with('.') {
            s = &s[1..];
            let b = s.as_bytes();
            let mut j = 0;
            let mut overflow = false;
            while j < b.len() && b[j].is_ascii_digit() {
                if !overflow {
                    if f > (u64::MAX - 9) / 10 {
                        overflow = true;
                    } else {
                        f = f * 10 + (b[j] - b'0') as u64;
                        scale *= 10.0;
                    }
                }
                j += 1;
            }
            post = j > 0;
            s = &s[j..];
        }
        if !pre && !post {
            return Err(bad());
        }
        let b = s.as_bytes();
        let mut k = 0;
        while k < b.len() && !(b[k] == b'.' || b[k].is_ascii_digit()) {
            k += 1;
        }
        if k == 0 {
            return Err(local(format!("missing unit in duration {}", quote(orig.as_bytes()))));
        }
        let u = &s[..k];
        s = &s[k..];
        let unit: u64 = match u {
            "ns" => 1,
            "us" | "µs" | "μs" => MICROSECOND as u64,
            "ms" => MILLISECOND as u64,
            "s" => SECOND as u64,
            "m" => MINUTE as u64,
            "h" => HOUR as u64,
            _ => return Err(local(format!("unknown unit {} in duration {}", quote(u.as_bytes()), quote(orig.as_bytes())))),
        };
        if v > (1u64 << 63) / unit {
            return Err(bad());
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1u64 << 63 {
                return Err(bad());
            }
        }
        d += v;
        if d > 1u64 << 63 {
            return Err(bad());
        }
    }
    if neg {
        return Ok((d as i128).wrapping_neg() as i64);
    }
    if d > (1u64 << 63) - 1 {
        return Err(bad());
    }
    Ok(d as i64)
}

// ---- time.Time --------------------------------------------------------------

/// A time zone for wall-clock fields: the host's local
/// zone, UTC, or a fixed offset (no IANA database here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    #[default]
    Local,
    Utc,
    Fixed(FixedOffset),
}

impl Zone {
    /// The same instant in this zone.
    pub fn convert(&self, t: &DateTime<FixedOffset>) -> DateTime<FixedOffset> {
        match self {
            Zone::Local => t.with_timezone(&Local).fixed_offset(),
            Zone::Utc => t.with_timezone(&Utc).fixed_offset(),
            Zone::Fixed(o) => t.with_timezone(o),
        }
    }

    /// Resolves a wall-clock time in this zone (gap / overlap: earliest).
    pub fn resolve(&self, ndt: &NaiveDateTime) -> DateTime<FixedOffset> {
        match self {
            Zone::Utc => Utc.from_utc_datetime(ndt).fixed_offset(),
            Zone::Fixed(o) => {
                let off = TimeDelta::seconds(o.local_minus_utc() as i64);
                o.from_utc_datetime(&(*ndt - off))
            }
            Zone::Local => match Local.from_local_datetime(ndt).earliest() {
                Some(t) => t.fixed_offset(),
                None => {
                    // inside a DST gap: use the offset in force just before it
                    let off = Local.offset_from_utc_datetime(ndt).fix();
                    let utc = *ndt - TimeDelta::seconds(off.local_minus_utc() as i64);
                    Local.from_utc_datetime(&utc).fixed_offset()
                }
            },
        }
    }

    /// Current time in this zone.
    pub fn now(&self) -> DateTime<FixedOffset> {
        self.convert(&Utc::now().fixed_offset())
    }
}

fn epoch_utc() -> DateTime<FixedOffset> {
    DateTime::<Utc>::UNIX_EPOCH.fixed_offset()
}

/// A date/time from calendar fields in `zone`, including the normalisation of
/// out-of-range fields (month 13 = January of next year, day 0 = last day of
/// previous month…).
#[allow(clippy::too_many_arguments)]
pub fn make_date(y: i64, mon: i64, d: i64, h: i64, mi: i64, s: i64, ns: i64, zone: Zone) -> DateTime<FixedOffset> {
    let m0 = mon - 1;
    let y = y + m0.div_euclid(12);
    let m = m0.rem_euclid(12) + 1;
    let Some(first) = i32::try_from(y).ok().and_then(|y| NaiveDate::from_ymd_opt(y, m as u32, 1)) else {
        return epoch_utc();
    };
    let delta = TimeDelta::try_days(d - 1)
        .and_then(|x| x.checked_add(&TimeDelta::try_hours(h)?))
        .and_then(|x| x.checked_add(&TimeDelta::try_minutes(mi)?))
        .and_then(|x| x.checked_add(&TimeDelta::try_seconds(s)?))
        .and_then(|x| x.checked_add(&TimeDelta::nanoseconds(ns)));
    let Some(ndt) = delta.and_then(|dl| first.and_hms_opt(0, 0, 0)?.checked_add_signed(dl)) else {
        return epoch_utc();
    };
    zone.resolve(&ndt)
}

/// Unix nanoseconds to a UTC time.
pub fn unix_nano_utc(ns: i64) -> DateTime<FixedOffset> {
    DateTime::<Utc>::from_timestamp_nanos(ns).fixed_offset()
}

/// Unix nanoseconds of a time.
pub fn unix_nano(t: &DateTime<FixedOffset>) -> i64 {
    t.timestamp_nanos_opt().unwrap_or(if t.timestamp() < 0 { i64::MIN } else { i64::MAX })
}

/// Weekday: Sunday = 0.
pub fn weekday(t: &DateTime<FixedOffset>) -> u32 {
    t.weekday().num_days_from_sunday()
}

fn frac9(ns: u32) -> String {
    if ns == 0 {
        return String::new();
    }
    let s = format!(".{ns:09}");
    s.trim_end_matches('0').to_string()
}

fn year4(y: i32) -> String {
    if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{y:04}")
    }
}

/// Layout "YYYY-MM-DD".
pub fn fmt_date(t: &DateTime<FixedOffset>) -> String {
    format!("{}-{:02}-{:02}", year4(t.year()), t.month(), t.day())
}

/// Layout "YYYY-MM-DD hh:mm:ss".
fn fmt_dt(t: &DateTime<FixedOffset>, sep: char) -> String {
    format!("{}{sep}{:02}:{:02}:{:02}", fmt_date(t), t.hour(), t.minute(), t.second())
}

/// Layout "YYYY-MM-DD hh:mm:ss.fffffffff" (trailing zeros dropped).
pub fn fmt_ns_layout(t: &DateTime<FixedOffset>) -> String {
    format!("{}{}", fmt_dt(t, ' '), frac9(t.nanosecond() % 1_000_000_000))
}

/// Layout "YYYY-MM-DD hh:mm:ss.fff".
pub fn fmt_ms_layout(t: &DateTime<FixedOffset>) -> String {
    format!("{}.{:03}", fmt_dt(t, ' '), (t.nanosecond() % 1_000_000_000) / 1_000_000)
}

/// RFC 3339 with nanoseconds ("YYYY-MM-DDThh:mm:ss.fffffffffZ07:00").
pub fn fmt_rfc3339_nano(t: &DateTime<FixedOffset>) -> String {
    let off = t.offset().local_minus_utc();
    let z = if off == 0 {
        "Z".to_string()
    } else {
        let a = off.unsigned_abs();
        format!("{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, a / 3600, a % 3600 / 60)
    };
    format!("{}{}{z}", fmt_dt(t, 'T'), frac9(t.nanosecond() % 1_000_000_000))
}

/// Layout "hh:mm:ss.fffffffff" of a duration since midnight (wraps every 24 h).
pub fn fmt_tod(ns: i64) -> String {
    let t = ns.rem_euclid(24 * HOUR);
    let (h, m, s, f) = (t / HOUR, t / MINUTE % 60, t / SECOND % 60, (t % SECOND) as u32);
    format!("{h:02}:{m:02}:{s:02}{}", frac9(f))
}

/// Parsing cursor for the fixed layouts.
struct Cur<'a> {
    s: &'a [u8],
}

impl<'a> Cur<'a> {
    fn num(&mut self, min: usize, max: usize) -> Option<i64> {
        let n = self.s.iter().take(max).take_while(|c| c.is_ascii_digit()).count();
        if n < min {
            return None;
        }
        let v = std::str::from_utf8(&self.s[..n]).ok()?.parse().ok()?;
        self.s = &self.s[n..];
        Some(v)
    }
    fn lit(&mut self, c: u8) -> Option<()> {
        if self.s.first() == Some(&c) {
            self.s = &self.s[1..];
            Some(())
        } else {
            None
        }
    }
    /// Optional ".ddd" / ",ddd" fraction → nanoseconds.
    fn frac(&mut self) -> i64 {
        if self.s.len() >= 2 && (self.s[0] == b'.' || self.s[0] == b',') && self.s[1].is_ascii_digit() {
            let n = self.s[1..].iter().take_while(|c| c.is_ascii_digit()).count();
            let mut ns: i64 = 0;
            for i in 0..9 {
                ns = ns * 10 + if i < n { (self.s[1 + i] - b'0') as i64 } else { 0 };
            }
            self.s = &self.s[1 + n..];
            ns
        } else {
            0
        }
    }
    fn date(&mut self) -> Option<(i64, i64, i64)> {
        let y = self.num(4, 4)?;
        self.lit(b'-')?;
        let m = self.num(2, 2)?;
        self.lit(b'-')?;
        let d = self.num(2, 2)?;
        Some((y, m, d))
    }
    /// "hh:mm:ss[.fff]" (or "hh:mm" when seconds are optional and absent).
    fn clock(&mut self, need_secs: bool) -> Option<(i64, i64, i64, i64)> {
        let h = self.num(1, 2)?;
        self.lit(b':')?;
        let mi = self.num(2, 2)?;
        if !need_secs {
            return Some((h, mi, 0, 0));
        }
        self.lit(b':')?;
        let s = self.num(2, 2)?;
        let ns = self.frac();
        Some((h, mi, s, ns))
    }
}

fn valid(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<NaiveDateTime> {
    if !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..60).contains(&s) {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, u32::try_from(m).ok()?, u32::try_from(d).ok()?)?
        .and_hms_opt(h as u32, mi as u32, s as u32)
}

/// Parses RFC 3339 with optional fractional seconds.
pub fn parse_rfc3339(s: &str) -> Option<DateTime<FixedOffset>> {
    let mut c = Cur { s: s.as_bytes() };
    let (y, m, d) = c.date()?;
    c.lit(b'T')?;
    let (h, mi, sec, ns) = c.clock(true)?;
    let ndt = valid(y, m, d, h, mi, sec)? + TimeDelta::nanoseconds(ns);
    let off = match c.s.first()? {
        b'Z' => {
            c.s = &c.s[1..];
            0
        }
        b'+' | b'-' => {
            let sign = if c.s[0] == b'-' { -1 } else { 1 };
            c.s = &c.s[1..];
            let oh = c.num(2, 2)?;
            c.lit(b':')?;
            let om = c.num(2, 2)?;
            if oh > 24 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60) as i32
        }
        _ => return None,
    };
    if !c.s.is_empty() {
        return None;
    }
    let fo = FixedOffset::east_opt(off)?;
    Some(Zone::Fixed(fo).resolve(&ndt))
}

/// The layouts `parseTimeText` tries after RFC 3339.
#[derive(Clone, Copy)]
enum Layout {
    NsSpace, // YYYY-MM-DD hh:mm:ss.fffffffff
    NsT,     // YYYY-MM-DDThh:mm:ss.fffffffff
    Date,    // YYYY-MM-DD
}

fn parse_layout(s: &str, l: Layout, zone: Zone) -> Option<DateTime<FixedOffset>> {
    let mut c = Cur { s: s.as_bytes() };
    let (y, m, d) = c.date()?;
    let (h, mi, sec, ns) = match l {
        Layout::Date => (0, 0, 0, 0),
        Layout::NsSpace | Layout::NsT => {
            c.lit(if matches!(l, Layout::NsSpace) { b' ' } else { b'T' })?;
            c.clock(true)?
        }
    };
    if !c.s.is_empty() {
        return None;
    }
    let ndt = valid(y, m, d, h, mi, sec)? + TimeDelta::nanoseconds(ns);
    Some(zone.resolve(&ndt))
}

/// Parses "now", RFC 3339, or "YYYY-MM-DD hh:mm:ss[.fff]" with an
/// optional " UTC"/"Z" suffix; plain text is read in `zone`.
pub fn parse_time_text(s: &str, zone: Zone) -> Result<DateTime<FixedOffset>> {
    let mut s = s.trim();
    let mut zone = zone;
    if s.eq_ignore_ascii_case("now") {
        return Ok(zone.now());
    }
    if let Some(t) = parse_rfc3339(s) {
        return Ok(zone.convert(&t));
    }
    if s.ends_with(" UTC") || s.ends_with('Z') {
        s = s.strip_suffix(" UTC").unwrap_or(s);
        s = s.strip_suffix('Z').unwrap_or(s);
        zone = Zone::Utc;
    }
    for l in [Layout::NsSpace, Layout::NsT, Layout::Date] {
        if let Some(t) = parse_layout(s, l, zone) {
            return Ok(t);
        }
    }
    Err(local(format!("bad date/time {} (use YYYY-MM-DD hh:mm:ss.fff, RFC 3339 or now)", quote(s.as_bytes()))))
}

/// "hh:mm:ss.fffffffff" or "hh:mm" → nanoseconds since midnight.
pub fn parse_clock_text(s: &str) -> Option<i64> {
    for need_secs in [true, false] {
        let mut c = Cur { s: s.as_bytes() };
        if let Some((h, mi, sec, ns)) = c.clock(need_secs) {
            if c.s.is_empty() && (0..24).contains(&h) && (0..60).contains(&mi) && (0..60).contains(&sec) {
                return Some(h * HOUR + mi * MINUTE + sec * SECOND + ns);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_format() {
        assert_eq!(format_float(3.25, 32), "3.25");
        assert_eq!(format_float(3.1f32 as f64, 32), "3.1");
        assert_eq!(format_float(-0.5, 64), "-0.5");
        assert_eq!(format_float(1e6, 64), "1e+06");
        assert_eq!(format_float(123456.0, 64), "123456");
        assert_eq!(format_float(1234567.0, 64), "1.234567e+06");
        assert_eq!(format_float(0.0001, 64), "0.0001");
        assert_eq!(format_float(0.00001, 64), "1e-05");
        assert_eq!(format_float(0.0, 64), "0");
        assert_eq!(format_float(100.0, 64), "100");
        assert_eq!(format_float(f64::INFINITY, 64), "+Inf");
        assert_eq!(format_float(f64::NAN, 32), "NaN");
    }

    #[test]
    fn ints_format() {
        assert_eq!(parse_uint("0x1234", 16).unwrap(), 0x1234);
        assert_eq!(parse_uint("010", 8).unwrap(), 8);
        assert_eq!(parse_int("-1234", 16).unwrap(), -1234);
        assert_eq!(parse_int("-32768", 16).unwrap(), -32768);
        assert!(parse_int("32768", 16).is_err());
        assert!(parse_uint("-1", 16).is_err());
        assert!(parse_uint("256", 8).is_err());
        assert_eq!(parse_int("0b101", 64).unwrap(), 5);
        assert_eq!(parse_uint("0x_ff", 16).unwrap(), 255);
        assert!(parse_uint("1_0", 16).is_err());
        assert!(parse_int("", 16).is_err());
        assert_eq!(parse_uint("18446744073709551615", 64).unwrap(), u64::MAX);
        assert_eq!(parse_int("-9223372036854775808", 64).unwrap(), i64::MIN);
    }

    #[test]
    fn durations_format() {
        assert_eq!(duration_string(0), "0s");
        assert_eq!(duration_string(1_500_000_000), "1.5s");
        assert_eq!(duration_string(-3_723_500 * MILLISECOND), "-1h2m3.5s");
        assert_eq!(duration_string(HOUR), "1h0m0s");
        assert_eq!(duration_string(1_500_000), "1.5ms");
        assert_eq!(duration_string(1_200), "1.2µs");
        assert_eq!(duration_string(5), "5ns");
        assert_eq!(parse_duration("1h2m3.5s").unwrap(), 3_723_500 * MILLISECOND);
        assert_eq!(parse_duration("-500ms").unwrap(), -500 * MILLISECOND);
        assert_eq!(parse_duration(".5s").unwrap(), 500 * MILLISECOND);
        assert!(parse_duration("30d").is_err());
        assert!(parse_duration("1").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn quote_format() {
        assert_eq!(quote(b"hello"), "\"hello\"");
        assert_eq!(quote(b"a\"b\\"), "\"a\\\"b\\\\\"");
        assert_eq!(quote(&[0x80]), "\"\\x80\"");
        assert_eq!(quote(&[0]), "\"\\x00\"");
        assert_eq!(quote("é\n".as_bytes()), "\"é\\n\"");
    }

    #[test]
    fn dates_format() {
        let t = make_date(2000, 0, 0, 0, 0, 0, 0, Zone::Utc); // normalised
        assert_eq!(fmt_ns_layout(&t), "1999-11-30 00:00:00");
        let t = parse_time_text("2026-11-04T14:45:30.5+01:00", Zone::Utc).unwrap();
        assert_eq!(fmt_rfc3339_nano(&t), "2026-11-04T13:45:30.5Z");
        assert!(parse_time_text("2026-02-30", Zone::Utc).is_err());
        assert_eq!(fmt_tod(-1), "23:59:59.999999999");
        assert_eq!(parse_clock_text("13:45").unwrap(), 13 * HOUR + 45 * MINUTE);
    }
}
