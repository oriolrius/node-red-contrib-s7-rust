//! Object creation/deletion and the CPU operating state.

use crate::client::Client;
use crate::error::{Error, Result};
use crate::ids::*;
use crate::object::Object;
use crate::value::Value;

/// CPUexecUnit.operatingStateReq value: STOP.
pub const OP_STATE_STOP: i32 = 1;
/// CPUexecUnit.operatingStateReq value: RUN.
pub const OP_STATE_RUN: i32 = 3;

impl Client {
    /// Creates an object below req_id (e.g. the session's second id for
    /// subscriptions). Returns the object ids assigned by the PLC. On error
    /// the ids the PLC returned anyway are given with the error (the object
    /// may exist).
    pub fn create_object(&self, req_id: u32, o: &Object) -> std::result::Result<Vec<u32>, (Vec<u32>, Error)> {
        self.conn().create_object(req_id, o)
    }

    /// Deletes an object (subscription, session, ...).
    pub fn delete_object(&self, id: u32) -> Result<()> {
        self.conn().delete_object(id)
    }

    /// Requests RUN (3) or STOP (1) on the CPU (SetVariable on object 52,
    /// attribute 2167). DANGEROUS on a real plant.
    pub fn set_plc_operating_state(&self, state: i32) -> Result<()> {
        self.set_variable(RID_CPU_EXEC_UNIT, ATTR_OPERATING_STATE_REQ, &Value::dint(state))
    }

    /// Reads the CPU state (52 / attribute 2237, struct element 3486; TIA
    /// subscribes to the same item). 8 = RUN on the S7-1200 FW V3.
    pub fn operating_state(&self) -> Result<i32> {
        let v = self.get_var_sub_streamed(RID_CPU_EXEC_UNIT, 2237)?;
        let s = v.v.as_struct().ok_or_else(|| Error::proto(format!("unexpected operating state value {v}")))?;
        let e = s.get(3486).ok_or_else(|| Error::proto("no element 3486 in the operating state"))?;
        Ok(e.v.as_i32().unwrap_or(0))
    }
}

/// Names the values seen in 2237/3486.
pub fn operating_state_name(s: i32) -> &'static str {
    if s == 8 {
        "RUN"
    } else {
        "unknown"
    }
}
