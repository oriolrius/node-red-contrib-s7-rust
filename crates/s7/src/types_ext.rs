//! IEC / S7-1200/1500 date and time types and wide strings [std: Siemens "Data
//! types" help; nodes7 for DTL/DTZ]. All big-endian.

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike};

use crate::error::{local, Result};
use crate::textfmt::{self, Zone, HOUR, MILLISECOND};
use crate::types::{be16, be32, be64, dt_time, put_dt, Native, Type};

/// Element size of the extended types (0 = not one).
pub(crate) fn ext_size(t: Type, str_len: usize) -> usize {
    match t {
        Type::Date | Type::WChar => 2,
        Type::TOD | Type::Time => 4,
        Type::DTZ | Type::LTime | Type::LTOD | Type::LDT => 8,
        Type::DTL | Type::DTLZ => 12,
        Type::WString => 4 + 2 * str_len,
        _ => 0,
    }
}

pub(crate) fn is_ext(t: Type) -> bool {
    ext_size(t, 0) != 0
}

fn date_epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1990, 1, 1).unwrap()
}

/// Decodes a 12-byte DTL: year(2) month day weekday hour minute second ns(4).
fn dtl_time(b: &[u8], zone: Zone) -> DateTime<FixedOffset> {
    textfmt::make_date(be16(b) as i64, b[2] as i64, b[3] as i64, b[5] as i64, b[6] as i64, b[7] as i64, be32(&b[8..]) as i64, zone)
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b.chunks_exact(2).map(be16).collect();
    String::from_utf16_lossy(&u)
}

/// The time value of a date/time type (None for the others).
fn ext_time(t: Type, b: &[u8]) -> Option<DateTime<FixedOffset>> {
    Some(match t {
        Type::DTZ => dt_time(b, Zone::Utc),
        Type::DTL => dtl_time(b, Zone::Local),
        Type::DTLZ => dtl_time(b, Zone::Utc),
        Type::Date => {
            let d = date_epoch() + chrono::TimeDelta::days(be16(b) as i64);
            Zone::Utc.resolve(&d.and_hms_opt(0, 0, 0).unwrap())
        }
        Type::LDT => textfmt::unix_nano_utc(be64(b) as i64),
        _ => return None,
    })
}

pub(crate) fn decode_ext(t: Type, b: &[u8]) -> Result<String> {
    if t != Type::WString && b.len() < ext_size(t, 0) {
        return Err(local(format!("{t} needs {} bytes, have {}", ext_size(t, 0), b.len())));
    }
    if let Some(tm) = ext_time(t, b) {
        return Ok(match t {
            Type::Date => textfmt::fmt_date(&tm),
            Type::DTZ | Type::DTLZ => textfmt::fmt_ns_layout(&tm) + " UTC",
            _ => textfmt::fmt_ns_layout(&tm),
        });
    }
    Ok(match t {
        Type::TOD => textfmt::fmt_tod(be32(b) as i64 * MILLISECOND),
        Type::LTOD => textfmt::fmt_tod(be64(b) as i64),
        Type::Time => textfmt::duration_string(be32(b) as i32 as i64 * MILLISECOND),
        Type::LTime => textfmt::duration_string(be64(b) as i64),
        Type::WChar => textfmt::quote(utf16be(&b[..2]).as_bytes()),
        Type::WString => {
            if b.len() < 4 {
                return Err(local("wstring too short"));
            }
            let mut n = be16(&b[2..]) as usize;
            if 4 + 2 * n > b.len() {
                n = (b.len() - 4) / 2;
            }
            textfmt::quote(utf16be(&b[4..4 + 2 * n]).as_bytes())
        }
        _ => return Err(local(format!("unknown type {t}"))),
    })
}

pub(crate) fn native_ext(t: Type, b: &[u8]) -> Result<Native> {
    if t != Type::WString && b.len() < ext_size(t, 0) {
        return Err(local(format!("{t} needs {} bytes", ext_size(t, 0))));
    }
    if let Some(tm) = ext_time(t, b) {
        return Ok(Native::Time(tm));
    }
    Ok(match t {
        Type::TOD => Native::U32(be32(b)),
        Type::LTOD => Native::U64(be64(b)),
        Type::Time => Native::I32(be32(b) as i32),
        Type::LTime => Native::I64(be64(b) as i64),
        Type::WChar => Native::Str(utf16be(&b[..2])),
        Type::WString => {
            if b.len() < 4 {
                return Err(local("wstring too short"));
            }
            let mut n = be16(&b[2..]) as usize;
            if 4 + 2 * n > b.len() {
                n = (b.len() - 4) / 2;
            }
            Native::Str(utf16be(&b[4..4 + 2 * n]))
        }
        _ => return Err(local(format!("unknown type {t}"))),
    })
}

/// Accepts an integer in `unit` (ms for TIME, ns for LTIME), a duration
/// ("1h2m3.5s", "-500ms") or an IEC time literal ("T#1H2M").
pub(crate) fn parse_dur_text(s: &str, unit: i64) -> Result<i64> {
    let s = s.trim();
    if let Some(v) = textfmt::parse_dec_i64(s) {
        return Ok(v.wrapping_mul(unit));
    }
    let u = s.to_lowercase();
    let u = u.strip_prefix("ltime#").unwrap_or(&u);
    let u = u.strip_prefix("time#").unwrap_or(u);
    let u = u.strip_prefix("lt#").unwrap_or(u);
    let u = u.strip_prefix("t#").unwrap_or(u);
    textfmt::parse_duration(&u.replace('_', ""))
}

/// Accepts an integer in `unit` or "hh:mm:ss.fffffffff" / "hh:mm" (optional
/// TOD# / LTOD# prefix); returns nanoseconds since midnight.
pub(crate) fn parse_tod_text(s: &str, unit: i64) -> Result<i64> {
    let s = s.trim();
    if let Some(v) = textfmt::parse_dec_u64(s) {
        return Ok((v as i64).wrapping_mul(unit));
    }
    let u = s.to_uppercase();
    let u = u.strip_prefix("TOD#").unwrap_or(&u);
    let u = u.strip_prefix("LTOD#").unwrap_or(u);
    textfmt::parse_clock_text(u).ok_or_else(|| local(format!("bad time of day {} (use hh:mm:ss.fff or ms)", textfmt::quote(u.as_bytes()))))
}

fn put_dtl(b: &mut [u8], tm: &DateTime<FixedOffset>) {
    b[0..2].copy_from_slice(&(tm.year() as u16).to_be_bytes());
    b[2] = tm.month() as u8;
    b[3] = tm.day() as u8;
    b[4] = textfmt::weekday(tm) as u8 + 1;
    b[5] = tm.hour() as u8;
    b[6] = tm.minute() as u8;
    b[7] = tm.second() as u8;
    b[8..12].copy_from_slice(&(tm.nanosecond() % 1_000_000_000).to_be_bytes());
}

pub(crate) fn encode_ext(t: Type, s: &str, str_len: usize) -> Result<Vec<u8>> {
    let mut b = vec![0u8; ext_size(t, str_len)];
    match t {
        Type::DTZ | Type::DTLZ => {
            let tm = Zone::Utc.convert(&textfmt::parse_time_text(s, Zone::Utc)?);
            if t == Type::DTZ {
                put_dt(&mut b, &tm);
            } else {
                put_dtl(&mut b, &tm);
            }
        }
        Type::DTL => {
            let tm = textfmt::parse_time_text(s, Zone::Local)?;
            put_dtl(&mut b, &Zone::Local.convert(&tm));
        }
        Type::Date => {
            let tm = textfmt::parse_time_text(s, Zone::Utc)?;
            let day = NaiveDate::from_ymd_opt(tm.year(), tm.month(), tm.day()).unwrap();
            let d = (day - date_epoch()).num_days();
            if !(0..=0xffff).contains(&d) {
                return Err(local("DATE out of range (1990-01-01 … 2168-12-31)"));
            }
            b.copy_from_slice(&(d as u16).to_be_bytes());
        }
        Type::LDT => {
            let tm = textfmt::parse_time_text(s, Zone::Utc)?;
            b.copy_from_slice(&(textfmt::unix_nano(&tm) as u64).to_be_bytes());
        }
        Type::TOD | Type::LTOD => {
            let unit = if t == Type::LTOD { 1 } else { MILLISECOND };
            let d = parse_tod_text(s, unit)?;
            if !(0..24 * HOUR).contains(&d) {
                return Err(local("time of day out of range"));
            }
            if t == Type::TOD {
                b.copy_from_slice(&((d / MILLISECOND) as u32).to_be_bytes());
            } else {
                b.copy_from_slice(&(d as u64).to_be_bytes());
            }
        }
        Type::Time | Type::LTime => {
            let unit = if t == Type::LTime { 1 } else { MILLISECOND };
            let d = parse_dur_text(s, unit)?;
            if t == Type::Time {
                let ms = d / MILLISECOND;
                if ms < i32::MIN as i64 || ms > i32::MAX as i64 {
                    return Err(local("TIME out of range (±24d20h31m23.647s)"));
                }
                b.copy_from_slice(&(ms as i32).to_be_bytes());
            } else {
                b.copy_from_slice(&(d as u64).to_be_bytes());
            }
        }
        Type::WChar => {
            let u: Vec<u16> = s.encode_utf16().collect();
            if u.len() != 1 {
                return Err(local("WCHAR needs one character"));
            }
            b.copy_from_slice(&u[0].to_be_bytes());
        }
        Type::WString => {
            let u: Vec<u16> = s.encode_utf16().collect();
            if u.len() > str_len {
                return Err(local(format!("wstring longer than {str_len}")));
            }
            b[0..2].copy_from_slice(&(str_len as u16).to_be_bytes());
            b[2..4].copy_from_slice(&(u.len() as u16).to_be_bytes());
            for (i, c) in u.iter().enumerate() {
                b[4 + 2 * i..6 + 2 * i].copy_from_slice(&c.to_be_bytes());
            }
        }
        _ => return Err(local(format!("writing type {t} is not supported"))),
    }
    Ok(b)
}
