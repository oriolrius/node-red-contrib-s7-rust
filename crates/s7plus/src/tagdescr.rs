//! Tag descriptions (element 0xa7 … 0xa8).
//!
//! The S7-1200 with firmware V3 (and older) does not send a vartype list
//! (0xab) and a varname list (0xac) in its type-info objects. It sends one
//! tag description per variable (layout from captures of a CPU 1212C V3.0.2):
//!
//! ```text
//! a7
//!   kind      byte   bit 2: top-level element (has the 00 04 marker)
//!                    bits 0..1: 0 scalar, 1 Bool (bit offsets follow),
//!                               2 one-dimensional array, 3 multi-dim array
//!   name      VLQ length + ASCII
//!   vflags    byte   value flags of the transport type (0x10 array, 0x08 mdim)
//!   vtype     byte   transport datatype
//!   sdt       VLQ    softdatatype
//!   attr      u32    0x80900000 optimized block, 0x81900000 standard block or tag table
//!   lid       VLQ    access id (LID) of the variable
//!   info      VLQ    String: max length; Struct: type-info RID; DTL: 0x02000043
//!   [00 04]          only when kind bit 2 is set
//!   stdoff    VLQ    byte offset in the standard image
//!   optoff    VLQ    byte offset in the optimized image
//!   Bool:     stdbit byte, optbit byte
//!   1-dim:    lower SVLQ, count VLQ, stdstride VLQ, optstride VLQ
//!   m-dim:    stdstride VLQ, optstride VLQ, ndims VLQ, ndims x (lower SVLQ,
//!             count VLQ), the last (fastest) dimension first
//! a8
//! ```

use crate::object::{Object, ELEM_TAG_DESCR, ELEM_TAG_DESCR_END};
use crate::softdt::{SDT_STRING, SDT_STRUCT};
use crate::typeinfo::*;
use crate::wire::{Reader, Writer};

const TD_TOP_LEVEL: u8 = 0x04;
const TD_SHAPE_MASK: u8 = 0x03;
const TD_BOOL: u8 = 1;
const TD_ARRAY1: u8 = 2;
const TD_ARRAYM: u8 = 3;
const TD_ATTR_STD_BLK: u32 = 0x0100_0000;

/// The fields of a tag description that have no place in a [`Vartype`], so
/// that it can be encoded again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagDescr {
    pub kind: u8,
    pub vflags: u8,
    pub vtype: u8,
    pub attr: u32,
    pub info: u32,
    pub std_stride: u32,
    pub opt_stride: u32,
    /// The PLC sent the softdatatype as a non-minimal VLQ (DTL: 80 43).
    pub sdt_pad: bool,
}

pub(crate) fn decode_tag_descr(r: &mut Reader, o: &mut Object) {
    let mut td = TagDescr { kind: r.byte(), ..Default::default() };
    let n = r.vlq32() as usize;
    let name = r.bytes(n).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    td.vflags = r.byte();
    td.vtype = r.byte();
    td.sdt_pad = r.peek() == 0x80;
    let sdt = r.vlq32();
    td.attr = r.u32();
    let mut e = Vartype { lid: r.vlq32(), softdatatype: sdt as u8, ..Default::default() };
    td.info = r.vlq32();
    if td.kind & TD_TOP_LEVEL != 0 {
        r.u16(); // 0x0004 marker
    }
    e.offset.non_opt_addr = r.vlq32();
    e.offset.opt_addr = r.vlq32();
    if td.attr & TD_ATTR_STD_BLK == 0 {
        e.flags |= VT_OPTIMIZED;
    }
    match td.kind & TD_SHAPE_MASK {
        TD_BOOL => {
            let std = r.byte();
            let opt = r.byte();
            e.flags |= opt as u16 & VT_BIT_OFFSET_MASK;
            e.bit_info = BOI_CLASSIC | ((std << 4) & BOI_NON_OPT_BIT_MASK);
        }
        TD_ARRAY1 => {
            e.offset.lower_bound = r.svlq32();
            e.offset.count = r.vlq32();
            td.std_stride = r.vlq32();
            td.opt_stride = r.vlq32();
        }
        TD_ARRAYM => {
            td.std_stride = r.vlq32();
            td.opt_stride = r.vlq32();
            let n = r.vlq32() as usize;
            e.offset.count = 1;
            let mut i = 0;
            while i < n && r.ok() {
                let lo = r.svlq32();
                let cnt = r.vlq32();
                if i < e.offset.m_count.len() {
                    e.offset.m_lower[i] = lo;
                    e.offset.m_count[i] = cnt;
                }
                e.offset.count = e.offset.count.wrapping_mul(cnt);
                i += 1;
            }
        }
        _ => {}
    }
    if r.byte() != ELEM_TAG_DESCR_END && r.ok() {
        r.fail();
    }
    e.offset.kind = td_offset_kind(&td, e.softdatatype);
    match e.offset.kind {
        OI_STRING | OI_STRUCT_ELEM_STRING => {
            e.offset.info1 = td.info as u16;
            e.offset.info2 = (td.info as u16).wrapping_add(2);
        }
        OI_STRUCT | OI_STRUCT_1DIM | OI_STRUCT_MDIM | OI_STRUCT_ELEM_STRUCT => e.offset.relation_id = td.info,
        _ => {}
    }
    let vt = o.vartypes.get_or_insert_with(|| VartypeList { first_id: e.lid, elems: Vec::new() });
    vt.elems.push(e);
    o.varnames.push(name);
    o.tag_descrs.push(td);
}

/// Maps a tag description to the offset-info kind that the browser understands.
fn td_offset_kind(td: &TagDescr, sdt: u8) -> u8 {
    let is_struct = sdt == SDT_STRUCT || td.info >> 28 == 9;
    let top = td.kind & TD_TOP_LEVEL != 0;
    match td.kind & TD_SHAPE_MASK {
        TD_ARRAY1 => return if is_struct { OI_STRUCT_1DIM } else { OI_ARRAY_1DIM },
        TD_ARRAYM => return if is_struct { OI_STRUCT_MDIM } else { OI_ARRAY_MDIM },
        _ => {}
    }
    if is_struct && top {
        OI_STRUCT
    } else if is_struct {
        OI_STRUCT_ELEM_STRUCT
    } else if sdt == SDT_STRING && top {
        OI_STRING
    } else if sdt == SDT_STRING {
        OI_STRUCT_ELEM_STRING
    } else if top {
        OI_STD
    } else {
        OI_STRUCT_ELEM_STD
    }
}

/// Writes the variables of o as tag descriptions (fake PLC).
pub(crate) fn encode_tag_descrs(w: &mut Writer, o: &Object) {
    let Some(vt) = &o.vartypes else { return };
    for (i, e) in vt.elems.iter().enumerate() {
        let td = o.tag_descrs.get(i).copied().unwrap_or_default();
        let name = o.varnames.get(i).map(|s| s.as_str()).unwrap_or("");
        w.byte(ELEM_TAG_DESCR);
        w.byte(td.kind);
        w.vlq32(name.len() as u32);
        w.raw(name.as_bytes());
        w.byte(td.vflags);
        w.byte(td.vtype);
        if td.sdt_pad {
            w.byte(0x80);
        }
        w.vlq32(e.softdatatype as u32);
        w.u32(td.attr);
        w.vlq32(e.lid);
        w.vlq32(td.info);
        if td.kind & TD_TOP_LEVEL != 0 {
            w.u16(4);
        }
        w.vlq32(e.offset.non_opt_addr);
        w.vlq32(e.offset.opt_addr);
        match td.kind & TD_SHAPE_MASK {
            TD_BOOL => {
                w.byte((e.bit_info & BOI_NON_OPT_BIT_MASK) >> 4);
                w.byte((e.flags & VT_BIT_OFFSET_MASK) as u8);
            }
            TD_ARRAY1 => {
                w.svlq32(e.offset.lower_bound);
                w.vlq32(e.offset.count);
                w.vlq32(td.std_stride);
                w.vlq32(td.opt_stride);
            }
            TD_ARRAYM => {
                w.vlq32(td.std_stride);
                w.vlq32(td.opt_stride);
                let n = e.offset.dims();
                w.vlq32(n as u32);
                for d in 0..n {
                    w.svlq32(e.offset.m_lower[d]);
                    w.vlq32(e.offset.m_count[d]);
                }
            }
            _ => {}
        }
        w.byte(ELEM_TAG_DESCR_END);
    }
}
