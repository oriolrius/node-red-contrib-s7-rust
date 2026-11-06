//! zlib blobs with preset dictionaries and comments.
//!
//! The PLC compresses XML blobs (comments, interface descriptions) with
//! preset dictionaries keyed by their Adler-32 (the DICTID of the zlib
//! header). They are Siemens data: the crate ships none; load them into a
//! [`Dictionaries`] registry (no global state, one registry per client or
//! shared through an `Arc`).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Mutex, RwLock};

use flate2::{Decompress, FlushDecompress, Status};

use crate::client::{lock, Client};
use crate::error::{Error, Result};
use crate::explore::ExploreReq;
use crate::ids::*;
use crate::value::V;

/// Adler-32 checksum.
pub fn adler32(b: &[u8]) -> u32 {
    let (mut a, mut s) = (1u32, 0u32);
    for chunk in b.chunks(5552) {
        for x in chunk {
            a += *x as u32;
            s += a;
        }
        a %= 65521;
        s %= 65521;
    }
    s << 16 | a
}

/// A registry of zlib preset dictionaries keyed by Adler-32.
#[derive(Debug, Default)]
pub struct Dictionaries {
    dicts: RwLock<HashMap<u32, Vec<u8>>>,
    warnings: Mutex<Vec<String>>,
}

impl Dictionaries {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a dictionary and returns its id.
    pub fn register(&self, d: &[u8]) -> u32 {
        let id = adler32(d);
        self.dicts.write().unwrap_or_else(|e| e.into_inner()).insert(id, d.to_vec());
        id
    }

    /// Ids of the registered dictionaries.
    pub fn ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.dicts.read().unwrap_or_else(|e| e.into_inner()).keys().copied().collect();
        v.sort();
        v
    }

    fn get(&self, id: u32) -> Option<Vec<u8>> {
        self.dicts.read().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()
    }

    /// Mismatches found while loading dictionaries.
    pub fn warnings(&self) -> Vec<String> {
        lock(&self.warnings).clone()
    }

    /// Clears the warnings.
    pub fn clear_warnings(&self) {
        lock(&self.warnings).clear();
    }

    /// Extracts the byte arrays of a C# source that declares
    /// "DICTID_x = 0x…" and "dict_x = new byte[] {…}". Every dictionary is
    /// checked against its declared Adler-32 (registered by the computed id).
    pub fn load_from_cs_source(&self, src: &str) -> usize {
        let ids = parse_dict_ids(src);
        let mut n = 0;
        for (name, body) in parse_dict_bodies(src) {
            let d = parse_hex_bytes(body);
            let got = adler32(&d);
            match ids.get(&name) {
                Some(want) if *want == got => {}
                want => {
                    // the reference declares NWT_90000001 as 0xfd9ac74, the bytes give 0xfd69ac74
                    lock(&self.warnings).push(format!("dictionary {name}: adler {got:#x}, declared {:#x}", want.copied().unwrap_or(0)));
                }
            }
            self.register(&d);
            n += 1;
        }
        n
    }

    /// Loads a C# source file (see [`Dictionaries::load_from_cs_source`]).
    pub fn load_from_cs(&self, path: impl AsRef<Path>) -> Result<usize> {
        let src = std::fs::read(path)?;
        Ok(self.load_from_cs_source(&String::from_utf8_lossy(&src)))
    }

    /// Registers every *.bin file of a directory.
    pub fn load_dir(&self, dir: impl AsRef<Path>) -> Result<usize> {
        let mut n = 0;
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "bin") {
                self.register(&std::fs::read(&p)?);
                n += 1;
            }
        }
        Ok(n)
    }

    /// Loads a .cs source, a directory of .bin files or one dictionary file.
    pub fn load(&self, src: &str) -> Result<usize> {
        if src.is_empty() {
            return Ok(0);
        }
        let p = Path::new(src);
        if p.is_dir() {
            return self.load_dir(p);
        }
        if src.ends_with(".cs") {
            return self.load_from_cs(p);
        }
        self.register(&std::fs::read(p)?);
        Ok(1)
    }

    /// Inflates a zlib blob that starts at offset (the PLC puts a 4-byte
    /// dictionary version before the zlib stream of comments). When the
    /// header asks for a preset dictionary it is looked up by its id.
    pub fn decompress_blob(&self, b: &[u8], offset: usize) -> Result<Vec<u8>> {
        if offset + 2 > b.len() {
            return Err(Error::Short);
        }
        let z = &b[offset..];
        let mut dict = None;
        if z[1] & 0x20 != 0 {
            // FDICT
            if z.len() < 6 {
                return Err(Error::Short);
            }
            let id = u32::from_be_bytes([z[2], z[3], z[4], z[5]]);
            dict = Some(self.get(id).ok_or_else(|| Error::proto(format!("no zlib dictionary {id:#x} (load the dictionaries)")))?);
        }
        inflate(z, dict.as_deref())
    }
}

fn inflate(z: &[u8], dict: Option<&[u8]>) -> Result<Vec<u8>> {
    let zerr = |e: &dyn std::fmt::Display| Error::proto(format!("zlib: {e}"));
    let mut d = Decompress::new(true);
    let mut out: Vec<u8> = Vec::with_capacity(z.len() * 4 + 256);
    loop {
        if out.capacity() - out.len() < 4096 {
            out.reserve(out.capacity().max(4096));
        }
        let pos = d.total_in() as usize;
        let before = (d.total_in(), d.total_out());
        match d.decompress_vec(&z[pos..], &mut out, FlushDecompress::Finish) {
            Ok(Status::StreamEnd) => return Ok(out),
            Ok(_) => {
                if (d.total_in(), d.total_out()) == before && d.total_in() as usize >= z.len() {
                    return Err(Error::proto("zlib: unexpected end of stream"));
                }
            }
            Err(e) => {
                if e.needs_dictionary().is_some() {
                    let dict = dict.ok_or_else(|| Error::proto("zlib: dictionary needed"))?;
                    d.set_dictionary(dict).map_err(|e| zerr(&e))?;
                    continue;
                }
                return Err(zerr(&e));
            }
        }
    }
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// `DICTID_(\w+)\s*=\s*0x([0-9a-fA-F]+)`
fn parse_dict_ids(src: &str) -> HashMap<String, u32> {
    let b = src.as_bytes();
    let mut out = HashMap::new();
    let mut from = 0;
    while let Some(k) = src[from..].find("DICTID_") {
        let s = from + k + 7;
        from = s;
        let mut i = s;
        while i < b.len() && is_word(b[i]) {
            i += 1;
        }
        if i == s {
            continue;
        }
        let name = &src[s..i];
        let mut j = skip_ws(b, i);
        if j >= b.len() || b[j] != b'=' {
            continue;
        }
        j = skip_ws(b, j + 1);
        if !src[j..].starts_with("0x") {
            continue;
        }
        let h0 = j + 2;
        let mut h = h0;
        while h < b.len() && b[h].is_ascii_hexdigit() {
            h += 1;
        }
        if h == h0 {
            continue;
        }
        if let Ok(v) = u64::from_str_radix(&src[h0..h], 16) {
            out.insert(name.to_string(), v as u32);
        }
    }
    out
}

/// `(?s)dict_(\w+)\s*=\s*new byte\[\]\s*\{(.*?)\}`
fn parse_dict_bodies(src: &str) -> Vec<(String, &str)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(k) = src[from..].find("dict_") {
        let s = from + k + 5;
        from = s;
        let mut i = s;
        while i < b.len() && is_word(b[i]) {
            i += 1;
        }
        if i == s {
            continue;
        }
        let name = &src[s..i];
        let mut j = skip_ws(b, i);
        if j >= b.len() || b[j] != b'=' {
            continue;
        }
        j = skip_ws(b, j + 1);
        if !src[j..].starts_with("new byte[]") {
            continue;
        }
        j = skip_ws(b, j + 10);
        if j >= b.len() || b[j] != b'{' {
            continue;
        }
        let Some(end) = src[j + 1..].find('}') else { continue };
        out.push((name.to_string(), &src[j + 1..j + 1 + end]));
        from = j + 1 + end;
    }
    out
}

/// `0x([0-9a-fA-F]{2})`
fn parse_hex_bytes(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= b.len() {
        if b[i] == b'0' && b[i + 1] == b'x' && b[i + 2].is_ascii_hexdigit() && b[i + 3].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).unwrap());
            i += 4;
        } else {
            i += 1;
        }
    }
    out
}

/// The comments of a DB or of the I/Q/M tag tables.
#[derive(Debug, Default)]
pub struct Comments {
    /// ASObjectES.Comment: language id -> text.
    pub object: BTreeMap<u32, String>,
    /// DataInterface.LineComments, inflated XML.
    pub line_xml: String,
    /// Compressed blob when it could not be inflated.
    pub raw_line: Vec<u8>,
    pub line_error: Option<Error>,
}

/// Where to load the dictionaries from: env S7PLUS_DICTS, or none.
pub fn default_dictionary_source() -> String {
    if let Ok(p) = std::env::var("S7PLUS_DICTS") {
        if !p.is_empty() {
            return p;
        }
    }
    String::new()
}

impl Client {
    /// Explores an area (0x8a0e0000+n DB, 82 M area, ...) for its comment
    /// (4288 / 4287) and its line comments (2546, zlib XML with a dictionary).
    pub fn get_comments(&self, rid: u32) -> Result<Comments> {
        let objs = self.explore_objects(&ExploreReq {
            object: rid,
            recursive: true,
            unknown: 1,
            attrs: vec![ATTR_AS_OBJECT_ES_COMMENT, ATTR_AS_OBJECT_COMMENT_V3, ATTR_DATA_IF_LINE_COMMENTS],
            ..Default::default()
        })?;
        let mut out = Comments::default();
        for o in &objs {
            for id in [ATTR_AS_OBJECT_ES_COMMENT, ATTR_AS_OBJECT_COMMENT_V3] {
                if let Some(m) = o.attr(id).and_then(|v| v.v.as_sparse()) {
                    for (k, x) in m {
                        out.object.insert(*k, x.as_str().unwrap_or("").to_string());
                    }
                }
            }
            if let Some(v) = o.attr(ATTR_DATA_IF_LINE_COMMENTS) {
                let blob = match &v.v {
                    // sparse: key 1 in DBs, 2 in the M area
                    V::Sparse(m) => m.values().next().and_then(|x| x.as_bytes()).map(|b| b.to_vec()),
                    V::Bytes(b) => Some(b.clone()),
                    _ => None,
                };
                if let Some(blob) = blob {
                    match self.dictionaries().decompress_blob(&blob, 4) {
                        Ok(x) => {
                            out.line_xml = String::from_utf8_lossy(&x).into_owned();
                            out.line_error = None;
                        }
                        Err(e) => {
                            out.line_xml.clear();
                            out.line_error = Some(e);
                            out.raw_line = blob;
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// zlib stream (best compression) with a preset dictionary (the inverse of
/// [`Dictionaries::decompress_blob`]; used by the fake PLC and tests).
pub fn compress_with_dictionary(data: &[u8], dict: &[u8]) -> Vec<u8> {
    let mut c = flate2::Compress::new(flate2::Compression::best(), true);
    let _ = c.set_dictionary(dict);
    let mut out = Vec::with_capacity(data.len() + 64);
    loop {
        out.reserve(4096);
        let pos = c.total_in() as usize;
        match c.compress_vec(&data[pos..], &mut out, flate2::FlushCompress::Finish) {
            Ok(flate2::Status::StreamEnd) | Err(_) => return out,
            Ok(_) => {}
        }
    }
}
