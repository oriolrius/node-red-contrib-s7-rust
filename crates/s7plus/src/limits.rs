//! SystemLimits and FreeItems.

use std::fmt;

use crate::client::{Client, Conn};
use crate::error::Result;
use crate::ids::*;
use crate::msg::ItemAddress;

/// The communication resources of the PLC (SystemLimits 1037 and FreeItems
/// 1081 of the Root object 201). LID meaning follows the reference: 0
/// subscriptions, 1 attributes, 2 subscription memory, 1000 tags per read,
/// 1001 tags per write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    pub tags_per_read_max: i32,
    pub tags_per_write_max: i32,
    pub subscriptions_max: i32,
    pub attributes_max: i32,
    pub sub_memory_max: i32,
    pub subscriptions_free: i32,
    pub attributes_free: i32,
    pub sub_memory_free: i32,
}

fn limit_addr(sub: u32, lid: u32) -> ItemAddress {
    ItemAddress { area: RID_ROOT, sub_area: sub, lid: vec![lid], ..Default::default() }
}

impl Conn {
    pub(crate) fn read_limits(&mut self) -> Result<Limits> {
        let addrs = [
            limit_addr(ATTR_SYSTEM_LIMITS, 1000),
            limit_addr(ATTR_SYSTEM_LIMITS, 1001),
            limit_addr(ATTR_SYSTEM_LIMITS, 0),
            limit_addr(ATTR_SYSTEM_LIMITS, 1),
            limit_addr(ATTR_SYSTEM_LIMITS, 2),
            limit_addr(ATTR_FREE_ITEMS, 0),
            limit_addr(ATTR_FREE_ITEMS, 1),
            limit_addr(ATTR_FREE_ITEMS, 2),
        ];
        let (vals, errs) = self.read_chunk(&addrs)?;
        let mut x = [0i32; 8];
        for i in 0..8 {
            if errs[i] == 0 {
                if let Some(v) = vals[i].v.as_i32() {
                    x[i] = v;
                }
            }
        }
        Ok(Limits {
            tags_per_read_max: x[0],
            tags_per_write_max: x[1],
            subscriptions_max: x[2],
            attributes_max: x[3],
            sub_memory_max: x[4],
            subscriptions_free: x[5],
            attributes_free: x[6],
            sub_memory_free: x[7],
        })
    }
}

impl Client {
    /// Reads SystemLimits and FreeItems (one request).
    pub fn read_limits(&self) -> Result<Limits> {
        self.conn().read_limits()
    }
}

impl fmt::Display for Limits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tagsPerRead={} tagsPerWrite={} subscriptions={}/{} free attributes={}/{} free subMemory={}/{} free",
            self.tags_per_read_max,
            self.tags_per_write_max,
            self.subscriptions_free,
            self.subscriptions_max,
            self.attributes_free,
            self.attributes_max,
            self.sub_memory_free,
            self.sub_memory_max
        )
    }
}
