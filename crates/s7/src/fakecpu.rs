//! In-process S7comm server with simulated memory, for tests (feature
//! `fakecpu`). It decodes ReadVar/WriteVar
//! items like a CPU, enforces the negotiated PDU, answers SZL (0x0000, 0x0011,
//! 0x0424), the clock (Userdata group 7), the block functions (group 3: count,
//! list, info), block upload (jobs 0x1D/0x1E/0x1F) and PLC control (STOP 0x29,
//! start 0x28). Long Userdata answers are split into data units of `ud_max`
//! bytes and sent one per follow-up request. It can simulate a broken link or a
//! device that ignores writes.
//!
//! ```no_run
//! let cpu = s7::fakecpu::FakeCpu::new(240);
//! let addr = cpu.start(); // 127.0.0.1:<port>; stops when `cpu` is dropped
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

pub const AREA_INPUTS: u8 = 0x81;
pub const AREA_FLAGS: u8 = 0x83;
pub const AREA_DB: u8 = 0x84;

/// A program block of the fake CPU.
#[derive(Debug, Clone, Default)]
pub struct Block {
    /// 0x08 OB, 0x0A DB, 0x0C FC, 0x0E FB…
    pub typ: u8,
    pub number: u16,
    pub lang: u8,
    pub flags: u8,
    pub author: String,
    pub family: String,
    pub name: String,
    pub version: u8,
    pub checksum: u16,
    pub local: u16,
    /// what upload returns (complete block)
    pub body: Vec<u8>,
}

/// Mutable state of the fake CPU (lock it with `FakeCpu::lock`).
pub struct State {
    pub pdu: u16,
    /// memory per (area, db)
    pub mem: HashMap<(u8, u16), Vec<u8>>,
    pub reqs: usize,
    pub conns: usize,
    /// close the first connection after N jobs (0 = never)
    pub drop_after: usize,
    /// acknowledge writes but keep the old values
    pub ignore_writes: bool,
    /// acknowledge but drop the next N write jobs (then works)
    pub ignore_writes_n: usize,
    /// payload size of one Userdata data unit (0 = PDU − 26)
    pub ud_max: usize,
    /// payload size of one upload step (0 = 200)
    pub upload_chunk: usize,
    /// the answer to SZL 0x0000
    pub szl_id_list: Vec<u16>,
    /// 10-byte clock record
    pub clock: Vec<u8>,
    /// SZL 0x0424 mode: 0x08 RUN, 0x04 STOP
    pub state: u8,
    pub blocks: Vec<Block>,
    pub stops: usize,
    /// true = cold start
    pub starts: Vec<bool>,
    pub follow_ups: usize,
    /// protocol violations seen
    pub errors: Vec<String>,
}

impl State {
    /// Memory of an area (panics if it does not exist).
    pub fn mem(&mut self, area: u8, db: u16) -> &mut Vec<u8> {
        self.mem.get_mut(&(area, db)).expect("no such area")
    }
}

struct Shared {
    st: Mutex<State>,
    stop: AtomicBool,
}

/// The fake device. Dropping it stops its listeners.
pub struct FakeCpu {
    sh: Arc<Shared>,
    listeners: Mutex<Vec<(SocketAddr, JoinHandle<()>)>>,
}

impl FakeCpu {
    /// A CPU with M (4096 B), I (1024 B) and DB1 (2000 B); DB2 does not exist.
    pub fn new(pdu: u16) -> FakeCpu {
        let mut mem = HashMap::new();
        mem.insert((AREA_FLAGS, 0), vec![0u8; 4096]);
        mem.insert((AREA_INPUTS, 0), vec![0u8; 1024]);
        mem.insert((AREA_DB, 1), vec![0u8; 2000]);
        let st = State {
            pdu,
            mem,
            reqs: 0,
            conns: 0,
            drop_after: 0,
            ignore_writes: false,
            ignore_writes_n: 0,
            ud_max: 0,
            upload_chunk: 0,
            szl_id_list: vec![
                0x0000, 0x0011, 0x0111, 0x0F11, 0x0424, 0x0131, 0x0132, 0x0F12, 0x0F14, 0x0019, 0x0074, 0x0F74, 0x00A0, 0x0FA0, 0x0D91,
                0x0091, 0x0012, 0x0013, 0x0014, 0x0015, 0x0025, 0x0037,
            ],
            clock: vec![0u8; 10],
            state: 0x08,
            blocks: Vec::new(),
            stops: 0,
            starts: Vec::new(),
            follow_ups: 0,
            errors: Vec::new(),
        };
        FakeCpu { sh: Arc::new(Shared { st: Mutex::new(st), stop: AtomicBool::new(false) }), listeners: Mutex::new(Vec::new()) }
    }

    /// Locks the state (memory, knobs, counters).
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.sh.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Listens on 127.0.0.1:0 and returns the address ("127.0.0.1:port").
    pub fn start(&self) -> String {
        self.listen("127.0.0.1:0").expect("fake CPU listen")
    }

    /// Serves on `addr` (e.g. "0.0.0.0:102") and returns the bound address.
    pub fn listen(&self, addr: &str) -> std::io::Result<String> {
        let ln = TcpListener::bind(addr)?;
        let local = ln.local_addr()?;
        let sh = self.sh.clone();
        let th = std::thread::Builder::new().name("fakecpu-accept".into()).spawn(move || serve(sh, ln))?;
        self.listeners.lock().unwrap().push((local, th));
        Ok(local.to_string())
    }

    /// Stops accepting connections (open connections end when their peer closes).
    pub fn stop(&self) {
        self.sh.stop.store(true, Ordering::SeqCst);
        let ls: Vec<_> = self.listeners.lock().unwrap().drain(..).collect();
        for (a, th) in ls {
            let _ = TcpStream::connect(a); // wake accept()
            let _ = th.join();
        }
    }

    // ---- convenience accessors ----

    pub fn add_block(&self, b: Block) {
        self.lock().blocks.push(b);
    }
    /// Creates (or replaces) data block n with size bytes of zeros.
    pub fn add_db(&self, n: u16, size: usize) {
        self.lock().mem.insert((AREA_DB, n), vec![0u8; size]);
    }
    /// Operating mode byte (0x08 RUN, 0x04 STOP).
    pub fn state(&self) -> u8 {
        self.lock().state
    }
    /// How many STOP and start requests arrived (starts: true = cold).
    pub fn control(&self) -> (usize, Vec<bool>) {
        let s = self.lock();
        (s.stops, s.starts.clone())
    }
    pub fn follow_ups(&self) -> usize {
        self.lock().follow_ups
    }
    /// Number of S7 job requests.
    pub fn reqs(&self) -> usize {
        self.lock().reqs
    }
    /// Number of accepted connections.
    pub fn conns(&self) -> usize {
        self.lock().conns
    }
    pub fn reset_reqs(&self) {
        self.lock().reqs = 0;
    }
    pub fn clock(&self) -> Vec<u8> {
        self.lock().clock.clone()
    }
    pub fn set_clock(&self, b: &[u8]) {
        self.lock().clock = b.to_vec();
    }
    /// A copy of an area's memory.
    pub fn mem(&self, area: u8, db: u16) -> Vec<u8> {
        self.lock().mem(area, db).clone()
    }
    /// Writes bytes into an area's memory.
    pub fn set_mem(&self, area: u8, db: u16, off: usize, b: &[u8]) {
        self.lock().mem(area, db)[off..off + b.len()].copy_from_slice(b);
    }
    pub fn set_drop_after(&self, n: usize) {
        self.lock().drop_after = n;
    }
    pub fn set_ignore_writes(&self, v: bool) {
        self.lock().ignore_writes = v;
    }
    pub fn set_ignore_writes_n(&self, n: usize) {
        self.lock().ignore_writes_n = n;
    }
    pub fn set_ud_max(&self, n: usize) {
        self.lock().ud_max = n;
    }
    pub fn set_upload_chunk(&self, n: usize) {
        self.lock().upload_chunk = n;
    }
    pub fn szl_id_list(&self) -> Vec<u16> {
        self.lock().szl_id_list.clone()
    }
    /// Protocol violations seen so far.
    pub fn errors(&self) -> Vec<String> {
        self.lock().errors.clone()
    }
}

impl Drop for FakeCpu {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(sh: Arc<Shared>, ln: TcpListener) {
    for conn in ln.incoming() {
        if sh.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(conn) = conn else { continue };
        let first = {
            let mut s = sh.st.lock().unwrap();
            s.conns += 1;
            s.conns == 1
        };
        let sh2 = sh.clone();
        let _ = std::thread::Builder::new().name("fakecpu-conn".into()).spawn(move || handle(sh2, conn, first));
    }
}

fn tpkt(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x03, 0x00];
    out.extend_from_slice(&((4 + payload.len()) as u16).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn reply(rosctr: u8, r: [u8; 2], param: &[u8], data: &[u8]) -> Vec<u8> {
    reply_err(rosctr, r, 0, 0, param, data)
}

fn reply_err(rosctr: u8, r: [u8; 2], class: u8, code: u8, param: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = vec![0x02, 0xF0, 0x80, 0x32, rosctr, 0, 0, r[0], r[1]];
    m.extend_from_slice(&(param.len() as u16).to_be_bytes());
    m.extend_from_slice(&(data.len() as u16).to_be_bytes());
    if rosctr == 0x03 {
        m.extend_from_slice(&[class, code]);
    }
    m.extend_from_slice(param);
    m.extend_from_slice(data);
    tpkt(&m)
}

/// Userdata reply param: 00 01 12 08 12 <8<<4|group> sub seq <data unit ref>
/// <more: 1 = not the last unit> <error BE>.
fn ud_param(group: u8, sub: u8, seq: u8, more: bool, err: u16) -> Vec<u8> {
    vec![0x00, 0x01, 0x12, 0x08, 0x12, 0x80 | group, sub, seq, 0x00, more as u8, (err >> 8) as u8, err as u8]
}

fn read_msg(c: &mut TcpStream) -> Option<Vec<u8>> {
    let mut h = [0u8; 4];
    c.read_exact(&mut h).ok()?;
    let n = (u16::from_be_bytes([h[2], h[3]]) as usize).checked_sub(4)?;
    let mut b = vec![0u8; n];
    c.read_exact(&mut b).ok()?;
    Some(b)
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn handle(sh: Arc<Shared>, mut conn: TcpStream, first: bool) {
    let _ = conn.set_nodelay(true);
    match read_msg(&mut conn) {
        Some(cr) if cr.len() > 1 && cr[1] == 0xE0 => {}
        _ => return,
    }
    if conn.write_all(&tpkt(&[0x11, 0xD0, 0x00, 0x01, 0x00, 0x02, 0x00, 0xC0, 0x01, 0x0A])).is_err() {
        return;
    }
    let mut jobs = 0usize;
    let mut pending: Vec<Vec<u8>> = Vec::new(); // Userdata data units still to send
    let mut pend_hdr = [0u8; 2]; // their group/sub
    let mut seq: u8 = 0;
    let mut up: Option<Vec<u8>> = None; // upload in progress
    let mut up_id: u32 = 0;
    loop {
        let Some(b) = read_msg(&mut conn) else { return };
        if b.len() < 13 {
            return;
        }
        let s7 = &b[3..];
        let r = [s7[4], s7[5]];
        let par_len = be16(&s7[6..]) as usize;
        if s7.len() < 10 + par_len {
            return;
        }
        let param = &s7[10..10 + par_len];
        let data = &s7[10 + par_len..];
        let out: Vec<u8>;
        if s7[1] == 0x07 {
            let (group, sub) = (param[5] & 0x0f, param[6]);
            if param[4] == 0x12 {
                // follow-up: next data unit
                sh.st.lock().unwrap().follow_ups += 1;
                if pending.is_empty() || param[7] != seq {
                    out = reply(0x07, r, &ud_param(group, sub, seq, false, 0xD0A1), &[0x0A, 0, 0, 0]);
                } else {
                    out = send_unit(r, pend_hdr, seq, &mut pending);
                }
                if conn.write_all(&out).is_err() {
                    return;
                }
                continue;
            }
            let (payload, err) = match group {
                0x07 => {
                    out = clock_reply(&sh, r, sub, data);
                    if conn.write_all(&out).is_err() {
                        return;
                    }
                    continue;
                }
                0x04 => szl_payload(&sh, data),
                0x03 => block_func(&sh, sub, data),
                _ => (None, 0x8104),
            };
            if err != 0 {
                out = reply(0x07, r, &ud_param(group, sub, 0, false, err), &[0x0A, 0, 0, 0]);
            } else if let Some(mut payload) = payload {
                let max = {
                    let s = sh.st.lock().unwrap();
                    if s.ud_max > 0 {
                        s.ud_max
                    } else {
                        s.pdu as usize - 26
                    }
                };
                let mut units = Vec::new();
                while payload.len() > max {
                    let rest = payload.split_off(max);
                    units.push(payload);
                    payload = rest;
                }
                units.push(payload);
                seq = seq.wrapping_add(1);
                if seq == 0 {
                    seq = 1;
                }
                pend_hdr = [group, sub];
                pending = units;
                out = send_unit(r, pend_hdr, seq, &mut pending);
            } else {
                // object does not exist
                out = reply(0x07, r, &ud_param(group, sub, 0, false, 0), &[0x0A, 0, 0, 0]);
            }
            if conn.write_all(&out).is_err() {
                return;
            }
            continue;
        }
        jobs += 1;
        {
            let mut s = sh.st.lock().unwrap();
            if first && s.drop_after > 0 && jobs > s.drop_after {
                return;
            }
            s.reqs += 1;
            if s7.len() > s.pdu as usize {
                let msg = format!("request of {} bytes exceeds PDU {}", s7.len(), s.pdu);
                s.errors.push(msg);
            }
        }
        out = match param[0] {
            0xF0 => {
                let mut p = param.to_vec();
                let pdu = sh.st.lock().unwrap().pdu;
                p[6..8].copy_from_slice(&pdu.to_be_bytes());
                reply(0x03, r, &p, &[])
            }
            0x04 => {
                let d = read_items(&mut sh.st.lock().unwrap(), param);
                reply(0x03, r, &param[..2], &d)
            }
            0x05 => {
                let d = write_items(&mut sh.st.lock().unwrap(), param, data);
                reply(0x03, r, &param[..2], &d)
            }
            0x1D => {
                // start upload: 1D st 00 00 <id 4> <len> "_TTNNNNNA"
                let n = param[8] as usize;
                let name = String::from_utf8_lossy(&param[9..9 + n]).into_owned();
                match find_file(&sh, &name) {
                    None => reply_err(0x03, r, 0xD2, 0x09, &param[..1], &[]),
                    Some(b) => {
                        up_id = up_id.wrapping_add(1);
                        let ln = format!("{:07}", b.body.len());
                        up = Some(b.body);
                        let mut p = vec![0x1D, 0x00, 0x01, 0x00];
                        p.extend_from_slice(&up_id.to_be_bytes());
                        p.push(ln.len() as u8);
                        p.extend_from_slice(ln.as_bytes());
                        reply(0x03, r, &p, &[])
                    }
                }
            }
            0x1E => {
                // upload: reply 1E <status: 1 = more>; data <len> 00 FB payload
                let id = u32::from_be_bytes([param[4], param[5], param[6], param[7]]);
                match up.as_mut() {
                    Some(buf) if id == up_id => {
                        let chunk = sh.st.lock().unwrap().upload_chunk;
                        let n = if chunk > 0 { chunk } else { 200 }.min(buf.len());
                        let st = if n < buf.len() { 1u8 } else { 0 };
                        let mut d = vec![(n >> 8) as u8, n as u8, 0x00, 0xFB];
                        d.extend(buf.drain(..n));
                        reply(0x03, r, &[0x1E, st], &d)
                    }
                    _ => reply_err(0x03, r, 0xD2, 0x0A, &param[..1], &[]),
                }
            }
            0x1F => {
                up = None;
                reply(0x03, r, &[0x1F], &[])
            }
            0x29 => {
                // STOP: 29 00×5 <len> "P_PROGRAM"
                let was = {
                    let mut s = sh.st.lock().unwrap();
                    s.stops += 1;
                    let w = s.state;
                    s.state = 0x04;
                    w
                };
                let mut p = vec![0x29];
                if was == 0x04 {
                    p.push(0x07);
                }
                reply(0x03, r, &p, &[])
            }
            0x28 => {
                // PI service: 28 00×6 FD <arglen 2> arg <len> "P_PROGRAM"
                let al = be16(&param[8..]) as usize;
                let sl = param[10 + al] as usize;
                let svc = &param[11 + al..11 + al + sl];
                if svc != b"P_PROGRAM" {
                    reply_err(0x03, r, 0xD2, 0x04, &param[..1], &[])
                } else {
                    let was = {
                        let mut s = sh.st.lock().unwrap();
                        s.starts.push(&param[10..10 + al] == b"C ");
                        let w = s.state;
                        s.state = 0x08;
                        w
                    };
                    let mut p = vec![0x28];
                    if was == 0x08 {
                        p.push(0x02);
                    }
                    reply(0x03, r, &p, &[])
                }
            }
            _ => continue,
        };
        if conn.write_all(&out).is_err() {
            return;
        }
    }
}

/// Resolves an item: (memory key, start, n, is bit) or None if it does not exist.
fn locate(s: &State, it: &[u8]) -> Option<((u8, u16), usize, usize, bool)> {
    let ts = it[3];
    let count = be16(&it[4..]) as usize;
    let db = be16(&it[6..]);
    let area = it[8];
    let addr = (it[9] as usize) << 16 | (it[10] as usize) << 8 | it[11] as usize;
    let m = s.mem.get(&(area, db))?;
    if ts == 0x01 {
        return (addr / 8 < m.len()).then_some(((area, db), addr, 1, true));
    }
    (addr / 8 + count <= m.len()).then_some(((area, db), addr / 8, count, false))
}

fn read_items(s: &mut State, param: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let n = param[1] as usize;
    for i in 0..n {
        let it = &param[2 + 12 * i..14 + 12 * i];
        match locate(s, it) {
            None => out.extend_from_slice(&[0x0A, 0x00, 0x00, 0x00]),
            Some((k, start, _, true)) => {
                let m = &s.mem[&k];
                out.extend_from_slice(&[0xFF, 0x03, 0x00, 0x01, (m[start / 8] >> (start % 8)) & 1]);
            }
            Some((k, start, cnt, false)) => {
                out.extend_from_slice(&[0xFF, 0x04]);
                out.extend_from_slice(&((cnt * 8) as u16).to_be_bytes());
                out.extend_from_slice(&s.mem[&k][start..start + cnt]);
            }
        }
        if out.len() % 2 != 0 && i < n - 1 {
            out.push(0);
        }
    }
    out
}

fn write_items(s: &mut State, param: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ignore = s.ignore_writes;
    if s.ignore_writes_n > 0 {
        s.ignore_writes_n -= 1;
        ignore = true;
    }
    let n = param[1] as usize;
    let mut p = 0;
    for i in 0..n {
        let it = &param[2 + 12 * i..14 + 12 * i];
        let mut ln = be16(&data[p + 2..]) as usize;
        if data[p + 1] == 0x03 || data[p + 1] == 0x04 {
            ln = ln.div_ceil(8);
        }
        let val = data[p + 4..p + 4 + ln].to_vec();
        p += 4 + ln;
        if !ln.is_multiple_of(2) && i < n - 1 {
            p += 1;
        }
        match locate(s, it) {
            None => out.push(0x0A),
            Some(_) if ignore => out.push(0xFF),
            Some((k, start, _, true)) => {
                let m = s.mem.get_mut(&k).unwrap();
                if val[0] & 1 == 1 {
                    m[start / 8] |= 1 << (start % 8);
                } else {
                    m[start / 8] &= !(1 << (start % 8));
                }
                out.push(0xFF);
            }
            Some((k, start, cnt, false)) => {
                s.mem.get_mut(&k).unwrap()[start..start + cnt].copy_from_slice(&val[..cnt]);
                out.push(0xFF);
            }
        }
    }
    out
}

/// Sends units[0] and keeps the rest.
fn send_unit(r: [u8; 2], hdr: [u8; 2], seq: u8, units: &mut Vec<Vec<u8>>) -> Vec<u8> {
    let u = units.remove(0);
    let mut d = vec![0xFF, 0x09];
    d.extend_from_slice(&(u.len() as u16).to_be_bytes());
    d.extend_from_slice(&u);
    reply(0x07, r, &ud_param(hdr[0], hdr[1], seq, !units.is_empty(), 0), &d)
}

/// Answers a read-SZL request (data FF 09 0004 <id> <index>).
fn szl_payload(sh: &Shared, data: &[u8]) -> (Option<Vec<u8>>, u16) {
    let id = be16(&data[4..]);
    let idx = be16(&data[6..]);
    let rec = |idx: u16, order: &str, v: [u8; 4]| {
        let mut r = vec![0u8; 28];
        r[0..2].copy_from_slice(&idx.to_be_bytes());
        let o = format!("{order}                    ");
        r[2..22].copy_from_slice(&o.as_bytes()[..20]);
        r[24..28].copy_from_slice(&v);
        r
    };
    let mut recs = Vec::new();
    let mut rec_len = 28;
    match id {
        0x0011 => {
            recs.extend(rec(1, "6ES7 212-1BE31-0XB0", [0, 1, 0, 1]));
            recs.extend(rec(7, "", [b'V', 3, 0, 2]));
        }
        0x0000 => {
            rec_len = 2;
            for x in sh.st.lock().unwrap().szl_id_list.iter() {
                recs.extend_from_slice(&x.to_be_bytes());
            }
        }
        0x0424 => {
            rec_len = 20;
            let mut r = vec![0u8; 20];
            r[0] = 0x51;
            r[1] = 0x44;
            r[2] = 0xFF;
            r[3] = sh.st.lock().unwrap().state;
            recs = r;
        }
        _ => return (None, 0xD401),
    }
    let mut d = vec![(id >> 8) as u8, id as u8, (idx >> 8) as u8, idx as u8, 0, rec_len as u8, 0, (recs.len() / rec_len) as u8];
    d.extend(recs);
    (Some(d), 0)
}

fn find_block(sh: &Shared, typ: u8, num: u16) -> Option<Block> {
    sh.st.lock().unwrap().blocks.iter().find(|b| b.typ == typ && b.number == num).cloned()
}

/// Resolves "_TTNNNNNA" (upload) or "TTNNNNNA" (block info).
fn find_file(sh: &Shared, name: &str) -> Option<Block> {
    let name = name.strip_prefix('_').unwrap_or(name);
    if name.len() != 8 || name.as_bytes()[7] != b'A' {
        return None;
    }
    let t = u8::from_str_radix(&name[..2], 16).ok()?;
    if !name[2..7].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u16 = name[2..7].parse().ok()?;
    find_block(sh, t, n)
}

/// Answers Userdata group 3: sub 1 count per type, sub 2 list of a type
/// (payload "0A"), sub 3 block info (payload "0A00001A").
fn block_func(sh: &Shared, sub: u8, data: &[u8]) -> (Option<Vec<u8>>, u16) {
    let arg: &[u8] = if data[0] == 0xFF { &data[4..4 + be16(&data[2..]) as usize] } else { &[] };
    let blocks = sh.st.lock().unwrap().blocks.clone();
    match sub {
        0x01 => {
            let mut out = Vec::new();
            for t in [0x08u8, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F] {
                let n = blocks.iter().filter(|b| b.typ == t).count();
                out.extend_from_slice(&[0x30, format!("{t:X}").as_bytes()[0], (n >> 8) as u8, n as u8]);
            }
            (Some(out), 0)
        }
        0x02 => {
            if arg.len() != 2 {
                return (None, 0);
            }
            let Some(t) = std::str::from_utf8(arg).ok().and_then(|s| u8::from_str_radix(s, 16).ok()) else {
                return (None, 0);
            };
            let mut out = Vec::new();
            for b in blocks.iter().filter(|b| b.typ == t) {
                out.extend_from_slice(&[(b.number >> 8) as u8, b.number as u8, 0x22, b.lang]);
            }
            (Some(out), 0)
        }
        0x03 => {
            let Some(b) = find_file(sh, &String::from_utf8_lossy(arg)) else { return (None, 0) };
            let mut d = vec![0u8; 78];
            d[0] = 0x01;
            d[2..4].copy_from_slice(&78u16.to_be_bytes());
            d[6] = b'p';
            d[7] = b'p';
            d[9] = b.flags;
            d[10] = b.lang;
            d[11] = b.typ;
            d[12..14].copy_from_slice(&b.number.to_be_bytes());
            d[14..18].copy_from_slice(&(b.body.len() as u32).to_be_bytes());
            // code timestamp: 2026-11-04 12:00:00.000 = 43200000 ms, day 15648 since 1984-01-01
            d[22..26].copy_from_slice(&43_200_000u32.to_be_bytes());
            d[26..28].copy_from_slice(&15648u16.to_be_bytes());
            d[32..34].copy_from_slice(&15648u16.to_be_bytes());
            d[38..40].copy_from_slice(&b.local.to_be_bytes());
            let mc7 = b.body.len().saturating_sub(36);
            d[40..42].copy_from_slice(&(mc7 as u16).to_be_bytes());
            let put = |d: &mut Vec<u8>, at: usize, s: &str| {
                let n = s.len().min(8);
                d[at..at + n].copy_from_slice(&s.as_bytes()[..n]);
            };
            put(&mut d, 42, &b.author);
            put(&mut d, 50, &b.family);
            put(&mut d, 58, &b.name);
            d[66] = b.version;
            d[68..70].copy_from_slice(&b.checksum.to_be_bytes());
            (Some(d), 0)
        }
        _ => (None, 0x8104),
    }
}

fn clock_reply(sh: &Shared, r: [u8; 2], sub: u8, data: &[u8]) -> Vec<u8> {
    let clk = {
        let mut s = sh.st.lock().unwrap();
        if sub == 0x02 && data.len() >= 14 {
            s.clock = data[4..14].to_vec();
        }
        s.clock.clone()
    };
    let param = [0x00, 0x01, 0x12, 0x08, 0x12, 0x87, sub, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mut d = vec![0x0A, 0x00, 0x00, 0x00];
    if sub == 0x01 {
        d = vec![0xFF, 0x09, 0x00, 0x0A];
        d.extend(clk);
    }
    reply(0x07, r, &param, &d)
}
