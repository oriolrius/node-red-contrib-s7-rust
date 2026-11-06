//! S7CommPlus (S7-1200/1500, protocol id 0x72) client.
//!
//! Thread based (no async runtime), no global state, nothing printed:
//! logging goes through an optional [`Logger`]. A [`Client`] is `Send + Sync`
//! and serializes its requests with an internal mutex.
//!
//! ```no_run
//! use s7plus::{Client, Options, AreaItem, AREA_FLAGS};
//! let c = Client::connect(Options::new("192.168.0.1:102"))?;
//! let r = c.read_items(&[AreaItem::bytes(AREA_FLAGS, 0, 2, 2)])?;
//! let mut tags = vec![c.resolve_tag("\"Data_block_1\".Speed")?];
//! let res = c.read_tags(&mut tags)?;
//! # Ok::<(), s7plus::Error>(())
//! ```

pub mod alarms;
pub mod blob;
pub mod browse;
pub mod client;
pub mod error;
pub mod explore;
pub mod ids;
pub mod legit;
pub mod limits;
pub mod log;
pub mod msg;
pub mod object;
pub mod objects;
pub mod plcvalue;
pub mod softdt;
pub mod stream;
pub mod subscribe;
pub mod symabs;
pub mod tagdescr;
pub mod tags;
pub mod textfmt;
pub mod tls;
pub(crate) mod transport;
pub mod typeinfo;
pub mod value;
pub mod wire;

#[cfg(feature = "fake")]
pub mod fake;

#[cfg(test)]
mod tests;

pub use alarms::{decode_alarm, parse_text_library, Alarm, AlarmCgs, AlarmSubOptions, AlarmText, HmiInfo};
pub use blob::{Comments, Dictionaries};
pub use browse::{DbInfo, VarInfo};
pub use client::{Client, Options, SessionEcho, SessionInfo, TraceFn};
pub use error::{Error, Result};
pub use explore::{class_filter, ExploreReq};
pub use legit::{legitimation_mode_for, LegitMode, ACCESS_FULL, ACCESS_HMI, ACCESS_NONE, ACCESS_READ};
pub use limits::Limits;
pub use log::{Level, LevelFilter, Logger};
pub use msg::{ItemAddress, Message};
pub use object::Object;
pub use objects::{operating_state_name, OP_STATE_RUN, OP_STATE_STOP};
pub use plcvalue::{decode_plc, decode_plc_ts, encode_plc, format_plc, PlcValue, Tag, TagResult};
pub use subscribe::{decode_notification, Notification, SubOptions, Subscription};
pub use tags::{area_address, db_area, AreaItem, ItemResult, AREA_DB, AREA_FLAGS, AREA_INPUTS, AREA_OUTPUTS};
pub use textfmt::DateTime;
pub use tls::{NssKeyLog, SharedBuf, TlsOptions};
pub use value::{Struct, Value, V};
