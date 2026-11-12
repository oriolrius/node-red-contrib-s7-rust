//! In-process fake S7-1200/1500 for tests of this crate and of the Node-API layer. Feature `fake`.
//!
//! Modelled on the S7-1200 FW V3 capture: InitSsl -> Error2 (or TLS 1.3 with
//! a self-signed certificate when enabled), session CreateObject, session
//! echo, byte images of I, Q, M and DB1, plus an object model: type
//! information and symbolic variables, SystemLimits, subscriptions with
//! notifications and credits, alarms, text library, comments, legitimation
//! and the operating state.
//!
//! ```no_run
//! let f = s7plus::fake::FakePlc::start().unwrap();
//! let c = s7plus::Client::connect(s7plus::Options::new(f.addr())).unwrap();
//! ```

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::client::lock;
use crate::explore::ExploreReq;
use crate::ids::*;
use crate::legit::{aes256_cbc_decrypt, legacy_response, sha1, sha256, ACCESS_FULL, LEGIT_NEW};
use crate::msg::ItemAddress;
use crate::object::{decode_object, Object};
use crate::objects::OP_STATE_STOP;
use crate::plcvalue::{bytes_of, encode_plc, time_to_dtl, Tag, DEFAULT_DTL_TIMESTAMP};
use crate::softdt::*;
use crate::stream::Reassembler;
use crate::tags::db_area;
use crate::transport::{tpkt, Link};
use crate::typeinfo::*;
use crate::value::*;
use crate::wire::{Reader, Writer};
use crate::{DateTime, Message};

/// Item error returned for bad addresses.
pub const ERR_ADDR: u64 = 0x8020_ab00_1bc8_fff4;
/// Explore refused by PLCSIM (FW S4.1) when a TLS request lacks the last 0 byte.
pub const ERR_EXPLORE_TRAILER: u64 = 0xa201_be00_017e_ff79;
/// Session CreateObject refused without TLS (PLCSIM Advanced V8, FW S4.1).
pub const ERR_SESSION_NEEDS_TLS: u64 = 0xa201_d600_0214_fdf8;

/// Type-info RIDs of the fake's data blocks.
pub const TI_DB1: u32 = 0x9100_0001;
pub const TI_DB2: u32 = 0x9100_0002;
pub const TI_UDT: u32 = 0x9100_0010;

/// The zlib preset dictionary used by the fake's comments.
pub const TEST_DICT: &[u8] = br#"<InterfaceLineComments><Part Kind="Comments"><Comment Path="1"><DictEntry Language="en-US">"#;

/// Mutable state of the fake PLC.
pub struct FakeState {
    /// Byte images: I, Q, M and DB1 (area RID -> bytes).
    pub mem: HashMap<u32, Vec<u8>>,
    /// The last ServerSessionVersion echo written by a client.
    pub echo: Option<Struct>,
    /// TLS server configuration (see [`FakePlc::enable_tls`]).
    pub tls: Option<Arc<rustls::ServerConfig>>,
    /// The PAOM string sent in ServerSessionVersion 319.
    pub paom: String,
    /// The open client connections (see [`FakePlc::drop_connections`]).
    conns: Vec<TcpStream>,
    /// Refuse a session without TLS, like PLCSIM Advanced V8 (FW S4.1).
    pub require_tls: bool,
    pub model: FakeModel,
}

/// The object model of the fake.
pub struct FakeModel {
    /// SystemLimits / FreeItems LID -> value.
    pub limits: HashMap<u32, i32>,
    pub types: HashMap<u32, Object>,
    pub dbs: Vec<Object>,
    /// DB rid -> type info rid.
    pub db_ti: HashMap<u32, u32>,
    /// Access string -> value.
    pub sym: HashMap<String, Value>,
    /// Largest Get/SetMultiVariables seen.
    pub max_items: usize,
    pub op_state: i32,
    pub op_reqs: Vec<i32>,
    pub protection: u32,
    pub password: String,
    pub user: String,
    pub challenge: Vec<u8>,
    pub legit_ok: bool,
    pub legit_kind: String,
    pub dais: Vec<Object>,
    pub textlib: Option<Object>,
    pub comments: HashMap<u32, Object>,
    subs: HashMap<u32, FakeSub>,
    pub next_obj: u32,
    pub credit_sets: Vec<i16>,
    pub explores: Vec<ExploreReq>,
    /// S7-1500 FW V4.x (PLCSIM S4.1): a subscription with cycle time 0 gets
    /// the first notification only, no changes.
    pub sub_needs_cycle: bool,
}

struct FakeSub {
    id: u32,
    /// ref -> "access string|sub-area".
    refs: HashMap<u32, String>,
    credit: i16,
    tick: u8,
    trigger: SyncSender<()>,
    /// Sends changes (false: only the first notification, see
    /// [`FakeModel::sub_needs_cycle`]).
    changes: bool,
}

/// A running fake PLC on 127.0.0.1 (stopped on drop).
pub struct FakePlc {
    addr: SocketAddr,
    state: Arc<Mutex<FakeState>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

type SharedLink = Arc<Mutex<Link>>;

impl FakePlc {
    /// Starts a fake on 127.0.0.1 with a free port.
    pub fn start() -> io::Result<FakePlc> {
        let ln = TcpListener::bind("127.0.0.1:0")?;
        let addr = ln.local_addr()?;
        let mut mem = HashMap::new();
        mem.insert(RID_I_AREA, vec![0u8; 1024]);
        mem.insert(RID_Q_AREA, vec![0u8; 1024]);
        mem.insert(RID_M_AREA, vec![0u8; 4096]);
        mem.insert(db_area(1), vec![0u8; 100]);
        let state = Arc::new(Mutex::new(FakeState {
            mem,
            echo: None,
            tls: None,
            paom: "1;6ES7 212-1BE31-0XB0 ;V3.0".into(),
            conns: Vec::new(),
            require_tls: false,
            model: FakeModel::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, sp) = (state.clone(), stop.clone());
        let accept = thread::Builder::new().name("s7plus-fake-accept".into()).spawn(move || {
            for c in ln.incoming() {
                if sp.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(c) = c else { continue };
                if let Ok(k) = c.try_clone() {
                    lock(&st).conns.push(k);
                }
                let (st, sp) = (st.clone(), sp.clone());
                let _ = thread::Builder::new().name("s7plus-fake-conn".into()).spawn(move || serve(st, sp, c));
            }
        })?;
        Ok(FakePlc { addr, state, stop, accept: Some(accept) })
    }

    /// "127.0.0.1:port".
    pub fn addr(&self) -> String {
        self.addr.to_string()
    }

    /// Locks the state (paom, memory, model…).
    pub fn lock(&self) -> MutexGuard<'_, FakeState> {
        lock(&self.state)
    }

    /// Answers InitSsl and runs TLS 1.3 with a fresh self-signed certificate.
    pub fn enable_tls(&self) -> io::Result<()> {
        let cfg = self_signed().map_err(io::Error::other)?;
        self.lock().tls = Some(cfg);
        Ok(())
    }

    /// Number of live subscriptions.
    pub fn subscriptions(&self) -> usize {
        self.lock().model.subs.len()
    }

    /// Closes every client connection, like a PLC restart: the sessions and
    /// their subscriptions are gone.
    pub fn drop_connections(&self) {
        let mut st = self.lock();
        for c in st.conns.drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        st.model.subs.clear(); // dropping the senders stops the pumps
    }
}

impl Drop for FakePlc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        lock(&self.state).model.subs.clear();
    }
}

fn self_signed() -> Result<Arc<rustls::ServerConfig>, String> {
    let ck = rcgen::generate_simple_self_signed(vec!["fake-plc".to_string()]).map_err(|e| e.to_string())?;
    let cert = ck.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.signing_key.serialize_der().into());
    let cfg = rustls::ServerConfig::builder_with_provider(crate::tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(cfg))
}

fn serve(state: Arc<Mutex<FakeState>>, stop: Arc<AtomicBool>, tcp: TcpStream) {
    let _ = tcp.set_nodelay(true);
    let mut l = Link::new(tcp);
    l.set_deadline(Some(Instant::now() + Duration::from_secs(5)));
    match l.read_raw() {
        Ok(cr) if cr.len() > 1 && cr[1] == 0xe0 => {}
        _ => return,
    }
    if l.write_raw(&tpkt(&[0x06, 0xd0, 0, 1, 0, 0x2b, 0])).is_err() {
        return;
    }
    let link: SharedLink = Arc::new(Mutex::new(l));
    let f = Fake { state: state.clone(), link: link.clone() };
    let mut re = Reassembler::new();
    while !stop.load(Ordering::Relaxed) {
        let tp = {
            let mut l = lock(&link);
            l.set_deadline(Some(Instant::now() + Duration::from_millis(20)));
            match l.read_tpdu() {
                Ok(tp) => tp,
                Err(e) if e.is_timeout() => {
                    drop(l);
                    // let the pumps take the link (std mutexes are not fair)
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(_) => break,
            }
        };
        let Ok(pdus) = re.push(&tp) else { break };
        for p in pdus {
            let Ok(m) = Message::parse(p.version, &p.data) else { return f.stop_subs() };
            if !f.handle(p.version, &m) {
                return f.stop_subs();
            }
        }
    }
    f.stop_subs();
    lock(&link).shutdown();
}

struct Fake {
    state: Arc<Mutex<FakeState>>,
    link: SharedLink,
}

fn send_pdu(link: &SharedLink, ver: u8, d: &[u8]) -> bool {
    let mut l = lock(link);
    l.set_deadline(Some(Instant::now() + Duration::from_secs(5)));
    let mut d = d;
    let mut first = true;
    while first || !d.is_empty() {
        first = false;
        let n = d.len().min(900);
        let mut p = vec![0x72, ver, (n >> 8) as u8, n as u8];
        p.extend_from_slice(&d[..n]);
        d = &d[n..];
        if d.is_empty() {
            p.extend_from_slice(&[0x72, ver, 0, 0]);
        }
        if l.write_fragment(&p).is_err() {
            return false;
        }
    }
    true
}

impl Fake {
    fn st(&self) -> MutexGuard<'_, FakeState> {
        lock(&self.state)
    }

    fn stop_subs(&self) {
        self.st().model.subs.clear(); // dropping the senders stops the pumps
    }

    fn reply_fn(&self, ver: u8, m: &Message, func: u16, body: &[u8]) -> bool {
        let mut w = Writer::new();
        w.byte(OP_RESPONSE);
        w.u16(0);
        w.u16(func);
        w.u16(0);
        w.u16(m.seq);
        if func != FN_INIT_SSL {
            w.byte(m.tflags);
        }
        w.raw(body);
        send_pdu(&self.link, ver, &w.b)
    }

    fn reply(&self, ver: u8, m: &Message, body: &[u8]) -> bool {
        self.reply_fn(ver, m, m.function, body)
    }

    /// Returns false when the connection must end.
    fn handle(&self, ver: u8, m: &Message) -> bool {
        let mut r = Reader::new(&m.body);
        let mut w = Writer::new();
        match m.function {
            FN_INIT_SSL => {
                let cfg = self.st().tls.clone();
                if let Some(cfg) = cfg {
                    self.reply(ver, m, &[0, 0, 0, 0, 0]);
                    let Ok(sc) = rustls::ServerConnection::new(cfg) else { return false };
                    let mut l = lock(&self.link);
                    l.set_deadline(Some(Instant::now() + Duration::from_secs(5)));
                    // Like PLCSIM: a client may skip TLS after InitSsl and go on in plain text.
                    match l.next_is_tls() {
                        Ok(true) => return l.start_tls(rustls::Connection::Server(sc)).is_ok(),
                        Ok(false) => return true,
                        Err(_) => return false,
                    }
                }
                // The FW V3 PLC answers with function Error2 and a negative return.
                w.vlq64(0xa201_9a00_0145_ff89);
                w.u32(0);
                self.reply_fn(ver, m, FN_ERROR2, &w.b)
            }
            FN_CREATE_OBJECT => {
                let req_id = r.u32();
                Value::decode(&mut r);
                r.u32();
                let obj = decode_object(&mut r);
                if req_id == OBJ_SERVER_SESS_CONT {
                    if self.st().require_tls && !lock(&self.link).is_tls() {
                        // PLCSIM Advanced V8 (FW S4.1): negative return, no ids
                        w.vlq64(ERR_SESSION_NEEDS_TLS);
                        w.byte(0);
                        w.u32(0);
                        return self.reply(ver, m, &w.b);
                    }
                    w.vlq64(0);
                    w.byte(2);
                    w.vlq32(0x39c);
                    w.vlq32(0x395);
                    let mut st = Struct::new(314);
                    st.set(315, Value::udint(256));
                    st.set(319, Value::wstring(self.st().paom.clone()));
                    st.set(320, Value::wstring("2;544"));
                    let mut o = Object::new(OBJ_NULL_SERVER_SESSION, CLASS_SERVER_SESSION);
                    o.set_attr(233, Value::wstring(""));
                    o.set_attr(ATTR_SESS_VERSION, Value::structv(st));
                    o.encode(&mut w);
                    w.u32(0);
                    return self.reply(ver, m, &w.b);
                }
                self.create_subscription(ver, m, obj)
            }
            FN_SET_MULTI_VARIABLES => self.set_multi(ver, m, &mut r),
            FN_GET_MULTI_VARIABLES => self.get_multi(ver, m, &mut r),
            FN_DELETE_OBJECT => {
                let id = r.u32();
                self.st().model.subs.remove(&id);
                w.vlq64(0);
                w.u32(0);
                self.reply(ver, m, &w.b)
            }
            FN_EXPLORE => self.explore(ver, m, &mut r),
            FN_GET_VAR_SUB_STREAMED => self.get_var_sub(ver, m, &mut r),
            FN_SET_VARIABLE => self.set_variable(ver, m, &mut r),
            _ => true,
        }
    }

    fn set_multi(&self, ver: u8, m: &Message, r: &mut Reader) -> bool {
        let obj = r.u32();
        let n = r.vlq32() as usize;
        r.vlq32();
        let mut addrs = vec![ItemAddress::default(); n.min(10_000)];
        for a in addrs.iter_mut() {
            if obj != 0 {
                r.vlq32();
            } else {
                *a = ItemAddress::decode(r);
            }
        }
        {
            let mut st = self.st();
            st.model.max_items = st.model.max_items.max(n);
        }
        let mut w = Writer::new();
        w.vlq64(0);
        let mut changed = Vec::new();
        for _ in 0..addrs.len() {
            let idx = r.vlq32() as usize;
            let v = Value::decode(r);
            if obj != 0 {
                self.st().echo = v.v.as_struct().cloned();
                continue;
            }
            let ok = match addrs.get(idx.wrapping_sub(1)) {
                Some(a) if a.lid.len() == 3 && a.lid[0] == 3 => self.write_area(a, &v),
                Some(a) => self.write_sym(a, &v),
                None => false,
            };
            if ok {
                changed.push(addrs[idx - 1].access_string());
            } else {
                w.vlq32(idx as u32);
                w.vlq64(ERR_ADDR);
            }
        }
        w.byte(0);
        w.u32(0);
        let ok = self.reply(ver, m, &w.b);
        self.notify_changed(&changed);
        ok
    }

    fn get_multi(&self, ver: u8, m: &Message, r: &mut Reader) -> bool {
        let obj = r.u32();
        let n = r.vlq32() as usize;
        r.vlq32();
        {
            let mut st = self.st();
            st.model.max_items = st.model.max_items.max(n);
        }
        let mut w = Writer::new();
        w.vlq64(0);
        let mut bad = Vec::new();
        for i in 1..=n.min(10_000) {
            if obj != 0 {
                r.vlq32();
                bad.push(i);
                continue;
            }
            let a = ItemAddress::decode(r);
            match self.read_any(&a) {
                Some(v) => {
                    w.vlq32(i as u32);
                    v.encode(&mut w);
                }
                None => bad.push(i),
            }
        }
        w.byte(0);
        for i in bad {
            w.vlq32(i as u32);
            w.vlq64(ERR_ADDR);
        }
        w.byte(0);
        w.u32(0);
        self.reply(ver, m, &w.b)
    }

    fn write_area(&self, a: &ItemAddress, v: &Value) -> bool {
        let mut st = self.st();
        let Some(b) = span_mut(&mut st.mem, a) else { return false };
        match v.v.as_bytes() {
            Some(d) if d.len() == b.len() => {
                b.copy_from_slice(d);
                true
            }
            _ => false,
        }
    }

    /// Serves area images, SystemLimits and symbolic values.
    fn read_any(&self, a: &ItemAddress) -> Option<Value> {
        let mut st = self.st();
        if a.lid.len() == 3 && a.lid[0] == 3 {
            return span_mut(&mut st.mem, a).map(|b| Value::blob(b.to_vec()));
        }
        let md = &st.model;
        if a.area == RID_ROOT && a.lid.len() == 1 {
            if a.sub_area == ATTR_SYSTEM_LIMITS || a.sub_area == ATTR_FREE_ITEMS {
                return md.limits.get(&a.lid[0]).map(|v| Value::dint(*v));
            }
            return None;
        }
        if let Some(ti) = md.db_ti.get(&a.area) {
            if a.lid == [1] {
                return Some(Value::rid(*ti));
            }
        }
        md.get_sym(&a.access_string())
    }

    fn write_sym(&self, a: &ItemAddress, v: &Value) -> bool {
        let mut st = self.st();
        let md = &mut st.model;
        let key = a.access_string();
        if let Some(old) = md.sym.get_mut(&key) {
            if old.typ != v.typ || old.flags != v.flags {
                return false;
            }
            *old = v.clone();
            return true;
        }
        let Some((parent, idx)) = split_last(&key) else { return false };
        let Some(arr) = md.sym.get_mut(&parent) else { return false };
        if arr.typ != v.typ {
            return false;
        }
        match &mut arr.v {
            V::Array(el) if (idx as usize) < el.len() => {
                el[idx as usize] = v.v.clone();
                true
            }
            _ => false,
        }
    }

    fn explore(&self, ver: u8, m: &Message, r: &mut Reader) -> bool {
        let mut q = ExploreReq {
            object: r.u32(),
            request_id: r.vlq32(),
            recursive: r.byte() == 1,
            unknown: r.byte(),
            parents: r.byte() == 1,
            ..Default::default()
        };
        if r.peek() == 1 {
            r.byte();
            q.filter = Value::decode(r).v.as_struct().cloned();
        }
        r.byte();
        let n = r.vlq32();
        for _ in 0..n.min(10_000) {
            q.attrs.push(r.vlq32());
        }
        let tls = lock(&self.link).is_tls();
        if tls {
            // Like PLCSIM (FW S4.1): with an integrity id the request ends with
            // the id, a 4-byte fill and one more 0 byte; without that byte the
            // PLC refuses the Explore.
            r.vlq32();
            r.u32();
            if r.left() < 1 || r.byte() != 0 {
                let mut w = Writer::new();
                w.vlq64(ERR_EXPLORE_TRAILER);
                w.u32(0);
                return self.reply(ver, m, &w.b);
            }
        }
        let objs = {
            let mut st = self.st();
            let md = &mut st.model;
            md.explores.push(q.clone());
            if q.parents && q.object >= 0x9000_0000 {
                // like the S7-1200 FW V3: type info explored with parents = nothing
                Some(Vec::new())
            } else if q.object == RID_PLC_PROGRAM {
                let mut prog = Object::new(RID_PLC_PROGRAM, CLASS_PLC_PROGRAM);
                prog.children = md.dbs.clone();
                Some(vec![prog])
            } else if q.object == OBJ_OMS_TYPE_INFO_CONTAINER {
                let mut cont = Object::new(OBJ_OMS_TYPE_INFO_CONTAINER, CLASS_OMS_TYPE_INFO_CONTAINER);
                for rid in [RID_I_TYPE_INFO, RID_Q_TYPE_INFO, RID_M_TYPE_INFO] {
                    if let Some(t) = md.types.get(&rid) {
                        cont.children.push(t.clone());
                    }
                }
                Some(vec![cont])
            } else if let Some(t) = md.types.get(&q.object) {
                Some(vec![t.clone()])
            } else if q.object == RID_ALARM_SUBSYSTEM && q.request_id == ATTR_SUBSYS_UPD_RELEVANT_DAI {
                let mut sys = Object::new(RID_ALARM_SUBSYSTEM, 2669);
                sys.children = md.dais.clone();
                Some(vec![sys])
            } else if q.object == RID_TEXT_LIB_BASE + 1033 && md.textlib.is_some() {
                Some(vec![md.textlib.clone().unwrap()])
            } else {
                md.comments.get(&q.object).map(|c| vec![c.clone()])
            }
        };
        let mut w = Writer::new();
        match objs {
            None => {
                w.vlq64(ERR_ADDR);
                w.u32(0);
            }
            Some(objs) => {
                w.vlq64(0);
                w.u32(q.object);
                if tls {
                    w.vlq32(0); // integrity id of the response
                }
                for o in &objs {
                    o.encode(&mut w);
                }
                w.u32(0);
            }
        }
        self.reply(ver, m, &w.b)
    }

    fn get_var_sub(&self, ver: u8, m: &Message, r: &mut Reader) -> bool {
        let obj = r.u32();
        let a = Value::decode(r);
        let attr = a.v.as_array().and_then(|x| x.first()).and_then(|x| x.as_u32()).unwrap_or(0);
        let v = {
            let mut st = self.st();
            let md = &mut st.model;
            if obj == 0x39c && attr == ATTR_EFF_PROT_LEVEL {
                Some(Value::udint(md.protection))
            } else if obj == 0x39c && attr == ATTR_SESS_REQUEST {
                let mut ch = vec![0u8; 20];
                let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut ch);
                md.challenge = ch.clone();
                Some(Value::usint_array(&ch))
            } else if obj == RID_CPU_EXEC_UNIT && attr == 2237 {
                let mut s = Struct::new(3481);
                s.set(3482, Value::dint(0));
                s.set(3486, Value::dint(md.op_state));
                Some(Value::structv(s))
            } else {
                None
            }
        };
        let mut w = Writer::new();
        match v {
            Some(v) => {
                w.vlq64(0);
                w.byte(0);
                v.encode(&mut w);
            }
            None => {
                w.vlq64(0xa033_8800_0048_ffe0);
                w.byte(0);
                Value::default().encode(&mut w);
            }
        }
        w.u32(0);
        self.reply(ver, m, &w.b)
    }

    fn set_variable(&self, ver: u8, m: &Message, r: &mut Reader) -> bool {
        let obj = r.u32();
        r.vlq32();
        let attr = r.vlq32();
        let v = Value::decode(r);
        let mut rv = 0u64;
        {
            let mut st = self.st();
            let md = &mut st.model;
            if obj == RID_CPU_EXEC_UNIT && attr == ATTR_OPERATING_STATE_REQ {
                let s = v.v.as_i32().unwrap_or(0);
                md.op_reqs.push(s);
                md.op_state = if s == OP_STATE_STOP { 1 } else { 8 };
            } else if obj == 0x39c && attr == ATTR_SESS_RESPONSE {
                let b = bytes_of(&v).unwrap_or_default();
                let want = legacy_response(&md.password, &md.challenge).unwrap_or_default();
                md.legit_kind = "legacy".into();
                if b == want {
                    md.legit_ok = true;
                    md.protection = ACCESS_FULL;
                } else {
                    rv = 0x8000_0000_0000_0000 | 0xd1;
                }
            } else if obj == 0x39c && attr == ATTR_LEGITIMATE {
                md.legit_kind = "new".into();
                let b = v.v.as_bytes().unwrap_or(&[]).to_vec();
                let sec = lock(&self.link).export_keying_material(b"EXPERIMENTAL_OMS", 32);
                if check_new_legit(md, sec, &b) {
                    md.legit_ok = true;
                    md.protection = ACCESS_FULL;
                } else {
                    rv = 0x8000_0000_0000_0000 | 0xd1;
                }
            } else if attr == ATTR_SUB_CREDIT_LIMIT {
                let lim = v.v.as_i16().unwrap_or(0);
                md.credit_sets.push(lim);
                if let Some(s) = md.subs.get_mut(&obj) {
                    s.credit = lim;
                    let _ = s.trigger.try_send(());
                }
            }
        }
        if m.tflags == 0x74 {
            return true; // no response wanted
        }
        let mut w = Writer::new();
        w.vlq64(rv);
        w.u32(0);
        self.reply(ver, m, &w.b)
    }

    /// Answers a CreateObject of class 1001: parses the reference list (1048)
    /// and starts a pump that sends notifications while the credit limit
    /// allows it (tick < limit), like the PLC.
    fn create_subscription(&self, ver: u8, m: &Message, obj: Option<Object>) -> bool {
        let mut w = Writer::new();
        let Some(obj) = obj.filter(|o| o.class == CLASS_SUBSCRIPTION) else {
            w.vlq64(ERR_ADDR);
            w.byte(0);
            w.u32(0);
            return self.reply(ver, m, &w.b);
        };
        let (id, rx) = {
            let mut st = self.st();
            let md = &mut st.model;
            md.next_obj += 1;
            let id = 0x1000_0000 + md.next_obj;
            if obj.attr(1055).is_some() {
                // refused by the S7-1200 FW V3
                drop(st);
                w.vlq64(0xa201_8800_0d8e_ffe0);
                w.byte(1);
                w.vlq32(id);
                w.u32(0);
                return self.reply(ver, m, &w.b);
            }
            let (tx, rx) = sync_channel(1);
            let cycle = obj.attr(ATTR_SUB_CYCLE_TIME).and_then(|v| v.v.as_u32()).unwrap_or(0);
            let changes = !(md.sub_needs_cycle && cycle == 0);
            let mut s = FakeSub { id, refs: HashMap::new(), credit: -1, tick: 0, trigger: tx, changes };
            if let Some(c) = obj.attr(ATTR_SUB_CREDIT_LIMIT).and_then(|v| v.v.as_i16()) {
                s.credit = c;
            }
            if let Some(l) = obj.attr(ATTR_SUB_REFERENCE_LIST).and_then(|v| v.v.as_array()) {
                let g = |i: usize| l.get(i).and_then(|x| x.as_u32()).unwrap_or(0);
                let mut i = 3;
                while i + 5 < l.len() {
                    let nf = (g(i) & 0xffff) as usize;
                    let rf = g(i + 1);
                    let mut a = ItemAddress { area: g(i + 3), symbol_crc: g(i + 4), sub_area: g(i + 5), lid: Vec::new() };
                    for k in 1..nf {
                        a.lid.push(g(i + 5 + k));
                    }
                    s.refs.insert(rf, format!("{}|{}", a.access_string(), a.sub_area));
                    i += 5 + nf.max(1);
                }
            }
            let _ = s.trigger.try_send(());
            md.subs.insert(id, s);
            (id, rx)
        };
        w.vlq64(0);
        let mut ids = vec![id];
        if !obj.children.is_empty() {
            ids.push(id + 1);
        }
        w.byte(ids.len() as u8);
        for x in &ids {
            w.vlq32(*x);
        }
        w.u32(0);
        let ok = self.reply(ver, m, &w.b);
        let (state, link) = (self.state.clone(), self.link.clone());
        let _ = thread::Builder::new().name("s7plus-fake-pump".into()).spawn(move || pump(state, link, ver, id, rx));
        ok
    }

    /// Triggers the subscriptions that contain a changed item.
    fn notify_changed(&self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let st = self.st();
        for s in st.model.subs.values() {
            let hit = s.refs.values().any(|r| keys.iter().any(|k| r.split('|').next() == Some(k.as_str())));
            if hit && s.changes {
                let _ = s.trigger.try_send(());
            }
        }
    }
}

/// Sends a notification with all values on every trigger, and an empty one
/// every 10 ms, while the credit allows it.
fn pump(state: Arc<Mutex<FakeState>>, link: SharedLink, ver: u8, id: u32, rx: Receiver<()>) {
    loop {
        let full = match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let body = {
            let mut st = lock(&state);
            let FakeState { mem, model, .. } = &mut *st;
            let Some(s) = model.subs.get_mut(&id) else { return };
            if s.credit >= 0 && s.tick as i16 >= s.credit {
                continue;
            }
            s.tick = s.tick.wrapping_add(1);
            let mut w = Writer::new();
            w.byte(OP_NOTIFICATION);
            w.u32(s.id);
            w.u16(0x0400);
            w.u16(0);
            w.u16(0);
            w.byte(if s.credit < 0 { 0 } else { s.tick });
            w.vlq32(1);
            if full {
                let refs = s.refs.clone();
                for rf in 1..=refs.len() as u32 {
                    let v = refs.get(&rf).and_then(|k| sub_value(mem, &model.sym, k));
                    match v {
                        None => {
                            w.byte(0x13);
                            w.u32(rf);
                        }
                        Some(v) => {
                            w.byte(0x92);
                            w.u32(rf);
                            v.encode(&mut w);
                        }
                    }
                }
            }
            w.byte(0);
            w.u32(0);
            w.b
        };
        if !send_pdu(&link, ver, &body) {
            return;
        }
    }
}

fn span_mut<'a>(mem: &'a mut HashMap<u32, Vec<u8>>, a: &ItemAddress) -> Option<&'a mut [u8]> {
    let m = mem.get_mut(&a.area)?;
    if a.lid.len() != 3 || a.lid[0] != 3 {
        return None;
    }
    let (off, n) = (a.lid[1] as usize, a.lid[2] as usize);
    if off + n > m.len() {
        return None;
    }
    Some(&mut m[off..off + n])
}

/// Reads a subscribed item.
fn sub_value(mem: &mut HashMap<u32, Vec<u8>>, sym: &HashMap<String, Value>, key: &str) -> Option<Value> {
    let acc = key.split('|').next().unwrap_or("");
    let a = ItemAddress::parse_access_string(acc).ok()?;
    if a.lid.len() == 3 && a.lid[0] == 3 {
        return span_mut(mem, &a).map(|b| Value::blob(b.to_vec()));
    }
    get_sym(sym, acc)
}

fn split_last(key: &str) -> Option<(String, u32)> {
    let mut a = ItemAddress::parse_access_string(key).ok()?;
    if a.lid.len() < 2 {
        return None;
    }
    let last = a.lid.pop().unwrap();
    Some((a.access_string(), last))
}

/// A stored value, or an element of a stored array.
fn get_sym(sym: &HashMap<String, Value>, key: &str) -> Option<Value> {
    if let Some(v) = sym.get(key) {
        return Some(v.clone());
    }
    let (parent, idx) = split_last(key)?;
    let v = sym.get(&parent)?;
    if v.flags & FLAG_ARRAY == 0 {
        return None;
    }
    let e = v.v.as_array()?.get(idx as usize)?;
    Some(Value::new(v.typ, e.clone()))
}

/// Decrypts the AES-256-CBC payload with the TLS exporter secret (key =
/// SHA256(secret), IV = challenge[:16]) and checks it.
fn check_new_legit(md: &FakeModel, sec: Option<Vec<u8>>, ct: &[u8]) -> bool {
    let Some(sec) = sec else { return false };
    if ct.is_empty() || !ct.len().is_multiple_of(16) || md.challenge.len() < 16 {
        return false;
    }
    let key = sha256(&sec);
    let mut pt = aes256_cbc_decrypt(&key, &md.challenge[..16], ct);
    let pad = *pt.last().unwrap() as usize;
    if pad == 0 || pad > pt.len() {
        return false;
    }
    pt.truncate(pt.len() - pad);
    let mut r = Reader::new(&pt);
    let v = Value::decode(&mut r);
    let Some(s) = v.v.as_struct().filter(|s| s.id == ATTR_LEGIT_PAYLOAD_STRUCT) else { return false };
    let typ = s.get(ATTR_LEGIT_PAYLOAD_TYPE).and_then(|v| v.v.as_u32()).unwrap_or(0);
    let user = s.get(ATTR_LEGIT_PAYLOAD_USER).and_then(|v| v.v.as_bytes()).unwrap_or(&[]);
    let pw = s.get(ATTR_LEGIT_PAYLOAD_PASSWORD).and_then(|v| v.v.as_bytes()).unwrap_or(&[]);
    if typ == LEGIT_NEW {
        return user == md.user.as_bytes() && pw == md.password.as_bytes();
    }
    pw == sha1(md.password.as_bytes())
}

// ---- the model ----

/// A variable of a fake type-info object.
#[derive(Clone)]
struct Vt {
    name: &'static str,
    lid: u32,
    sdt: u8,
    oi: OffsetInfo,
    bit: u8,
}

fn vt(name: &'static str, lid: u32, sdt: u8, oi: OffsetInfo, bit: u8) -> Vt {
    Vt { name, lid, sdt, oi, bit }
}

fn std_oi(opt: u32, non: u32) -> OffsetInfo {
    OffsetInfo { kind: OI_STD, opt_addr: opt, non_opt_addr: non, ..Default::default() }
}

fn type_obj(rid: u32, optimized: bool, size: u32, vars: &[Vt]) -> Object {
    let mut o = Object::new(rid, CLASS_TYPE_INFO);
    o.set_attr(ATTR_OBJECT_VARIABLE_TYPE_NAME, Value::wstring("ti"));
    o.set_attr(ATTR_TI_TCOM_SIZE, Value::udint(size));
    let mut vl = VartypeList { first_id: vars[0].lid, elems: Vec::new() };
    for v in vars {
        let mut e = Vartype {
            lid: v.lid,
            symbol_crc: 0x1000 + v.lid,
            softdatatype: v.sdt,
            offset: v.oi,
            bit_info: v.bit,
            flags: (v.oi.kind as u16) << 12,
        };
        if optimized {
            e.flags |= VT_OPTIMIZED;
        }
        vl.elems.push(e);
        o.varnames.push(v.name.to_string());
    }
    o.vartypes = Some(vl);
    o
}

fn db1_vars() -> Vec<Vt> {
    let oi = |kind: u8| OffsetInfo { kind, ..Default::default() };
    vec![
        vt("Int1", 0xA, SDT_INT, std_oi(0, 0), 0),
        vt("Real1", 0xB, SDT_REAL, std_oi(4, 4), 0),
        vt("Flag", 0xC, SDT_BOOL, std_oi(8, 8), 0),
        vt("Str", 0xD, SDT_STRING, OffsetInfo { info1: 10, info2: 12, opt_addr: 10, non_opt_addr: 10, ..oi(OI_STRING) }, 0),
        vt("WStr", 0xE, SDT_WSTRING, OffsetInfo { info1: 8, info2: 10, opt_addr: 22, non_opt_addr: 22, ..oi(OI_STRING) }, 0),
        vt("Arr", 0xF, SDT_INT, OffsetInfo { opt_addr: 42, non_opt_addr: 42, lower_bound: 1, count: 5, ..oi(OI_ARRAY_1DIM) }, 0),
        vt(
            "Mat",
            0x10,
            SDT_DINT,
            OffsetInfo { opt_addr: 52, non_opt_addr: 52, count: 6, m_count: [3, 2, 0, 0, 0, 0], ..oi(OI_ARRAY_MDIM) },
            0,
        ),
        vt("S", 0x11, SDT_STRUCT, OffsetInfo { opt_addr: 76, non_opt_addr: 76, relation_id: TI_UDT, ..oi(OI_STRUCT) }, 0),
        vt(
            "SA",
            0x12,
            SDT_STRUCT,
            OffsetInfo {
                opt_addr: 82,
                non_opt_addr: 82,
                count: 2,
                non_opt_size: 6,
                opt_size: 6,
                relation_id: TI_UDT,
                ..oi(OI_STRUCT_1DIM)
            },
            0,
        ),
        vt("Stamp", 0x13, SDT_DTL, OffsetInfo { opt_addr: 94, non_opt_addr: 94, relation_id: TI_DTL, ..oi(OI_STRUCT) }, 0),
        vt("D", 0x14, SDT_DATE, std_oi(106, 106), 0),
        vt("Tod", 0x15, SDT_TIME_OF_DAY, std_oi(108, 108), 0),
        vt("T", 0x16, SDT_TIME, std_oi(112, 112), 0),
        vt("S5", 0x17, SDT_S5TIME, std_oi(116, 116), 0),
        vt("LT", 0x18, SDT_LTIME, std_oi(118, 118), 0),
        vt("LTod", 0x19, SDT_LTOD, std_oi(126, 126), 0),
        vt("Ldt", 0x1A, SDT_LDT, std_oi(134, 134), 0),
        vt("Dt", 0x1B, SDT_DATE_AND_TIME, std_oi(142, 142), 0),
        vt("Ch", 0x1C, SDT_CHAR, std_oi(150, 150), 0),
        vt("WCh", 0x1D, SDT_WCHAR, std_oi(152, 152), 0),
        vt("LR", 0x1E, SDT_LREAL, std_oi(154, 154), 0),
        vt("UL", 0x1F, SDT_ULINT, std_oi(162, 162), 0),
        vt("LI", 0x20, SDT_LINT, std_oi(170, 170), 0),
        vt("LW", 0x21, SDT_LWORD, std_oi(178, 178), 0),
        vt("US", 0x22, SDT_USINT, std_oi(186, 186), 0),
        vt("UI", 0x23, SDT_UINT, std_oi(188, 188), 0),
        vt("UD", 0x24, SDT_UDINT, std_oi(190, 190), 0),
        vt("SI", 0x25, SDT_SINT, std_oi(194, 194), 0),
        vt("B", 0x26, SDT_BYTE, std_oi(195, 195), 0),
        vt("W", 0x27, SDT_WORD, std_oi(196, 196), 0),
        vt("DW", 0x28, SDT_DWORD, std_oi(198, 198), 0),
        vt("DI", 0x29, SDT_DINT, std_oi(202, 202), 0),
        vt("Bools", 0x2A, SDT_BOOL, OffsetInfo { opt_addr: 206, non_opt_addr: 206, count: 10, ..oi(OI_ARRAY_1DIM) }, 0),
    ]
}

/// Number of top-level variables of the fake's DB1.
pub fn db1_var_count() -> usize {
    db1_vars().len()
}

fn udt_vars() -> Vec<Vt> {
    vec![vt("x", 1, SDT_INT, std_oi(0, 0), 0), vt("y", 2, SDT_REAL, std_oi(2, 2), 0)]
}

/// db2 is a standard (non-optimized) DB: Bool at 2.3.
fn db2_vars() -> Vec<Vt> {
    vec![
        vt("A", 0xA, SDT_INT, OffsetInfo { kind: OI_STD, non_opt_addr: 0, ..Default::default() }, 0),
        vt("Bit", 0xB, SDT_BOOL, OffsetInfo { kind: OI_STD, non_opt_addr: 2, ..Default::default() }, BOI_CLASSIC | 3 << 4),
    ]
}

fn zero_for(sdt: u8, max_len: u16) -> Value {
    match sdt {
        SDT_STRING => {
            let mut b = vec![0u8; max_len as usize + 2];
            b[0] = max_len as u8;
            return Value::usint_array(&b);
        }
        SDT_WSTRING => {
            let mut a = vec![V::U16(0); max_len as usize + 2];
            a[0] = V::U16(max_len);
            return Value { flags: FLAG_ARRAY, typ: T_UINT, v: V::Array(a) };
        }
        SDT_DATE_AND_TIME => return Value::usint_array(&[0x90, 1, 1, 0, 0, 0, 0, 2]),
        SDT_DTL => {
            return Value::structv(Struct {
                id: TI_DTL,
                packed_ts: DEFAULT_DTL_TIMESTAMP,
                packed_tf: 2,
                packed: Some(time_to_dtl(&DateTime::default())),
                elems: Vec::new(),
            })
        }
        _ => {}
    }
    let s = match sdt {
        SDT_BOOL => "false",
        SDT_CHAR | SDT_WCHAR => " ",
        SDT_DATE => "1990-01-01",
        SDT_TIME_OF_DAY | SDT_LTOD | SDT_S5TIME => "0",
        SDT_LDT => "1970-01-01",
        _ => "0",
    };
    encode_plc(&Tag::of_type(sdt), s).unwrap_or_else(|e| panic!("{}: {e}", softdatatype_name(sdt)))
}

impl FakeModel {
    /// The default model: DB1 "Data_block_1" (optimized, every type), DB2
    /// "Std_DB" (standard), DB3 "LoadOnly", M tags Motor_On/Speed, Q tag Lamp,
    /// one active alarm, a text library (1033) and DB1 comments.
    pub fn new() -> FakeModel {
        let mut md = FakeModel {
            limits: HashMap::from([(1000, 50), (1001, 50), (0, 33), (1, 165), (2, 0)]),
            types: HashMap::new(),
            dbs: Vec::new(),
            db_ti: HashMap::from([(db_area(1), TI_DB1), (db_area(2), TI_DB2), (db_area(3), 0)]),
            sym: HashMap::new(),
            max_items: 0,
            op_state: 8,
            op_reqs: Vec::new(),
            protection: 0,
            password: String::new(),
            user: String::new(),
            challenge: Vec::new(),
            legit_ok: false,
            legit_kind: String::new(),
            dais: Vec::new(),
            textlib: None,
            comments: HashMap::new(),
            subs: HashMap::new(),
            next_obj: 0,
            credit_sets: Vec::new(),
            explores: Vec::new(),
            sub_needs_cycle: false,
        };
        md.types.insert(TI_DB1, type_obj(TI_DB1, true, 220, &db1_vars()));
        md.types.insert(TI_DB2, type_obj(TI_DB2, false, 4, &db2_vars()));
        md.types.insert(TI_UDT, type_obj(TI_UDT, true, 6, &udt_vars()));
        md.types.insert(
            RID_M_TYPE_INFO,
            type_obj(
                RID_M_TYPE_INFO,
                false,
                1,
                &[
                    vt(
                        "Motor_On",
                        0xA,
                        SDT_BOOL,
                        OffsetInfo { kind: OI_STD, non_opt_addr: 10, ..Default::default() },
                        BOI_CLASSIC | 2 << 4,
                    ),
                    vt("Speed", 0xB, SDT_INT, OffsetInfo { kind: OI_STD, non_opt_addr: 12, ..Default::default() }, 0),
                ],
            ),
        );
        md.types.insert(
            RID_Q_TYPE_INFO,
            type_obj(
                RID_Q_TYPE_INFO,
                false,
                1,
                &[vt("Lamp", 0xA, SDT_BOOL, OffsetInfo { kind: OI_STD, ..Default::default() }, BOI_CLASSIC)],
            ),
        );
        md.types.insert(RID_I_TYPE_INFO, Object::new(RID_I_TYPE_INFO, CLASS_TYPE_INFO)); // empty table
        for (i, n) in ["Data_block_1", "Std_DB", "LoadOnly"].iter().enumerate() {
            let mut db = Object::new(db_area(i as u16 + 1), CLASS_DB);
            db.set_attr(ATTR_OBJECT_VARIABLE_TYPE_NAME, Value::wstring(*n));
            md.dbs.push(db);
        }
        let dbb = |n: u16| ItemAddress { area: db_area(n), sub_area: ATTR_DB_VALUE_ACTUAL, ..Default::default() };
        let ctl = |a: u32| ItemAddress { area: a, sub_area: ATTR_CTRL_AREA_VALUE_ACT, ..Default::default() };
        md.fill(&dbb(1), &db1_vars());
        md.fill(&dbb(2), &db2_vars());
        md.fill(
            &ctl(RID_M_AREA),
            &[vt("Motor_On", 0xA, SDT_BOOL, OffsetInfo::default(), 0), vt("Speed", 0xB, SDT_INT, OffsetInfo::default(), 0)],
        );
        md.fill(&ctl(RID_Q_AREA), &[vt("Lamp", 0xA, SDT_BOOL, OffsetInfo::default(), 0)]);

        // one active alarm and one free DAI
        for (i, id) in [(db_area(1) as u64) << 32 | 1 << 16, 0u64].into_iter().enumerate() {
            let mut d = Object::new(0x1000_0006 + i as u32, CLASS_DAI);
            d.set_attr(ATTR_OBJECT_VARIABLE_TYPE_NAME, Value::wstring(format!("DAI_{i}")));
            d.set_attr(ATTR_DAI_CPU_ALARM_ID, Value::new(T_LWORD, V::U64(id)));
            d.set_attr(ATTR_DAI_ALL_STATES_INFO, Value::usint(1));
            d.set_attr(ATTR_DAI_ALARM_DOMAIN, Value::uint(1));
            d.set_attr(ATTR_DAI_MESSAGE_TYPE, Value::dint(1));
            let mut cgs = Struct::new(3473);
            cgs.set(ATTR_AS_CGS_ALL_STATES_INFO, Value::usint(1));
            cgs.set(ATTR_AS_CGS_TIMESTAMP, Value::timestamp(1_700_000_000_000_000_000));
            cgs.set(
                ATTR_AS_CGS_ASSOC_VALUES,
                Value { flags: FLAG_ARRAY, typ: T_BLOB, v: V::Array(vec![V::Bytes(vec![1, 2]), V::Bytes(vec![])]) },
            );
            cgs.set(ATTR_AS_CGS_ACK_TIMESTAMP, Value::timestamp(0));
            d.set_attr(ATTR_DAI_COMING, Value::structv(cgs));
            d.set_attr(ATTR_DAI_HMI_INFO, Value::blob(vec![1, 2, 0, 1, 0, 0, 0, 7, 5, 0, 0, 0, 0, 3, 1, 2, 0]));
            d.set_attr(ATTR_DAI_SEQUENCE_COUNTER, Value::udint(4));
            let mut texts = std::collections::BTreeMap::new();
            texts.insert(1033 << 16 | 2, V::Bytes(b"Motor overload".to_vec()));
            d.set_attr(ATTR_DAI_ALARM_TEXTS, Value { flags: FLAG_SPARSE_ARRAY, typ: T_BLOB, v: V::Sparse(texts) });
            md.dais.push(d);
        }

        // text library (1033): source DB1, alarm 1, alarm text + info text
        let mut t1 = vec![0u8; 28];
        t1[16..20].copy_from_slice(&1u32.to_le_bytes());
        t1[20..24].copy_from_slice(&db_area(1).to_le_bytes());
        let mut t2 = vec![0u8; 10];
        t2[0..4].copy_from_slice(&1u32.to_le_bytes());
        t2[4..6].copy_from_slice(&1u16.to_le_bytes());
        let mut strs: Vec<u8> = Vec::new();
        let add_str = |s: &str, strs: &mut Vec<u8>| {
            let off = strs.len() as u32;
            strs.extend_from_slice(&(s.len() as u16).to_le_bytes());
            strs.extend_from_slice(s.as_bytes());
            off
        };
        let o1 = add_str("Motor overload", &mut strs);
        let o2 = add_str("Check the motor", &mut strs);
        let o3 = strs.len() as u32;
        strs.extend_from_slice(&7u16.to_le_bytes());
        strs.extend_from_slice(&9u16.to_le_bytes());
        let mut t3 = 3u32.to_le_bytes().to_vec();
        for (typ, off) in [(1u8, o1), (0, o2), (255, o3)] {
            t3.push(typ);
            t3.extend_from_slice(&off.to_le_bytes());
        }
        let mut lib = Object::new(RID_TEXT_LIB_BASE + 1033, CLASS_TEXT_LIBRARY);
        lib.set_attr(
            ATTR_TEXT_LIB_OFFSET_AREA,
            Value { flags: FLAG_ARRAY, typ: T_BLOB, v: V::Array(vec![V::Bytes(t1), V::Bytes(t2), V::Bytes(t3)]) },
        );
        lib.set_attr(ATTR_TEXT_LIB_STRING_AREA, Value::blob(strs));
        md.textlib = Some(lib);

        // comments of DB1: object comment + line comments (zlib with a dictionary)
        let xml = br#"<InterfaceLineComments><Part Kind="Comments"><Comment Path="1"><DictEntry Language="en-US">Speed setpoint</DictEntry></Comment></Part></InterfaceLineComments>"#;
        let mut blob = vec![0x90, 0, 0, 1];
        blob.extend_from_slice(&crate::blob::compress_with_dictionary(xml, TEST_DICT));
        let mut cm = Object::new(db_area(1), CLASS_DB);
        let mut oc = std::collections::BTreeMap::new();
        oc.insert(1033, V::Str("Test DB".into()));
        cm.set_attr(ATTR_AS_OBJECT_ES_COMMENT, Value { flags: FLAG_SPARSE_ARRAY, typ: T_WSTRING, v: V::Sparse(oc) });
        let mut lc = std::collections::BTreeMap::new();
        lc.insert(1, V::Bytes(blob));
        cm.set_attr(ATTR_DATA_IF_LINE_COMMENTS, Value { flags: FLAG_SPARSE_ARRAY, typ: T_BLOB, v: V::Sparse(lc) });
        md.comments.insert(db_area(1), cm);
        md
    }

    /// Vartype flags as S7-1500 FW V4.x (PLCSIM S4.1) sends them: the
    /// optimized-access flag on the elements of every DB, and the classic
    /// bit on every element of a standard DB.
    pub fn use_fw4_flags(&mut self) {
        for rid in [TI_DB1, TI_DB2] {
            let Some(vt) = self.types.get_mut(&rid).and_then(|o| o.vartypes.as_mut()) else { continue };
            let standard = !vt.elems.iter().any(|e| e.optimized());
            for e in &mut vt.elems {
                e.flags |= VT_OPTIMIZED;
                if standard {
                    e.bit_info |= BOI_CLASSIC;
                }
            }
        }
    }

    fn fill(&mut self, base: &ItemAddress, vars: &[Vt]) {
        for v in vars {
            let a = base.child(v.lid);
            if v.oi.kind == OI_STRUCT_1DIM {
                for i in 0..v.oi.count {
                    self.fill(&a.child(i).child(1), &udt_vars());
                }
            } else if v.oi.kind == OI_STRUCT && v.sdt != SDT_DTL {
                self.fill(&a, &udt_vars());
            } else if v.oi.is_1dim() || v.oi.is_mdim() {
                let z = zero_for(v.sdt, 0);
                let el = vec![z.v.clone(); v.oi.count as usize];
                self.sym.insert(a.access_string(), Value { flags: FLAG_ARRAY, typ: z.typ, v: V::Array(el) });
            } else {
                self.sym.insert(a.access_string(), zero_for(v.sdt, v.oi.info1));
            }
        }
    }

    /// A stored value, or an element of a stored array.
    pub fn get_sym(&self, key: &str) -> Option<Value> {
        get_sym(&self.sym, key)
    }
}

impl Default for FakeModel {
    fn default() -> Self {
        FakeModel::new()
    }
}
