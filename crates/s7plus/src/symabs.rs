//! Symbol → classic absolute address mapping.
//!
//! On the S7-1200 FW V3 a subscription item with a symbolic DB LID fails
//! with item error 0x13 while a read with the same address works; the
//! absolute DB address (LID [3, offset, length]) is accepted. Optimized DBs
//! have no absolute image, so they are not mapped.

use std::collections::HashMap;

use crate::browse::VarInfo;
use crate::client::Client;
use crate::error::Result;
use crate::ids::*;
use crate::softdt::*;

impl Client {
    /// Maps symbols of standard DBs and of the I/Q/M tag tables to classic
    /// absolute addresses ("DB11.DBW4:int", "DB11,S24.10", "M300.0").
    /// Symbols that cannot be mapped are absent from the result.
    pub fn classic_addresses(&self, syms: &[&str]) -> Result<HashMap<String, String>> {
        let vars = self.browse()?;
        let dbs = self.list_datablocks(true)?;
        let optimized: HashMap<u32, bool> = dbs.iter().map(|d| (d.rid, d.optimized)).collect();
        let by_name: HashMap<String, &VarInfo> = vars.iter().map(|v| (norm_sym(&v.name), v)).collect();
        let mut out = HashMap::new();
        for s in syms {
            let Some(v) = by_name.get(&norm_sym(s)) else { continue };
            let area = v.addr.area;
            let prefix = if area >> 16 == 0x8a0e {
                if optimized.get(&area).copied().unwrap_or(false) {
                    continue;
                }
                format!("DB{}.DB", area as u16)
            } else if area == RID_I_AREA {
                "I".into()
            } else if area == RID_Q_AREA {
                "Q".into()
            } else if area == RID_M_AREA {
                "M".into()
            } else {
                continue;
            };
            let a = classic_for(&prefix, v);
            if !a.is_empty() {
                out.insert(s.to_string(), a);
            }
        }
        Ok(out)
    }
}

fn norm_sym(s: &str) -> String {
    s.replace('"', "")
}

/// The classic address of one variable. prefix is "DB11.DB", "M", "I" or "Q".
pub(crate) fn classic_for(prefix: &str, v: &VarInfo) -> String {
    let off = v.non_opt_offset;
    let db = prefix.starts_with("DB");
    let area = prefix.strip_suffix(".DB").unwrap_or(prefix);
    let sized = |width: &str, typ: &str| format!("{prefix}{width}{off}:{typ}");
    match v.softdatatype {
        SDT_BOOL => {
            if db {
                format!("{prefix}X{off}.{}", v.non_opt_bit)
            } else {
                format!("{prefix}{off}.{}", v.non_opt_bit)
            }
        }
        SDT_BYTE => sized("B", "byte"),
        SDT_CHAR => sized("B", "char"),
        SDT_USINT => sized("B", "byte"),
        SDT_WORD => sized("W", "word"),
        SDT_INT => sized("W", "int"),
        SDT_UINT => sized("W", "word"),
        SDT_DATE => sized("W", "date"),
        SDT_DWORD => sized("D", "dword"),
        SDT_DINT => sized("D", "dint"),
        SDT_UDINT => sized("D", "dword"),
        SDT_REAL => sized("D", "real"),
        SDT_TIME_OF_DAY => sized("D", "tod"),
        SDT_TIME => sized("D", "time"),
        SDT_LREAL => sized("B", "lreal"),
        SDT_STRING if db => format!("{area},S{off}.{}", v.max_len),
        SDT_DTL if db => format!("{area},DTL{off}"),
        _ => String::new(),
    }
}
