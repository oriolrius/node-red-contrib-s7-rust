//! Object streams.

use std::fmt;

use crate::error::Error;
use crate::ids::*;
use crate::tagdescr::{decode_tag_descr, encode_tag_descrs, TagDescr};
use crate::textfmt::hex;
use crate::typeinfo::{decode_varname_list, decode_vartype_list, encode_varname_list, encode_vartype_list, VartypeList};
use crate::value::Value;
use crate::wire::{Reader, Writer};

// Element tags inside an object stream.
pub const ELEM_START_OBJECT: u8 = 0xa1;
pub const ELEM_END_OBJECT: u8 = 0xa2;
pub const ELEM_ATTRIBUTE: u8 = 0xa3;
pub const ELEM_RELATION: u8 = 0xa4;
pub const ELEM_TAG_DESCR: u8 = 0xa7;
pub const ELEM_TAG_DESCR_END: u8 = 0xa8;
pub const ELEM_VARTYPE_LIST: u8 = 0xab;
pub const ELEM_VARNAME_LIST: u8 = 0xac;

/// An S7CommPlus object: header (relation id, class id, class flags,
/// attribute id), attributes and child objects.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Object {
    pub rid: u32,
    pub class: u32,
    pub class_flags: u32,
    pub aid: u32,
    /// Attributes in insertion order.
    pub attrs: Vec<(u32, Value)>,
    pub children: Vec<Object>,
    /// 0xa4 elements in insertion order.
    pub relations: Vec<(u32, u32)>,
    /// 0xab.
    pub vartypes: Option<VartypeList>,
    /// 0xac (empty = absent).
    pub varnames: Vec<String>,
    /// 0xa7 (S7-1200 FW <= V3); encode emits 0xa7 when not empty.
    pub tag_descrs: Vec<TagDescr>,
    /// Undecoded tail (unknown element tags).
    pub raw: Vec<u8>,
}

impl Object {
    pub fn new(rid: u32, class: u32) -> Self {
        Object { rid, class, ..Default::default() }
    }

    /// Adds a relation element (0xa4).
    pub fn set_relation(&mut self, id: u32, rid: u32) {
        if let Some(e) = self.relations.iter_mut().find(|(k, _)| *k == id) {
            e.1 = rid;
        } else {
            self.relations.push((id, rid));
        }
    }

    /// Returns a relation.
    pub fn relation(&self, id: u32) -> Option<u32> {
        self.relations.iter().find(|(k, _)| *k == id).map(|(_, v)| *v)
    }

    /// Attribute 233 (ObjectVariableTypeName) when present.
    pub fn name(&self) -> String {
        self.attr(ATTR_OBJECT_VARIABLE_TYPE_NAME).and_then(|v| v.v.as_str()).unwrap_or("").to_string()
    }

    /// The first object (self or a descendant) that matches f.
    pub fn find(&self, f: &dyn Fn(&Object) -> bool) -> Option<&Object> {
        if f(self) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(f))
    }

    /// Returns an attribute.
    pub fn attr(&self, id: u32) -> Option<&Value> {
        self.attrs.iter().find(|(k, _)| *k == id).map(|(_, v)| v)
    }

    /// Adds or replaces an attribute.
    pub fn set_attr(&mut self, id: u32, v: Value) {
        if let Some(e) = self.attrs.iter_mut().find(|(k, _)| *k == id) {
            e.1 = v;
        } else {
            self.attrs.push((id, v));
        }
    }

    /// Reads an object that starts with 0xa1 up to its 0xa2.
    pub fn decode(r: &mut Reader) -> Option<Object> {
        decode_object(r)
    }

    /// Appends the object.
    pub fn encode(&self, w: &mut Writer) {
        w.byte(ELEM_START_OBJECT);
        w.u32(self.rid);
        w.vlq32(self.class);
        w.vlq32(self.class_flags);
        w.vlq32(self.aid);
        for (id, v) in &self.attrs {
            w.byte(ELEM_ATTRIBUTE);
            w.vlq32(*id);
            v.encode(w);
        }
        if let Some(vt) = &self.vartypes {
            if !self.tag_descrs.is_empty() {
                encode_tag_descrs(w, self);
            } else {
                w.byte(ELEM_VARTYPE_LIST);
                encode_vartype_list(w, vt);
            }
        }
        if !self.varnames.is_empty() && self.tag_descrs.is_empty() {
            w.byte(ELEM_VARNAME_LIST);
            encode_varname_list(w, &self.varnames);
        }
        for c in &self.children {
            c.encode(w);
        }
        for (id, rid) in &self.relations {
            w.byte(ELEM_RELATION);
            w.vlq32(*id);
            w.u32(*rid);
        }
        w.byte(ELEM_END_OBJECT);
    }

    /// Visits self and all descendants (depth first).
    pub fn walk(&self, f: &mut dyn FnMut(&Object)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    fn format(&self, sb: &mut String, ind: usize) {
        let p = "  ".repeat(ind);
        sb.push_str(&format!(
            "{p}object rid={} class={} flags={:#x} aid={}\n",
            id_name(self.rid),
            id_name(self.class),
            self.class_flags,
            id_name(self.aid)
        ));
        for (id, v) in &self.attrs {
            let mut vs = String::new();
            v_format(v, &mut vs, ind + 1);
            sb.push_str(&format!("{p}  @{} = {vs}\n", id_name(*id)));
        }
        for (id, rid) in &self.relations {
            sb.push_str(&format!("{p}  ->{} = {}\n", id_name(*id), id_name(*rid)));
        }
        if let Some(vt) = &self.vartypes {
            sb.push_str(&format!("{p}  vartypes first={:#x}\n", vt.first_id));
            for (i, e) in vt.elems.iter().enumerate() {
                let name = self.varnames.get(i).map(|s| s.as_str()).unwrap_or("");
                sb.push_str(&format!("{p}    {name:<20} {e}\n"));
            }
        }
        for c in &self.children {
            c.format(sb, ind + 1);
        }
        if !self.raw.is_empty() {
            sb.push_str(&format!("{p}  raw({}) {}\n", self.raw.len(), hex(&self.raw[..self.raw.len().min(64)])));
        }
    }
}

fn v_format(v: &Value, sb: &mut String, ind: usize) {
    // indentation of nested structs follows the object level
    let s = v.to_string();
    if ind == 0 {
        sb.push_str(&s);
    } else {
        sb.push_str(&s.replace('\n', &format!("\n{}", "  ".repeat(ind))));
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sb = String::new();
        self.format(&mut sb, 0);
        f.write_str(&sb)
    }
}

/// Reads an object that starts with 0xa1 up to its 0xa2.
pub fn decode_object(r: &mut Reader) -> Option<Object> {
    if !r.enter() {
        return None;
    }
    let o = decode_object_inner(r);
    r.leave();
    o
}

fn decode_object_inner(r: &mut Reader) -> Option<Object> {
    if r.byte() != ELEM_START_OBJECT {
        let at = r.pos.saturating_sub(1);
        r.set_err(Error::proto(format!("object does not start with 0xa1 at {at}")));
        return None;
    }
    let mut o = Object { rid: r.u32(), class: r.vlq32(), class_flags: r.vlq32(), aid: r.vlq32(), ..Default::default() };
    while r.ok() {
        match r.peek() {
            ELEM_END_OBJECT => {
                r.byte();
                return Some(o);
            }
            ELEM_START_OBJECT => {
                if let Some(c) = decode_object(r) {
                    o.children.push(c);
                }
            }
            ELEM_ATTRIBUTE => {
                r.byte();
                let id = r.vlq32();
                let v = Value::decode(r);
                o.set_attr(id, v);
            }
            ELEM_RELATION => {
                r.byte();
                let id = r.vlq32();
                let rid = r.u32();
                o.set_relation(id, rid);
            }
            ELEM_VARTYPE_LIST => {
                r.byte();
                o.vartypes = Some(decode_vartype_list(r));
            }
            ELEM_VARNAME_LIST => {
                r.byte();
                o.varnames = decode_varname_list(r);
            }
            ELEM_TAG_DESCR => {
                r.byte();
                decode_tag_descr(r, &mut o);
            }
            _ => {
                // Unknown tags are kept raw.
                o.raw.extend_from_slice(r.rest());
                r.pos = r.b.len();
                return Some(o);
            }
        }
    }
    Some(o)
}
