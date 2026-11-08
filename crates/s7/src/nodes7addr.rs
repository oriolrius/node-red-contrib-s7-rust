//! nodes7 / node-red-contrib-s7 address notation (our own parser of the syntax
//! documented in the node-red-contrib-s7 README "Variable addressing").
//!
//! ```text
//! DB5,X0.1   DB23,B1 / BYTE1   DB100,C2 / CHAR2   DB42,I3 / INT3   DB57,WORD4
//! DB13,DI5 / DINT5   DB19,DW6 / DWORD6   DB21,R7 / REAL7   DB2,S7.10 (STRING[10] at 7)
//! DB1,DT0   DB1,DTZ10 (UTC)   DB2,DTL2   DB2,DTLZ12 (UTC)
//! DB57,RWORD4  DB13,RDI5 …   R-prefixed types are little-endian
//! I1.0 E1.0 Q2.1 A2.1 M3.2   IB4 QB5 MB6   IC7 MC9 (char)   II10 MI14 (int)
//! IW16 MW20 (word)   IDI22 MDI26 (dint)   ID28 MD32 (dword)   IR34 MR38 (real)
//! MRW20 (word, little-endian)   PIW0 (peripheral)   T0  C0
//! ```
//!
//! A trailing ".n" is an array length ("DB1,INT0.5" = 5 INTs, "MW10.3"); for X
//! it is the bit and for S the string length, so their array length is a
//! second ".n" ("DB2,S7.10.3" = 3 strings). Bit arrays are not supported.

use crate::addr::{digits, Tag};
use crate::error::{local, Result};
use crate::types::Type;
use crate::wire::{Item, AREA_DB, AREA_FLAGS, AREA_INPUTS, AREA_OUTPUTS, AREA_PERIPHERAL};

/// (type, little-endian) of a nodes7 data type name.
fn n7_type(name: &str) -> Option<(Type, bool)> {
    Some(match name {
        "X" => (Type::Bool, false),
        "BYTE" | "B" => (Type::Byte, false),
        "CHAR" | "C" => (Type::Char, false),
        "STRING" | "S" => (Type::String, false),
        "INT" | "I" => (Type::Int, false),
        "DINT" | "DI" => (Type::DInt, false),
        "WORD" | "W" => (Type::Word, false),
        "DWORD" | "D" | "DW" => (Type::DWord, false),
        "REAL" | "R" => (Type::Real, false),
        "RINT" | "RI" => (Type::Int, true),
        "RDINT" | "RDI" => (Type::DInt, true),
        "RWORD" | "RW" => (Type::Word, true),
        "RDWORD" | "RD" | "RDW" => (Type::DWord, true),
        "RREAL" | "RR" => (Type::Real, true),
        "DT" => (Type::DT, false),
        "DTZ" => (Type::DTZ, false),
        "DTL" => (Type::DTL, false),
        "DTLZ" => (Type::DTLZ, false),
        _ => return None,
    })
}

/// The nodes7 data types allowed for I/Q/M/P (not X/S/DT…).
fn type_outside_db(name: &str) -> bool {
    matches!(name, "B" | "C" | "I" | "DI" | "W" | "D" | "DW" | "R" | "RI" | "RDI" | "RW" | "RD" | "RDW" | "RR")
}

fn n7_area(letter: u8) -> Option<u8> {
    Some(match letter {
        b'I' | b'E' => AREA_INPUTS,
        b'Q' | b'A' => AREA_OUTPUTS,
        b'M' => AREA_FLAGS,
        b'P' => AREA_PERIPHERAL,
        _ => return None,
    })
}

/// `^(?:DB(\d+),)?([A-Z]+)(\d+)(?:\.(\d+))?(?:\.(\d+))?$` →
/// (db or None, name, offset, f4, f5) with -1 for absent fields.
fn match_node_s7(a: &str) -> Option<(Option<i64>, &str, i64, i64, i64)> {
    let mut r = a;
    let mut db = None;
    if let Some(x) = r.strip_prefix("DB") {
        if let Some((n, rest)) = digits(x) {
            if let Some(rest) = rest.strip_prefix(',') {
                db = Some(n);
                r = rest;
            }
        }
    }
    let n = r.bytes().take_while(|c| c.is_ascii_uppercase()).count();
    if n == 0 {
        return None;
    }
    let name = &r[..n];
    let (off, mut r) = digits(&r[n..])?;
    let mut f = [-1i64, -1];
    for slot in f.iter_mut() {
        if let Some(x) = r.strip_prefix('.') {
            let (v, rest) = digits(x)?;
            *slot = v;
            r = rest;
        }
    }
    if !r.is_empty() {
        return None;
    }
    Some((db, name, off, f[0], f[1]))
}

/// Parses one address in nodes7 notation into a Tag.
pub fn parse_node_s7(s: &str) -> Result<Tag> {
    let mut tag = Tag::empty(s);
    let a = s.trim().to_uppercase();
    let Some((dbn, name0, off, f4, f5)) = match_node_s7(&a) else {
        return Err(local(format!("cannot parse nodes7 address {s:?}")));
    };
    if off == i64::MAX {
        return Err(local(format!("{s}: bad offset")));
    }
    let (area, db, name): (u8, i64, String);
    if let Some(n) = dbn {
        if !(1..=0xffff).contains(&n) {
            return Err(local(format!("{s}: invalid DB number")));
        }
        if n7_type(name0).is_none() {
            return Err(local(format!("{s}: unknown DB data type {name0:?}")));
        }
        (area, db, name) = (AREA_DB, n, name0.to_string());
    } else {
        let letter = name0.as_bytes()[0];
        let mut rest = &name0[1..];
        match letter {
            b'T' | b'C' => {
                if !rest.is_empty() || f5 >= 0 {
                    return Err(local(format!("{s}: unknown data type {name0:?}")));
                }
                let n = if f4 >= 0 { f4 } else { 1 };
                if n < 1 {
                    return Err(local(format!("{s}: invalid array length")));
                }
                tag.elems = n as usize;
                crate::addr::check_tc(s, off, tag.elems)?;
                if letter == b'T' {
                    tag.typ = Type::S5Time;
                    tag.item = Item::timers(off as u32, n as u16);
                } else {
                    tag.typ = Type::Counter;
                    tag.item = Item::counters(off as u32, n as u16);
                }
                return Ok(tag);
            }
            b'P' => {
                if rest.is_empty() || !b"IQEA".contains(&rest.as_bytes()[0]) {
                    return Err(local(format!("{s}: peripheral area needs PI/PQ/PE/PA")));
                }
                rest = &rest[1..];
            }
            _ => {}
        }
        let Some(ar) = n7_area(letter) else {
            return Err(local(format!("{s}: unknown area {:?}", (letter as char).to_string())));
        };
        let nm = if rest.is_empty() {
            "X".to_string()
        } else if type_outside_db(rest) {
            rest.to_string()
        } else {
            return Err(local(format!("{s}: unknown data type {rest:?}")));
        };
        (area, db, name) = (ar, 0, nm);
    }

    let (t, le) = n7_type(&name).unwrap();
    tag.typ = t;
    tag.little_endian = le;
    match t {
        Type::Bool => {
            if f4 < 0 {
                return Err(local(format!("{s}: bit address needs .bit")));
            }
            if f4 > 7 {
                return Err(local(format!("{s}: bit offset out of range 0-7")));
            }
            if f5 > 1 {
                return Err(local(format!("{s}: bit arrays are not supported")));
            }
            crate::addr::check_bytes(s, off, 1)?;
            tag.item = Item::bit(area, db as u16, off as u32, f4 as u8);
            return Ok(tag);
        }
        Type::String => {
            if !(1..=254).contains(&f4) {
                return Err(local(format!("{s}: STRING needs a length 1-254 (S<offset>.<length>)")));
            }
            tag.str_len = f4 as usize;
            if f5 >= 0 {
                tag.elems = f5 as usize;
            }
        }
        _ => {
            if f5 >= 0 {
                return Err(local(format!("{s}: invalid use of bit offset")));
            }
            if f4 >= 0 {
                tag.elems = f4.max(0) as usize;
            }
        }
    }
    if tag.elems < 1 {
        return Err(local(format!("{s}: invalid array length")));
    }
    let total = tag.typ.size(tag.str_len).saturating_mul(tag.elems);
    if total > 0xffff {
        return Err(local(format!("{s}: too large")));
    }
    crate::addr::check_bytes(s, off, total)?;
    tag.item = Item::bytes(area, db as u16, off as u32, total as u16);
    Ok(tag)
}
