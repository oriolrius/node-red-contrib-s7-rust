//! S7comm over ISO-on-TCP: TPKT + COTP + Setup Communication + ReadVar/WriteVar.
//! Frame layouts follow the public S7comm descriptions (Wireshark s7comm
//! dissector, snap7, nodes7).
//!
//! Scope: classic S7comm (S7-300/400/1200/1500 with absolute addressing, S7-200,
//! LOGO!). NOT S7CommPlus (see the `s7plus` crate).

use std::fmt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{io, local};
pub use crate::error::{Error, Result};
use crate::log::hex_spaced;

// Area codes.
pub const AREA_PERIPHERAL: u8 = 0x80; // PI/PQ (PE/PA)
pub const AREA_INPUTS: u8 = 0x81; // I / E
pub const AREA_OUTPUTS: u8 = 0x82; // Q / A
pub const AREA_FLAGS: u8 = 0x83; // M / F (Merker)
pub const AREA_DB: u8 = 0x84; // data block
pub const AREA_COUNTER: u8 = 0x1c; // C / Z (not on S7-1200/1500)
pub const AREA_TIMER: u8 = 0x1d; // T     (not on S7-1200/1500)

// Request transport sizes, item byte[3].
pub(crate) const TS_BIT: u8 = 0x01;
pub(crate) const TS_BYTE: u8 = 0x02;
pub(crate) const TS_COUNTER: u8 = 0x1c;
pub(crate) const TS_TIMER: u8 = 0x1d;

// Data-section transport sizes (responses and WriteVar data items).
pub(crate) const DT_BIT: u8 = 0x03; // length in bits (1)
pub(crate) const DT_BYTE: u8 = 0x04; // byte/word/dword; length in bits
pub(crate) const DT_OCTETS: u8 = 0x09; // octet string; length in bytes (timers/counters)

/// Per-request overhead the driver reserves: S7 header (10) + param (2) + one
/// item (12) + data item header (4). With PDU 960 this gives the documented
/// 932-byte maximum array.
pub const PDU_OVERHEAD: usize = 28;

/// Frame trace hook: direction ("tx" / "rx") and the frame bytes.
pub type TraceFn = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;
/// Called after every S7 request with its duration and error.
pub type OnRequestFn = Arc<dyn Fn(Duration, Option<&Error>) + Send + Sync>;

/// Connection options.
#[derive(Clone)]
pub struct DialOptions {
    /// host:port, port usually 102.
    pub addr: String,
    /// dest TSAP byte[0]: PG=0x01, OP=0x02, Other/S7-Basic=0x03.
    pub link_type: u8,
    pub rack: u8,
    /// CPU slot (S7-300: 2; S7-1200/1500: 1).
    pub slot: u8,
    /// src TSAP; 0 -> 0x0100.
    pub local_tsap: u16,
    /// explicit dst TSAP (S7-200, LOGO!, routing); 0 = [LinkType, (Rack<<5)|Slot].
    pub remote_tsap: u16,
    /// proposed PDU size; 0 -> 960.
    pub max_pdu: u16,
    /// per request; zero -> 5 s.
    pub timeout: Duration,
    /// optional local IP to bind (NIC selection).
    pub local_addr: String,
    /// receives every frame sent ("tx") and received ("rx").
    pub trace: Option<TraceFn>,
    /// called after every S7 request with its duration.
    pub on_request: Option<OnRequestFn>,
}

impl Default for DialOptions {
    /// Zero value (all zeros: the family presets or `dial` fill defaults).
    fn default() -> Self {
        DialOptions {
            addr: String::new(),
            link_type: 0,
            rack: 0,
            slot: 0,
            local_tsap: 0,
            remote_tsap: 0,
            max_pdu: 0,
            timeout: Duration::ZERO,
            local_addr: String::new(),
            trace: None,
            on_request: None,
        }
    }
}

impl fmt::Debug for DialOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialOptions")
            .field("addr", &self.addr)
            .field("link_type", &self.link_type)
            .field("rack", &self.rack)
            .field("slot", &self.slot)
            .field("local_tsap", &format_args!("0x{:04x}", self.local_tsap))
            .field("remote_tsap", &format_args!("0x{:04x}", self.remote_tsap))
            .field("max_pdu", &self.max_pdu)
            .field("timeout", &self.timeout)
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

// ---- TPKT (RFC1006) + COTP (ISO 8073) -----------------------------------------

pub(crate) fn write_tpkt(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&[0x03, 0x00]);
    out.extend_from_slice(&((4 + payload.len()) as u16).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Builds the COTP CR (ISO 8073 CR): dest TSAP =
/// [LinkType, (Rack<<5)|Slot], src TSAP = LocalTSAP, params
/// C0 01 0A (TPDU 1024), C1 02 <src>, C2 02 <dst>.
pub fn cotp_connect_request(o: &DialOptions) -> Vec<u8> {
    let src = o.local_tsap.to_be_bytes();
    let mut p = vec![
        0x00, // length (filled below)
        0xE0, // CR
        0x00,
        0x00, // destination reference
        0x00,
        0x01, // source reference
        0x00, // class 0
        0xC0,
        0x01,
        0x0A, // TPDU size 1024
        0xC1,
        0x02,
        src[0],
        src[1], // source TSAP
        0xC2,
        0x02,
        o.link_type,
        (o.rack << 5) | o.slot, // destination TSAP
    ];
    if o.remote_tsap != 0 {
        p[16] = (o.remote_tsap >> 8) as u8;
        p[17] = o.remote_tsap as u8;
    }
    p[0] = (p.len() - 1) as u8;
    p
}

/// Setup Communication parameters: F0 00 <AMQ calling=1> <AMQ called=1> <PDU>.
pub fn setup_comm_params(max_pdu: u16) -> Vec<u8> {
    let mut p = vec![0xF0, 0x00, 0x00, 0x01, 0x00, 0x01];
    p.extend_from_slice(&max_pdu.to_be_bytes());
    p
}

/// Frames an S7 PDU: TPKT + COTP DT (02 F0 80) + header 32 <rosctr> 0000
/// <pduref> <parlen> <datalen> + params + data.
pub fn s7_frame(pdu_ref: u16, rosctr: u8, params: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(17 + params.len() + data.len());
    m.extend_from_slice(&[0x03, 0x00, 0x00, 0x00, 0x02, 0xF0, 0x80, 0x32, rosctr, 0x00, 0x00]);
    m.extend_from_slice(&pdu_ref.to_be_bytes());
    m.extend_from_slice(&(params.len() as u16).to_be_bytes());
    m.extend_from_slice(&(data.len() as u16).to_be_bytes());
    m.extend_from_slice(params);
    m.extend_from_slice(data);
    let n = m.len() as u16;
    m[2..4].copy_from_slice(&n.to_be_bytes());
    m
}

// ---- items -------------------------------------------------------------------

/// One S7ANY variable: 12 bytes on the wire. The transport size is
/// derived: `is_bit` → bit; area 0x1D → timers; area 0x1C → counters; else bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Item {
    pub area: u8,
    pub db: u16,
    /// byte offset (timer/counter number for T/C)
    pub start: u32,
    /// bit 0-7 for bit items
    pub bit: u8,
    /// bytes for byte items, 1 for bits, n for T/C
    pub count: u16,
    pub is_bit: bool,
}

impl Item {
    /// A single bit (M0.0, DB1.DBX0.3, I0.1).
    pub fn bit(area: u8, db: u16, byte_off: u32, bit: u8) -> Item {
        Item { area, db, start: byte_off, bit, count: 1, is_bit: true }
    }
    /// n raw bytes (MB0, DB1.DBW0 = 2 bytes, ...).
    pub fn bytes(area: u8, db: u16, byte_off: u32, n: u16) -> Item {
        Item { area, db, start: byte_off, bit: 0, count: n, is_bit: false }
    }
    /// n timers starting at T<num> (2 bytes S5TIME each).
    pub fn timers(num: u32, n: u16) -> Item {
        Item { area: AREA_TIMER, db: 0, start: num, bit: 0, count: n, is_bit: false }
    }
    /// n counters starting at C<num> (2 bytes BCD each).
    pub fn counters(num: u32, n: u16) -> Item {
        Item { area: AREA_COUNTER, db: 0, start: num, bit: 0, count: n, is_bit: false }
    }

    /// Request transport size.
    pub fn kind(&self) -> u8 {
        if self.is_bit {
            TS_BIT
        } else if self.area == AREA_TIMER {
            TS_TIMER
        } else if self.area == AREA_COUNTER {
            TS_COUNTER
        } else {
            TS_BYTE
        }
    }

    /// True for S5 timers or counters.
    pub fn is_timer_counter(&self) -> bool {
        matches!(self.kind(), TS_TIMER | TS_COUNTER)
    }

    /// Number of data bytes the item carries.
    pub fn size(&self) -> usize {
        match self.kind() {
            TS_BIT => 1,
            TS_TIMER | TS_COUNTER => 2 * self.count as usize,
            _ => self.count as usize,
        }
    }

    /// The 12-byte request item: 12 0a 10 <TS> <count BE> <DB BE> <area> <address 24b BE>.
    pub fn encode(&self) -> [u8; 12] {
        let k = self.kind();
        let addr = if k == TS_TIMER || k == TS_COUNTER {
            self.start // T/C use the element number, not a bit address
        } else {
            self.start.wrapping_mul(8).wrapping_add(self.bit as u32)
        };
        let c = self.count.to_be_bytes();
        let d = self.db.to_be_bytes();
        [0x12, 0x0a, 0x10, k, c[0], c[1], d[0], d[1], self.area, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]
    }

    /// WriteVar data item header: 00 <transport> <length BE>; length in bits
    /// for 0x03/0x04, in bytes for 0x09.
    pub fn data_header(&self, n: usize) -> [u8; 4] {
        match self.kind() {
            TS_BIT => [0x00, DT_BIT, 0x00, 0x01],
            TS_TIMER | TS_COUNTER => {
                let l = (n as u16).to_be_bytes();
                [0x00, DT_OCTETS, l[0], l[1]]
            }
            _ => {
                let l = ((n * 8) as u16).to_be_bytes();
                [0x00, DT_BYTE, l[0], l[1]]
            }
        }
    }
}

/// Outcome of one item: its data or the CPU's item error.
pub type ItemResult = std::result::Result<Vec<u8>, Error>;

// ---- reply parsing (pure: also the fuzz entry points) -----------------------------

/// Parses one TPKT body (COTP DT + S7 PDU). The ranges index `b`.
pub(crate) fn parse_reply(b: &[u8]) -> Result<Reply> {
    if b.len() < 3 || b[1] != 0xF0 {
        return Err(io(format!("not a COTP DT: {}", hex_spaced(b))));
    }
    let s7 = &b[3..];
    if s7.len() < 10 || s7[0] != 0x32 {
        return Err(io(format!("not an S7 PDU: {}", hex_spaced(s7))));
    }
    let rosctr = s7[1];
    let par_len = u16::from_be_bytes([s7[6], s7[7]]) as usize;
    let data_len = u16::from_be_bytes([s7[8], s7[9]]) as usize;
    let mut hdr = 10;
    if rosctr == 0x02 || rosctr == 0x03 {
        hdr = 12;
        if s7.len() < 12 {
            return Err(io("short Ack-Data header"));
        }
        if s7[10] != 0 || s7[11] != 0 {
            return Err(Error::Proto(s7[10], s7[11]));
        }
    }
    if s7.len() < hdr + par_len + data_len {
        return Err(io("truncated S7 PDU"));
    }
    let p0 = 3 + hdr;
    Ok(Reply { rosctr, param: p0..p0 + par_len, data: p0 + par_len..p0 + par_len + data_len })
}

/// Splits the data part of a ReadVar reply into `n` item results.
pub(crate) fn parse_read_data(d: &[u8], n: usize, mut put: impl FnMut(usize, std::result::Result<&[u8], Error>)) -> Result<()> {
    let mut p = 0;
    for i in 0..n {
        if p + 4 > d.len() {
            return Err(io(format!("truncated reply at item {i}")));
        }
        let (ret, tsize) = (d[p], d[p + 1]);
        let ln = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        p += 4;
        if ret != 0xFF {
            put(i, Err(Error::Item(ret))); // an error item carries no data
            continue;
        }
        let k = if tsize == DT_BIT || tsize == DT_BYTE || tsize == 0x05 { ln.div_ceil(8) } else { ln };
        if p + k > d.len() {
            return Err(io(format!("truncated data at item {i}")));
        }
        put(i, Ok(&d[p..p + k]));
        p += k;
        if k % 2 != 0 && i < n - 1 {
            p += 1; // fill byte between items
        }
    }
    Ok(())
}

// ---- connection -------------------------------------------------------------------

/// A parsed S7 response (ranges index the connection's receive buffer).
pub(crate) struct Reply {
    pub rosctr: u8,
    pub param: std::ops::Range<usize>,
    pub data: std::ops::Range<usize>,
}

/// An owned S7 response.
pub(crate) struct OwnedReply {
    pub rosctr: u8,
    pub param: Vec<u8>,
    pub data: Vec<u8>,
}

/// A live S7comm connection to one CPU. Not shared: one request in flight.
pub struct Conn {
    s: TcpStream,
    trace: Option<TraceFn>,
    on_req: Option<OnRequestFn>,
    pdu_ref: u16,
    timeout: Duration,
    buf: Vec<u8>,
    /// Negotiated at Setup Communication.
    pub max_pdu: u16,
}

impl fmt::Debug for Conn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn").field("peer", &self.s.peer_addr().ok()).field("max_pdu", &self.max_pdu).finish()
    }
}

fn connect(o: &DialOptions, deadline: Instant) -> Result<TcpStream> {
    use socket2::{Domain, Protocol, SockAddr, Socket, TcpKeepalive, Type};
    let addrs: Vec<SocketAddr> = o.addr.to_socket_addrs().map_err(|e| io(format!("dial tcp {}: {e}", o.addr)))?.collect();
    if addrs.is_empty() {
        return Err(io(format!("dial tcp {}: no address", o.addr)));
    }
    let local_ip: Option<std::net::IpAddr> = if o.local_addr.is_empty() {
        None
    } else {
        Some(o.local_addr.parse().map_err(|_| local(format!("bad local address {:?}", o.local_addr)))?)
    };
    let mut last = io(format!("dial tcp {}: no address", o.addr));
    for a in addrs {
        if let Some(ip) = local_ip {
            if ip.is_ipv4() != a.is_ipv4() {
                continue;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io(format!("dial tcp {}: i/o timeout", o.addr)));
        }
        let sock = Socket::new(Domain::for_address(a), Type::STREAM, Some(Protocol::TCP))?;
        if let Some(ip) = local_ip {
            sock.bind(&SockAddr::from(SocketAddr::new(ip, 0))).map_err(|e| io(format!("bind {ip}: {e}")))?;
        }
        match sock.connect_timeout(&SockAddr::from(a), left) {
            Ok(()) => {
                // TCP keepalive 30 s and Nagle off.
                let _ = sock.set_tcp_keepalive(&TcpKeepalive::new().with_time(Duration::from_secs(30)));
                let _ = sock.set_tcp_nodelay(true);
                return Ok(sock.into());
            }
            Err(e) => last = io(format!("dial tcp {a}: {e}")),
        }
    }
    Err(last)
}

impl Conn {
    /// Connects, negotiates COTP, and runs Setup Communication.
    pub fn dial(o: &DialOptions) -> Result<Conn> {
        let mut o = o.clone();
        if o.timeout.is_zero() {
            o.timeout = Duration::from_secs(5);
        }
        if o.max_pdu == 0 {
            o.max_pdu = 960;
        }
        if o.local_tsap == 0 {
            o.local_tsap = 0x0100;
        }
        let deadline = Instant::now() + o.timeout;
        let s = connect(&o, deadline)?;
        let mut cn = Conn {
            s,
            trace: o.trace.clone(),
            on_req: o.on_request.clone(),
            pdu_ref: 0,
            timeout: o.timeout,
            buf: Vec::with_capacity(1024),
            max_pdu: 0,
        };
        cn.write_all(&write_tpkt(&cotp_connect_request(&o)), deadline).map_err(|e| e.context("send CR"))?;
        let cc = cn.read_tpkt(deadline).map_err(|e| e.context("recv CC (check rack/slot/TSAP)"))?;
        if cc.len() < 2 || cn.buf[1] != 0xD0 {
            return Err(io(format!("not a Connect Confirm: {}", hex_spaced(&cn.buf[cc]))));
        }
        let r = cn.round_trip(0x01, &setup_comm_params(o.max_pdu), &[]).map_err(|e| e.context("setup communication"))?;
        let param = &cn.buf[r.param];
        if param.len() < 8 || param[0] != 0xF0 {
            return Err(io(format!("bad setup reply param: {}", hex_spaced(param))));
        }
        cn.max_pdu = u16::from_be_bytes([param[6], param[7]]);
        Ok(cn)
    }

    /// Partner (BSEND/BRCV): sends one S7 message with the next PDU reference.
    pub(crate) fn send_s7(&mut self, rosctr: u8, params: &[u8], data: &[u8]) -> Result<()> {
        self.pdu_ref = self.pdu_ref.wrapping_add(1);
        self.send_s7_ref(self.pdu_ref, rosctr, params, data)
    }

    /// Partner: sends one S7 message with a given PDU reference (an answer
    /// carries the reference of its request).
    pub(crate) fn send_s7_ref(&mut self, pdu_ref: u16, rosctr: u8, params: &[u8], data: &[u8]) -> Result<()> {
        let msg = s7_frame(pdu_ref, rosctr, params, data);
        if let Some(t) = &self.trace {
            t("tx", &msg);
        }
        let deadline = Instant::now() + self.timeout;
        self.write_all(&msg, deadline)
    }

    /// Partner: waits up to `wait` for the next S7 message (no request
    /// sent): Ok(None) on timeout before the first byte; the frame (COTP +
    /// S7) otherwise.
    pub(crate) fn recv_s7(&mut self, wait: Duration) -> Result<Option<Vec<u8>>> {
        self.s.set_read_timeout(Some(wait.max(Duration::from_millis(1))))?;
        let mut first = [0u8; 1];
        match self.s.read(&mut first) {
            Ok(0) => return Err(io("EOF")),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        if first[0] != 0x03 {
            return Err(io(format!("bad TPKT version 0x{:02x}", first[0])));
        }
        let deadline = Instant::now() + self.timeout;
        self.buf.resize(3, 0);
        self.read_full(0, 3, deadline)?;
        let total = u16::from_be_bytes([self.buf[1], self.buf[2]]) as usize;
        if total < 7 {
            return Err(io(format!("short TPKT len {total}")));
        }
        self.buf.resize(total - 4, 0);
        self.read_full(0, total - 4, deadline)?;
        if let Some(t) = &self.trace {
            t("rx", &self.buf);
        }
        Ok(Some(self.buf.clone()))
    }

    /// The request timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Ends the connection.
    pub fn close(&self) {
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }

    fn write_all(&mut self, b: &[u8], deadline: Instant) -> Result<()> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io("i/o timeout"));
        }
        self.s.set_write_timeout(Some(left))?;
        self.s.write_all(b)?;
        Ok(())
    }

    fn read_full(&mut self, at: usize, n: usize, deadline: Instant) -> Result<()> {
        let mut got = 0;
        while got < n {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io("i/o timeout"));
            }
            self.s.set_read_timeout(Some(left))?;
            match self.s.read(&mut self.buf[at + got..at + n]) {
                Ok(0) => return Err(io(if got == 0 { "EOF" } else { "unexpected EOF" })),
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Reads one whole TPKT message into self.buf (COTP + payload).
    fn read_tpkt(&mut self, deadline: Instant) -> Result<std::ops::Range<usize>> {
        self.buf.resize(4, 0);
        self.read_full(0, 4, deadline)?;
        if self.buf[0] != 0x03 {
            return Err(io(format!("bad TPKT version 0x{:02x}", self.buf[0])));
        }
        let total = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
        if total < 7 {
            return Err(io(format!("short TPKT len {total}")));
        }
        self.buf.resize(total - 4, 0);
        self.read_full(0, total - 4, deadline)?;
        Ok(0..total - 4)
    }

    /// Sends one S7 message and parses the reply. Ack-Data (0x02/0x03) has a
    /// 12-byte header (10 + error class/code); Userdata (0x07) has 10.
    pub(crate) fn round_trip(&mut self, rosctr: u8, params: &[u8], data: &[u8]) -> Result<Reply> {
        let t0 = Instant::now();
        let r = self.round_trip_inner(rosctr, params, data);
        if let Some(f) = &self.on_req {
            f(t0.elapsed(), r.as_ref().err());
        }
        r
    }

    fn round_trip_inner(&mut self, rosctr: u8, params: &[u8], data: &[u8]) -> Result<Reply> {
        self.pdu_ref = self.pdu_ref.wrapping_add(1);
        let msg = s7_frame(self.pdu_ref, rosctr, params, data);
        let deadline = Instant::now() + self.timeout;
        if let Some(t) = &self.trace {
            t("tx", &msg);
        }
        self.write_all(&msg, deadline)?;
        let body = self.read_tpkt(deadline)?;
        if let Some(t) = &self.trace {
            t("rx", &self.buf[body.clone()]);
        }
        // the TPKT body starts at offset 0 of self.buf
        parse_reply(&self.buf[body])
    }

    /// round_trip with the reply copied out.
    pub(crate) fn round_trip_owned(&mut self, rosctr: u8, params: &[u8], data: &[u8]) -> Result<OwnedReply> {
        let r = self.round_trip(rosctr, params, data)?;
        Ok(OwnedReply { rosctr: r.rosctr, param: self.buf[r.param].to_vec(), data: self.buf[r.data].to_vec() })
    }

    // ---- read -----------------------------------------------------------------

    /// Reads all items, splitting them into as many ReadVar requests as the
    /// negotiated PDU needs, and splitting any byte item larger than the PDU
    /// into chunks (PDU − 28, as the driver does). Item errors are per item.
    pub fn read(&mut self, items: &[Item]) -> Result<Vec<ItemResult>> {
        let pdu = self.max_pdu as usize;
        let chunk = pdu.saturating_sub(PDU_OVERHEAD).max(1);
        // (item, original index, offset)
        let mut req: Vec<(Item, usize, usize)> = Vec::with_capacity(items.len());
        for (i, it) in items.iter().enumerate() {
            if it.kind() == TS_BYTE && it.count as usize > chunk {
                let mut off = 0;
                while off < it.count as usize {
                    let n = chunk.min(it.count as usize - off);
                    let mut c = *it;
                    c.start += off as u32;
                    c.count = n as u16;
                    req.push((c, i, off));
                    off += n;
                }
                continue;
            }
            req.push((*it, i, 0));
        }
        let mut out: Vec<ItemResult> = items.iter().map(|it| Ok(vec![0u8; it.size()])).collect();
        let mut start = 0;
        while start < req.len() {
            // Pack items while both request and reply fit in the PDU.
            let (mut rq, mut rp, mut end) = (10 + 2usize, 12 + 2usize, start);
            while end < req.len() {
                let n = req[end].0.size();
                if rq + 12 > pdu || rp + 4 + n + n % 2 > pdu || end - start == 255 {
                    break;
                }
                rq += 12;
                rp += 4 + n + n % 2;
                end += 1;
            }
            if end == start {
                return Err(local(format!("item {} does not fit in PDU {pdu}", req[start].1)));
            }
            let batch: Vec<Item> = req[start..end].iter().map(|r| r.0).collect();
            self.read_batch(&batch, |k, r| {
                let (_, idx, off) = req[start + k];
                match r {
                    Err(e) => out[idx] = Err(e),
                    Ok(d) => {
                        if let Ok(buf) = &mut out[idx] {
                            let n = d.len().min(buf.len().saturating_sub(off));
                            buf[off..off + n].copy_from_slice(&d[..n]);
                        }
                    }
                }
            })?;
            start = end;
        }
        Ok(out)
    }

    fn read_batch(&mut self, items: &[Item], put: impl FnMut(usize, std::result::Result<&[u8], Error>)) -> Result<()> {
        let mut param = Vec::with_capacity(2 + 12 * items.len());
        param.extend_from_slice(&[0x04, items.len() as u8]);
        for it in items {
            param.extend_from_slice(&it.encode());
        }
        let r = self.round_trip(0x01, &param, &[])?;
        let pr = &self.buf[r.param.clone()];
        if r.rosctr != 0x03 || pr.len() < 2 || pr[0] != 0x04 {
            return Err(io(format!("unexpected read reply rosctr=0x{:02x} param={}", r.rosctr, hex_spaced(pr))));
        }
        parse_read_data(&self.buf[r.data], items.len(), put)
    }

    // ---- write ----------------------------------------------------------------

    /// Writes values[i] to items[i] with WriteVar (function 0x05), splitting into
    /// several requests when the PDU requires it. Errors are per item.
    pub fn write<V: AsRef<[u8]>>(&mut self, items: &[Item], values: &[V]) -> Result<Vec<Option<Error>>> {
        if items.len() != values.len() {
            return Err(local("items and values differ in length"));
        }
        let pdu = self.max_pdu as usize;
        let limit = pdu.saturating_sub(PDU_OVERHEAD);
        for (i, it) in items.iter().enumerate() {
            let n = values[i].as_ref().len();
            if n != it.size() {
                return Err(local(format!("item {i}: value has {n} bytes, item needs {}", it.size())));
            }
            if it.size() > limit {
                return Err(local(format!("item {i}: {} bytes exceeds PDU limit {limit}", it.size())));
            }
        }
        let mut errs = vec![None; items.len()];
        let mut start = 0;
        while start < items.len() {
            let (mut size, mut end) = (10 + 2usize, start);
            while end < items.len() {
                let n = items[end].size();
                let add = 12 + 4 + n + n % 2;
                if size + add > pdu || end - start == 255 {
                    break;
                }
                size += add;
                end += 1;
            }
            if end == start {
                return Err(local(format!("item {start} does not fit in PDU {pdu}")));
            }
            let batch = self.write_batch(&items[start..end], &values[start..end])?;
            for (k, e) in batch.into_iter().enumerate() {
                errs[start + k] = e;
            }
            start = end;
        }
        Ok(errs)
    }

    fn write_batch<V: AsRef<[u8]>>(&mut self, items: &[Item], values: &[V]) -> Result<Vec<Option<Error>>> {
        let mut param = Vec::with_capacity(2 + 12 * items.len());
        param.extend_from_slice(&[0x05, items.len() as u8]); // WriteVar
        let mut data = Vec::new();
        for (i, it) in items.iter().enumerate() {
            let v = values[i].as_ref();
            param.extend_from_slice(&it.encode());
            data.extend_from_slice(&it.data_header(v.len()));
            data.extend_from_slice(v);
            if v.len() % 2 != 0 && i < items.len() - 1 {
                data.push(0x00); // fill byte between items
            }
        }
        let r = self.round_trip(0x01, &param, &data)?;
        let pr = &self.buf[r.param.clone()];
        if r.rosctr != 0x03 || pr.len() < 2 || pr[0] != 0x05 {
            return Err(io(format!("unexpected write reply rosctr=0x{:02x} param={}", r.rosctr, hex_spaced(pr))));
        }
        let d = &self.buf[r.data];
        if d.len() < items.len() {
            return Err(io(format!("short write reply: {}", hex_spaced(d))));
        }
        Ok((0..items.len()).map(|i| if d[i] != 0xFF { Some(Error::Item(d[i])) } else { None }).collect())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.close();
    }
}
