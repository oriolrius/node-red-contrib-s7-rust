//! Type information cache, DB list, browse and symbolic resolution.

use std::collections::HashMap;
use std::sync::Arc;

use crate::client::{lock, Client};
use crate::error::{Error, Result};
use crate::explore::{class_filter, ExploreReq};
use crate::ids::*;
use crate::msg::{sub_area_for, ItemAddress};
use crate::object::Object;
use crate::plcvalue::Tag;
use crate::softdt::*;
use crate::typeinfo::{OffsetInfo, Vartype};

/// Cached type-info objects and DB list of a client.
#[derive(Debug, Default)]
pub(crate) struct TypeCache {
    pub types: HashMap<u32, Arc<Object>>,
    pub dbs: Option<Vec<DbInfo>>,
}

/// Explore request for type information (recursive, no parents: the PLC
/// returns nothing for type-info objects when the parents flag is set).
fn type_info_req(rid: u32) -> ExploreReq {
    ExploreReq { object: rid, recursive: true, unknown: 1, ..Default::default() }
}

/// A data block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DbInfo {
    pub name: String,
    pub number: u16,
    /// Access area (0x8a0e0000 + number).
    pub rid: u32,
    /// Read from LID 1 of the DB.
    pub type_info_rid: u32,
    /// From the type info (set by list_datablocks when with_types).
    pub optimized: bool,
    /// Number of top-level variables (with_types).
    pub vars: usize,
}

/// One browsed variable (leaf).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VarInfo {
    pub name: String,
    pub softdatatype: u8,
    pub addr: ItemAddress,
    pub opt_offset: u32,
    pub opt_bit: u8,
    pub non_opt_offset: u32,
    pub non_opt_bit: u8,
    /// String/WString.
    pub max_len: u16,
}

const MAX_BROWSE_DEPTH: usize = 32;

impl Client {
    /// Explores a type-info object (e.g. 0x90030000 = M-area tag table, or
    /// the type-info RID of a DB) and caches every object returned.
    pub fn get_type_information(&self, rid: u32) -> Result<Vec<Object>> {
        let objs = self.explore_objects(&type_info_req(rid))?;
        self.cache_types(&objs);
        Ok(objs)
    }

    fn cache_types(&self, objs: &[Object]) {
        let mut tc = lock(&self.types);
        for o in objs {
            o.walk(&mut |x| {
                if x.class == CLASS_TYPE_INFO || x.vartypes.is_some() {
                    tc.types.insert(x.rid, Arc::new(x.clone()));
                }
            });
        }
    }

    /// The type-info object of rid, from the cache or the PLC.
    pub fn type_info(&self, rid: u32) -> Result<Arc<Object>> {
        if let Some(o) = lock(&self.types).types.get(&rid) {
            return Ok(o.clone());
        }
        self.get_type_information(rid)?;
        lock(&self.types).types.get(&rid).cloned().ok_or_else(|| Error::proto(format!("no type info {rid:#x}")))
    }

    /// Explores the OMS type-info container (537) in one request.
    pub fn load_all_types(&self) -> Result<()> {
        self.get_type_information(OBJ_OMS_TYPE_INFO_CONTAINER).map(|_| ())
    }

    /// Forgets the cached type information and DB list (after a download).
    pub fn clear_type_cache(&self) {
        let mut tc = lock(&self.types);
        tc.types.clear();
        tc.dbs = None;
    }

    /// Lists the DBs of the PLC program: Explore of PLCProgram with a class
    /// filter (DB_Class), then a read of LID 1 of every DB which returns the
    /// RID of its type information (instance DBs point to the FB type).
    pub fn list_datablocks(&self, with_types: bool) -> Result<Vec<DbInfo>> {
        let objs = self.explore_objects(&ExploreReq {
            object: RID_PLC_PROGRAM,
            recursive: true,
            unknown: 1,
            filter: Some(class_filter(CLASS_DB)),
            attrs: vec![ATTR_OBJECT_VARIABLE_TYPE_NAME],
            ..Default::default()
        })?;
        let mut dbs = Vec::new();
        for o in &objs {
            o.walk(&mut |x| {
                if x.class == CLASS_DB && x.rid >> 16 == 0x8a0e {
                    dbs.push(DbInfo { name: x.name(), number: x.rid as u16, rid: x.rid, ..Default::default() });
                }
            });
        }
        if dbs.is_empty() {
            return Ok(dbs);
        }
        let addrs: Vec<ItemAddress> =
            dbs.iter().map(|d| ItemAddress { area: d.rid, sub_area: ATTR_DB_VALUE_ACTUAL, lid: vec![1], ..Default::default() }).collect();
        let (vals, errs) = self.read(&addrs)?;
        let mut out = Vec::new();
        for (i, mut d) in dbs.into_iter().enumerate() {
            let rid = vals[i].v.as_u32().unwrap_or(0);
            if errs[i] != 0 || rid == 0 {
                continue; // e.g. DBs only in load memory
            }
            d.type_info_rid = rid;
            if with_types {
                if let Ok(ti) = self.type_info(rid) {
                    if let Some(vt) = &ti.vartypes {
                        d.vars = vt.elems.len();
                        d.optimized = vt.elems.iter().any(|e| e.optimized());
                    }
                }
            }
            out.push(d);
        }
        Ok(out)
    }

    /// Lists every variable of the DBs and of the I/Q/M tag tables, with
    /// name, softdatatype, access address (LID path) and offsets.
    pub fn browse(&self) -> Result<Vec<VarInfo>> {
        let dbs = self.list_datablocks(false)?;
        self.load_all_types()?;
        let mut roots: Vec<(u32, u32, String)> = dbs.iter().map(|d| (d.rid, d.type_info_rid, quote_sym(&d.name))).collect();
        roots.push((RID_I_AREA, RID_I_TYPE_INFO, String::new()));
        roots.push((RID_Q_AREA, RID_Q_TYPE_INFO, String::new()));
        roots.push((RID_M_AREA, RID_M_TYPE_INFO, String::new()));
        let mut out = Vec::new();
        for (area, tirid, prefix) in roots {
            let Ok(ti) = self.type_info(tirid) else { continue };
            let base = ItemAddress { area, sub_area: sub_area_for(area), ..Default::default() };
            self.walk(&ti, &prefix, &base, 0, 0, 0, &mut out);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(&self, ti: &Object, prefix: &str, base: &ItemAddress, opt: u32, nonopt: u32, depth: usize, out: &mut Vec<VarInfo>) {
        let Some(vt) = &ti.vartypes else { return };
        if depth > MAX_BROWSE_DEPTH {
            return;
        }
        for (i, e) in vt.elems.iter().enumerate() {
            let name = ti.varnames.get(i).cloned().unwrap_or_else(|| format!("lid{}", e.lid));
            let full = if prefix.is_empty() { name } else { format!("{prefix}.{name}") };
            let addr = base.child(e.lid);
            let o = &e.offset;
            let e_opt = opt.wrapping_add(o.opt_addr);
            let e_non = nonopt.wrapping_add(o.non_opt_addr);
            if o.is_1dim() || o.is_mdim() {
                self.array(e, &full, &addr, e_opt, e_non, depth, out);
            } else if o.has_relation() && e.softdatatype != SDT_DTL {
                if let Ok(sub) = self.type_info(o.relation_id) {
                    self.walk(&sub, &full, &addr, e_opt, e_non, depth + 1, out);
                }
            } else {
                leaf(e, full, addr, e_opt, e_non, true, out);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn array(&self, e: &Vartype, name: &str, addr: &ItemAddress, opt: u32, non: u32, depth: usize, out: &mut Vec<VarInfo>) {
        let o = &e.offset;
        let (names, ids) = array_indexes(o, e.softdatatype);
        if o.has_relation() {
            let Ok(sub) = self.type_info(o.relation_id) else { return };
            let size = sub.attr(ATTR_TI_TCOM_SIZE).and_then(|v| v.v.as_u32()).unwrap_or(0);
            for (k, n) in names.iter().enumerate() {
                // arrays of structs have an extra 1 between the index and the member LID
                let a = addr.child(ids[k]).child(1);
                let off = (k as u32).wrapping_mul(size);
                self.walk(&sub, &format!("{name}{n}"), &a, opt.wrapping_add(off), non.wrapping_add(off), depth + 1, out);
            }
            return;
        }
        let size = elem_size(e);
        for (k, n) in names.iter().enumerate() {
            let off = (k as u32).wrapping_mul(size);
            leaf(e, format!("{name}{n}"), addr.child(ids[k]), opt.wrapping_add(off), non.wrapping_add(off), false, out);
        }
    }

    /// Resolves a symbol: "Data_block_1".Tag, "DB".arr[3], "DB".s.x,
    /// "DB".m[1,2], or a tag of the I/Q/M tag tables ("Motor_On").
    pub fn resolve_tag(&self, sym: &str) -> Result<Tag> {
        let (first, rest) = split_symbol(sym)?;
        let cached = lock(&self.types).dbs.clone();
        let dbs = match cached {
            Some(d) => d,
            None => {
                let d = self.list_datablocks(false)?;
                lock(&self.types).dbs = Some(d.clone());
                d
            }
        };
        for d in &dbs {
            if d.name == first {
                if rest.is_empty() {
                    return Err(Error::proto(format!("{sym:?} is a DB, give a member")));
                }
                let base = ItemAddress { area: d.rid, sub_area: ATTR_DB_VALUE_ACTUAL, ..Default::default() };
                return self.resolve_in(d.type_info_rid, &rest, sym, base, 0);
            }
        }
        for (area, ti) in [(RID_M_AREA, RID_M_TYPE_INFO), (RID_Q_AREA, RID_Q_TYPE_INFO), (RID_I_AREA, RID_I_TYPE_INFO)] {
            let base = ItemAddress { area, sub_area: ATTR_CTRL_AREA_VALUE_ACT, ..Default::default() };
            match self.resolve_in(ti, sym, sym, base, 0) {
                Ok(t) => return Ok(t),
                Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Err(Error::NotFound(sym.to_string()))
    }

    fn resolve_in(&self, tirid: u32, sym: &str, full: &str, addr: ItemAddress, depth: usize) -> Result<Tag> {
        if depth > MAX_BROWSE_DEPTH {
            return Err(Error::proto("symbol too deep"));
        }
        let ti = self.type_info(tirid)?;
        let (lvl, mut rest) = split_symbol(sym)?;
        let idx = ti.varnames.iter().position(|n| *n == lvl);
        let e = match (idx, &ti.vartypes) {
            (Some(i), Some(vt)) if i < vt.elems.len() => vt.elems[i],
            _ => return Err(Error::NotFound(full.to_string())),
        };
        let o = &e.offset;
        let mut addr = addr.child(e.lid);
        let mut whole = false;
        if o.is_1dim() || o.is_mdim() {
            if rest.is_empty() || !rest.starts_with('[') {
                if o.is_mdim() || o.has_relation() || !rest.is_empty() {
                    return Err(Error::proto(format!("{full} needs an index")));
                }
                whole = true;
            } else {
                let (ix, r2) = split_symbol(&rest)?;
                let id = array_access_id(o, e.softdatatype, &ix).map_err(|m| Error::proto(format!("{full}: {m}")))?;
                addr = addr.child(id);
                if o.has_relation() {
                    addr = addr.child(1);
                }
                rest = r2;
            }
        }
        if o.has_relation() && e.softdatatype != SDT_DTL {
            if rest.is_empty() {
                return Err(Error::proto(format!("{full} is a struct, give a member")));
            }
            return self.resolve_in(o.relation_id, &rest, full, addr, depth + 1);
        }
        if !rest.is_empty() {
            return Err(Error::NotFound(format!("{full} (no member {rest:?})")));
        }
        let mut t = Tag { name: full.to_string(), addr, softdatatype: e.softdatatype, array: whole, ..Default::default() };
        if whole {
            t.count = o.count;
        }
        if e.softdatatype == SDT_STRING || e.softdatatype == SDT_WSTRING {
            t.max_len = o.info1;
        }
        Ok(t)
    }
}

fn leaf(e: &Vartype, name: String, addr: ItemAddress, opt: u32, non: u32, bits: bool, out: &mut Vec<VarInfo>) {
    let mut v = VarInfo { name, softdatatype: e.softdatatype, addr, opt_offset: opt, non_opt_offset: non, ..Default::default() };
    if bits {
        let (o, n) = e.bit_offsets();
        v.opt_bit = o;
        v.non_opt_bit = n;
    }
    if e.softdatatype == SDT_STRING || e.softdatatype == SDT_WSTRING {
        v.max_len = e.offset.info1;
    }
    out.push(v);
}

fn quote_sym(s: &str) -> String {
    format!("\"{s}\"")
}

/// Enumerates the element names and access ids of an array in the order of
/// the access ids (the reference's rule: id starts at 0 and the lowest
/// dimension varies fastest; BBool arrays pad the lowest dim to 8).
pub(crate) fn array_indexes(o: &OffsetInfo, sdt: u8) -> (Vec<String>, Vec<u32>) {
    let mut names = Vec::new();
    let mut ids = Vec::new();
    if o.is_1dim() {
        for i in 0..o.count {
            names.push(format!("[{}]", o.lower_bound as i64 + i as i64));
            ids.push(i);
        }
        return (names, ids);
    }
    let dims = o.dims();
    if dims == 0 {
        return (names, ids);
    }
    let mut x = [0u32; 7];
    let mut id = 0u32;
    for _ in 0..o.count {
        let mut parts = vec![String::new(); dims];
        for j in (0..dims).rev() {
            parts[dims - 1 - j] = (x[j] as i64 + o.m_lower[j] as i64).to_string();
        }
        names.push(format!("[{}]", parts.join(",")));
        ids.push(id);
        x[0] += 1;
        if sdt == SDT_BBOOL && x[0] >= o.m_count[0] && !o.m_count[0].is_multiple_of(8) {
            id += 8 - x[0] % 8;
        }
        for d in 0..5 {
            if x[d] >= o.m_count[d] {
                x[d] = 0;
                x[d + 1] += 1;
            }
        }
        id += 1;
    }
    (names, ids)
}

/// Splits the first level of a symbol: "\"DB 1\".a.b" -> ("DB 1", "a.b").
pub(crate) fn split_symbol(s: &str) -> Result<(String, String)> {
    if let Some(r) = s.strip_prefix('"') {
        let i = r.find('"').ok_or_else(|| Error::proto(format!("unbalanced quote in {s:?}")))?;
        let lvl = &r[..i];
        let rest = &s[2 + i..];
        return Ok((lvl.to_string(), rest.strip_prefix('.').unwrap_or(rest).to_string()));
    }
    let i = s.find(['.', '[']);
    if i == Some(0) && s.starts_with('[') {
        let j = s.find(']').ok_or_else(|| Error::proto(format!("unbalanced [ in {s:?}")))?;
        let rest = &s[j + 1..];
        return Ok((s[..j + 1].to_string(), rest.strip_prefix('.').unwrap_or(rest).to_string()));
    }
    match i {
        None => Ok((s.to_string(), String::new())),
        Some(i) => {
            let rest = &s[i..];
            Ok((s[..i].to_string(), rest.strip_prefix('.').unwrap_or(rest).to_string()))
        }
    }
}

/// Access id of "[i]" or "[i,j,...]".
pub(crate) fn array_access_id(o: &OffsetInfo, sdt: u8, ix: &str) -> std::result::Result<u32, String> {
    let ix = ix.strip_prefix('[').unwrap_or(ix);
    let ix = ix.strip_suffix(']').unwrap_or(ix);
    let mut idx = Vec::new();
    for p in ix.split(',') {
        let v: i32 = p.trim().parse().map_err(|_| format!("bad index {ix:?}"))?;
        idx.push(v as i64);
    }
    if o.is_1dim() {
        if idx.len() != 1 {
            return Err("one index expected".into());
        }
        let k = idx[0] - o.lower_bound as i64;
        if k < 0 || k >= o.count as i64 {
            return Err("index out of bounds".into());
        }
        return Ok(k as u32);
    }
    let dims = o.dims();
    if idx.len() != dims {
        return Err(format!("{dims} indexes expected"));
    }
    let mut counts = o.m_count;
    if sdt == SDT_BBOOL && !counts[0].is_multiple_of(8) {
        counts[0] += 8 - counts[0] % 8;
    }
    // written order: the first index is the highest dimension
    let (mut id, mut mul) = (0i64, 1i64);
    for d in 0..dims {
        let k = idx[dims - 1 - d] - o.m_lower[d] as i64;
        if k < 0 || k >= o.m_count[d] as i64 {
            return Err("index out of bounds".into());
        }
        id += k * mul;
        mul *= counts[d] as i64;
    }
    Ok(id as u32)
}
