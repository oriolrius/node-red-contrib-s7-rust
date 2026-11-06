//! Type information: vartype list (0xab), varname list (0xac) and the 16
//! offset-info kinds.

use std::fmt;

use crate::softdt::{softdatatype_name, SDT_BBOOL, SDT_BOOL};
use crate::wire::{Reader, Writer};

// OffsetInfo kinds (bits 12..15 of the vartype attribute flags). Values 1..7
// are used by older firmware inside structs, 8..15 by newer firmware.
pub const OI_FB_ARRAY: u8 = 0;
pub const OI_STRUCT_ELEM_STD: u8 = 1;
pub const OI_STRUCT_ELEM_STRING: u8 = 2;
pub const OI_STRUCT_ELEM_ARR1: u8 = 3;
pub const OI_STRUCT_ELEM_ARRM: u8 = 4;
pub const OI_STRUCT_ELEM_STRUCT: u8 = 5;
pub const OI_STRUCT_ELEM_STR_A1: u8 = 6;
pub const OI_STRUCT_ELEM_STR_AM: u8 = 7;
pub const OI_STD: u8 = 8;
pub const OI_STRING: u8 = 9;
pub const OI_ARRAY_1DIM: u8 = 10;
pub const OI_ARRAY_MDIM: u8 = 11;
pub const OI_STRUCT: u8 = 12;
pub const OI_STRUCT_1DIM: u8 = 13;
pub const OI_STRUCT_MDIM: u8 = 14;
pub const OI_FB_SFB: u8 = 15;

/// The decoded offset-info part of a vartype element. Only the fields that
/// apply to `kind` are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OffsetInfo {
    pub kind: u8,
    /// Strings: max length; arrays: element info.
    pub info1: u16,
    /// Strings: max length + 2.
    pub info2: u16,
    /// Byte offset in the optimized image.
    pub opt_addr: u32,
    /// Byte offset in the standard (non-optimized) image.
    pub non_opt_addr: u32,
    pub lower_bound: i32,
    /// Arrays: total element count.
    pub count: u32,
    pub m_lower: [i32; 6],
    pub m_count: [u32; 6],
    /// Struct arrays: element size.
    pub non_opt_size: u32,
    pub opt_size: u32,
    /// Structs/FBs: type-info RID of the element type.
    pub relation_id: u32,
    pub info4: [u32; 4],
    pub retain_off: u32,
    pub volatile_off: u32,
    pub classic_size: u32,
    pub retain_size: u32,
    pub volatile_sz: u32,
}

impl OffsetInfo {
    /// True for a struct/UDT/FB with its own type info.
    pub fn has_relation(&self) -> bool {
        matches!(
            self.kind,
            OI_FB_ARRAY
                | OI_STRUCT_ELEM_STRUCT
                | OI_STRUCT_ELEM_STR_A1
                | OI_STRUCT_ELEM_STR_AM
                | OI_STRUCT
                | OI_STRUCT_1DIM
                | OI_STRUCT_MDIM
                | OI_FB_SFB
        )
    }

    /// True for a one-dimensional array.
    pub fn is_1dim(&self) -> bool {
        matches!(self.kind, OI_STRUCT_ELEM_ARR1 | OI_ARRAY_1DIM | OI_STRUCT_ELEM_STR_A1 | OI_STRUCT_1DIM)
    }

    /// True for a multi-dimensional array.
    pub fn is_mdim(&self) -> bool {
        matches!(self.kind, OI_STRUCT_ELEM_ARRM | OI_ARRAY_MDIM | OI_STRUCT_ELEM_STR_AM | OI_STRUCT_MDIM)
    }

    /// Number of used dimensions of a multi-dimensional array.
    pub fn dims(&self) -> usize {
        self.m_count.iter().filter(|c| **c > 0).count()
    }
}

pub(crate) fn decode_offset_info(r: &mut Reader, kind: u8) -> OffsetInfo {
    let mut o = OffsetInfo { kind, ..Default::default() };
    if kind == OI_STD || kind == OI_STRUCT_ELEM_STD {
        let a = r.le_u16() as u32;
        let b = r.le_u16() as u32;
        if kind == OI_STD {
            o.opt_addr = a;
            o.non_opt_addr = b;
        } else {
            // the old struct-element form swaps the two offsets
            o.non_opt_addr = a;
            o.opt_addr = b;
        }
        return o;
    }
    o.info1 = r.le_u16();
    o.info2 = r.le_u16();
    o.opt_addr = r.le_u32();
    o.non_opt_addr = r.le_u32();
    let mdim = |r: &mut Reader, o: &mut OffsetInfo| {
        for i in 0..6 {
            o.m_lower[i] = r.le_u32() as i32;
        }
        for i in 0..6 {
            o.m_count[i] = r.le_u32();
        }
    };
    let rel = |r: &mut Reader, o: &mut OffsetInfo| {
        o.relation_id = r.le_u32();
        for i in 0..4 {
            o.info4[i] = r.le_u32();
        }
    };
    match kind {
        OI_STRING | OI_STRUCT_ELEM_STRING => {}
        OI_ARRAY_1DIM | OI_STRUCT_ELEM_ARR1 => {
            o.lower_bound = r.le_u32() as i32;
            o.count = r.le_u32();
        }
        OI_ARRAY_MDIM | OI_STRUCT_ELEM_ARRM => {
            o.lower_bound = r.le_u32() as i32;
            o.count = r.le_u32();
            mdim(r, &mut o);
        }
        OI_STRUCT | OI_STRUCT_ELEM_STRUCT => rel(r, &mut o),
        OI_STRUCT_1DIM | OI_STRUCT_ELEM_STR_A1 => {
            o.lower_bound = r.le_u32() as i32;
            o.count = r.le_u32();
            o.non_opt_size = r.le_u32();
            o.opt_size = r.le_u32();
            rel(r, &mut o);
        }
        OI_STRUCT_MDIM | OI_STRUCT_ELEM_STR_AM => {
            o.lower_bound = r.le_u32() as i32;
            o.count = r.le_u32();
            mdim(r, &mut o);
            o.non_opt_size = r.le_u32();
            o.opt_size = r.le_u32();
            rel(r, &mut o);
        }
        OI_FB_SFB => {
            rel(r, &mut o);
            o.retain_off = r.le_u32();
            o.volatile_off = r.le_u32();
        }
        OI_FB_ARRAY => {
            rel(r, &mut o);
            o.retain_off = r.le_u32();
            o.volatile_off = r.le_u32();
            o.count = r.le_u32();
            o.classic_size = r.le_u32();
            o.retain_size = r.le_u32();
            o.volatile_sz = r.le_u32();
            mdim(r, &mut o);
        }
        _ => {}
    }
    o
}

pub(crate) fn encode_offset_info(w: &mut Writer, o: &OffsetInfo) {
    match o.kind {
        OI_STD => {
            w.le_u16(o.opt_addr as u16);
            w.le_u16(o.non_opt_addr as u16);
            return;
        }
        OI_STRUCT_ELEM_STD => {
            w.le_u16(o.non_opt_addr as u16);
            w.le_u16(o.opt_addr as u16);
            return;
        }
        _ => {}
    }
    w.le_u16(o.info1);
    w.le_u16(o.info2);
    w.le_u32(o.opt_addr);
    w.le_u32(o.non_opt_addr);
    let mdim = |w: &mut Writer| {
        for v in o.m_lower {
            w.le_u32(v as u32);
        }
        for v in o.m_count {
            w.le_u32(v);
        }
    };
    let rel = |w: &mut Writer| {
        w.le_u32(o.relation_id);
        for v in o.info4 {
            w.le_u32(v);
        }
    };
    match o.kind {
        OI_ARRAY_1DIM | OI_STRUCT_ELEM_ARR1 => {
            w.le_u32(o.lower_bound as u32);
            w.le_u32(o.count);
        }
        OI_ARRAY_MDIM | OI_STRUCT_ELEM_ARRM => {
            w.le_u32(o.lower_bound as u32);
            w.le_u32(o.count);
            mdim(w);
        }
        OI_STRUCT | OI_STRUCT_ELEM_STRUCT => rel(w),
        OI_STRUCT_1DIM | OI_STRUCT_ELEM_STR_A1 => {
            w.le_u32(o.lower_bound as u32);
            w.le_u32(o.count);
            w.le_u32(o.non_opt_size);
            w.le_u32(o.opt_size);
            rel(w);
        }
        OI_STRUCT_MDIM | OI_STRUCT_ELEM_STR_AM => {
            w.le_u32(o.lower_bound as u32);
            w.le_u32(o.count);
            mdim(w);
            w.le_u32(o.non_opt_size);
            w.le_u32(o.opt_size);
            rel(w);
        }
        OI_FB_SFB => {
            rel(w);
            w.le_u32(o.retain_off);
            w.le_u32(o.volatile_off);
        }
        OI_FB_ARRAY => {
            rel(w);
            w.le_u32(o.retain_off);
            w.le_u32(o.volatile_off);
            w.le_u32(o.count);
            w.le_u32(o.classic_size);
            w.le_u32(o.retain_size);
            w.le_u32(o.volatile_sz);
            mdim(w);
        }
        _ => {}
    }
}

// Attribute flags of a vartype element.
pub const VT_OFFSET_INFO_MASK: u16 = 0xf000;
pub const VT_HMI_VISIBLE: u16 = 0x0800;
pub const VT_HMI_READONLY: u16 = 0x0400;
pub const VT_HMI_ACCESSIBLE: u16 = 0x0200;
pub const VT_OPTIMIZED: u16 = 0x0080;
pub const VT_SECTION_MASK: u16 = 0x0070;
pub const VT_BIT_OFFSET_MASK: u16 = 0x0007;
pub const BOI_RETAIN: u8 = 0x80;
pub const BOI_NON_OPT_BIT_MASK: u8 = 0x70;
pub const BOI_CLASSIC: u8 = 0x08;
pub const BOI_OPT_BIT_MASK: u8 = 0x07;

/// One element of a vartype list (0xab): the type of one variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vartype {
    pub lid: u32,
    pub symbol_crc: u32,
    pub softdatatype: u8,
    pub flags: u16,
    pub bit_info: u8,
    pub offset: OffsetInfo,
}

impl Vartype {
    pub fn optimized(&self) -> bool {
        self.flags & VT_OPTIMIZED != 0
    }

    pub fn section(&self) -> u32 {
        ((self.flags & VT_SECTION_MASK) >> 4) as u32
    }

    /// Bit offset of a Bool in the optimized and the standard image.
    pub fn bit_offsets(&self) -> (u8, u8) {
        match self.softdatatype {
            SDT_BOOL => {
                let opt = (self.flags & VT_BIT_OFFSET_MASK) as u8;
                let nonopt = if self.bit_info & BOI_CLASSIC != 0 { (self.bit_info & BOI_NON_OPT_BIT_MASK) >> 4 } else { opt };
                (opt, nonopt)
            }
            SDT_BBOOL => (self.bit_info & BOI_OPT_BIT_MASK, 0),
            _ => (0, 0),
        }
    }
}

impl fmt::Display for Vartype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let o = &self.offset;
        write!(
            f,
            "lid={} crc={:#x} {} flags={:#x} bits={:#x} oi={} opt={} std={}",
            self.lid,
            self.symbol_crc,
            softdatatype_name(self.softdatatype),
            self.flags,
            self.bit_info,
            o.kind,
            o.opt_addr,
            o.non_opt_addr
        )?;
        if o.is_1dim() || o.is_mdim() {
            write!(f, " lb={} n={}", o.lower_bound, o.count)?;
            if o.is_mdim() {
                let d = o.dims();
                write!(f, " dims={:?}/{:?}", &o.m_lower[..d], &o.m_count[..d])?;
            }
        }
        if o.has_relation() {
            write!(f, " ti={:#x}", o.relation_id)?;
        }
        if o.kind == OI_STRING || o.kind == OI_STRUCT_ELEM_STRING {
            write!(f, " maxlen={}", o.info1)?;
        }
        Ok(())
    }
}

/// A decoded 0xab element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VartypeList {
    pub first_id: u32,
    pub elems: Vec<Vartype>,
}

pub(crate) fn decode_vartype_list(r: &mut Reader) -> VartypeList {
    let mut vl = VartypeList::default();
    let mut first = true;
    while r.ok() {
        let n = r.u16() as usize;
        if n == 0 {
            break;
        }
        let end = r.pos + n;
        if end > r.b.len() {
            r.fail();
            break;
        }
        if first {
            vl.first_id = r.le_u32();
            first = false;
        }
        while r.pos < end && r.ok() {
            let lid = r.le_u32();
            let symbol_crc = r.le_u32();
            let softdatatype = r.byte();
            let flags = r.u16();
            let bit_info = r.byte();
            let offset = decode_offset_info(r, ((flags & VT_OFFSET_INFO_MASK) >> 12) as u8);
            vl.elems.push(Vartype { lid, symbol_crc, softdatatype, flags, bit_info, offset });
        }
    }
    vl
}

pub(crate) fn encode_vartype_list(w: &mut Writer, vl: &VartypeList) {
    let mut b = Writer::new();
    b.le_u32(vl.first_id);
    for e in &vl.elems {
        b.le_u32(e.lid);
        b.le_u32(e.symbol_crc);
        b.byte(e.softdatatype);
        b.u16((e.flags & !VT_OFFSET_INFO_MASK) | ((e.offset.kind as u16) << 12));
        b.byte(e.bit_info);
        encode_offset_info(&mut b, &e.offset);
    }
    w.u16(b.b.len() as u16);
    w.raw(&b.b);
    w.u16(0);
}

pub(crate) fn decode_varname_list(r: &mut Reader) -> Vec<String> {
    let mut names = Vec::new();
    while r.ok() {
        let n = r.u16() as usize;
        if n == 0 {
            break;
        }
        let end = r.pos + n;
        while r.pos < end && r.ok() {
            let l = r.byte() as usize;
            let s = r.bytes(l).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            names.push(s);
            r.byte(); // 0 terminator
        }
    }
    names
}

pub(crate) fn encode_varname_list(w: &mut Writer, names: &[String]) {
    let mut b = Writer::new();
    for n in names {
        b.byte(n.len() as u8);
        b.raw(n.as_bytes());
        b.byte(0);
    }
    w.u16(b.b.len() as u16);
    w.raw(&b.b);
    w.u16(0);
}
