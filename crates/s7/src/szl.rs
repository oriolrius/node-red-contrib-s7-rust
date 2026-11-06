//! System Status List (SZL): raw read, ID list, identification, components,
//! CPU state.

use std::collections::BTreeMap;

use crate::error::{io, Result};
use crate::log::hex_spaced;
use crate::userdata::GROUP_CPU;
use crate::wire::Conn;

/// SZL ID names (low byte = list, high byte = extract).
pub fn szl_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0000 => "list of all SZL IDs",
        0x0011 => "module identification (all)",
        0x0111 => "module identification (one)",
        0x0F11 => "module identification (header)",
        0x0012 => "CPU characteristics",
        0x0013 => "user memory areas",
        0x0014 => "system areas",
        0x0015 => "block types",
        0x0019 => "module LED status",
        0x001C => "component identification (all)",
        0x011C => "component identification (one)",
        0x0022 => "interrupt status",
        0x0025 => "process image partitions",
        0x0032 => "communication status data",
        0x0037 => "Ethernet details",
        0x0074 => "LED status",
        0x0091 => "module status",
        0x00A0 => "diagnostic buffer",
        0x0131 => "communication capability parameters",
        0x0132 => "communication status",
        0x0424 => "CPU operating mode",
        0x0524 => "CPU mode transitions",
        0x0D91 => "module status (rack)",
        0x0F14 => "system areas (header)",
        0x0F12 => "CPU characteristics (header)",
        0x0F1C => "component identification (header)",
        0x0F74 => "LED status (header)",
        0x0FA0 => "diagnostic buffer (header)",
        _ => return None,
    })
}

/// SZL 0x0011 record (module identification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderInfo {
    pub index: u16,
    pub order_no: String,
    pub version: String,
}

/// SZL 0x001C index names.
pub fn component_name(idx: u16) -> Option<&'static str> {
    Some(match idx {
        1 => "PLC name",
        2 => "module name",
        3 => "plant ID",
        4 => "copyright",
        5 => "serial number",
        7 => "module type",
        8 => "memory card serial",
        9 => "manufacturer/profile",
        10 => "OEM ID",
        11 => "location",
        _ => return None,
    })
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().trim_end_matches('\0').to_string()
}

impl Conn {
    /// Reads one SZL entry (Userdata group 4, sub 0x01). Returns the record
    /// length and the raw records. Only the first data unit carries the 8-byte
    /// SZL header (id, index, record length, record count); the others are
    /// records only.
    pub fn read_szl(&mut self, id: u16, index: u16) -> Result<(usize, Vec<u8>)> {
        let parts = self.userdata(GROUP_CPU, 0x01, Some(&[(id >> 8) as u8, id as u8, (index >> 8) as u8, index as u8])).map_err(|e| {
            if e.userdata_code().is_some() {
                e.context(format!("SZL 0x{id:04x}/{index}"))
            } else {
                e
            }
        })?;
        let d = &parts[0];
        if d.len() < 8 {
            return Err(io(format!("short SZL reply: {}", hex_spaced(d))));
        }
        let rec_len = u16::from_be_bytes([d[4], d[5]]) as usize;
        let count = u16::from_be_bytes([d[6], d[7]]) as usize;
        let mut recs = d[8..].to_vec();
        for p in &parts[1..] {
            recs.extend_from_slice(p);
        }
        if parts.len() == 1 && rec_len * count < recs.len() {
            recs.truncate(rec_len * count);
        } else if let Some(n) = recs.len().checked_div(rec_len) {
            recs.truncate(n * rec_len);
        }
        Ok((rec_len, recs))
    }

    /// SZL 0x0000: the SZL IDs this CPU supports (one 2-byte ID per record).
    pub fn szl_ids(&mut self) -> Result<Vec<u16>> {
        let (rec_len, recs) = self.read_szl(0x0000, 0)?;
        if rec_len < 2 {
            return Err(io(format!("SZL 0x0000 record length {rec_len}")));
        }
        Ok(recs.chunks_exact(rec_len).map(|r| u16::from_be_bytes([r[0], r[1]])).collect())
    }

    /// SZL 0x0011: order numbers and versions of the CPU (index 1), hardware
    /// (6) and firmware (7).
    pub fn module_identification(&mut self) -> Result<Vec<OrderInfo>> {
        let (rec_len, recs) = self.read_szl(0x0011, 0)?;
        if rec_len < 28 {
            return Err(io(format!("SZL 0x0011 record length {rec_len}")));
        }
        Ok(recs
            .chunks_exact(rec_len)
            .map(|r| {
                let be = |i: usize| u16::from_be_bytes([r[i], r[i + 1]]);
                // Ausbg (2) + Ausbe (2): for firmware 'V' + major, minor, patch.
                let version = if r[24] == b'V' {
                    format!("V{}.{}.{}", r[25], r[26], r[27])
                } else if be(26) == 0x2020 {
                    // Ausbe is blank: Ausbg alone is the product state (e.g. "FS 02" → 2).
                    be(24).to_string()
                } else {
                    format!("{}.{}", be(24), be(26))
                };
                OrderInfo { index: be(0), order_no: String::from_utf8_lossy(&r[2..22]).trim().to_string(), version }
            })
            .collect())
    }

    /// SZL 0x001C (component identification): plant/module names, copyright,
    /// serial number, module type… index → text. Falls back to single records
    /// (SZL 0x011C, indices 1-11) when the CPU rejects "all records".
    pub fn component_info(&mut self) -> Result<BTreeMap<u16, String>> {
        let mut out = BTreeMap::new();
        match self.read_szl(0x001C, 0) {
            Err(err) => {
                for idx in 1..=11u16 {
                    match self.read_szl(0x011C, idx) {
                        Ok((rl, r)) if rl > 2 && r.len() >= rl => {
                            out.insert(idx, text(&r[2..rl]));
                        }
                        _ => {}
                    }
                }
                if out.is_empty() {
                    return Err(err);
                }
                Ok(out)
            }
            Ok((rec_len, recs)) => {
                if rec_len > 2 {
                    for r in recs.chunks_exact(rec_len) {
                        out.insert(u16::from_be_bytes([r[0], r[1]]), text(&r[2..]));
                    }
                }
                Ok(out)
            }
        }
    }

    /// SZL 0x0424 (operating mode): byte 3 of the record holds the state:
    /// 0x08 RUN, 0x03/0x04 STOP, 0x05 STARTUP [std, snap7 convention].
    pub fn cpu_state(&mut self) -> Result<(String, u8)> {
        let (rec_len, recs) = self.read_szl(0x0424, 0)?;
        if rec_len < 4 || recs.len() < 4 {
            return Err(io("short SZL 0x0424 record"));
        }
        let st = recs[3];
        let name = match st {
            0x08 => "RUN",
            0x03 | 0x04 => "STOP",
            0x05 => "STARTUP",
            _ => "unknown",
        };
        Ok((name.to_string(), st))
    }
}
