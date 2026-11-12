//! Subscriptions and notifications.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::ids::*;
use crate::msg::ItemAddress;
use crate::object::{decode_object, Object, ELEM_START_OBJECT};
use crate::textfmt::DateTime;
use crate::value::{Value, FLAG_ADDRESS_ARRAY, T_LINT, T_UDINT, V};
use crate::wire::Reader;

/// A decoded S7CommPlus notification (opcode 0x33).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notification {
    pub subscription_id: u32,
    pub unknown: [u16; 3],
    pub credit_tick: u8,
    pub seq: u32,
    /// Newer firmware only.
    pub change_counter: u8,
    /// Newer S7-1500 (change counter 0 + 8-byte µs timestamp).
    pub timestamp: Option<DateTime>,
    /// Values by reference id, in arrival order.
    pub values: Vec<(u32, Value)>,
    /// 0x03 / 0x13 addressing errors by reference id.
    pub item_errors: BTreeMap<u32, u8>,
    /// Second part (alarm subscriptions).
    pub alarm_objects: Vec<Object>,
    pub alarm_sub_id: u32,
}

impl Notification {
    /// The value of a reference id.
    pub fn value(&self, r: u32) -> Option<&Value> {
        self.values.iter().find(|(k, _)| *k == r).map(|(_, v)| v)
    }

    fn add(&mut self, r: u32, v: Value) {
        if let Some(e) = self.values.iter_mut().find(|(k, _)| *k == r) {
            e.1 = v;
        } else {
            self.values.push((r, v));
        }
    }
}

// Item return codes in a notification.
const NOTIF_END: u8 = 0x00;
const NOTIF_ERR03: u8 = 0x03;
const NOTIF_ERR13: u8 = 0x13;
const NOTIF_VALUE92: u8 = 0x92; // u32 ref + value (S7-1200)
const NOTIF_VALUE9B: u8 = 0x9b; // vlq ref + value
const NOTIF_STATUS9C: u8 = 0x9c; // u32, then unknown data (variable status table)

/// Decodes the body of an 0x33 message. The S7-1200 FW V3 sends no change
/// counter between the sequence number and the items; newer firmware sends
/// one byte (or 0 + an 8-byte timestamp). The layouts are tried in order and
/// the first one that parses cleanly is kept.
pub fn decode_notification(body: &[u8]) -> Result<Notification> {
    let mut first = None;
    for layout in 0..3 {
        match decode_notification_layout(body, layout) {
            Ok(n) => return Ok(n),
            Err(e) => {
                if first.is_none() {
                    first = Some(e);
                }
            }
        }
    }
    Err(first.unwrap())
}

pub(crate) fn decode_notification_layout(body: &[u8], layout: u8) -> Result<Notification> {
    let mut r = Reader::new(body);
    let mut n = Notification { subscription_id: r.u32(), ..Default::default() };
    for i in 0..3 {
        n.unknown[i] = r.u16();
    }
    n.credit_tick = r.byte();
    n.seq = r.vlq32();
    match layout {
        1 => {
            n.change_counter = r.byte();
            if n.change_counter == 0 {
                return Err(Error::proto("layout 1 needs a change counter"));
            }
        }
        2 => {
            if r.peek() != 0 {
                return Err(Error::proto("layout 2 needs a 0 byte"));
            }
            let us = r.u64();
            n.timestamp = Some(DateTime::from_unix_nanos((us as i64).wrapping_mul(1000)));
            n.change_counter = r.byte();
        }
        _ => {}
    }
    while r.ok() {
        let code = r.byte();
        if code == NOTIF_END {
            break;
        }
        match code {
            NOTIF_VALUE92 => {
                let rf = r.u32();
                let v = Value::decode(&mut r);
                n.add(rf, v);
            }
            NOTIF_VALUE9B => {
                let rf = r.vlq32();
                let v = Value::decode(&mut r);
                n.add(rf, v);
            }
            NOTIF_STATUS9C => {
                r.u32();
                return Err(Error::proto("notification item 0x9c (status table) not decoded"));
            }
            NOTIF_ERR03 | NOTIF_ERR13 => {
                let rf = r.u32();
                n.item_errors.insert(rf, code);
            }
            c => return Err(Error::proto(format!("unknown notification item code {c:#x}"))),
        }
    }
    r.result()?;
    // Optional second part: alarm objects.
    if r.left() >= 7 && r.peek() != 0 {
        n.alarm_sub_id = r.u32();
        r.u16();
        let rv = r.byte();
        if rv != 0x81 {
            return Err(Error::proto(format!("notification part 2 return {rv:#x}")));
        }
        while r.peek() == ELEM_START_OBJECT && r.ok() {
            if let Some(o) = decode_object(&mut r) {
                n.alarm_objects.push(o);
            }
        }
        r.result()?;
    }
    // What is left must be the 4 zero bytes of the trailer area.
    if r.rest().iter().any(|b| *b != 0) {
        return Err(Error::proto(format!("{} undecoded bytes in notification", r.left())));
    }
    Ok(n)
}

impl fmt::Display for Notification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "notification sub={:#x} tick={} seq={}", self.subscription_id, self.credit_tick, self.seq)?;
        if self.change_counter != 0 {
            write!(f, " cc={}", self.change_counter)?;
        }
        if let Some(t) = &self.timestamp {
            write!(f, " ts={}", t.rfc3339_nano())?;
        }
        for (r, v) in &self.values {
            write!(f, "\n  [{r}] {}", v.to_string().replace('\n', "\n  "))?;
        }
        for (r, e) in &self.item_errors {
            write!(f, "\n  [{r}] error {e:#x}")?;
        }
        for o in &self.alarm_objects {
            write!(f, "\n  {}", o.to_string().trim_end().replace('\n', "\n  "))?;
        }
        Ok(())
    }
}

/// Subscription options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubOptions {
    /// Cycle time (ms): the PLC checks the items each cycle and sends the
    /// changes. 0 = on change only (S7-1200 FW V3); the S7-1500 FW V4.x then
    /// sends only the first notification, so give it a cycle time (≥ 100 ms).
    pub cycle_ms: u32,
    /// -1 = unlimited; >0 = credits renewed by the client (default 10).
    pub credit_limit: i16,
    /// Added when the limit is close (default 5).
    pub credit_step: i16,
    /// Default 0x14 (reference); TIA uses 1/2/5.
    pub route_mode: u8,
    /// 1082 SubscriptionFunctionClassId (reference 0; TIA 1/3).
    pub fn_class: u8,
    /// High half of the item header: 0x8004 (reference) or 0x8014/0x8013 (TIA).
    pub item_head: u16,
    /// Send attribute 1055 = 0 like the reference (the FW V3 refuses it).
    pub attr1055: bool,
    pub name: String,
}

impl SubOptions {
    fn defaults(&mut self) {
        if self.credit_limit == 0 {
            self.credit_limit = 10;
        }
        if self.credit_step == 0 {
            self.credit_step = 5;
        }
        if self.route_mode == 0 {
            self.route_mode = 0x14;
        }
        if self.item_head == 0 {
            self.item_head = 0x8004;
        }
    }
}

/// A live subscription object on the PLC. It holds no reference to the
/// client: pass the client to [`Subscription::next`] and [`Subscription::close`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub object_id: u32,
    pub rid: u32,
    /// Reference id -> index of the address.
    pub refs: HashMap<u32, usize>,
    credit: i16,
    step: i16,
}

/// The attribute 1048 list: header 0x8<cc>0000, items to remove, items to
/// add, then per item 0x8004xxxx (xxxx = fields after the CRC), reference
/// id, 0, area, CRC, sub-area, LIDs.
pub fn subscription_ref_list(change_counter: u8, addrs: &[ItemAddress], first_ref: u32) -> Value {
    subscription_ref_list_head(change_counter, addrs, first_ref, 0x8004)
}

fn subscription_ref_list_head(change_counter: u8, addrs: &[ItemAddress], first_ref: u32, head: u16) -> Value {
    let mut a = vec![V::U32(0x8000_0000 | (change_counter as u32) << 16), V::U32(0), V::U32(addrs.len() as u32)];
    for (i, ad) in addrs.iter().enumerate() {
        a.push(V::U32((head as u32) << 16 | (1 + ad.lid.len() as u32)));
        a.push(V::U32(first_ref + i as u32));
        a.push(V::U32(0));
        a.push(V::U32(ad.area));
        a.push(V::U32(ad.symbol_crc));
        a.push(V::U32(ad.sub_area));
        a.extend(ad.lid.iter().map(|l| V::U32(*l)));
    }
    Value { flags: FLAG_ADDRESS_ARRAY, typ: T_UDINT, v: V::Array(a) }
}

pub(crate) fn subscription_object(o: &SubOptions, rid: u32, fn_class: u8, refs: Value) -> Object {
    let name = if o.name.is_empty() { format!("Subscription_{rid}") } else { o.name.clone() };
    let mut s = Object::new(rid, CLASS_SUBSCRIPTION);
    s.set_attr(ATTR_OBJECT_VARIABLE_TYPE_NAME, Value::wstring(name));
    s.set_attr(ATTR_SUB_FUNCTION_CLASS_ID, Value::usint(fn_class));
    s.set_attr(ATTR_SUB_MISSED_SENDINGS, Value::uint(0));
    s.set_attr(ATTR_SUB_SUBSYSTEM_ERROR, Value::new(T_LINT, V::I64(0)));
    s.set_attr(ATTR_SUB_ROUTE_MODE, Value::usint(o.route_mode));
    s.set_attr(ATTR_SUB_ACTIVE, Value::bool(true));
    s.set_attr(ATTR_SUB_REFERENCE_LIST, refs);
    s.set_attr(ATTR_SUB_CYCLE_TIME, Value::udint(o.cycle_ms));
    s.set_attr(ATTR_SUB_DISABLED, Value::usint(0));
    s.set_attr(ATTR_SUB_COUNT, Value::usint(0));
    s.set_attr(ATTR_SUB_CREDIT_LIMIT, Value::int(o.credit_limit));
    s.set_attr(ATTR_SUB_TICKS, Value::uint(65535));
    s
}

impl Client {
    /// Creates a subscription on the given addresses. The PLC then sends
    /// notifications with the values (all at creation, then changes). Item
    /// reference ids are 1..n in address order.
    pub fn subscribe(&self, addrs: &[ItemAddress], mut o: SubOptions) -> Result<Subscription> {
        o.defaults();
        let rid = self.next_sub_rid();
        let mut obj = subscription_object(&o, rid, o.fn_class, subscription_ref_list_head(1, addrs, 1, o.item_head));
        if o.attr1055 {
            obj.set_attr(1055, Value::usint(0));
        }
        let ids = self.create_or_cleanup(&obj)?;
        Ok(Subscription {
            object_id: ids[0],
            rid,
            refs: (0..addrs.len()).map(|i| (i as u32 + 1, i)).collect(),
            credit: o.credit_limit,
            step: o.credit_step,
        })
    }

    /// CreateObject below the session; deletes the object when the create
    /// fails but the PLC returned an id anyway.
    pub(crate) fn create_or_cleanup(&self, obj: &Object) -> Result<Vec<u32>> {
        match self.create_object(self.session2(), obj) {
            Ok(ids) => Ok(ids),
            Err((ids, e)) => {
                if let Some(id) = ids.first() {
                    let _ = self.delete_object(*id); // the object exists even when the create fails
                }
                Err(e)
            }
        }
    }

    pub(crate) fn new_subscription(&self, object_id: u32, rid: u32, credit: i16, step: i16) -> Subscription {
        Subscription { object_id, rid, refs: HashMap::new(), credit, step }
    }
}

impl Subscription {
    /// Waits for the next notification of this subscription and renews the
    /// credit limit one tick before it runs out. The client mutex is held
    /// while waiting: use short timeouts when other threads share the client.
    pub fn next(&mut self, c: &Client, timeout: Duration) -> Result<Notification> {
        let mut conn = c.conn_low_priority();
        let m = conn.wait_notification(timeout, self.object_id)?;
        let n = decode_notification(&m.body)?;
        if self.credit > 0 && n.credit_tick as i16 >= self.credit - 1 {
            self.credit = ((self.credit as i32 + self.step as i32) % 255) as i16;
            conn.set_variable_no_reply(self.object_id, ATTR_SUB_CREDIT_LIMIT, &Value::int(self.credit))?;
        }
        Ok(n)
    }

    /// Calls f for every notification until `stop` is set or an error other
    /// than a timeout happens. `poll` bounds how long the client mutex is
    /// held per wait.
    pub fn run(&mut self, c: &Client, poll: Duration, stop: &AtomicBool, mut f: impl FnMut(Notification)) -> Result<()> {
        while !stop.load(Ordering::Relaxed) {
            match self.next(c, poll) {
                Ok(n) => f(n),
                Err(e) if e.is_timeout() => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Deletes the subscription object.
    pub fn close(self, c: &Client) -> Result<()> {
        c.delete_object(self.object_id)
    }
}
