//! Explore requests.

use crate::client::Client;
use crate::error::Result;
use crate::ids::*;
use crate::object::Object;
use crate::value::{Struct, Value, FLAG_ARRAY, T_UDINT, V};
use crate::wire::Writer;

/// An Explore request. TIA Portal V21 sends recursive=1, unknown=0,
/// parents=1; the S7CommPlusDriver reference sends recursive=1, unknown=1,
/// parents=0 (needed for type-info objects on the S7-1200 FW V3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExploreReq {
    pub object: u32,
    /// Explore request id (e.g. 2667 itsUpdateRelevantDAI).
    pub request_id: u32,
    pub recursive: bool,
    pub unknown: u8,
    pub parents: bool,
    /// Optional filter (1246 Filter).
    pub filter: Option<Struct>,
    pub attrs: Vec<u32>,
}

/// Filter operation "instance of".
pub const FILTER_INSTANCE_OF: i32 = 8;

/// An explore filter "instance of class".
pub fn class_filter(class: u32) -> Struct {
    let mut f = Struct::new(ATTR_FILTER);
    f.set(ATTR_FILTER_OPERATION, Value::dint(FILTER_INSTANCE_OF));
    f.set(ATTR_FILTER_ADDR_COUNT, Value::udint(0));
    f.set(ATTR_FILTER_ADDRESS, Value { flags: FLAG_ARRAY, typ: T_UDINT, v: V::Array(vec![V::U32(0); 32]) });
    f.set(ATTR_FILTER_VALUE, Value::rid(class));
    f
}

impl ExploreReq {
    pub(crate) fn encode(&self, w: &mut Writer) {
        w.u32(self.object);
        w.vlq32(self.request_id);
        w.byte(self.recursive as u8);
        w.byte(self.unknown);
        w.byte(self.parents as u8);
        if let Some(f) = &self.filter {
            w.byte(1);
            Value::structv(f.clone()).encode(w);
        }
        w.byte(0);
        w.vlq32(self.attrs.len() as u32);
        for a in &self.attrs {
            w.vlq32(*a);
        }
    }
}

impl Client {
    /// Runs an Explore request and returns the object list.
    pub fn explore_objects(&self, q: &ExploreReq) -> Result<Vec<Object>> {
        self.conn().explore_objects(q)
    }

    /// Lists the children of an object like TIA Portal V21 (parents on).
    pub fn explore(&self, object: u32, recursive: bool, attrs: &[u32]) -> Result<Vec<Object>> {
        self.explore_objects(&ExploreReq { object, recursive, parents: true, attrs: attrs.to_vec(), ..Default::default() })
    }
}
