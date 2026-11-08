//! S7comm (Siemens classic, ISO-on-TCP) client: S7-200/300/400/1200/1500 and LOGO!.
//!
//! Thread-based (std threads, no async runtime), no global state, never prints:
//! logging goes to an optional `Log` sink. Types are `Send + Sync` where they
//! are meant to be shared (`Client`, `Poller`, `Endpoint`).
//!

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod addr;
pub mod blocks;
pub mod client;
pub mod clock;
pub mod control;
pub mod cpu;
pub mod endpoint;
pub mod error;
#[cfg(any(test, feature = "fakecpu"))]
pub mod fakecpu;
pub mod log;
pub mod metrics;
pub mod nodes7addr;
mod optimize;
pub mod poll;
pub mod szl;
pub mod textfmt;
pub mod types;
mod types_ext;
pub mod userdata;
pub mod value;
pub mod verify;
pub mod wire;

#[cfg(test)]
mod tests;

/// Fuzz entry points (cargo feature `fuzzing`, used by the fuzz/ crate only).
/// Each one runs the same parsing as the client does on bytes from the network.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    use crate::wire::{parse_read_data, parse_reply, OwnedReply};

    /// One TPKT body from the CPU: COTP + S7 header, then the ReadVar or
    /// Userdata payload, as the connection parses it.
    pub fn reply(b: &[u8]) {
        let Ok(r) = parse_reply(b) else { return };
        let (param, data) = (&b[r.param], &b[r.data]);
        if r.rosctr == 0x03 && param.len() >= 2 && param[0] == 0x04 {
            let _ = parse_read_data(data, param[1] as usize, |_, _| {});
        }
        let _ = crate::userdata::parse_userdata(&OwnedReply { rosctr: r.rosctr, param: param.to_vec(), data: data.to_vec() });
    }

    /// A tag in the three address syntaxes, then its value decoded from `data`.
    pub fn tag_value(tag: &str, data: &[u8]) {
        for t in [crate::parse_tag(tag), crate::parse_siemens(tag), crate::parse_node_s7(tag)].into_iter().flatten() {
            let _ = t.format(data);
            let _ = t.natives(data);
            let _ = t.json_value(data);
        }
    }

    /// Block information (SZL group 3) and the CPU clock (group 7).
    pub fn block_and_clock(b: &[u8]) {
        let _ = crate::blocks::parse_block_info(b);
        if b.len() >= 6 {
            let _ = crate::blocks::decode_s7_timestamp(b);
        }
        let _ = crate::clock::decode_clock(b, crate::Zone::Utc);
        let _ = crate::clock::decode_clock(b, crate::Zone::Local);
    }
}

pub use addr::{parse_siemens, parse_tag, swappable, Tag};
pub use blocks::{
    language_name, parse_block_type, upload_filename, BlockCount, BlockEntry, BlockInfo, BlockType, UploadedBlock, BLOCK_TYPES,
};
pub use client::{Client, State};
pub use cpu::{parse_family, parse_tsap, Family, FamilyInfo, FAMILIES};
pub use endpoint::{DataEvent, Endpoint, EndpointConfig, OpenInfo, StatusInfo, Subscription, Supervisor, TagValue};
pub use error::{Error, Result};
pub use log::{Level, Log, LogSink};
pub use metrics::{Snapshot, Stats};
pub use nodes7addr::parse_node_s7;
pub use poll::{GroupMetrics, Mode, PollConfig, PollValue, Poller, PollerHandle, Quality, ScanGroup};
pub use szl::{component_name, szl_name, OrderInfo};
pub use textfmt::Zone;
pub use types::{decode, encode, native, s5time_to_ms, Native, Type};
pub use userdata::{GROUP_BLOCK, GROUP_CPU, GROUP_TIME};
pub use value::Val;
pub use verify::{VerifyItem, WriteReport};
pub use wire::{
    Conn, DialOptions, Item, ItemResult, AREA_COUNTER, AREA_DB, AREA_FLAGS, AREA_INPUTS, AREA_OUTPUTS, AREA_PERIPHERAL, AREA_TIMER,
};
