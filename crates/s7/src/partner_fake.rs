//! In-process fake PLC for the S7 partner (BSEND / BRCV) tests: it accepts
//! the COTP connection with the TSAPs of its S7 connection, negotiates a
//! small PDU (so blocks are fragmented), receives BSEND blocks (BRCV,
//! acknowledged as an S7-1500 does: sequence = IDSeq, the PDU reference of the
//! request) and sends BSEND blocks on request. It also sends USENDs (the
//! frames of an S7-1500, from PLCSIM) and keeps the USENDs it gets (URCV).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::partner::{Area, Block, Usend};

enum Out {
    Bsend(Block),
    Usend(Usend),
}

#[derive(Default)]
struct State {
    received: Vec<Block>,
    /// USENDs of the partner (URCV).
    usends: Vec<Usend>,
    /// BRCV refuses this R_ID (error 0x0001 in the answer).
    refuse: Option<u32>,
    conns: Vec<TcpStream>,
    errors: Vec<String>,
}

/// A fake PLC with one S7 connection to an unspecified partner.
pub struct FakePartnerPlc {
    addr: String,
    state: Arc<Mutex<State>>,
    out: Arc<Mutex<Option<Sender<Out>>>>,
    stop: Arc<AtomicBool>,
}

/// TSAPs of the fake S7 connection: the PLC (remote for the client) 10.01,
/// the partner (local for the client) 03.00.
pub const PLC_TSAP: u16 = 0x1001;
pub const PARTNER_TSAP: u16 = 0x0300;

fn tpkt(payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x03, 0x00, 0, 0];
    v.extend_from_slice(payload);
    let n = v.len() as u16;
    v[2..4].copy_from_slice(&n.to_be_bytes());
    v
}

fn read_tpkt(s: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut h = [0u8; 4];
    s.read_exact(&mut h)?;
    let n = u16::from_be_bytes([h[2], h[3]]) as usize;
    let mut b = vec![0u8; n.saturating_sub(4)];
    s.read_exact(&mut b)?;
    Ok(b)
}

fn userdata(pdu_ref: u16, params: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = vec![0x02, 0xF0, 0x80, 0x32, 0x07, 0x00, 0x00];
    m.extend_from_slice(&pdu_ref.to_be_bytes());
    m.extend_from_slice(&(params.len() as u16).to_be_bytes());
    m.extend_from_slice(&(data.len() as u16).to_be_bytes());
    m.extend_from_slice(params);
    m.extend_from_slice(data);
    tpkt(&m)
}

impl FakePartnerPlc {
    /// Starts on 127.0.0.1 with this PDU size.
    pub fn start(pdu: u16) -> FakePartnerPlc {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().expect("addr").to_string();
        let state = Arc::new(Mutex::new(State::default()));
        let out: Arc<Mutex<Option<Sender<Out>>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, o, sp) = (state.clone(), out.clone(), stop.clone());
        thread::spawn(move || {
            for c in l.incoming() {
                if sp.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(c) = c else { continue };
                let (tx, rx) = channel();
                *o.lock().unwrap() = Some(tx);
                if let Ok(c2) = c.try_clone() {
                    st.lock().unwrap().conns.push(c2);
                }
                let st2 = st.clone();
                thread::spawn(move || {
                    if let Err(e) = serve(c, pdu, &st2, rx) {
                        st2.lock().unwrap().errors.push(e.to_string());
                    }
                });
            }
        });
        FakePartnerPlc { addr, state, out, stop }
    }

    pub fn addr(&self) -> String {
        self.addr.clone()
    }

    /// The blocks the BRCV got.
    pub fn received(&self) -> Vec<Block> {
        self.state.lock().unwrap().received.clone()
    }

    /// The BRCV refuses blocks with this R_ID.
    pub fn refuse(&self, r_id: Option<u32>) {
        self.state.lock().unwrap().refuse = r_id;
    }

    /// The PLC program calls BSEND with this block (on the open connection).
    pub fn bsend(&self, r_id: u32, data: Vec<u8>) -> bool {
        self.out.lock().unwrap().as_ref().is_some_and(|tx| tx.send(Out::Bsend(Block { r_id, data })).is_ok())
    }

    /// The PLC program calls USEND with these areas.
    pub fn usend(&self, r_id: u32, areas: Vec<Area>) -> bool {
        self.out.lock().unwrap().as_ref().is_some_and(|tx| tx.send(Out::Usend(Usend { r_id, areas })).is_ok())
    }

    /// The USENDs the URCV got.
    pub fn usends(&self) -> Vec<Usend> {
        self.state.lock().unwrap().usends.clone()
    }

    /// Closes the open connections (a PLC restart).
    pub fn drop_connections(&self) {
        for c in self.state.lock().unwrap().conns.drain(..) {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }

    pub fn errors(&self) -> Vec<String> {
        self.state.lock().unwrap().errors.clone()
    }
}

impl Drop for FakePartnerPlc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.drop_connections();
        let _ = TcpStream::connect(&self.addr);
    }
}

fn serve(mut s: TcpStream, pdu: u16, st: &Arc<Mutex<State>>, rx: Receiver<Out>) -> std::io::Result<()> {
    // COTP CR: accept only the TSAPs of the S7 connection
    let cr = read_tpkt(&mut s)?;
    let find = |code: u8| -> Option<u16> {
        let mut i = 7;
        while i + 1 < cr.len() {
            let (c, n) = (cr[i], cr[i + 1] as usize);
            if c == code && n == 2 && i + 3 < cr.len() + 1 {
                return Some(u16::from_be_bytes([cr[i + 2], *cr.get(i + 3)?]));
            }
            i += 2 + n;
        }
        None
    };
    if cr.get(1) != Some(&0xE0) || find(0xC1) != Some(PARTNER_TSAP) || find(0xC2) != Some(PLC_TSAP) {
        return Ok(()); // an S7-1500 resets the connection
    }
    let mut cc = cr.clone();
    cc[1] = 0xD0;
    s.write_all(&tpkt(&cc))?;
    // Setup Communication
    let sc = read_tpkt(&mut s)?;
    let pref = [sc[7], sc[8]];
    let mut a = vec![0x02, 0xF0, 0x80, 0x32, 0x03, 0x00, 0x00, pref[0], pref[1], 0x00, 0x08, 0x00, 0x00, 0x00, 0x00];
    a.extend_from_slice(&[0xF0, 0x00, 0x00, 0x01, 0x00, 0x01]);
    a.extend_from_slice(&pdu.to_be_bytes());
    s.write_all(&tpkt(&a))?;
    s.set_read_timeout(Some(Duration::from_millis(20)))?;
    let mut my_ref: u16 = 0x100;
    let mut rx_block: Option<Block> = None;
    loop {
        // a BSEND of the PLC program
        let next = rx.try_recv();
        if let Ok(Out::Usend(u)) = &next {
            // as PLCSIM sends it: PDU reference 0, R_ID in the parameters
            let mut p = vec![0x00, 0x01, 0x12, 0x06, 0x13, 0x00];
            p.extend_from_slice(&u.r_id.to_be_bytes());
            let mut d = vec![0xFF, 0x02];
            d.extend_from_slice(&(u.areas.len() as u16).to_be_bytes());
            for (i, a) in u.areas.iter().enumerate() {
                let len = if (0x03..=0x05).contains(&a.tsize) { a.data.len() * 8 } else { a.data.len() };
                d.extend_from_slice(&[0x00, a.tsize]);
                d.extend_from_slice(&(len as u16).to_be_bytes());
                d.extend_from_slice(&a.data);
                if a.data.len() % 2 == 1 && i + 1 < u.areas.len() {
                    d.push(0x00);
                }
            }
            s.write_all(&userdata(0, &p, &d))?;
        }
        if let Ok(Out::Bsend(b)) = next {
            let max = pdu as usize - (10 + 12 + 12 + 2);
            let multi = b.data.len() > max;
            let id_seq: u8 = if multi { 0x21 } else { 0 };
            let (mut off, mut first, mut seq) = (0usize, true, 0u8);
            let mut slice = max;
            while off < b.data.len() {
                let n = (b.data.len() - off).min(slice);
                let last = off + n == b.data.len();
                let extra = if first { 2 } else { 0 };
                let mut d = vec![0xFF, 0x09];
                d.extend_from_slice(&((n + 8 + extra) as u16).to_be_bytes());
                d.extend_from_slice(&[0x12, 0x06, 0x13, 0x00]);
                d.extend_from_slice(&b.r_id.to_be_bytes());
                if first {
                    d.extend_from_slice(&(b.data.len() as u16).to_be_bytes());
                }
                d.extend_from_slice(&b.data[off..off + n]);
                my_ref = my_ref.wrapping_add(1);
                let p = [0x00, 0x01, 0x12, 0x08, 0x12, 0x46, 0x01, seq, id_seq, if last { 0 } else { 1 }, 0x00, 0x00];
                s.write_all(&userdata(my_ref, &p, &d))?;
                // wait for the acknowledgement of this fragment; a BSEND of the
                // partner in between goes to the BRCV (a PLC serves both)
                s.set_read_timeout(Some(Duration::from_secs(3)))?;
                let ack = loop {
                    let f = read_tpkt(&mut s)?;
                    if (f.len() > 18 && f[4] == 0x07 && f[18] == 0x46) || matches!(crate::partner::parse_usend(&f), Ok(Some(_))) {
                        brcv(&mut s, &f, st, &mut rx_block)?;
                        continue;
                    }
                    break f;
                };
                s.set_read_timeout(Some(Duration::from_millis(20)))?;
                if ack.len() < 25 || ack[18] != 0x86 || u16::from_be_bytes([ack[7], ack[8]]) != my_ref || (multi && ack[20] != id_seq) {
                    st.lock().unwrap().errors.push(format!("bad BSEND acknowledgement {:02x?}", ack));
                    break;
                }
                seq = ack[20];
                off += n;
                if first {
                    first = false;
                    slice += 2;
                }
            }
        }
        let f = match read_tpkt(&mut s) {
            Ok(f) => f,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        brcv(&mut s, &f, st, &mut rx_block)?;
    }
}

/// One BSEND fragment of the partner: store it, acknowledge it like an
/// S7-1500 (IDSeq as sequence, the PDU reference of the request).
fn brcv(s: &mut TcpStream, f: &[u8], st: &Arc<Mutex<State>>, rx_block: &mut Option<Block>) -> std::io::Result<()> {
    if let Ok(Some(u)) = crate::partner::parse_usend(f) {
        st.lock().unwrap().usends.push(u);
        return Ok(());
    }
    if f.len() < 25 || f[4] != 0x07 || f[18] != 0x46 {
        return Ok(());
    }
    let pref = u16::from_be_bytes([f[7], f[8]]);
    let plen = u16::from_be_bytes([f[9], f[10]]) as usize;
    let p = &f[13..13 + plen];
    let d = &f[13 + plen..];
    let (seq, id_seq, eos) = (p[7], p[8], p[9]);
    let len = u16::from_be_bytes([d[2], d[3]]) as usize;
    let r_id = u32::from_be_bytes([d[8], d[9], d[10], d[11]]);
    let first = seq == 0;
    let payload = if first { &d[14..4 + len] } else { &d[12..4 + len] };
    let refused = st.lock().unwrap().refuse == Some(r_id);
    if first {
        *rx_block = Some(Block { r_id, data: Vec::new() });
    }
    if let Some(b) = rx_block.as_mut() {
        b.data.extend_from_slice(payload);
    }
    let ack_seq = if id_seq != 0 { id_seq } else { 0x31 };
    let err: u16 = if refused { 0x0001 } else { 0 };
    let e = err.to_be_bytes();
    let ap = [0x00, 0x01, 0x12, 0x08, 0x12, 0x86, 0x01, ack_seq, 0x00, 0x00, e[0], e[1]];
    // store the block before the acknowledgement: the client checks it after send()
    if eos == 0 || refused {
        if let Some(b) = rx_block.take() {
            if !refused {
                st.lock().unwrap().received.push(b);
            }
        }
    }
    s.write_all(&userdata(pref, &ap, &[0x0A, 0x00, 0x00, 0x00]))?;
    Ok(())
}
