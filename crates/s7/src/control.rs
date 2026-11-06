//! PLC control services. DANGEROUS: they change the CPU operating mode and stop
//! the machine. Only tested against the fake CPU; never use them against a
//! production PLC without an explicit operator decision. Frame layouts [std; nodes7, snap7, Wireshark s7comm]:
//!
//! ```text
//! STOP       job 0x29: 29 00 00 00 00 00 <len> "P_PROGRAM"
//! hot start  job 0x28: 28 00 00 00 00 00 00 FD <arglen BE=0> <len> "P_PROGRAM"
//! cold start job 0x28: 28 00 00 00 00 00 00 FD 00 02 "C " <len> "P_PROGRAM"
//! ```

use crate::error::{io, Result};
use crate::log::hex_spaced;
use crate::wire::Conn;

const PI_PROGRAM: &[u8] = b"P_PROGRAM";

pub fn plc_stop_param() -> Vec<u8> {
    let mut p = vec![0x29, 0x00, 0x00, 0x00, 0x00, 0x00, PI_PROGRAM.len() as u8];
    p.extend_from_slice(PI_PROGRAM);
    p
}

pub fn plc_start_param(cold: bool) -> Vec<u8> {
    let arg: &[u8] = if cold { b"C " } else { b"" };
    let mut p = vec![0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFD, 0x00, arg.len() as u8];
    p.extend_from_slice(arg);
    p.push(PI_PROGRAM.len() as u8);
    p.extend_from_slice(PI_PROGRAM);
    p
}

/// PLC control replies carry an optional second param byte: 0x02 already in
/// RUN (start), 0x07 already in STOP (stop) [snap7].
fn control_reply(func: u8, param: &[u8]) -> Result<bool> {
    if param.is_empty() || param[0] != func {
        return Err(io(format!("unexpected reply param {}", hex_spaced(param))));
    }
    Ok(param.len() > 1 && matches!(param[1], 0x02 | 0x07))
}

impl Conn {
    /// Puts the CPU in STOP (PI service P_PROGRAM, job 0x29). Returns true
    /// when the CPU says it was already in STOP.
    pub fn stop_cpu(&mut self) -> Result<bool> {
        let r = self.round_trip_owned(0x01, &plc_stop_param(), &[]).map_err(|e| e.context("stop"))?;
        control_reply(0x29, &r.param)
    }

    /// Runs the CPU (PI service P_PROGRAM, job 0x28): hot start (warm
    /// restart), or cold start with `cold`. Returns true when the CPU says it
    /// was already in RUN.
    pub fn start_cpu(&mut self, cold: bool) -> Result<bool> {
        let r = self.round_trip_owned(0x01, &plc_start_param(cold), &[]).map_err(|e| e.context("start"))?;
        control_reply(0x28, &r.param)
    }
}
