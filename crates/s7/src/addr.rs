//! Address parser and tag codec.
//!
//! Address syntax (Siemens notation, the common SCADA/OPC subset):
//!
//! ```text
//! DB1.DBX0.3  DB1.DBB0  DB1.DBW2  DB1.DBD4   (also DB1.X0.3, DB1.B0, DB1.W2, DB1.D4)
//! M0.0 MB0 MW0 MD0   I0.0 IB0 IW0 ID0 (E = I)   Q0.0 QB0 QW0 QD0 (A = Q)
//! PIB0 PIW0 PQW0 (peripheral)   T0  C0 (Z = C)
//! V10.3 VB10 VW20 VD30                       (LOGO! 8 / S7-200 V memory = DB1)
//! ```
//!
//! Optional suffixes: `[n]` array of n elements, `:type` (int, dint, real,
//! lreal, lint, lword, word, dword, byte, char, bool, string[N], wstring[N],
//! wchar, dt, dtz, dtl, dtlz, date, tod, time, ltime, ltod, ldt, bcd, lbcd…).
//! The nodes7 notation is in `nodes7addr`.

use crate::error::{local, Result};
use crate::nodes7addr::parse_node_s7;
use crate::types::{self, Native, Type};
use crate::value::Val;
use crate::wire::{Item, AREA_DB, AREA_FLAGS, AREA_INPUTS, AREA_OUTPUTS, AREA_PERIPHERAL};

/// A parsed address: the wire item plus how to interpret it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub text: String,
    pub item: Item,
    pub typ: Type,
    /// array element count (1 = scalar)
    pub elems: usize,
    /// STRING[n] / WSTRING[n] maximum length
    pub str_len: usize,
    /// Swap the bytes of each 16/32/64-bit value (driver "Byte Order: Little
    /// Endian"; the S7 default is big-endian).
    pub little_endian: bool,
}

impl Tag {
    pub(crate) fn empty(text: &str) -> Tag {
        Tag { text: text.to_string(), item: Item::bytes(0, 0, 0, 0), typ: Type::Raw, elems: 1, str_len: 0, little_endian: false }
    }
}

/// Numeric multi-byte types affected by byte order.
pub fn swappable(t: Type) -> bool {
    matches!(
        t,
        Type::Word | Type::Int | Type::DWord | Type::DInt | Type::Real | Type::LWord | Type::LInt | Type::LReal | Type::BCD | Type::LBCD
    )
}

fn reversed(b: &[u8]) -> Vec<u8> {
    b.iter().rev().copied().collect()
}

/// Leading ASCII digits: (value saturated, rest). None if there is no digit.
pub(crate) fn digits(s: &str) -> Option<(i64, &str)> {
    let n = s.bytes().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    Some((s[..n].parse().unwrap_or(i64::MAX), &s[n..]))
}

/// All of `s` is digits.
fn all_digits(s: &str) -> Option<i64> {
    match digits(s) {
        Some((v, "")) => Some(v),
        _ => None,
    }
}

/// `<digits>(?:\.([0-7]))?$` → (offset, bit or -1)
fn off_bit(s: &str) -> Option<(i64, i64)> {
    let (off, rest) = digits(s)?;
    if rest.is_empty() {
        return Some((off, -1));
    }
    let b = rest.as_bytes();
    if b.len() == 2 && b[0] == b'.' && (b'0'..=b'7').contains(&b[1]) {
        return Some((off, (b[1] - b'0') as i64));
    }
    None
}

fn size_letter(c: u8) -> bool {
    matches!(c, b'X' | b'B' | b'W' | b'D')
}

/// `^DB(\d+)\.(?:DB)?([XBWD])(\d+)(?:\.([0-7]))?$`
fn match_db(a: &str) -> Option<(i64, char, i64, i64)> {
    let r = a.strip_prefix("DB")?;
    let (n, r) = digits(r)?;
    let r = r.strip_prefix('.')?;
    let try_at = |r: &str| -> Option<(char, i64, i64)> {
        let c = *r.as_bytes().first()?;
        if !size_letter(c) {
            return None;
        }
        let (off, bit) = off_bit(&r[1..])?;
        Some((c as char, off, bit))
    };
    if let Some(rr) = r.strip_prefix("DB") {
        if let Some((c, off, bit)) = try_at(rr) {
            return Some((n, c, off, bit));
        }
    }
    let (c, off, bit) = try_at(r)?;
    Some((n, c, off, bit))
}

/// `^(PI|PE|PQ|PA|I|E|Q|A|M|F)([XBWD]?)(\d+)(?:\.([0-7]))?$`
fn match_area(a: &str) -> Option<(u8, String, i64, i64)> {
    for (p, area) in [
        ("PI", AREA_PERIPHERAL),
        ("PE", AREA_PERIPHERAL),
        ("PQ", AREA_PERIPHERAL),
        ("PA", AREA_PERIPHERAL),
        ("I", AREA_INPUTS),
        ("E", AREA_INPUTS),
        ("Q", AREA_OUTPUTS),
        ("A", AREA_OUTPUTS),
        ("M", AREA_FLAGS),
        ("F", AREA_FLAGS),
    ] {
        let Some(r) = a.strip_prefix(p) else { continue };
        if let Some(&c) = r.as_bytes().first() {
            if size_letter(c) {
                if let Some((off, bit)) = off_bit(&r[1..]) {
                    return Some((area, (c as char).to_string(), off, bit));
                }
            }
        }
        if let Some((off, bit)) = off_bit(r) {
            return Some((area, String::new(), off, bit));
        }
    }
    None
}

/// `^([TCZ])(\d+)$`
fn match_tc(a: &str) -> Option<(char, i64)> {
    let c = *a.as_bytes().first()?;
    if !matches!(c, b'T' | b'C' | b'Z') {
        return None;
    }
    Some((c as char, all_digits(&a[1..])?))
}

/// `^(.*)\[(\d+)\]$`
fn match_arr(a: &str) -> Option<(&str, i64)> {
    let r = a.strip_suffix(']')?;
    let i = r.rfind('[')?;
    Some((&r[..i], all_digits(&r[i + 1..])?))
}

/// `^(W?STRING)\[(\d+)\]$`
fn match_str(ts: &str) -> Option<(bool, i64)> {
    let (wide, r) = match ts.strip_prefix("WSTRING") {
        Some(r) => (true, r),
        None => (false, ts.strip_prefix("STRING")?),
    };
    let r = r.strip_prefix('[')?.strip_suffix(']')?;
    Some((wide, all_digits(r)?))
}

/// `^V([XBWD]?)(\d+)(?:\.([0-7]))?$` → rewritten as the DB1 address.
fn rewrite_v(a: &str) -> Option<String> {
    let r = a.strip_prefix('V')?;
    let (size, r) = match r.as_bytes().first() {
        Some(&c) if size_letter(c) => match off_bit(&r[1..]) {
            Some(_) => ((c as char).to_string(), &r[1..]),
            None => (String::new(), r),
        },
        _ => (String::new(), r),
    };
    let (off, bit) = off_bit(r)?;
    let size = if size.is_empty() {
        if bit >= 0 {
            "X".to_string()
        } else {
            "B".to_string()
        }
    } else {
        size
    };
    let mut out = format!("DB1.DB{size}{off}");
    if bit >= 0 {
        out.push_str(&format!(".{bit}"));
    }
    Some(out)
}

/// Parses one address. It accepts the Siemens notation (with optional `[n]`
/// and `:type`) and the nodes7 / node-red-contrib-s7 notation ("DB1,REAL0",
/// "MI14", "MRW20"…): text with a comma goes to `parse_node_s7`, and text the
/// Siemens parser rejects is tried as nodes7.
pub fn parse_tag(s: &str) -> Result<Tag> {
    if s.contains(',') {
        return parse_node_s7(s);
    }
    match parse_siemens(s) {
        Ok(t) => Ok(t),
        Err(e) => {
            if !s.contains([':', '[']) {
                if let Ok(t2) = parse_node_s7(s) {
                    return Ok(t2);
                }
            }
            Err(e)
        }
    }
}

/// Parses the Siemens notation with optional `[n]` and `:type`.
pub fn parse_siemens(s: &str) -> Result<Tag> {
    let mut tag = Tag::empty(s);
    let upper = s.trim().to_uppercase();
    let mut a: &str = &upper;
    let mut typ: Option<Type> = None;
    if let Some(i) = a.rfind(':') {
        let ts = &a[i + 1..];
        a = &a[..i];
        if let Some((wide, n)) = match_str(ts) {
            tag.str_len = n.max(0) as usize;
            if wide {
                if !(1..=16382).contains(&n) {
                    return Err(local(format!("{s}: WSTRING length 1-16382")));
                }
                typ = Some(Type::WString);
            } else {
                if !(1..=254).contains(&n) {
                    return Err(local(format!("{s}: STRING length 1-254")));
                }
                typ = Some(Type::String);
            }
        } else {
            let name = ts.to_lowercase();
            typ = Some(Type::from_name(&name).ok_or_else(|| local(format!("{s}: unknown type {name:?}")))?);
        }
    }
    if let Some((inner, n)) = match_arr(a) {
        a = inner;
        if n < 1 {
            return Err(local(format!("{s}: array size must be >= 1")));
        }
        tag.elems = n as usize;
    }
    let v;
    if let Some(r) = rewrite_v(a) {
        v = r;
        a = &v;
    }

    let (area, db, size, off, bit, tc): (u8, u16, String, i64, i64, Option<char>);
    if let Some((n, c, o, b)) = match_db(a) {
        if n > 0xffff {
            return Err(local(format!("{s}: invalid DB number")));
        }
        (area, db, size, off, bit, tc) = (AREA_DB, n as u16, c.to_string(), o, b, None);
    } else if let Some((c, o)) = match_tc(a) {
        (area, db, size, off, bit, tc) = (0, 0, String::new(), o, -1, Some(c));
    } else if let Some((ar, sz, o, b)) = match_area(a) {
        let mut sz = sz;
        if b >= 0 {
            if !sz.is_empty() && sz != "X" {
                // "MW10.3" is not a Siemens address (nodes7 reads it as an array)
                return Err(local(format!("{s}: .bit only with X addressing")));
            }
            sz = "X".into();
        } else if sz.is_empty() {
            sz = "B".into();
        }
        (area, db, size, off, bit, tc) = (ar, 0, sz, o, b, None);
    } else {
        return Err(local(format!("cannot parse address {s:?}")));
    }

    match tc {
        Some('T') => {
            tag.typ = Type::S5Time;
            tag.item = Item::timers(off as u32, tag.elems as u16);
            return Ok(tag);
        }
        Some(_) => {
            tag.typ = Type::Counter;
            tag.item = Item::counters(off as u32, tag.elems as u16);
            return Ok(tag);
        }
        None => {}
    }

    if size == "X" {
        if bit < 0 {
            return Err(local(format!("{s}: bit address needs .bit")));
        }
        if tag.elems != 1 {
            return Err(local(format!("{s}: bit arrays are not supported")));
        }
        tag.typ = Type::Bool;
        tag.item = Item::bit(area, db, off as u32, bit as u8);
        return Ok(tag);
    }
    if bit >= 0 {
        return Err(local(format!("{s}: .bit only with X addressing")));
    }
    let typ = typ.unwrap_or(match size.as_str() {
        "B" => Type::Byte,
        "W" => Type::Word,
        _ => Type::DWord,
    });
    if typ == Type::Bool {
        return Err(local(format!("{s}: bool needs a bit address")));
    }
    let elem = typ.size(tag.str_len);
    tag.typ = typ;
    let total = elem.saturating_mul(tag.elems);
    if total > 0xffff {
        return Err(local(format!("{s}: too large")));
    }
    tag.item = Item::bytes(area, db, off as u32, total as u16);
    Ok(tag)
}

impl Tag {
    /// Byte size of one element of the tag.
    pub fn elem_size(&self) -> usize {
        if self.item.is_bit {
            return 1;
        }
        if self.typ == Type::S5Time || self.typ == Type::Counter {
            return 2;
        }
        self.typ.size(self.str_len)
    }

    fn elem<'a>(&self, b: &'a [u8], i: usize, n: usize) -> Result<std::borrow::Cow<'a, [u8]>> {
        if (i + 1) * n > b.len() {
            return Err(local("short data"));
        }
        let e = &b[i * n..(i + 1) * n];
        Ok(if self.little_endian && swappable(self.typ) { reversed(e).into() } else { e.into() })
    }

    /// Decodes raw data, element by element, to text: "-1234",
    /// "7 (0x07)", "\"hello\"", "[1, 2]".
    pub fn format(&self, b: &[u8]) -> Result<String> {
        if self.typ == Type::Raw {
            return Ok(crate::log::hex_spaced(b));
        }
        if self.item.is_bit {
            return types::decode(Type::Bool, b);
        }
        let n = self.elem_size();
        let mut parts = Vec::with_capacity(self.elems);
        for i in 0..self.elems {
            parts.push(types::decode(self.typ, &self.elem(b, i, n)?)?);
        }
        if self.elems == 1 {
            return Ok(parts.remove(0));
        }
        Ok(format!("[{}]", parts.join(", ")))
    }

    /// Encodes text (comma-separated for arrays) into the tag's bytes.
    pub fn value(&self, s: &str) -> Result<Vec<u8>> {
        if self.item.is_bit {
            return types::encode(Type::Bool, s, 0);
        }
        let vals: Vec<&str> = if self.elems > 1 {
            let v: Vec<&str> = s.trim_matches(['[', ']']).split(',').collect();
            if v.len() != self.elems {
                return Err(local(format!("{}: need {} values, got {}", self.text, self.elems, v.len())));
            }
            v
        } else {
            vec![s]
        };
        let mut out = Vec::with_capacity(self.elem_size() * self.elems);
        for v in vals {
            let b = types::encode(self.typ, v, self.str_len).map_err(|e| e.context(self.text.clone()))?;
            if self.little_endian && swappable(self.typ) {
                out.extend(b.iter().rev());
            } else {
                out.extend(b);
            }
        }
        Ok(out)
    }

    /// Encodes a JSON-ish value.
    pub fn value_from(&self, v: &Val) -> Result<Vec<u8>> {
        if let Some(b) = self.fast_int(v) {
            return Ok(b);
        }
        self.value(&v.to_text().map_err(|e| e.context(self.text.clone()))?)
    }

    /// Hot path of value_from: a scalar integer into an integer type, in
    /// range. Anything else (and every error) goes through the text path,
    /// so results and messages stay the same.
    fn fast_int(&self, v: &Val) -> Option<Vec<u8>> {
        let Val::Int(i) = v else { return None };
        if self.elems != 1 || self.item.is_bit {
            return None;
        }
        let i = *i;
        let mut b: Vec<u8> = match self.typ {
            Type::Byte if (0..=0xff).contains(&i) => vec![i as u8],
            Type::Word if (0..=0xffff).contains(&i) => (i as u16).to_be_bytes().to_vec(),
            Type::Int if (i16::MIN as i64..=i16::MAX as i64).contains(&i) => (i as i16).to_be_bytes().to_vec(),
            Type::DWord if (0..=0xffff_ffff).contains(&i) => (i as u32).to_be_bytes().to_vec(),
            Type::DInt if (i32::MIN as i64..=i32::MAX as i64).contains(&i) => (i as i32).to_be_bytes().to_vec(),
            Type::LInt => i.to_be_bytes().to_vec(),
            _ => return None,
        };
        if self.little_endian && swappable(self.typ) {
            b.reverse();
        }
        Some(b)
    }

    /// Decodes raw data into one typed value per element (byte order applied).
    pub fn natives(&self, b: &[u8]) -> Result<Vec<Native>> {
        if self.item.is_bit {
            if b.is_empty() {
                return Err(local("short data"));
            }
            return Ok(vec![Native::Bool(b[0] & 1 == 1)]);
        }
        let n = self.elem_size();
        let mut out = Vec::with_capacity(self.elems);
        for i in 0..self.elems {
            out.push(types::native(self.typ, &self.elem(b, i, n)?)?);
        }
        Ok(out)
    }

    /// Raw data → transport value (scalar, or `List`
    /// for `[n]` tags; types without a native form fall back to `format`).
    pub fn json_value(&self, data: &[u8]) -> Result<Val> {
        // hot path: one big-endian element of a fixed-size type
        if self.elems == 1 && !self.item.is_bit && !self.little_endian && data.len() >= self.elem_size() {
            if let Ok(n) = types::native(self.typ, &data[..self.elem_size()]) {
                return Ok(n.to_val());
            }
        }
        let vals = match self.natives(data) {
            Ok(v) => v,
            Err(e) => return self.format(data).map(Val::Str).map_err(|_| e),
        };
        let mut vals: Vec<Val> = vals.iter().map(Native::to_val).collect();
        if self.elems == 1 {
            return Ok(vals.remove(0));
        }
        Ok(Val::List(vals))
    }
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;

    /// The hot paths of value_from / json_value give what the text path gives.
    #[test]
    fn fast_paths_match_text_path() {
        let vals = [
            i64::MIN,
            -2147483649,
            -2147483648,
            -32769,
            -32768,
            -1,
            0,
            1,
            255,
            256,
            32767,
            32768,
            65535,
            65536,
            2147483647,
            4294967295,
            4294967296,
            i64::MAX,
        ];
        for t in ["MB0", "MW0", "MW0:int", "MD0", "MD0:dint", "MD0:lint", "MW0:int:le", "MD0:real", "MB0:bcd"] {
            let Ok(mut tag) = parse_tag(t.trim_end_matches(":le")) else { continue };
            tag.little_endian = t.ends_with(":le");
            for v in vals {
                let fast = tag.value_from(&Val::Int(v)).map_err(|e| e.to_string());
                let text = tag.value(&v.to_string()).map_err(|e| e.context(tag.text.clone()).to_string());
                assert_eq!(fast.is_ok(), text.is_ok(), "{t} {v}");
                if let (Ok(f), Ok(x)) = (&fast, &text) {
                    assert_eq!(f, x, "{t} {v}");
                    let a = tag.json_value(f).unwrap();
                    let b = tag.natives(f).unwrap()[0].to_val();
                    assert_eq!(a, b, "{t} {v}");
                }
            }
        }
    }
}
