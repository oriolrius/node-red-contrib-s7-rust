//! S7CommPlus values on the wire.

use std::collections::BTreeMap;
use std::fmt;

use crate::error::Error;
use crate::ids::id_name;
use crate::textfmt::{format_float, hex, quote};
use crate::wire::{Reader, Writer};

// Datatype codes of a value on the wire.
pub const T_NULL: u8 = 0x00;
pub const T_BOOL: u8 = 0x01;
pub const T_USINT: u8 = 0x02;
pub const T_UINT: u8 = 0x03;
pub const T_UDINT: u8 = 0x04;
pub const T_ULINT: u8 = 0x05;
pub const T_SINT: u8 = 0x06;
pub const T_INT: u8 = 0x07;
pub const T_DINT: u8 = 0x08;
pub const T_LINT: u8 = 0x09;
pub const T_BYTE: u8 = 0x0a;
pub const T_WORD: u8 = 0x0b;
pub const T_DWORD: u8 = 0x0c;
pub const T_LWORD: u8 = 0x0d;
pub const T_REAL: u8 = 0x0e;
pub const T_LREAL: u8 = 0x0f;
pub const T_TIMESTAMP: u8 = 0x10;
pub const T_TIMESPAN: u8 = 0x11;
pub const T_RID: u8 = 0x12;
pub const T_AID: u8 = 0x13;
pub const T_BLOB: u8 = 0x14;
pub const T_WSTRING: u8 = 0x15;
pub const T_VARIANT: u8 = 0x16;
pub const T_STRUCT: u8 = 0x17;
pub const T_S7STRING: u8 = 0x19;

// Value flags.
pub const FLAG_ARRAY: u8 = 0x10;
pub const FLAG_ADDRESS_ARRAY: u8 = 0x20;
pub const FLAG_SPARSE_ARRAY: u8 = 0x40;

/// Name of a datatype code.
pub fn type_name(t: u8) -> String {
    let n = match t {
        T_NULL => "Null",
        T_BOOL => "Bool",
        T_USINT => "USInt",
        T_UINT => "UInt",
        T_UDINT => "UDInt",
        T_ULINT => "ULInt",
        T_SINT => "SInt",
        T_INT => "Int",
        T_DINT => "DInt",
        T_LINT => "LInt",
        T_BYTE => "Byte",
        T_WORD => "Word",
        T_DWORD => "DWord",
        T_LWORD => "LWord",
        T_REAL => "Real",
        T_LREAL => "LReal",
        T_TIMESTAMP => "Timestamp",
        T_TIMESPAN => "Timespan",
        T_RID => "RID",
        T_AID => "AID",
        T_BLOB => "Blob",
        T_WSTRING => "WString",
        T_VARIANT => "Variant",
        T_STRUCT => "Struct",
        T_S7STRING => "S7String",
        _ => return format!("type{t:#04x}"),
    };
    n.to_string()
}

/// The payload of a [`Value`]. Scalars use the types of the
/// decoder: Bool, USInt/Byte = U8, SInt = I8, UInt/Word = U16, Int = I16,
/// UDInt/AID/DWord/RID = U32, DInt = I32, ULInt/LWord/Timestamp = U64,
/// LInt/Timespan = I64, Real = F32, LReal = F64, WString = Str, Blob = Bytes.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum V {
    #[default]
    Null,
    Bool(bool),
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Str(String),
    Bytes(Vec<u8>),
    Struct(Box<Struct>),
    /// Arrays and address arrays: the element payloads.
    Array(Vec<V>),
    /// Sparse arrays: key → element.
    Sparse(BTreeMap<u32, V>),
}

impl V {
    pub fn as_u8(&self) -> Option<u8> {
        if let V::U8(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_u16(&self) -> Option<u16> {
        if let V::U16(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_u32(&self) -> Option<u32> {
        if let V::U32(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_i16(&self) -> Option<i16> {
        if let V::I16(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        if let V::I32(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        if let V::U64(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        if let V::I64(x) = self {
            Some(*x)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let V::Str(x) = self {
            Some(x)
        } else {
            None
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let V::Bytes(x) = self {
            Some(x)
        } else {
            None
        }
    }
    pub fn as_struct(&self) -> Option<&Struct> {
        if let V::Struct(x) = self {
            Some(x)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[V]> {
        if let V::Array(x) = self {
            Some(x)
        } else {
            None
        }
    }
    pub fn as_sparse(&self) -> Option<&BTreeMap<u32, V>> {
        if let V::Sparse(x) = self {
            Some(x)
        } else {
            None
        }
    }

    // numeric views used by the encoder (lenient: any integer payload)
    fn uint(&self) -> u64 {
        match *self {
            V::Bool(b) => b as u64,
            V::U8(x) => x as u64,
            V::I8(x) => x as u64,
            V::U16(x) => x as u64,
            V::I16(x) => x as u64,
            V::U32(x) => x as u64,
            V::I32(x) => x as u64,
            V::U64(x) => x,
            V::I64(x) => x as u64,
            V::F32(x) => x as u64,
            V::F64(x) => x as u64,
            _ => 0,
        }
    }
    fn int(&self) -> i64 {
        match *self {
            V::I8(x) => x as i64,
            V::I16(x) => x as i64,
            V::I32(x) => x as i64,
            V::I64(x) => x,
            V::F32(x) => x as i64,
            V::F64(x) => x as i64,
            _ => self.uint() as i64,
        }
    }
    fn float(&self) -> f64 {
        match *self {
            V::F32(x) => x as f64,
            V::F64(x) => x,
            V::I8(_) | V::I16(_) | V::I32(_) | V::I64(_) => self.int() as f64,
            _ => self.uint() as f64,
        }
    }
}

/// One S7CommPlus value: flags, datatype and payload.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Value {
    pub flags: u8,
    pub typ: u8,
    pub v: V,
}

/// A struct value: an id and its elements by id (in insertion order), or a
/// packed byte image (system structs such as DTL).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Struct {
    pub id: u32,
    pub elems: Vec<(u32, Value)>,
    pub packed: Option<Vec<u8>>,
    pub packed_ts: u64,
    pub packed_tf: u32,
}

impl Struct {
    pub fn new(id: u32) -> Self {
        Struct { id, ..Default::default() }
    }

    /// Returns an element.
    pub fn get(&self, id: u32) -> Option<&Value> {
        self.elems.iter().find(|(k, _)| *k == id).map(|(_, v)| v)
    }

    /// Adds or replaces an element, keeping the insertion order.
    pub fn set(&mut self, id: u32, v: Value) {
        if let Some(e) = self.elems.iter_mut().find(|(k, _)| *k == id) {
            e.1 = v;
        } else {
            self.elems.push((id, v));
        }
    }

    /// Ids in insertion order.
    pub fn order(&self) -> impl Iterator<Item = u32> + '_ {
        self.elems.iter().map(|(k, _)| *k)
    }
}

pub(crate) fn is_packed(id: u32) -> bool {
    (id > 0x9000_0000 && id < 0x9fff_ffff) || (id > 0x0200_0000 && id < 0x02ff_ffff)
}

impl Value {
    pub fn new(typ: u8, v: V) -> Self {
        Value { flags: 0, typ, v }
    }
    pub fn null() -> Self {
        Value::default()
    }
    pub fn bool(b: bool) -> Self {
        Value::new(T_BOOL, V::Bool(b))
    }
    pub fn udint(v: u32) -> Self {
        Value::new(T_UDINT, V::U32(v))
    }
    pub fn dint(v: i32) -> Self {
        Value::new(T_DINT, V::I32(v))
    }
    pub fn int(v: i16) -> Self {
        Value::new(T_INT, V::I16(v))
    }
    pub fn uint(v: u16) -> Self {
        Value::new(T_UINT, V::U16(v))
    }
    pub fn usint(v: u8) -> Self {
        Value::new(T_USINT, V::U8(v))
    }
    pub fn byte(v: u8) -> Self {
        Value::new(T_BYTE, V::U8(v))
    }
    pub fn word(v: u16) -> Self {
        Value::new(T_WORD, V::U16(v))
    }
    pub fn dword(v: u32) -> Self {
        Value::new(T_DWORD, V::U32(v))
    }
    pub fn real(v: f32) -> Self {
        Value::new(T_REAL, V::F32(v))
    }
    pub fn lreal(v: f64) -> Self {
        Value::new(T_LREAL, V::F64(v))
    }
    pub fn lint(v: i64) -> Self {
        Value::new(T_LINT, V::I64(v))
    }
    pub fn ulint(v: u64) -> Self {
        Value::new(T_ULINT, V::U64(v))
    }
    pub fn rid(v: u32) -> Self {
        Value::new(T_RID, V::U32(v))
    }
    pub fn aid(v: u32) -> Self {
        Value::new(T_AID, V::U32(v))
    }
    pub fn wstring(s: impl Into<String>) -> Self {
        Value::new(T_WSTRING, V::Str(s.into()))
    }
    pub fn blob(b: impl Into<Vec<u8>>) -> Self {
        Value::new(T_BLOB, V::Bytes(b.into()))
    }
    pub fn timestamp(v: u64) -> Self {
        Value::new(T_TIMESTAMP, V::U64(v))
    }
    pub fn structv(s: Struct) -> Self {
        Value::new(T_STRUCT, V::Struct(Box::new(s)))
    }
    pub fn usint_array(b: &[u8]) -> Self {
        Value { flags: FLAG_ARRAY, typ: T_USINT, v: V::Array(b.iter().map(|x| V::U8(*x)).collect()) }
    }

    /// Encodes the value into a new buffer.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.b
    }

    /// Appends the value (flags, datatype, payload).
    pub fn encode(&self, w: &mut Writer) {
        w.byte(self.flags);
        w.byte(self.typ);
        if self.flags & (FLAG_ARRAY | FLAG_ADDRESS_ARRAY) != 0 {
            let empty = Vec::new();
            let a = self.v.as_array().map(|a| a as &[V]).unwrap_or(&empty);
            if self.typ == T_STRUCT {
                // array of structs: struct id once, count, then the members of
                // each element (id + value) up to a 0 (checked on PLCSIM with
                // the AckJob AcknowledgementList, flags 0x20)
                let id = a.first().and_then(|e| e.as_struct()).map(|s| s.id).unwrap_or(0);
                w.u32(id);
                w.vlq32(a.len() as u32);
                for e in a {
                    if let Some(s) = e.as_struct() {
                        for (id, v) in &s.elems {
                            w.vlq32(*id);
                            v.encode(w);
                        }
                    }
                    w.byte(0);
                }
                return;
            }
            w.vlq32(a.len() as u32);
            for e in a {
                encode_scalar(w, self.typ, e);
            }
        } else if self.flags & FLAG_SPARSE_ARRAY != 0 {
            if let V::Sparse(m) = &self.v {
                for (k, e) in m {
                    w.vlq32(*k);
                    if self.typ == T_BLOB {
                        let b = e.as_bytes().unwrap_or(&[]);
                        w.vlq32(0);
                        w.vlq32(b.len() as u32);
                        w.raw(b);
                        continue;
                    }
                    encode_scalar(w, self.typ, e);
                }
            }
            w.byte(0);
        } else {
            encode_scalar(w, self.typ, &self.v);
        }
    }

    /// Reads flags, datatype and payload.
    pub fn decode(r: &mut Reader) -> Value {
        decode_value(r)
    }

    fn format(&self, sb: &mut String, ind: usize) {
        let mut name = type_name(self.typ);
        if self.flags & FLAG_ARRAY != 0 {
            name += "[]";
        } else if self.flags & FLAG_ADDRESS_ARRAY != 0 {
            name += "[addr]";
        } else if self.flags & FLAG_SPARSE_ARRAY != 0 {
            name += "[sparse]";
        }
        match &self.v {
            V::Struct(s) => s.format(sb, ind, &name),
            V::Array(a) => {
                if self.typ == T_USINT || self.typ == T_BYTE {
                    let b: Vec<u8> = a.iter().map(|e| e.as_u8().unwrap_or(0)).collect();
                    sb.push_str(&format!("{name} {}", hex(&b)));
                    return;
                }
                sb.push_str(&format!("{name}{}", fmt_v(&self.v)));
            }
            V::Bytes(x) => {
                if x.len() > 48 {
                    let nz = x.iter().filter(|c| **c != 0).count();
                    sb.push_str(&format!("{name}({}) {}… ({nz} non-zero)", x.len(), hex(&x[..48])));
                    return;
                }
                sb.push_str(&format!("{name}({}) {}", x.len(), hex(x)));
            }
            V::Str(s) => sb.push_str(&format!("{name} {}", quote(s))),
            V::U32(x) => {
                if self.typ == T_RID || self.typ == T_DWORD {
                    sb.push_str(&format!("{name} {x:#x}"));
                } else {
                    sb.push_str(&format!("{name} {x}"));
                }
            }
            v => sb.push_str(&format!("{name} {}", fmt_v(v))),
        }
    }
}

/// Text form of a decoded payload (`[a b]` for arrays, `map[k:v]` for sparse arrays).
pub(crate) fn fmt_v(v: &V) -> String {
    match v {
        V::Null => "<nil>".into(),
        V::Bool(b) => b.to_string(),
        V::U8(x) => x.to_string(),
        V::I8(x) => x.to_string(),
        V::U16(x) => x.to_string(),
        V::I16(x) => x.to_string(),
        V::U32(x) => x.to_string(),
        V::I32(x) => x.to_string(),
        V::U64(x) => x.to_string(),
        V::I64(x) => x.to_string(),
        V::F32(x) => format_float(*x as f64, true),
        V::F64(x) => format_float(*x, false),
        V::Str(s) => s.clone(),
        V::Bytes(b) => format!("[{}]", b.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")),
        V::Struct(s) => {
            let mut sb = String::new();
            s.format(&mut sb, 0, "Struct");
            format!("&{{{sb}}}")
        }
        V::Array(a) => format!("[{}]", a.iter().map(fmt_v).collect::<Vec<_>>().join(" ")),
        V::Sparse(m) => format!("map[{}]", m.iter().map(|(k, e)| format!("{k}:{}", fmt_v(e))).collect::<Vec<_>>().join(" ")),
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sb = String::new();
        self.format(&mut sb, 0);
        f.write_str(&sb)
    }
}

impl Struct {
    fn format(&self, sb: &mut String, ind: usize, name: &str) {
        if let Some(p) = &self.packed {
            sb.push_str(&format!("{name} {:#x} packed(ts={:#x} tf={:#x}) {}", self.id, self.packed_ts, self.packed_tf, hex(p)));
            return;
        }
        sb.push_str(&format!("{name} {} {{", id_name(self.id)));
        for (id, v) in &self.elems {
            sb.push('\n');
            sb.push_str(&"  ".repeat(ind + 1));
            sb.push_str(&id_name(*id));
            sb.push_str(": ");
            v.format(sb, ind + 1);
        }
        sb.push('\n');
        sb.push_str(&"  ".repeat(ind));
        sb.push('}');
    }
}

/// Reads flags, datatype and payload.
pub fn decode_value(r: &mut Reader) -> Value {
    let flags = r.byte();
    let t = r.byte();
    let mut v = Value { flags, typ: t, v: V::Null };
    if flags & (FLAG_ARRAY | FLAG_ADDRESS_ARRAY) != 0 {
        let struct_id = if t == T_STRUCT { r.u32() } else { 0 };
        let n = r.vlq32() as usize;
        if n > r.left() + 1 {
            r.fail();
            return v;
        }
        let mut a = Vec::with_capacity(n);
        let mut i = 0;
        while i < n && r.ok() {
            if t == T_STRUCT {
                let mut s = Struct::new(struct_id);
                loop {
                    let id = r.vlq32();
                    if id == 0 || !r.ok() {
                        break;
                    }
                    let e = decode_value(r);
                    s.set(id, e);
                }
                a.push(V::Struct(Box::new(s)));
            } else {
                a.push(decode_scalar(r, t));
            }
            i += 1;
        }
        v.v = V::Array(a);
    } else if flags & FLAG_SPARSE_ARRAY != 0 {
        let mut m = BTreeMap::new();
        loop {
            let k = r.vlq32();
            if k == 0 || !r.ok() {
                break;
            }
            if t == T_BLOB {
                // sparse blobs: root id, size, bytes (no blob type)
                r.vlq32();
                let n = r.vlq32() as usize;
                let b = r.bytes(n).map(|b| b.to_vec()).unwrap_or_default();
                m.insert(k, V::Bytes(b));
                continue;
            }
            m.insert(k, decode_scalar(r, t));
        }
        v.v = V::Sparse(m);
    } else {
        v.v = decode_scalar(r, t);
    }
    v
}

fn decode_scalar(r: &mut Reader, t: u8) -> V {
    match t {
        T_NULL => V::Null,
        T_BOOL => V::Bool(r.byte() != 0),
        T_USINT | T_BYTE => V::U8(r.byte()),
        T_SINT => V::I8(r.byte() as i8),
        T_UINT | T_WORD => V::U16(r.u16()),
        T_INT => V::I16(r.u16() as i16),
        T_UDINT | T_AID => V::U32(r.vlq32()),
        T_DINT => V::I32(r.svlq32()),
        T_ULINT => V::U64(r.vlq64()),
        T_LINT | T_TIMESPAN => V::I64(r.svlq64()),
        T_DWORD | T_RID => V::U32(r.u32()),
        T_LWORD | T_TIMESTAMP => V::U64(r.u64()),
        T_REAL => V::F32(f32::from_bits(r.u32())),
        T_LREAL => V::F64(f64::from_bits(r.u64())),
        T_WSTRING => {
            let n = r.vlq32() as usize;
            V::Str(r.bytes(n).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default())
        }
        T_BLOB => {
            let root = r.vlq32();
            if root > 1 {
                r.u64();
                let bt = r.byte();
                if bt != 2 && bt != 3 {
                    r.set_err(Error::proto(format!("blob type {bt:#x} not supported")));
                    return V::Null;
                }
            }
            let n = r.vlq32() as usize;
            V::Bytes(r.bytes(n).map(|b| b.to_vec()).unwrap_or_default())
        }
        T_STRUCT => {
            let mut s = Struct::new(r.u32());
            if is_packed(s.id) {
                s.packed_ts = r.u64();
                s.packed_tf = r.vlq32();
                let mut n = r.vlq32();
                if s.packed_tf & 0x400 != 0 {
                    // count2 present
                    n = r.vlq32();
                }
                s.packed = Some(r.bytes(n as usize).map(|b| b.to_vec()).unwrap_or_default());
                return V::Struct(Box::new(s));
            }
            loop {
                let id = r.vlq32();
                if id == 0 || !r.ok() {
                    break;
                }
                let e = decode_value(r);
                s.set(id, e);
            }
            V::Struct(Box::new(s))
        }
        _ => {
            r.set_err(Error::proto(format!("datatype {t:#x} not supported")));
            V::Null
        }
    }
}

fn encode_scalar(w: &mut Writer, t: u8, x: &V) {
    match t {
        T_NULL => {}
        T_BOOL => w.byte((x.uint() != 0) as u8),
        T_USINT | T_BYTE => w.byte(x.uint() as u8),
        T_SINT => w.byte(x.int() as i8 as u8),
        T_UINT | T_WORD => w.u16(x.uint() as u16),
        T_INT => w.u16(x.int() as i16 as u16),
        T_UDINT | T_AID => w.vlq32(x.uint() as u32),
        T_DINT => w.svlq32(x.int() as i32),
        T_ULINT => w.vlq64(x.uint()),
        T_LINT | T_TIMESPAN => w.svlq64(x.int()),
        T_DWORD | T_RID => w.u32(x.uint() as u32),
        T_LWORD | T_TIMESTAMP => w.u64(x.uint()),
        T_REAL => w.f32(x.float() as f32),
        T_LREAL => w.f64(x.float()),
        T_WSTRING => {
            let s = x.as_str().unwrap_or("");
            w.vlq32(s.len() as u32);
            w.raw(s.as_bytes());
        }
        T_BLOB => {
            let b = x.as_bytes().unwrap_or(&[]);
            w.vlq32(0);
            w.vlq32(b.len() as u32);
            w.raw(b);
        }
        T_STRUCT => {
            let empty = Struct::default();
            let s = x.as_struct().unwrap_or(&empty);
            w.u32(s.id);
            if is_packed(s.id) {
                let p = s.packed.as_deref().unwrap_or(&[]);
                w.u64(s.packed_ts);
                w.vlq32(s.packed_tf);
                w.vlq32(p.len() as u32);
                w.raw(p);
                return;
            }
            for (id, v) in &s.elems {
                w.vlq32(*id);
                v.encode(w);
            }
            w.byte(0);
        }
        // Nothing is written: the datatype cannot be encoded, and the PLC
        // will reject the item.
        _ => {}
    }
}
