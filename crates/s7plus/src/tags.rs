//! Classic absolute area access over S7CommPlus. The classic
//! crate is not a dependency: [`AreaItem`] carries the same fields as a
//! classic S7 item, with the same area codes.

use crate::client::Client;
use crate::error::{Error, Result};
use crate::ids::*;
use crate::msg::ItemAddress;
use crate::value::Value;

/// Classic area codes (same values as S7comm).
pub const AREA_INPUTS: u8 = 0x81;
pub const AREA_OUTPUTS: u8 = 0x82;
pub const AREA_FLAGS: u8 = 0x83;
pub const AREA_DB: u8 = 0x84;
pub const AREA_COUNTERS: u8 = 0x1c;
pub const AREA_TIMERS: u8 = 0x1d;

/// A classic absolute item: area, DB number, byte offset, bit, byte count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AreaItem {
    pub area: u8,
    pub db: u16,
    /// Byte offset (0-based).
    pub start: u32,
    /// Bit number (0..7) when `is_bit`.
    pub bit: u8,
    /// Number of bytes (ignored for bits).
    pub count: u16,
    pub is_bit: bool,
}

impl AreaItem {
    pub fn bytes(area: u8, db: u16, start: u32, count: u16) -> Self {
        AreaItem { area, db, start, count, ..Default::default() }
    }
    pub fn bit(area: u8, db: u16, start: u32, bit: u8) -> Self {
        AreaItem { area, db, start, bit, count: 1, is_bit: true }
    }
    pub fn is_timer_counter(&self) -> bool {
        self.area == AREA_COUNTERS || self.area == AREA_TIMERS
    }
}

/// Access area id of DB n (0x8a0e0000 + n).
pub fn db_area(n: u16) -> u32 {
    0x8a0e_0000 + n as u32
}

/// Maps a classic item (I/Q/M/DB) to an S7CommPlus absolute address: area
/// object, ValueActual attribute, LID [3, byte offset, length]. Verified on
/// the S7-1200 FW V3: LID 3 is the raw byte image of the area (MW2 = [3 2 2]).
pub fn area_address(it: &AreaItem) -> Result<ItemAddress> {
    let n = if it.is_bit { 1 } else { it.count as u32 };
    if it.is_timer_counter() {
        return Err(Error::proto("S5 timers/counters are not mapped"));
    }
    let mut a = ItemAddress { sub_area: ATTR_CTRL_AREA_VALUE_ACT, lid: vec![3, it.start, n], ..Default::default() };
    match it.area {
        AREA_INPUTS => a.area = RID_I_AREA,
        AREA_OUTPUTS => a.area = RID_Q_AREA,
        AREA_FLAGS => a.area = RID_M_AREA,
        AREA_DB => {
            a.area = db_area(it.db);
            a.sub_area = ATTR_DB_VALUE_ACTUAL;
        }
        x => return Err(Error::proto(format!("area {x:#x} not mapped"))),
    }
    Ok(a)
}

/// Result of one item read: same layout as a classic read (a bit = 1 byte 0/1).
#[derive(Debug)]
pub struct ItemResult {
    pub data: Vec<u8>,
    pub err: Option<Error>,
}

impl Client {
    /// Reads classic items over S7CommPlus.
    pub fn read_items(&self, items: &[AreaItem]) -> Result<Vec<ItemResult>> {
        let mut res: Vec<ItemResult> = items.iter().map(|_| ItemResult { data: Vec::new(), err: None }).collect();
        let mut addrs = Vec::new();
        let mut idx = Vec::new();
        for (i, it) in items.iter().enumerate() {
            match area_address(it) {
                Ok(a) => {
                    addrs.push(a);
                    idx.push(i);
                }
                Err(e) => res[i].err = Some(e),
            }
        }
        if addrs.is_empty() {
            return Ok(res);
        }
        let (vals, errs) = self.read(&addrs)?;
        for (k, &i) in idx.iter().enumerate() {
            let it = &items[i];
            let b = vals[k].v.as_bytes();
            if errs[k] != 0 {
                res[i].err = Some(Error::Item(errs[k]));
            } else if b.is_none() {
                res[i].err = Some(Error::proto(format!("no data for {} (out of range?)", addrs[k])));
            } else if it.is_bit {
                let b = b.unwrap();
                res[i].data = vec![(b.first().copied().unwrap_or(0) >> (it.bit & 7)) & 1];
            } else {
                res[i].data = b.unwrap().to_vec();
            }
        }
        Ok(res)
    }

    /// Writes classic byte items over S7CommPlus. Bit items give
    /// [`Error::BitWrite`] (a read-modify-write would race with the CPU).
    pub fn write_items(&self, items: &[AreaItem], data: &[Vec<u8>]) -> Result<Vec<Option<Error>>> {
        let mut out: Vec<Option<Error>> = items.iter().map(|_| None).collect();
        let mut addrs = Vec::new();
        let mut vals = Vec::new();
        let mut idx = Vec::new();
        for (i, it) in items.iter().enumerate() {
            if it.is_bit {
                out[i] = Some(Error::BitWrite);
                continue;
            }
            match area_address(it) {
                Ok(a) => {
                    addrs.push(a);
                    vals.push(Value::blob(data.get(i).cloned().unwrap_or_default()));
                    idx.push(i);
                }
                Err(e) => out[i] = Some(e),
            }
        }
        if addrs.is_empty() {
            return Ok(out);
        }
        let errs = self.write(&addrs, &vals)?;
        for (k, &i) in idx.iter().enumerate() {
            if errs[k] != 0 {
                out[i] = Some(Error::Item(errs[k]));
            }
        }
        Ok(out)
    }
}
