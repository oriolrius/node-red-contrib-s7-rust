//! Alarms: DAI objects, alarm subscription and text library.

use std::collections::HashMap;
use std::fmt;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::explore::ExploreReq;
use crate::ids::*;
use crate::object::Object;
use crate::subscribe::{subscription_object, SubOptions, Subscription};
use crate::textfmt::{quote, DateTime};
use crate::value::{Struct, Value, FLAG_ADDRESS_ARRAY, FLAG_ARRAY, T_LWORD, T_STRUCT, T_UDINT, T_UINT, T_USINT, V};

/// The coming or going state of an alarm (struct 3473).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlarmCgs {
    pub all_states: u8,
    pub timestamp: Option<DateTime>,
    pub ack_time: Option<DateTime>,
    pub assoc_values: Vec<Vec<u8>>,
}

/// DAI.HmiInfo (7813).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HmiInfo {
    pub syntax_id: u16,
    pub version: u16,
    pub client_alarm_id: u32,
    pub priority: u8,
    pub alarm_class: u16,
    pub producer: u8,
    pub group_id: u8,
    pub flags: u8,
}

/// A decoded DAI object (class 2681): an alarm instance.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Alarm {
    pub name: String,
    /// RID of the source << 32 | alarm id << 16.
    pub cpu_alarm_id: u64,
    pub all_states: u8,
    pub domain: u16,
    pub message_type: i32,
    pub seq_counter: u32,
    pub coming: Option<AlarmCgs>,
    pub going: Option<AlarmCgs>,
    pub hmi: Option<HmiInfo>,
    /// Key = language << 16 | text id (1 info, 2 alarm, 3.. additional).
    pub texts: HashMap<u32, String>,
}

/// An S7CommPlus timestamp (ns since 1970); 0 = None.
fn ts_time(v: &Value) -> Option<DateTime> {
    match v.v.as_u64() {
        Some(u) if u != 0 => Some(DateTime::from_unix_nanos(u as i64)),
        _ => None,
    }
}

fn decode_cgs(v: &Value) -> Option<AlarmCgs> {
    let st = v.v.as_struct()?;
    let mut c = AlarmCgs::default();
    if let Some(e) = st.get(ATTR_AS_CGS_ALL_STATES_INFO) {
        c.all_states = e.v.as_u8().unwrap_or(0);
    }
    if let Some(e) = st.get(ATTR_AS_CGS_TIMESTAMP) {
        c.timestamp = ts_time(e);
    }
    if let Some(e) = st.get(ATTR_AS_CGS_ACK_TIMESTAMP) {
        c.ack_time = ts_time(e);
    }
    if let Some(a) = st.get(ATTR_AS_CGS_ASSOC_VALUES).and_then(|e| e.v.as_array()) {
        c.assoc_values = a.iter().map(|x| x.as_bytes().map(|b| b.to_vec()).unwrap_or_default()).collect();
    }
    Some(c)
}

/// Decodes the HmiInfo blob (big endian, layout by syntax id).
pub fn decode_hmi_info(b: &[u8]) -> Option<HmiInfo> {
    if b.len() < 9 {
        return None;
    }
    let mut h = HmiInfo {
        syntax_id: u16::from_be_bytes([b[0], b[1]]),
        version: u16::from_be_bytes([b[2], b[3]]),
        client_alarm_id: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
        priority: b[8],
        ..Default::default()
    };
    if h.syntax_id >= 258 && b.len() >= 17 {
        h.alarm_class = u16::from_be_bytes([b[12], b[13]]);
        h.producer = b[14];
        h.group_id = b[15];
        h.flags = b[16];
    }
    Some(h)
}

/// Decodes a DAI object (from Explore or an alarm notification).
pub fn decode_alarm(o: &Object) -> Alarm {
    let mut a = Alarm { name: o.name(), ..Default::default() };
    if let Some(v) = o.attr(ATTR_DAI_CPU_ALARM_ID) {
        a.cpu_alarm_id = v.v.as_u64().unwrap_or(0);
    }
    if let Some(v) = o.attr(ATTR_DAI_ALL_STATES_INFO) {
        a.all_states = v.v.as_u8().unwrap_or(0);
    }
    if let Some(v) = o.attr(ATTR_DAI_ALARM_DOMAIN) {
        a.domain = v.v.as_u16().unwrap_or(0);
    }
    if let Some(v) = o.attr(ATTR_DAI_MESSAGE_TYPE) {
        a.message_type = v.v.as_i32().unwrap_or(0);
    }
    if let Some(v) = o.attr(ATTR_DAI_SEQUENCE_COUNTER) {
        a.seq_counter = v.v.as_u32().unwrap_or(0);
    }
    if let Some(v) = o.attr(ATTR_DAI_COMING) {
        a.coming = decode_cgs(v);
    }
    if let Some(v) = o.attr(ATTR_DAI_GOING) {
        a.going = decode_cgs(v);
    }
    if let Some(b) = o.attr(ATTR_DAI_HMI_INFO).and_then(|v| v.v.as_bytes()) {
        a.hmi = decode_hmi_info(b);
    }
    if let Some(m) = o.attr(ATTR_DAI_ALARM_TEXTS).and_then(|v| v.v.as_sparse()) {
        for (k, x) in m {
            let s = x.as_bytes().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            a.texts.insert(*k, s);
        }
    }
    a
}

impl fmt::Display for Alarm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "alarm {} id={:#x} states={:#x} domain={} type={} seq={}",
            self.name, self.cpu_alarm_id, self.all_states, self.domain, self.message_type, self.seq_counter
        )?;
        for (n, c) in [("coming", &self.coming), ("going", &self.going)] {
            if let Some(t) = c.as_ref().and_then(|c| c.timestamp) {
                write!(f, " {n}={}", t.rfc3339_nano())?;
            }
        }
        if let Some(h) = &self.hmi {
            write!(f, " prio={} class={}", h.priority, h.alarm_class)?;
        }
        let mut keys: Vec<_> = self.texts.keys().collect();
        keys.sort();
        for k in keys {
            write!(f, " text[{}/{}]={}", k >> 16, k & 0xffff, quote(&self.texts[k]))?;
        }
        Ok(())
    }
}

const DAI_ATTRS: [u32; 10] = [
    ATTR_DAI_CPU_ALARM_ID,
    ATTR_DAI_ALL_STATES_INFO,
    ATTR_DAI_ALARM_DOMAIN,
    ATTR_DAI_COMING,
    ATTR_DAI_GOING,
    ATTR_DAI_MESSAGE_TYPE,
    ATTR_DAI_HMI_INFO,
    ATTR_OBJECT_VARIABLE_TYPE_NAME,
    ATTR_DAI_SEQUENCE_COUNTER,
    ATTR_DAI_ALARM_TEXTS,
];

/// Alarm subscription layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlarmSubOptions {
    /// Adds the S7CommPlusDriver extras (DelayTime 1050, AlarmDomain2 7731,
    /// text languages 8181, SendAlarmTexts 8173). The default is the TIA
    /// Portal V21 layout (subsystem as a 0xa4 relation).
    pub reference: bool,
    pub languages: Vec<u32>,
    pub with_texts: bool,
}

impl AlarmSubOptions {
    /// The layout for a PLC, from its PAOM string (`SessionInfo::paom`):
    /// the TIA Portal layout for the S7-1200 ("6ES7 21…", captured with TIA
    /// Portal V21 and FW V3), else the S7CommPlusDriver layout. The S7-1500
    /// FW V4.x (PLCSIM Advanced V8, FW S4.1) accepts the TIA layout but sends
    /// no alarm notifications with it.
    pub fn for_plc(paom: &str) -> AlarmSubOptions {
        let s7_1200 = paom.split(';').nth(1).is_some_and(|m| m.trim_start().starts_with("6ES7 21"));
        AlarmSubOptions { reference: !s7_1200, languages: vec![1033], with_texts: true }
    }
}

pub(crate) fn alarm_subscription_object(o: &SubOptions, ao: &AlarmSubOptions, rid: u32, ref_rid: u32) -> Object {
    let refs = Value { flags: FLAG_ADDRESS_ARRAY, typ: T_UDINT, v: V::Array(vec![V::U32(0x8001_0000), V::U32(0), V::U32(0)]) };
    let mut s = subscription_object(o, rid, 2, refs);
    let mut r = Object::new(ref_rid, CLASS_ALARM_SUBSCRIPTION_REF);
    r.set_attr(ATTR_OBJECT_VARIABLE_TYPE_NAME, Value::wstring(""));
    r.set_attr(ATTR_SUB_REF_TRIGGER_MODE, Value::usint(3));
    r.set_attr(ATTR_ALARM_SUB_REF_DOMAIN, Value { flags: FLAG_ARRAY, typ: T_UINT, v: V::Array(vec![V::U16(0); 10]) });
    if ao.reference {
        s.set_attr(ATTR_SUB_DELAY_TIME, Value::udint(0));
        r.rid = 0x5101_0001;
        r.set_attr(ATTR_ALARM_SUB_REF_DOMAIN2, Value { flags: FLAG_ADDRESS_ARRAY, typ: T_UINT, v: V::Array(vec![V::U16(65535)]) });
        let langs = ao.languages.iter().map(|l| V::U32(*l)).collect();
        r.set_attr(ATTR_ALARM_SUB_REF_TEXT_LANGS, Value { flags: FLAG_ADDRESS_ARRAY, typ: T_UDINT, v: V::Array(langs) });
        r.set_attr(ATTR_ALARM_SUB_REF_SEND_TEXTS, Value::bool(ao.with_texts));
    }
    r.set_relation(ATTR_ALARM_SUB_REF_SUBSYSTEM, RID_ALARM_SUBSYSTEM);
    s.children.push(r);
    s
}

/// Most alarms per AckJob (TIA Portal sends at most 100 per job).
pub const ACK_JOB_MAX: usize = 100;

/// AllStatesInfo of an acknowledgement: acknowledge the coming state
/// (bit ACK_STATE_COMING), as TIA Portal does.
pub const ACK_COMING: u8 = 0x02;

/// The AckJob object (class 3636) for a list of CPU alarm ids: User, Host
/// and Application empty, AcknowledgementList = a dynamic array (flags 0x20)
/// of AS_AlarmAcknfyElem structs (AllStatesInfo, CPUAlarmID), as TIA Portal
/// sends it.
pub(crate) fn ack_job_object(rid: u32, ids: &[u64]) -> Object {
    let mut o = Object::new(rid, CLASS_ACK_JOB);
    o.set_attr(ATTR_ALARMING_JOB_USER, Value::wstring(""));
    o.set_attr(ATTR_ALARMING_JOB_HOST, Value::wstring(""));
    o.set_attr(ATTR_ALARMING_JOB_APPLICATION, Value::wstring(""));
    let list = ids
        .iter()
        .map(|id| {
            let mut s = Struct::new(STRUCT_ALARM_ACKNFY_ELEM);
            s.elems.push((ATTR_ACKNFY_ALL_STATES_INFO, Value::new(T_USINT, V::U8(ACK_COMING))));
            s.elems.push((ATTR_ACKNFY_CPU_ALARM_ID, Value::new(T_LWORD, V::U64(*id))));
            V::Struct(Box::new(s))
        })
        .collect();
    o.set_attr(ATTR_ACK_JOB_LIST, Value { flags: FLAG_ADDRESS_ARRAY, typ: T_STRUCT, v: V::Array(list) });
    o
}

/// The result of one alarm in an AckJob notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckResult {
    pub cpu_alarm_id: u64,
    /// 0 = acknowledged; PLCSIM answers 0x8005 (-32763) for an alarm whose
    /// class needs no acknowledgement.
    pub result: i16,
}

/// Decodes an AckJob object (class 3636) of an alarm notification: the
/// result per alarm. None for other objects.
pub fn decode_ack_job(o: &Object) -> Option<Vec<AckResult>> {
    if o.class != CLASS_ACK_JOB {
        return None;
    }
    let list = o.attr(ATTR_ACK_JOB_LIST).and_then(|v| v.v.as_array()).unwrap_or(&[]);
    Some(
        list.iter()
            .filter_map(|e| e.as_struct())
            .map(|s| AckResult {
                cpu_alarm_id: s.get(ATTR_ACKNFY_CPU_ALARM_ID).and_then(|v| v.v.as_u64()).unwrap_or(0),
                result: s.get(ATTR_ACKNFY_ACK_RESULT).and_then(|v| v.v.as_i16()).unwrap_or(-1),
            })
            .collect(),
    )
}

/// The texts of one alarm in one language.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AlarmText {
    /// 0 info text, 1 alarm text, 2..10 additional texts 1..9.
    pub texts: HashMap<u8, String>,
    pub unknown1: u16,
    pub unknown2: u16,
}

fn le32(b: &[u8], p: u32) -> Option<u32> {
    let p = p as usize;
    b.get(p..p + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap()))
}

fn le16(b: &[u8], p: u32) -> u16 {
    let p = p as usize;
    u16::from_le_bytes([b[p], b[p + 1]])
}

/// Decodes the text library of one language (object 0x8a360000 + LCID):
/// three offset tables (attribute 608, blob array) and the string area
/// (609). Tables are little endian: table 1 (from byte 16) maps a source RID
/// to table 2, table 2 maps an alarm id to table 3, table 3 lists (text
/// index, offset) into the string area (u16 length + UTF-8).
pub fn parse_text_library(t1: &[u8], t2: &[u8], t3: &[u8], strs: &[u8]) -> Result<HashMap<u64, AlarmText>> {
    let bad = || Error::proto("text library out of range");
    let mut out = HashMap::new();
    let n1 = le32(t1, 16).ok_or_else(bad)?;
    let mut p1 = 20u32;
    for _ in 0..n1 {
        let rel = le32(t1, p1);
        let off2 = le32(t1, p1 + 4);
        p1 += 8;
        let (Some(rel), Some(off2)) = (rel, off2) else { return Err(bad()) };
        let n2 = le32(t2, off2).ok_or_else(bad)?;
        let mut p2 = off2 + 4;
        for _ in 0..n2 {
            if p2 as usize + 6 > t2.len() {
                return Err(bad());
            }
            let alid = le16(t2, p2);
            let off3 = le32(t2, p2 + 2).ok_or_else(bad)?;
            p2 += 6;
            let id = (rel as u64) << 32 | (alid as u64) << 16;
            let mut at = AlarmText::default();
            let n3 = le32(t3, off3).ok_or_else(bad)?;
            let mut p3 = off3 + 4;
            for _ in 0..n3 {
                if p3 as usize + 5 > t3.len() {
                    return Err(bad());
                }
                let typ = t3[p3 as usize];
                let so = le32(t3, p3 + 1).ok_or_else(bad)?;
                p3 += 5;
                if so as usize + 4 > strs.len() {
                    return Err(bad());
                }
                if typ == 255 {
                    at.unknown1 = le16(strs, so);
                    at.unknown2 = le16(strs, so + 2);
                    continue;
                }
                let l = le16(strs, so) as usize;
                let s = so as usize + 2;
                if s + l > strs.len() {
                    return Err(bad());
                }
                at.texts.insert(typ, String::from_utf8_lossy(&strs[s..s + l]).into_owned());
            }
            out.insert(id, at);
        }
    }
    Ok(out)
}

impl Client {
    /// Explores the alarm subsystem (8) through itsUpdateRelevantDAI (2667)
    /// and returns the active DAIs. With `all`, the whole DAI pool of the
    /// subsystem (free DAIs have CPUAlarmID 0).
    pub fn active_alarms(&self, all: bool) -> Result<Vec<Alarm>> {
        let q = ExploreReq {
            object: RID_ALARM_SUBSYSTEM,
            request_id: if all { 0 } else { ATTR_SUBSYS_UPD_RELEVANT_DAI },
            recursive: true,
            unknown: 1,
            attrs: DAI_ATTRS.to_vec(),
            ..Default::default()
        };
        let objs = self.explore_objects(&q)?;
        let mut out = Vec::new();
        for o in &objs {
            o.walk(&mut |x| {
                if x.class == CLASS_DAI {
                    let a = decode_alarm(x);
                    if all || a.cpu_alarm_id != 0 {
                        out.push(a);
                    }
                }
            });
        }
        Ok(out)
    }

    /// Creates an alarm subscription (function class 2, route mode 2) with
    /// an AlarmSubscriptionRef child (class 2662) bound to the alarm
    /// subsystem (8). Notifications carry DAI objects
    /// (`Notification::alarm_objects`, decode with [`decode_alarm`]).
    pub fn subscribe_alarms(&self, ao: &AlarmSubOptions) -> Result<Subscription> {
        let o = SubOptions { route_mode: 2, credit_limit: if ao.reference { 10 } else { -1 }, ..Default::default() };
        let rid = self.next_sub_rid();
        let ref_rid = self.next_sub_rid();
        let s = alarm_subscription_object(&o, ao, rid, ref_rid);
        let ids = self.create_or_cleanup(&s)?;
        Ok(self.new_subscription(ids[0], rid, o.credit_limit, 5))
    }

    /// Acknowledges alarms (the coming state) by their CPU alarm id
    /// (`Alarm::cpu_alarm_id`): one AckJob (class 3636) per 100 alarms,
    /// created under the alarm subsystem (8), as TIA Portal does. The PLC
    /// reports the result through the alarm subscription (an AckJob object,
    /// then the DAI with the acknowledged state). This WRITES to the PLC.
    pub fn acknowledge_alarms(&self, ids: &[u64]) -> Result<()> {
        if ids.is_empty() {
            return Err(Error::proto("acknowledge: no alarm id"));
        }
        if let Some(bad) = ids.iter().find(|i| **i == 0) {
            return Err(Error::proto(format!("acknowledge: bad alarm id {bad:#x}")));
        }
        for chunk in ids.chunks(ACK_JOB_MAX) {
            let o = ack_job_object(self.next_sub_rid(), chunk);
            self.create_object(RID_ALARM_SUBSYSTEM, &o).map_err(|(_, e)| e)?;
        }
        Ok(())
    }

    /// Reads the text library of a language (e.g. 1033 en-US), keyed like
    /// `Alarm::cpu_alarm_id`.
    pub fn alarm_texts(&self, lcid: u16) -> Result<HashMap<u64, AlarmText>> {
        let objs = self.explore_objects(&ExploreReq { object: RID_TEXT_LIB_BASE + lcid as u32, unknown: 1, ..Default::default() })?;
        for o in &objs {
            let Some(lib) = o.find(&|x| x.class == CLASS_TEXT_LIBRARY) else { continue };
            let arr = lib.attr(ATTR_TEXT_LIB_OFFSET_AREA).and_then(|v| v.v.as_array()).unwrap_or(&[]);
            let strs = lib.attr(ATTR_TEXT_LIB_STRING_AREA).and_then(|v| v.v.as_bytes()).unwrap_or(&[]);
            if arr.len() < 3 {
                return Err(Error::proto(format!("text library offset area has {} blobs", arr.len())));
            }
            let b = |i: usize| arr[i].as_bytes().unwrap_or(&[]);
            return parse_text_library(b(0), b(1), b(2), strs);
        }
        Err(Error::proto(format!("no text library for language {lcid}")))
    }
}
