//! Userdata jobs (ROSCTR 0x07) with multi-part reassembly.

use crate::error::{io, Error, Result};
use crate::log::hex_spaced;
use crate::wire::{Conn, OwnedReply};

/// Userdata function groups (param byte 5, low nibble) [std].
pub const GROUP_BLOCK: u8 = 0x03; // block functions: list, list of type, block info
pub const GROUP_CPU: u8 = 0x04; // CPU functions: read SZL, …
pub const GROUP_TIME: u8 = 0x07; // clock

// Userdata data-section transport sizes.
pub(crate) const UD_TS_NULL: u8 = 0x00;
pub(crate) const UD_TS_OCTET: u8 = 0x09; // octet string

/// Bounds a multi-part Userdata reply (a CPU that never sets "last data unit").
const MAX_FOLLOW_UPS: usize = 256;

/// First request of a Userdata job: param 00 01 12 04 11 <4<<4|group> <sub>
/// <seq=0>; data FF <ts> <len> payload, or 0A 00 0000 when there is no payload.
pub fn userdata_req(group: u8, sub: u8, ts: u8, payload: Option<&[u8]>) -> (Vec<u8>, Vec<u8>) {
    let param = vec![0x00, 0x01, 0x12, 0x04, 0x11, 0x40 | group, sub, 0x00];
    let Some(payload) = payload else {
        return (param, vec![0x0A, UD_TS_NULL, 0x00, 0x00]);
    };
    let mut data = vec![0xFF, ts];
    data.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    data.extend_from_slice(payload);
    (param, data)
}

/// Follow-up request that asks for the next data unit: param 00 01 12 08 12
/// <4<<4|group> <sub> <seq> 00 00 0000; data 0A 00 0000. The sequence number
/// is the one the CPU gave in its previous reply; the data unit reference
/// stays 0 (snap7 and nodes7 do the same and real CPUs accept it).
pub fn userdata_next(group: u8, sub: u8, seq: u8) -> (Vec<u8>, Vec<u8>) {
    (vec![0x00, 0x01, 0x12, 0x08, 0x12, 0x40 | group, sub, seq, 0x00, 0x00, 0x00, 0x00], vec![0x0A, UD_TS_NULL, 0x00, 0x00])
}

/// One parsed Userdata response (one data unit).
pub(crate) struct UdReply {
    pub seq: u8,
    /// param[9] != 0: "last data unit" = no
    pub more: bool,
    pub ret: u8,
    #[allow(dead_code)]
    pub ts: u8,
    pub payload: Vec<u8>,
}

/// The error carried by a Userdata reply parameter (00 01 12 08 12
/// <type|group> <sub> <seq> <dur> <last> <err BE>), if any.
pub(crate) fn userdata_err(param: &[u8]) -> Option<Error> {
    if param.len() >= 12 {
        let e = u16::from_be_bytes([param[10], param[11]]);
        if e != 0 {
            return Some(Error::Userdata(e));
        }
    }
    None
}

pub(crate) fn parse_userdata(r: &OwnedReply) -> Result<UdReply> {
    if r.rosctr != 0x07 {
        return Err(io(format!("unexpected rosctr 0x{:02x} (want Userdata)", r.rosctr)));
    }
    let p = &r.param;
    if p.len() < 8 || p[0] != 0x00 || p[1] != 0x01 || p[2] != 0x12 {
        return Err(io(format!("bad Userdata param {}", hex_spaced(p))));
    }
    let mut more = false;
    if p.len() >= 12 {
        more = p[9] != 0;
        if let Some(e) = userdata_err(p) {
            return Err(e);
        }
    }
    let d = &r.data;
    if d.len() < 4 {
        return Err(io(format!("short Userdata data {}", hex_spaced(d))));
    }
    let mut n = u16::from_be_bytes([d[2], d[3]]) as usize;
    if 4 + n > d.len() {
        n = d.len() - 4;
    }
    Ok(UdReply { seq: p[7], more, ret: d[0], ts: d[1], payload: d[4..4 + n].to_vec() })
}

impl Conn {
    /// Runs one Userdata job and reassembles a multi-part answer: while the CPU
    /// says "more data", it sends a follow-up request with the CPU's sequence
    /// number. Returns the payloads of every data unit, in order (SZL needs the
    /// first one apart). `payload` None means "no data" (0A 00 0000).
    pub fn userdata(&mut self, group: u8, sub: u8, payload: Option<&[u8]>) -> Result<Vec<Vec<u8>>> {
        let (mut param, mut data) = userdata_req(group, sub, UD_TS_OCTET, payload);
        let mut parts = Vec::new();
        let mut i = 0;
        loop {
            let r = self.round_trip_owned(0x07, &param, &data)?;
            let u = parse_userdata(&r)?;
            if u.ret != 0xFF {
                return Err(Error::Item(u.ret));
            }
            parts.push(u.payload);
            if !u.more {
                return Ok(parts);
            }
            if i >= MAX_FOLLOW_UPS {
                return Err(io(format!("userdata group {group} sub {sub}: more than {MAX_FOLLOW_UPS} data units")));
            }
            (param, data) = userdata_next(group, sub, u.seq);
            i += 1;
        }
    }
}
