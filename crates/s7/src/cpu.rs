//! CPU families: connection defaults and type/area limits.

use std::fmt;

use crate::addr::Tag;
use crate::error::{local, Result};
use crate::types::Type;
use crate::wire::DialOptions;

/// The CPU model option of the driver's device (Device Properties — model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    S7200,
    S7300,
    S7400,
    S71200,
    S71500,
    /// LOGO! 8 (0BA8 and 8.FS4+): rack 0 / slot 2, i.e. remote TSAP 0x0102 with
    /// local 0x0100. V memory (VB/VW/VD/V0.0) is DB1.
    Logo8,
    /// Classic LOGO! server connection set up in LOGO!Soft Comfort (LOGO TSAP
    /// 20.00, PC TSAP 10.00): local 0x1000, remote 0x2000 [snap7 LOGO guide].
    Logo8Tsap,
}

pub const FAMILIES: [Family; 7] =
    [Family::S7200, Family::S7300, Family::S7400, Family::S71200, Family::S71500, Family::Logo8, Family::Logo8Tsap];

/// Per-family defaults and limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FamilyInfo {
    pub rack: u8,
    pub slot: u8,
    pub link_type: u8,
    /// 0 = default 0x0100
    pub local_tsap: u16,
    /// 0 = derived from LinkType/Rack/Slot
    pub remote_tsap: u16,
    /// S7 timers/counters (T, C)
    pub has_timers_ctr: bool,
    /// DATE_AND_TIME
    pub has_dt: bool,
    /// LINT/LWORD
    pub has_lint: bool,
    /// LREAL
    pub has_lreal: bool,
    /// STRING maximum length
    pub max_string: usize,
    /// DTL (S7-1200/1500)
    pub has_dtl: bool,
    /// WCHAR / WSTRING (S7-1200/1500)
    pub has_wide: bool,
    /// LTIME / LTOD / LDT (S7-1500)
    pub has_ltime: bool,
}

impl Family {
    /// Family name ("s7-1200", "logo8", "logo8-tsap").
    pub fn as_str(self) -> &'static str {
        match self {
            Family::S7200 => "s7-200",
            Family::S7300 => "s7-300",
            Family::S7400 => "s7-400",
            Family::S71200 => "s7-1200",
            Family::S71500 => "s7-1500",
            Family::Logo8 => "logo8",
            Family::Logo8Tsap => "logo8-tsap",
        }
    }

    /// Connection defaults (S7-200 local/remote TSAP 0x4D57; CPU
    /// rack/slot) and the type limits per family (STRING max 210,
    /// DT only 300/400/1500, LINT 1500, LREAL 1200/1500, no T/C on 1200/1500).
    pub fn info(self) -> FamilyInfo {
        let d = FamilyInfo { max_string: 210, ..Default::default() };
        match self {
            Family::S7200 => FamilyInfo { slot: 1, link_type: 0x01, local_tsap: 0x4D57, remote_tsap: 0x4D57, has_timers_ctr: true, ..d },
            Family::S7300 => FamilyInfo { slot: 2, link_type: 0x01, has_timers_ctr: true, has_dt: true, ..d },
            Family::S7400 => FamilyInfo { slot: 3, link_type: 0x01, has_timers_ctr: true, has_dt: true, ..d },
            Family::S71200 => FamilyInfo { slot: 1, link_type: 0x03, has_lreal: true, has_dtl: true, has_wide: true, ..d },
            Family::S71500 => FamilyInfo {
                slot: 1,
                link_type: 0x03,
                has_dt: true,
                has_lint: true,
                has_lreal: true,
                has_dtl: true,
                has_wide: true,
                has_ltime: true,
                ..d
            },
            Family::Logo8 => FamilyInfo { slot: 2, link_type: 0x01, local_tsap: 0x0100, ..d },
            Family::Logo8Tsap => FamilyInfo { slot: 2, link_type: 0x01, local_tsap: 0x1000, remote_tsap: 0x2000, ..d },
        }
    }

    /// Fills unset connection fields of `o` with the family defaults.
    pub fn apply(self, o: &mut DialOptions, rack_set: bool, slot_set: bool, link_set: bool) {
        let fi = self.info();
        if !rack_set {
            o.rack = fi.rack;
        }
        if !slot_set {
            o.slot = fi.slot;
        }
        if !link_set {
            o.link_type = fi.link_type;
        }
        if o.local_tsap == 0 {
            o.local_tsap = fi.local_tsap;
        }
        if o.remote_tsap == 0 {
            o.remote_tsap = fi.remote_tsap;
        }
    }

    /// Reports whether the family supports the tag.
    pub fn check(self, t: &Tag) -> Result<()> {
        let fi = self.info();
        let ty = t.typ;
        let f = self.as_str();
        let msg = if t.item.is_timer_counter() && !fi.has_timers_ctr {
            format!("{}: {f} has no S7 timers/counters", t.text)
        } else if matches!(ty, Type::DT | Type::DTZ) && !fi.has_dt {
            format!("{}: {f} does not support DATE_AND_TIME", t.text)
        } else if matches!(ty, Type::LInt | Type::LWord) && !fi.has_lint {
            format!("{}: {f} does not support LINT/LWORD", t.text)
        } else if ty == Type::LReal && !fi.has_lreal {
            format!("{}: {f} does not support LREAL", t.text)
        } else if matches!(ty, Type::DTL | Type::DTLZ) && !fi.has_dtl {
            format!("{}: {f} does not support DTL", t.text)
        } else if matches!(ty, Type::WChar | Type::WString) && !fi.has_wide {
            format!("{}: {f} does not support WCHAR/WSTRING", t.text)
        } else if matches!(ty, Type::LTime | Type::LTOD | Type::LDT) && !fi.has_ltime {
            format!("{}: {f} does not support LTIME/LTOD/LDT", t.text)
        } else if ty == Type::String && t.str_len > fi.max_string {
            format!("{}: STRING longer than {}", t.text, fi.max_string)
        } else {
            return Ok(());
        };
        Err(local(msg))
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Accepts "1200", "s7-1200", "S71200", "logo", "logo8", "logo-tsap"…
pub fn parse_family(s: &str) -> Result<Family> {
    let k = s.trim().to_lowercase();
    match k.replace(['!', ' '], "").as_str() {
        "logo" | "logo8" | "logo0ba8" => return Ok(Family::Logo8),
        "logo-tsap" | "logo8-tsap" | "logotsap" => return Ok(Family::Logo8Tsap),
        _ => {}
    }
    let k = k.strip_prefix("s7-").unwrap_or(&k);
    let k = k.strip_prefix("s7").unwrap_or(k);
    let name = format!("s7-{k}");
    FAMILIES
        .iter()
        .copied()
        .find(|f| f.as_str() == name)
        .ok_or_else(|| local(format!("unknown CPU family {s:?} (200, 300, 400, 1200, 1500, logo, logo-tsap)")))
}

/// Parses a TSAP written in hex ("4D57", "10.00", "0x0301"; "" = 0).
pub fn parse_tsap(s: &str) -> Result<u16> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    let s = s.replace(['.', ':'], "");
    let h = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(&s);
    if h.is_empty() || !h.bytes().all(|c| c.is_ascii_hexdigit() || c == b'_') {
        return Err(local(format!("bad TSAP {s:?}")));
    }
    u16::from_str_radix(&h.replace('_', ""), 16).map_err(|_| local(format!("bad TSAP {s:?}")))
}
