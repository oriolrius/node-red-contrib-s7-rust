//! Object relations (GetLink 0x0524) and object path resolution
//! (GetVariablesAddress 0x0590). Both read only. The request and response
//! layouts were found on PLCSIM Advanced V8 (S7-1500, FW S4.1); TIA Portal
//! uses GetLink in system diagnostics and the resolution for technology
//! objects ("ASRoot.PLCProgram.<name>").

use crate::client::{return_value, Client, Conn};
use crate::error::Result;
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

/// Decodes a GetLink response body: return value, count, RIDs (u32).
pub(crate) fn decode_links(r: &mut Reader) -> Vec<u32> {
    let n = r.byte() as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        if !r.ok() {
            break;
        }
        out.push(r.u32());
    }
    out
}

/// Decodes a GetVariablesAddress response body after the return value:
/// 0x05, count, then per element the RID (VLQ) and two u32; count 0 or no
/// 0x05 = not found.
pub(crate) fn decode_resolve(r: &mut Reader) -> Option<u32> {
    if r.byte() != 0x05 || !r.ok() {
        return None;
    }
    let n = r.byte();
    if n == 0 || !r.ok() {
        return None;
    }
    let rid = r.vlq32();
    (r.ok() && rid != 0).then_some(rid)
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
        Ok(decode_links(&mut r))
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
        Ok(decode_resolve(&mut r))
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
