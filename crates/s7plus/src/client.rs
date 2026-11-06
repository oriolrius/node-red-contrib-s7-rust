//! Session and request/response engine.

use std::collections::{HashSet, VecDeque};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::blob::Dictionaries;
use crate::browse::TypeCache;
use crate::error::{Error, Result};
use crate::ids::*;
use crate::limits::Limits;
use crate::log::{log_with, Level, Logger};
use crate::msg::{encode_object_qualifier, ItemAddress, Message};
use crate::object::{decode_object, Object};
use crate::stream::Reassembler;
use crate::textfmt::hex;
use crate::tls::{client_config, server_name, TlsOptions};
use crate::transport::{cotp_cr, tpkt, Link, MAX_TPDU};
use crate::value::{Struct, Value, FLAG_ADDRESS_ARRAY, T_STRUCT, T_UDINT, V};
use crate::wire::{Reader, Writer};

/// Callback that sees every message (">" sent, "<" received).
pub type TraceFn = Arc<dyn Fn(&str, &Message) + Send + Sync>;

/// How the ServerSessionVersion struct is echoed to the PLC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SessionEcho {
    /// Like TIA Portal V21: 319 empty, 320 = PAOM string without blanks.
    #[default]
    Tia,
    /// Sent back unchanged.
    Raw,
}

/// Connection options.
#[derive(Clone)]
pub struct Options {
    /// host:port (port 102).
    pub addr: String,
    /// Remote TSAP string: "SIMATIC-ROOT-HMI" (default) or "SIMATIC-ROOT-ES".
    pub tsap: String,
    /// Per request (default 5 s); also the connect timeout.
    pub timeout: Duration,
    /// Attribute 233 of the session object (default "node-red-s7rs").
    pub client_name: String,
    /// Adds the integrity id to requests (V3 / TLS sessions; set by TLS).
    /// The S7-1200 FW V3 uses V2 without it.
    pub integrity: bool,
    pub session_echo: SessionEcho,
    /// Skips reading SystemLimits after the session setup (20 items per
    /// request are then used).
    pub no_limits: bool,
    /// Runs InitSsl and then TLS 1.3 inside COTP (S7-1500 >= V2.9,
    /// S7-1200 >= V4.5). Not available on the S7-1200 FW V3.
    pub tls: Option<TlsOptions>,
    /// Legitimation after the session setup (empty = none). A username
    /// selects the new (user management) login.
    pub password: String,
    pub username: String,
    /// Optional log sink (Trace = frame dumps in hex).
    pub logger: Option<Arc<dyn Logger>>,
    /// Optional message callback.
    pub trace: Option<TraceFn>,
    /// zlib dictionaries for comments (default: a new empty registry).
    pub dictionaries: Option<Arc<Dictionaries>>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            addr: String::new(),
            tsap: "SIMATIC-ROOT-HMI".into(),
            timeout: Duration::from_secs(5),
            client_name: "node-red-s7rs".into(),
            integrity: false,
            session_echo: SessionEcho::Tia,
            no_limits: false,
            tls: None,
            password: String::new(),
            username: String::new(),
            logger: None,
            trace: None,
            dictionaries: None,
        }
    }
}

impl Options {
    /// Default options for an address ("192.168.0.1:102" or "192.168.0.1").
    pub fn new(addr: impl Into<String>) -> Self {
        Options { addr: addr.into(), ..Default::default() }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("addr", &self.addr)
            .field("tsap", &self.tsap)
            .field("timeout", &self.timeout)
            .field("client_name", &self.client_name)
            .field("integrity", &self.integrity)
            .field("session_echo", &self.session_echo)
            .field("no_limits", &self.no_limits)
            .field("tls", &self.tls)
            .field("password", &(!self.password.is_empty()))
            .field("username", &self.username)
            .finish()
    }
}

/// Session data set up by [`Client::connect`].
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub session: u32,
    pub session2: u32,
    /// Attribute 306 as sent by the PLC.
    pub server_session: Struct,
    pub protocol_ver: u8,
    pub init_ssl_refused: bool,
    pub tls: bool,
}

impl SessionInfo {
    /// The PAOM string (order number and firmware), from 319 or 320.
    pub fn paom(&self) -> String {
        for id in [319, 320] {
            if let Some(s) = self.server_session.get(id).and_then(|v| v.v.as_str()) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        String::new()
    }
}

const MAX_QUEUED_NOTIFICATIONS: usize = 10_000;

/// The connection state behind the client mutex.
pub(crate) struct Conn {
    link: Link,
    timeout: Duration,
    pub(crate) integrity: bool,
    logger: Option<Arc<dyn Logger>>,
    trace: Option<TraceFn>,
    seq: u16,
    integ_set: u32,
    integ_get: u32,
    re: Reassembler,
    pub(crate) session: u32,
    pub(crate) protocol_ver: u8,
    notifications: VecDeque<Message>,
    no_reply: HashSet<u16>,
    pub(crate) oms_secret: Option<Vec<u8>>,
    closed: bool,
}

/// An S7CommPlus session. All methods take `&self`: the client can be shared
/// between threads (requests are serialized by an internal mutex).
pub struct Client {
    conn: Mutex<Conn>,
    info: SessionInfo,
    limits: Limits,
    read_max: AtomicUsize,
    write_max: AtomicUsize,
    sub_rid: AtomicU32,
    /// Request callers waiting for the mutex (they go before notification waits).
    waiting: AtomicUsize,
    pub(crate) types: Mutex<TypeCache>,
    dicts: Arc<Dictionaries>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn connect_tcp(addr: &str, timeout: Duration) -> Result<TcpStream> {
    use std::net::{IpAddr, SocketAddr};
    let full = if addr.parse::<SocketAddr>().is_ok() {
        addr.to_string()
    } else if let Ok(ip) = addr.parse::<IpAddr>() {
        SocketAddr::new(ip, 102).to_string()
    } else if addr.rsplit_once(':').is_some_and(|(h, p)| !h.contains(':') && p.parse::<u16>().is_ok()) {
        addr.to_string()
    } else {
        format!("{addr}:102")
    };
    let mut last = None;
    for sa in full.to_socket_addrs()? {
        match TcpStream::connect_timeout(&sa, timeout) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) => e.into(),
        None => Error::proto(format!("cannot resolve {addr}")),
    })
}

impl Client {
    /// Opens the TCP connection, the COTP connection and the session,
    /// reads SystemLimits and logs in when a password is given.
    pub fn connect(mut o: Options) -> Result<Client> {
        if o.tsap.is_empty() {
            o.tsap = "SIMATIC-ROOT-HMI".into();
        }
        if o.timeout.is_zero() {
            o.timeout = Duration::from_secs(5);
        }
        if o.client_name.is_empty() {
            o.client_name = "node-red-s7rs".into();
        }
        let tcp = connect_tcp(&o.addr, o.timeout)?;
        log_with(&o.logger, Level::Debug, || format!("s7plus: connected to {}", o.addr));
        let mut conn = Conn {
            link: Link::new(tcp),
            timeout: o.timeout,
            integrity: o.integrity,
            logger: o.logger.clone(),
            trace: o.trace.clone(),
            seq: 0,
            integ_set: 0,
            integ_get: 0,
            re: Reassembler::new(),
            session: 0,
            protocol_ver: 0,
            notifications: VecDeque::new(),
            no_reply: HashSet::new(),
            oms_secret: None,
            closed: false,
        };
        let info = match conn.setup(&o) {
            Ok(i) => i,
            Err(e) => {
                conn.link.shutdown();
                return Err(e);
            }
        };
        let mut limits = Limits::default();
        let (mut read_max, mut write_max) = (20usize, 20usize);
        if !o.no_limits {
            match conn.read_limits() {
                Ok(l) => {
                    if l.tags_per_read_max > 0 {
                        read_max = l.tags_per_read_max as usize;
                    }
                    if l.tags_per_write_max > 0 {
                        write_max = l.tags_per_write_max as usize;
                    }
                    limits = l;
                }
                Err(e) => log_with(&o.logger, Level::Warn, || format!("s7plus: SystemLimits: {e}")),
            }
        }
        let c = Client {
            conn: Mutex::new(conn),
            info,
            limits,
            read_max: AtomicUsize::new(read_max),
            write_max: AtomicUsize::new(write_max),
            sub_rid: AtomicU32::new(0),
            waiting: AtomicUsize::new(0),
            types: Mutex::new(TypeCache::default()),
            dicts: o.dictionaries.clone().unwrap_or_default(),
        };
        if !o.password.is_empty() {
            if let Err(e) = c.legitimate(&o.password, &o.username) {
                let _ = c.close();
                return Err(e);
            }
        }
        Ok(c)
    }

    /// Session data (ids, ServerSessionVersion, TLS).
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// The session object id.
    pub fn session(&self) -> u32 {
        self.info.session
    }

    /// The second id of the session (subscription container).
    pub fn session2(&self) -> u32 {
        self.info.session2
    }

    /// True when the PLC answered InitSsl with an error (no TLS support).
    pub fn init_ssl_refused(&self) -> bool {
        self.info.init_ssl_refused
    }

    /// SystemLimits / FreeItems read after the session setup.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The zlib dictionaries used for comments.
    pub fn dictionaries(&self) -> &Arc<Dictionaries> {
        &self.dicts
    }

    /// Overrides the maximum number of items per read/write request (0 = keep).
    pub fn set_chunk_sizes(&self, read: usize, write: usize) {
        if read > 0 {
            self.read_max.store(read, Ordering::Relaxed);
        }
        if write > 0 {
            self.write_max.store(write, Ordering::Relaxed);
        }
    }

    /// Current (read, write) chunk sizes.
    pub fn chunk_sizes(&self) -> (usize, usize) {
        (self.read_max.load(Ordering::Relaxed), self.write_max.load(Ordering::Relaxed))
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Conn> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let g = lock(&self.conn);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        g
    }

    /// Lock for notification waits: requests from other threads go first
    /// (std mutexes are not fair, a polling loop would starve them).
    pub(crate) fn conn_low_priority(&self) -> MutexGuard<'_, Conn> {
        let t0 = Instant::now();
        while self.waiting.load(Ordering::SeqCst) > 0 && t0.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_micros(200));
        }
        lock(&self.conn)
    }

    pub(crate) fn next_sub_rid(&self) -> u32 {
        0x7fff_c000 + self.sub_rid.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Writes attributes of an object (SetMultiVariables with an object id).
    pub fn set_attributes(&self, object: u32, attrs: &[u32], vals: &[Value]) -> Result<Vec<u64>> {
        self.conn().set_attributes(object, attrs, vals)
    }

    /// Reads attributes of an object (GetMultiVariables with an object id).
    pub fn get_attributes(&self, object: u32, attrs: &[u32]) -> Result<(Vec<Value>, Vec<u64>)> {
        self.conn().get_attributes(object, attrs)
    }

    /// Reads variables by address (GetMultiVariables, link 0). Errors per
    /// item are returned in the second vector (0 = ok). Requests are split by
    /// the PLC limit (SystemLimits 1000, 50 on the S7-1200 FW V3).
    pub fn read(&self, addrs: &[ItemAddress]) -> Result<(Vec<Value>, Vec<u64>)> {
        let max = self.read_max.load(Ordering::Relaxed).max(1);
        let mut vals = Vec::with_capacity(addrs.len());
        let mut errs = Vec::with_capacity(addrs.len());
        let mut start = 0;
        loop {
            let end = (start + max).min(addrs.len());
            let (v, e) = self.conn().read_chunk(&addrs[start..end])?;
            vals.extend(v);
            errs.extend(e);
            if end == addrs.len() {
                break;
            }
            start += max;
        }
        Ok((vals, errs))
    }

    /// Writes variables by address (SetMultiVariables, object 0), split by
    /// the PLC limit (SystemLimits 1001).
    pub fn write(&self, addrs: &[ItemAddress], vals: &[Value]) -> Result<Vec<u64>> {
        if addrs.len() != vals.len() {
            return Err(Error::proto("addresses and values differ in length"));
        }
        let max = self.write_max.load(Ordering::Relaxed).max(1);
        let mut errs = Vec::with_capacity(addrs.len());
        let mut start = 0;
        loop {
            let end = (start + max).min(addrs.len());
            errs.extend(self.conn().write_chunk(&addrs[start..end], &vals[start..end])?);
            if end == addrs.len() {
                break;
            }
            start += max;
        }
        Ok(errs)
    }

    /// Reads one attribute of an object.
    pub fn get_var_sub_streamed(&self, object: u32, attr: u32) -> Result<Value> {
        self.conn().get_var_sub_streamed(object, attr)
    }

    /// Writes one attribute of an object.
    pub fn set_variable(&self, object: u32, attr: u32, v: &Value) -> Result<()> {
        self.conn().set_variable(object, attr, v)
    }

    /// Returns the next notification (queued or from the wire) of any
    /// subscription.
    pub fn wait_notification(&self, timeout: Duration) -> Result<Message> {
        self.conn_low_priority().wait_notification(timeout, 0)
    }

    /// Deletes the session object and closes the connection (idempotent;
    /// also done on drop).
    pub fn close(&self) -> Result<()> {
        let mut c = self.conn();
        if c.closed {
            return Ok(());
        }
        c.closed = true;
        let mut res = Ok(());
        if c.session != 0 {
            let mut w = Writer::new();
            w.u32(c.session);
            w.byte(0);
            encode_object_qualifier(&mut w);
            c.integrity_id(&mut w, true);
            w.u32(0);
            let ver = c.protocol_ver;
            if let Err(e) = c.call(ver, FN_DELETE_OBJECT, 0x34, w.b) {
                res = Err(e);
            }
        }
        c.link.shutdown();
        res
    }

    /// The TLS exporter secret ("EXPERIMENTAL_OMS", 32 bytes) used by the
    /// new legitimation; None without TLS.
    pub fn oms_secret(&self) -> Option<Vec<u8>> {
        self.conn().oms_secret.clone()
    }
}

impl Drop for Client {
    /// Best-effort close: the DeleteObject of the session waits at most 1 s.
    fn drop(&mut self) {
        {
            let mut c = self.conn();
            c.timeout = c.timeout.min(Duration::from_secs(1));
        }
        let _ = self.close();
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("info", &self.info).field("limits", &self.limits).finish()
    }
}

/// tia_echo copies the struct like TIA Portal V21: 319 empty, 320 = PAOM
/// string without blanks.
pub(crate) fn tia_echo(st: &Struct) -> Struct {
    let mut out = Struct::new(st.id);
    let paom = st.get(319).and_then(|v| v.v.as_str()).unwrap_or("");
    let clean = paom.split(';').map(|p| p.trim()).collect::<Vec<_>>().join(";");
    for (id, v) in &st.elems {
        let v = match id {
            319 => Value::wstring(""),
            320 => Value::wstring(clean.clone()),
            _ => v.clone(),
        };
        out.set(*id, v);
    }
    out
}

pub(crate) fn return_value(m: &Message, r: &mut Reader) -> Result<()> {
    let rv = r.vlq64();
    if rv & 0x4000_0000_0000_0000 != 0 || (rv as i64) < 0 {
        return Err(Error::Return { function: m.function, ret: rv });
    }
    r.result()
}

pub(crate) fn item_errors(m: &Message, n: usize) -> Result<Vec<u64>> {
    let mut r = Reader::new(&m.body);
    return_value(m, &mut r)?;
    let mut errs = vec![0u64; n];
    loop {
        let i = r.vlq32();
        if i == 0 || !r.ok() {
            break;
        }
        let e = r.vlq64();
        if (i as usize) <= n {
            errs[i as usize - 1] = e;
        }
    }
    r.result()?;
    Ok(errs)
}

pub(crate) fn decode_values(m: &Message, n: usize) -> Result<(Vec<Value>, Vec<u64>)> {
    let mut r = Reader::new(&m.body);
    return_value(m, &mut r)?;
    let mut vals = vec![Value::default(); n];
    let mut errs = vec![0u64; n];
    loop {
        let i = r.vlq32();
        if i == 0 || !r.ok() {
            break;
        }
        let v = Value::decode(&mut r);
        if (i as usize) <= n {
            vals[i as usize - 1] = v;
        }
    }
    loop {
        let i = r.vlq32();
        if i == 0 || !r.ok() {
            break;
        }
        let e = r.vlq64();
        if (i as usize) <= n {
            errs[i as usize - 1] = e;
        }
    }
    r.result()?;
    Ok((vals, errs))
}

pub(crate) fn notif_sub(m: &Message) -> u32 {
    if m.body.len() < 4 {
        return 0;
    }
    u32::from_be_bytes([m.body[0], m.body[1], m.body[2], m.body[3]])
}

impl Conn {
    fn setup(&mut self, o: &Options) -> Result<SessionInfo> {
        self.link.set_deadline(Some(Instant::now() + self.timeout));
        self.link.write_raw(&tpkt(&cotp_cr(&o.tsap)))?;
        let cc = self.link.read_raw().map_err(|e| Error::proto(format!("COTP connect: {e}")))?;
        if cc.len() < 2 || cc[1] != 0xd0 {
            return Err(Error::proto(format!("COTP connect refused ({})", hex(&cc))));
        }

        // 1. InitSsl (V1). The S7-1200 FW V3 has no TLS and answers Error2.
        let m = self.call(1, FN_INIT_SSL, 0x30, vec![0, 0, 0, 0]).map_err(|e| Error::proto(format!("InitSsl: {e}")))?;
        let init_ssl_refused = m.function != FN_INIT_SSL;
        if let Some(t) = &o.tls {
            if init_ssl_refused {
                return Err(Error::proto("the PLC refused InitSsl (no TLS support)"));
            }
            self.start_tls(t, &o.addr)?;
        }

        // 2. CreateObject: the server session.
        let mut w = Writer::new();
        w.u32(OBJ_SERVER_SESS_CONT);
        Value::udint(0).encode(&mut w);
        w.u32(0);
        let mut sess = Object::new(RID_GET_NEW_ON_SERVER, CLASS_SERVER_SESSION);
        sess.set_attr(233, Value::wstring(o.client_name.clone()));
        sess.set_attr(ATTR_SESS_CLIENT_RID, Value::rid(0x80c3_c901));
        let mut sub = Object::new(RID_GET_NEW_ON_SERVER, CLASS_SUBSCRIPTIONS);
        sub.set_attr(233, Value::wstring("SubscriptionContainer"));
        sess.children.push(sub);
        sess.encode(&mut w);
        w.u32(0);
        let m = self.call(1, FN_CREATE_OBJECT, 0x36, w.b).map_err(|e| Error::proto(format!("CreateObject session: {e}")))?;
        let mut r = Reader::new(&m.body);
        let rv = r.vlq64();
        if rv & 0x4000_0000_0000_0000 != 0 {
            return Err(Error::Return { function: FN_CREATE_OBJECT, ret: rv });
        }
        let n = r.byte();
        let ids: Vec<u32> = (0..n).map(|_| r.vlq32()).collect();
        if ids.is_empty() {
            return Err(Error::proto("CreateObject returned no session id"));
        }
        self.session = ids[0];
        let session2 = ids.get(1).copied().unwrap_or(0);
        let obj = decode_object(&mut r);
        let obj = match (r.err.take(), obj) {
            (None, Some(o)) => o,
            (e, _) => {
                return Err(Error::proto(format!(
                    "CreateObject response: {}",
                    e.map(|e| e.to_string()).unwrap_or_else(|| "no object".into())
                )))
            }
        };
        let st = match obj.attr(ATTR_SESS_VERSION).map(|v| &v.v) {
            Some(V::Struct(s)) => (**s).clone(),
            _ => return Err(Error::proto("no ServerSessionVersion in the response")),
        };

        // 3. SetMultiVariables (V2): echo ServerSessionVersion to the session.
        let echo = match o.session_echo {
            SessionEcho::Tia => tia_echo(&st),
            SessionEcho::Raw => st.clone(),
        };
        self.protocol_ver = 2;
        let session = self.session;
        let errs = self
            .set_attributes(session, &[ATTR_SESS_VERSION], &[Value::new(T_STRUCT, V::Struct(Box::new(echo)))])
            .map_err(|e| Error::proto(format!("session setup: {e}")))?;
        if errs[0] != 0 {
            return Err(Error::proto(format!("session setup refused: {:#x}", errs[0])));
        }
        log_with(&self.logger, Level::Info, || {
            format!("s7plus: session {:#x} paom {:?} tls={}", self.session, st.get(319).map(|v| v.to_string()), self.link.is_tls())
        });
        Ok(SessionInfo { session: self.session, session2, server_session: st, protocol_ver: 2, init_ssl_refused, tls: self.link.is_tls() })
    }

    fn start_tls(&mut self, t: &TlsOptions, addr: &str) -> Result<()> {
        let cfg = client_config(t)?;
        let conn = rustls::ClientConnection::new(cfg, server_name(t, addr)).map_err(|e| Error::Tls(e.to_string()))?;
        self.link.set_deadline(Some(Instant::now() + self.timeout));
        self.link.start_tls(rustls::Connection::Client(conn)).map_err(|e| Error::proto(format!("TLS: {e}")))?;
        self.oms_secret = self.link.export_keying_material(b"EXPERIMENTAL_OMS", 32);
        self.integrity = true;
        Ok(())
    }

    /// Sends one request and waits for its response. Notifications that
    /// arrive in between are queued.
    pub(crate) fn call(&mut self, ver: u8, func: u16, tflags: u8, body: Vec<u8>) -> Result<Message> {
        let req_seq = self.send_req(ver, func, tflags, body)?;
        self.link.set_deadline(Some(Instant::now() + self.timeout));
        loop {
            let m = match self.recv() {
                Ok(m) => m,
                Err(e) => {
                    if e.is_timeout() {
                        // a late response must not break the next request
                        self.no_reply.insert(req_seq);
                    }
                    return Err(e);
                }
            };
            if m.opcode == OP_NOTIFICATION {
                self.queue_notification(m);
                continue;
            }
            if m.seq != req_seq {
                if self.no_reply.remove(&m.seq) {
                    continue;
                }
                return Err(Error::proto(format!("response seq {}, want {}", m.seq, req_seq)));
            }
            return Ok(m);
        }
    }

    fn queue_notification(&mut self, m: Message) {
        if self.notifications.len() >= MAX_QUEUED_NOTIFICATIONS {
            self.notifications.pop_front();
            log_with(&self.logger, Level::Warn, || "s7plus: notification queue full, oldest dropped".into());
        }
        self.notifications.push_back(m);
    }

    pub(crate) fn send_req(&mut self, ver: u8, func: u16, tflags: u8, body: Vec<u8>) -> Result<u16> {
        self.seq = self.seq.wrapping_add(1);
        if self.seq == 0 {
            self.seq = 1;
        }
        let mut session = if self.session == 0 { OBJ_NULL_SERVER_SESSION } else { self.session };
        if func == FN_INIT_SSL {
            session = 0;
        }
        let req = Message { version: ver, opcode: OP_REQUEST, function: func, seq: self.seq, session, tflags, body };
        self.trace(">", &req);
        self.link.set_deadline(Some(Instant::now() + self.timeout));
        self.send(ver, &req.encode())?;
        Ok(req.seq)
    }

    fn trace(&self, dir: &str, m: &Message) {
        if let Some(t) = &self.trace {
            t(dir, m);
        }
        log_with(&self.logger, Level::Trace, || format!("s7plus: {dir} {}", m.dump()));
    }

    /// Returns the next complete message (system events are skipped).
    fn recv(&mut self) -> Result<Message> {
        loop {
            let tp = self.link.read_tpdu()?;
            let pdus = self.re.push(&tp)?;
            for p in pdus {
                if p.version == 0xfe {
                    continue; // system event (acknowledges)
                }
                let m = Message::parse(p.version, &p.data)?;
                self.trace("<", &m);
                return Ok(m);
            }
        }
    }

    /// Returns the next notification of object sub (0 = any); notifications
    /// of other objects stay queued.
    pub(crate) fn wait_notification(&mut self, timeout: Duration, sub: u32) -> Result<Message> {
        if let Some(i) = self.notifications.iter().position(|m| sub == 0 || notif_sub(m) == sub) {
            return Ok(self.notifications.remove(i).unwrap());
        }
        self.link.set_deadline(Some(Instant::now() + timeout));
        loop {
            let m = self.recv()?;
            if m.opcode != OP_NOTIFICATION {
                self.no_reply.remove(&m.seq);
                continue;
            }
            if sub == 0 || notif_sub(&m) == sub {
                return Ok(m);
            }
            self.queue_notification(m);
        }
    }

    /// Splits the data into fragments that fit a 1024-byte TPDU.
    fn send(&mut self, ver: u8, data: &[u8]) -> Result<()> {
        let mut max = MAX_TPDU - 3 - 4 - 4;
        if self.link.is_tls() {
            max -= 5 + 17; // TLS 1.3 record header + AEAD tag + content type
        }
        let mut data = data;
        let mut first = true;
        while first || !data.is_empty() {
            first = false;
            let n = data.len().min(max);
            let (chunk, rest) = data.split_at(n);
            data = rest;
            let mut p = Vec::with_capacity(n + 8);
            p.extend_from_slice(&[0x72, ver, (n >> 8) as u8, n as u8]);
            p.extend_from_slice(chunk);
            if data.is_empty() {
                p.extend_from_slice(&[0x72, ver, 0, 0]);
            }
            self.link.write_fragment(&p)?;
        }
        Ok(())
    }

    pub(crate) fn integrity_id(&mut self, w: &mut Writer, set: bool) {
        if !self.integrity {
            return;
        }
        if set {
            w.vlq32(self.integ_set);
            self.integ_set = self.integ_set.wrapping_add(1);
        } else {
            w.vlq32(self.integ_get);
            self.integ_get = self.integ_get.wrapping_add(1);
        }
    }

    pub(crate) fn set_attributes(&mut self, object: u32, attrs: &[u32], vals: &[Value]) -> Result<Vec<u64>> {
        let mut w = Writer::new();
        w.u32(object);
        w.vlq32(attrs.len() as u32);
        w.vlq32(attrs.len() as u32);
        for a in attrs {
            w.vlq32(*a);
        }
        for (i, v) in vals.iter().enumerate() {
            w.vlq32(i as u32 + 1);
            v.encode(&mut w);
        }
        w.byte(0);
        encode_object_qualifier(&mut w);
        self.integrity_id(&mut w, true);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_SET_MULTI_VARIABLES, 0x34, w.b)?;
        item_errors(&m, attrs.len())
    }

    pub(crate) fn read_chunk(&mut self, addrs: &[ItemAddress]) -> Result<(Vec<Value>, Vec<u64>)> {
        let mut w = Writer::new();
        w.u32(0);
        w.vlq32(addrs.len() as u32);
        w.vlq32(addrs.iter().map(|a| a.fields()).sum());
        for a in addrs {
            a.encode(&mut w);
        }
        encode_object_qualifier(&mut w);
        self.integrity_id(&mut w, false);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_GET_MULTI_VARIABLES, 0x34, w.b)?;
        decode_values(&m, addrs.len())
    }

    pub(crate) fn get_attributes(&mut self, object: u32, attrs: &[u32]) -> Result<(Vec<Value>, Vec<u64>)> {
        let mut w = Writer::new();
        w.u32(object);
        w.vlq32(attrs.len() as u32);
        w.vlq32(attrs.len() as u32);
        for a in attrs {
            w.vlq32(*a);
        }
        encode_object_qualifier(&mut w);
        self.integrity_id(&mut w, false);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_GET_MULTI_VARIABLES, 0x34, w.b)?;
        decode_values(&m, attrs.len())
    }

    pub(crate) fn write_chunk(&mut self, addrs: &[ItemAddress], vals: &[Value]) -> Result<Vec<u64>> {
        let mut w = Writer::new();
        w.u32(0);
        w.vlq32(addrs.len() as u32);
        w.vlq32(addrs.iter().map(|a| a.fields()).sum());
        for a in addrs {
            a.encode(&mut w);
        }
        for (i, v) in vals.iter().enumerate() {
            w.vlq32(i as u32 + 1);
            v.encode(&mut w);
        }
        w.byte(0);
        encode_object_qualifier(&mut w);
        self.integrity_id(&mut w, true);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_SET_MULTI_VARIABLES, 0x34, w.b)?;
        item_errors(&m, addrs.len())
    }

    pub(crate) fn get_var_sub_streamed(&mut self, object: u32, attr: u32) -> Result<Value> {
        let mut w = Writer::new();
        w.u32(object);
        Value { flags: FLAG_ADDRESS_ARRAY, typ: T_UDINT, v: V::Array(vec![V::U32(attr)]) }.encode(&mut w);
        encode_object_qualifier(&mut w);
        w.u16(0x0001);
        self.integrity_id(&mut w, false);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_GET_VAR_SUB_STREAMED, 0x34, w.b)?;
        let mut r = Reader::new(&m.body);
        return_value(&m, &mut r)?;
        r.byte();
        let v = Value::decode(&mut r);
        r.result()?;
        Ok(v)
    }

    fn set_variable_body(&mut self, object: u32, attr: u32, v: &Value) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(object);
        w.vlq32(1);
        w.vlq32(attr);
        v.encode(&mut w);
        encode_object_qualifier(&mut w);
        w.byte(0);
        self.integrity_id(&mut w, true);
        w.u32(0);
        w.b
    }

    pub(crate) fn set_variable(&mut self, object: u32, attr: u32, v: &Value) -> Result<()> {
        let body = self.set_variable_body(object, attr, v);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_SET_VARIABLE, 0x34, body)?;
        return_value(&m, &mut Reader::new(&m.body))
    }

    /// SetVariable with transport flags 0x74 (no response wanted), as the
    /// reference does for the subscription credit limit.
    pub(crate) fn set_variable_no_reply(&mut self, object: u32, attr: u32, v: &Value) -> Result<()> {
        let body = self.set_variable_body(object, attr, v);
        let ver = self.protocol_ver;
        let seq = self.send_req(ver, FN_SET_VARIABLE, 0x74, body)?;
        self.no_reply.insert(seq);
        Ok(())
    }

    pub(crate) fn create_object(&mut self, req_id: u32, o: &Object) -> std::result::Result<Vec<u32>, (Vec<u32>, Error)> {
        let mut w = Writer::new();
        w.u32(req_id);
        Value::udint(0).encode(&mut w);
        w.u32(0);
        self.integrity_id(&mut w, true);
        o.encode(&mut w);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_CREATE_OBJECT, 0x34, w.b).map_err(|e| (Vec::new(), e))?;
        decode_create_object(&m)
    }

    pub(crate) fn delete_object(&mut self, id: u32) -> Result<()> {
        let mut w = Writer::new();
        w.u32(id);
        w.byte(0);
        encode_object_qualifier(&mut w);
        self.integrity_id(&mut w, true);
        w.u32(0);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_DELETE_OBJECT, 0x34, w.b)?;
        return_value(&m, &mut Reader::new(&m.body))
    }

    pub(crate) fn explore_body(&mut self, q: &crate::explore::ExploreReq) -> Vec<u8> {
        let mut w = Writer::new();
        q.encode(&mut w);
        self.integrity_id(&mut w, false);
        w.u32(0);
        w.b
    }

    pub(crate) fn explore_objects(&mut self, q: &crate::explore::ExploreReq) -> Result<Vec<Object>> {
        let body = self.explore_body(q);
        let ver = self.protocol_ver;
        let m = self.call(ver, FN_EXPLORE, 0x34, body)?;
        let mut r = Reader::new(&m.body);
        return_value(&m, &mut r)?;
        r.u32(); // explored object id
        if self.integrity {
            r.vlq32();
        }
        let mut out = Vec::new();
        while r.peek() == crate::object::ELEM_START_OBJECT && r.ok() {
            if let Some(o) = decode_object(&mut r) {
                out.push(o);
            }
        }
        r.result()?;
        Ok(out)
    }
}

pub(crate) fn decode_create_object(m: &Message) -> std::result::Result<Vec<u32>, (Vec<u32>, Error)> {
    let mut r = Reader::new(&m.body);
    let rv = r.vlq64();
    let n = r.byte();
    let ids: Vec<u32> = (0..n).map(|_| r.vlq32()).collect();
    if rv & 0x4000_0000_0000_0000 != 0 || (rv as i64) < 0 {
        return Err((ids, Error::Return { function: FN_CREATE_OBJECT, ret: rv }));
    }
    if ids.is_empty() {
        return Err((ids, Error::proto("CreateObject returned no id")));
    }
    if let Some(e) = r.err.take() {
        return Err((ids, e));
    }
    Ok(ids)
}
