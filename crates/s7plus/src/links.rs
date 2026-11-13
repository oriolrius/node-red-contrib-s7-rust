//! Object relations (GetLink 0x0524) and object path resolution
//! (GetVariablesAddress 0x0590). Both read only. The request and response
//! layouts were found on PLCSIM Advanced V8 (S7-1500, FW S4.1); TIA Portal
//! uses GetLink in system diagnostics and the resolution for technology
//! objects ("ASRoot.PLCProgram.<name>").

use crate::client::{return_value, Client, Conn};
use crate::error::{Error, Result};
use crate::ids::{FN_GET_LINK, FN_GET_VARIABLES_ADDR};
use crate::wire::{Reader, Writer};

/// The OMS path of a PLC program object (DB, FB, FC, OB, technology
/// object): "ASRoot.PLCProgram." + the name, '$' before the characters
/// . [ ] : ( ) , @ ? $ * - and before a first digit, as TIA Portal does.
pub fn program_object_path(name: &str) -> String {
    const SPECIAL: &[char] = &['.', '[', ']', ':', '(', ')', ',', '@', '?', '$', '*', '-'];
    let mut p = String::from("ASRoot.PLCProgram.");
    for (i, c) in name.chars().enumerate() {
        if SPECIAL.contains(&c) || (i == 0 && c.is_ascii_digit()) {
            p.push('$');
        }
        p.push(c);
    }
    p
}

/// Decodes a GetLink response body after the return value: count, RIDs
/// (u32). A truncated response is an error, not a shorter list.
pub(crate) fn decode_links(r: &mut Reader) -> Result<Vec<u32>> {
    let n = r.byte() as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(r.u32());
    }
    r.result()?;
    if !r.ok() {
        return Err(Error::proto("GetLink: truncated response"));
    }
    Ok(out)
}

/// A VLQ of at most 5 octets that must end (no continuation bit on the last
/// octet) inside the buffer and fit in 32 bits.
fn strict_vlq32(r: &mut Reader) -> Result<u32> {
    let mut v: u64 = 0;
    for _ in 0..5 {
        if r.left() == 0 {
            return Err(Error::proto("resolve: truncated object id"));
        }
        let o = r.byte();
        v = (v << 7) | (o & 0x7f) as u64;
        if o & 0x80 == 0 {
            return u32::try_from(v).map_err(|_| Error::proto(format!("resolve: object id {v:#x} larger than 32 bits")));
        }
    }
    Err(Error::proto("resolve: object id longer than 5 octets"))
}

/// Decodes a GetVariablesAddress response body after the return value.
/// Found: 0x05, count n >= 1, then n elements of RID (VLQ) and two u32; all n
/// must be present and valid, the first one is the object. Not found
/// (PLCSIM): 0x01, 0x00. Anything else, or a truncated response, is an error.
pub(crate) fn decode_resolve(r: &mut Reader) -> Result<Option<u32>> {
    if r.left() < 2 {
        return Err(Error::proto("resolve: truncated response"));
    }
    match (r.byte(), r.byte()) {
        (0x05, 0) => Err(Error::proto("resolve: empty address list")),
        (0x05, n) => {
            let mut first = 0;
            for i in 0..n {
                let rid = strict_vlq32(r)?;
                if r.left() < 8 {
                    return Err(Error::proto(format!("resolve: truncated address element {} of {n}", i + 1)));
                }
                r.u32();
                r.u32();
                if i == 0 {
                    first = rid;
                }
            }
            if first == 0 {
                return Err(Error::proto("resolve: object id 0"));
            }
            Ok(Some(first))
        }
        (0x01, 0x00) => Ok(None),
        (a, b) => Err(Error::proto(format!("resolve: unexpected response {a:#04x} {b:#04x}"))),
    }
}

impl Conn {
    pub(crate) fn get_links(&mut self, rid: u32, aid: u32) -> Result<Vec<u32>> {
        let mut w = Writer::new();
        w.u32(rid);
        w.vlq32(aid);
        w.u16(0);
        self.integrity_id(&mut w, false);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_GET_LINK, 0x34, w.b)?;
        let mut r = Reader::new(&m.body);
        return_value(&m, &mut r)?;
        decode_links(&mut r)
    }

    pub(crate) fn resolve_object(&mut self, path: &str) -> Result<Option<u32>> {
        let mut w = Writer::new();
        w.byte(1); // one path (two in one request close the connection)
        w.vlq32(path.len() as u32);
        w.raw(path.as_bytes());
        self.integrity_id(&mut w, false);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_GET_VARIABLES_ADDR, 0x34, w.b)?;
        let mut r = Reader::new(&m.body);
        return_value(&m, &mut r)?;
        decode_resolve(&mut r)
    }
}

impl Client {
    /// The objects linked to `rid` by the relation `aid` (GetLink), e.g. the
    /// alarm subsystem (8) and itsUpdateRelevantDAI (2667): the active DAIs.
    /// Read only.
    pub fn get_links(&self, rid: u32, aid: u32) -> Result<Vec<u32>> {
        self.conn().get_links(rid, aid)
    }

    /// The RID of an object by its OMS path (GetVariablesAddress), e.g.
    /// "ASRoot" (1), "ASRoot.PLCProgram" (3) or
    /// [`program_object_path`]("Data_block_1") (the DB). None when the PLC
    /// knows no such object; variables inside a DB are not objects. Read only.
    pub fn resolve_object(&self, path: &str) -> Result<Option<u32>> {
        if path.is_empty() || path.len() > 1000 {
            return Err(crate::error::Error::proto("resolve: empty or too long path"));
        }
        self.conn().resolve_object(path)
    }
}
