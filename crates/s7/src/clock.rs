//! CPU clock: Userdata function group 7 (time functions), subfunction 0x01 read
//! clock, 0x02 set clock [std].
//! Synchronization: absolute or with a threshold (0–600 s), device time zone.

use chrono::{DateTime, Datelike, FixedOffset, TimeDelta, Timelike};

use crate::error::{io, local, Error, Result};
use crate::log::hex_spaced;
use crate::textfmt::{self, Zone};
use crate::types::{bcd_byte, to_bcd};
use crate::userdata::userdata_err;
use crate::wire::Conn;

pub(crate) fn clock_param(sub: u8) -> Vec<u8> {
    vec![0x00, 0x01, 0x12, 0x04, 0x11, 0x47, sub, 0x00]
}

/// The 10-byte clock record: reserved, year (2 BCD bytes), month, day, hour,
/// minute, second, ms (3 BCD digits) and weekday (1 = Sunday).
pub fn encode_clock(t: &DateTime<FixedOffset>) -> [u8; 10] {
    let ms = (t.nanosecond() % 1_000_000_000 / 1_000_000) as i64;
    let y = t.year() as i64;
    [
        0x00,
        to_bcd(y / 100),
        to_bcd(y % 100),
        to_bcd(t.month() as i64),
        to_bcd(t.day() as i64),
        to_bcd(t.hour() as i64),
        to_bcd(t.minute() as i64),
        to_bcd(t.second() as i64),
        to_bcd(ms / 10),
        ((ms % 10) as u8) << 4 | (textfmt::weekday(t) as u8 + 1),
    ]
}

/// Decodes a clock record as wall-clock time in `zone`.
pub fn decode_clock(b: &[u8], zone: Zone) -> Result<DateTime<FixedOffset>> {
    if b.len() < 10 {
        return Err(local(format!("short clock record {}", hex_spaced(b))));
    }
    let mut year = bcd_byte(b[1]) * 100 + bcd_byte(b[2]);
    if b[1] == 0 {
        // some CPUs leave the century byte empty
        year = 1900 + bcd_byte(b[2]);
        if year < 1990 {
            year += 100;
        }
    }
    let ms = bcd_byte(b[8]) * 10 + (b[9] >> 4) as i64;
    Ok(textfmt::make_date(year, bcd_byte(b[3]), bcd_byte(b[4]), bcd_byte(b[5]), bcd_byte(b[6]), bcd_byte(b[7]), ms * 1_000_000, zone))
}

impl Conn {
    /// Reads the CPU clock, interpreted in `zone` (the device time zone).
    pub fn read_clock(&mut self, zone: Zone) -> Result<DateTime<FixedOffset>> {
        let r = self.round_trip_owned(0x07, &clock_param(0x01), &[0x0A, 0x00, 0x00, 0x00])?;
        if let Some(e) = userdata_err(&r.param) {
            return Err(e.context("read clock"));
        }
        if r.data.len() < 4 || r.data[0] != 0xFF {
            if let Some(&c) = r.data.first() {
                return Err(Error::Item(c));
            }
            return Err(io("empty clock reply"));
        }
        decode_clock(&r.data[4..], zone)
    }

    /// Writes `t` (converted to `zone`) to the CPU clock.
    pub fn set_clock(&mut self, t: &DateTime<FixedOffset>, zone: Zone) -> Result<()> {
        let mut data = vec![0xFF, 0x09, 0x00, 0x0A];
        data.extend_from_slice(&encode_clock(&zone.convert(t)));
        let r = self.round_trip_owned(0x07, &clock_param(0x02), &data)?;
        if let Some(e) = userdata_err(&r.param) {
            return Err(e.context("set clock"));
        }
        if let Some(&c) = r.data.first() {
            if c != 0xFF && c != 0x0A {
                return Err(Error::Item(c));
            }
        }
        Ok(())
    }

    /// Sets the CPU clock to now when it differs from `now` by more than
    /// `threshold` (driver "Time Sync Threshold"; zero = always). Returns the
    /// measured offset (PLC − now) and whether it synced.
    pub fn sync_clock(&mut self, now: &DateTime<FixedOffset>, threshold: TimeDelta, zone: Zone) -> Result<(TimeDelta, bool)> {
        let plc = self.read_clock(zone)?;
        let offset = plc.signed_duration_since(*now);
        if threshold > TimeDelta::zero() && offset < threshold && offset > -threshold {
            return Ok((offset, false));
        }
        self.set_clock(&Zone::Utc.now(), zone)?;
        Ok((offset, true))
    }
}
