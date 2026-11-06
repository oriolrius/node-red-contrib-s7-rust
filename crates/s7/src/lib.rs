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
