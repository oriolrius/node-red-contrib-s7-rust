//! S7 partner: block transfer with BSEND / BRCV over an S7 connection
//! (S7comm userdata, function group 6 "BSEND/BRCV", type/group 0x46 request,
//! 0x86 response; layout as in snap7 `TSnap7Partner`).
//!
//! The PLC has an S7 connection to an unspecified partner (TIA Portal: S7
//! connection, "Active connection establishment" off on the PLC side) and
//! calls BSEND / BRCV with its connection ID. This side connects actively to
//! the PLC TSAPs of that connection (for example local 03.00, remote 10.01),
//! then:
//! - [`Partner::recv`] returns the blocks of the PLC BSEND (and acknowledges
//!   each fragment);
//! - [`Partner::send`] sends a block to a PLC BRCV with the same R_ID.
//!
//! A block is at most 65534 bytes; the PLC limits it further (S7-1500: 65534,
//! S7-300: 32 KB).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::error::{io, local, Result};
use crate::wire::{Conn, DialOptions};

/// One block of a BSEND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub r_id: u32,
    pub data: Vec<u8>,
}

/// The largest block (the length is a 16-bit field).
pub const MAX_BLOCK: usize = 65534;

const TG_REQ: u8 = 0x46;
const TG_RES: u8 = 0x86;
const SUBFUN: u8 = 0x01;
/// S7 userdata header (10) + params (12) + data header (12) + 2.
const OVERHEAD: usize = 10 + 12 + 12 + 2;

struct Rx {
    r_id: u32,
    total: usize,
    data: Vec<u8>,
    seq_out: u8,
}

/// A BSEND/BRCV connection to one PLC.
pub struct Partner {
    conn: Conn,
    next: u8,
    rx: Option<Rx>,
    done: VecDeque<Block>,
}

/// A parsed userdata BSEND/BRCV frame.
struct Frame<'a> {
    pdu_ref: u16,
    tg: u8,
    seq: u8,
    id_seq: u8,
    eos: u8,
    err: u16,
    data: &'a [u8],
}

fn parse(f: &[u8]) -> Result<Option<Frame<'_>>> {
    // COTP DT (3) + S7 header 32 07 0000 ref parlen datalen (10)
    if f.len() < 13 || f[0] != 0x02 || f[1] != 0xF0 || f[3] != 0x32 {
        return Ok(None);
    }
    if f[4] != 0x07 {
        return Ok(None); // not userdata
    }
    let plen = u16::from_be_bytes([f[9], f[10]]) as usize;
    let dlen = u16::from_be_bytes([f[11], f[12]]) as usize;
    let p = f.get(13..13 + plen).ok_or_else(|| io("short userdata params"))?;
    let d = f.get(13 + plen..13 + plen + dlen).ok_or_else(|| io("short userdata data"))?;
    if p.len() < 12 || p[0..3] != [0x00, 0x01, 0x12] || p[6] != SUBFUN || (p[5] != TG_REQ && p[5] != TG_RES) {
        return Ok(None);
    }
    Ok(Some(Frame {
        pdu_ref: u16::from_be_bytes([f[7], f[8]]),
        tg: p[5],
        seq: p[7],
        id_seq: p[8],
        eos: p[9],
        err: u16::from_be_bytes([p[10], p[11]]),
        data: d,
    }))
}

fn params(tg: u8, seq: u8, id_seq: u8, eos: u8) -> [u8; 12] {
    [0x00, 0x01, 0x12, 0x08, 0x12, tg, SUBFUN, seq, id_seq, eos, 0x00, 0x00]
}

impl Partner {
    /// Connects to the PLC (COTP with `local_tsap` / `remote_tsap` of the S7
    /// connection) and negotiates the PDU.
    pub fn connect(o: &DialOptions) -> Result<Partner> {
        Ok(Partner { conn: Conn::dial(o)?, next: 0, rx: None, done: VecDeque::new() })
    }

    /// The negotiated PDU size.
    pub fn max_pdu(&self) -> u16 {
        self.conn.max_pdu
    }

    pub fn close(&self) {
        self.conn.close();
    }

    fn next_byte(&mut self) -> u8 {
        self.next = self.next.wrapping_add(1);
        if self.next == 0 {
            self.next = 1;
        }
        self.next
    }

    /// A fragment of a PLC BSEND: store it and acknowledge it. The
    /// acknowledgement of a fragmented block carries the IDSeq of the
    /// fragments as its sequence (what an S7-1500 does, checked on PLCSIM);
    /// a single fragment (IDSeq 0) gets a new sequence number.
    fn on_request(&mut self, pdu_ref: u16, seq_in: u8, id_seq: u8, eos: u8, d: &[u8]) -> Result<()> {
        if d.len() < 12 || d[0] != 0xFF || d[4..8] != [0x12, 0x06, 0x13, 0x00] {
            return Err(io(format!("bad BSEND data header ({} bytes)", d.len())));
        }
        let len = u16::from_be_bytes([d[2], d[3]]) as usize;
        let r_id = u32::from_be_bytes([d[8], d[9], d[10], d[11]]);
        let first = self.rx.is_none() || seq_in == 0;
        let (payload, total) = if first {
            if d.len() < 14 || len < 10 {
                return Err(io("short first BSEND fragment"));
            }
            (&d[14..(4 + len).min(d.len())], u16::from_be_bytes([d[12], d[13]]) as usize)
        } else {
            if len < 8 {
                return Err(io("short BSEND fragment"));
            }
            (&d[12..(4 + len).min(d.len())], 0)
        };
        if first {
            let seq_out = if id_seq != 0 { id_seq } else { self.next_byte() };
            self.rx = Some(Rx { r_id, total, data: Vec::with_capacity(total), seq_out });
        }
        let rx = self.rx.as_mut().expect("receive state");
        if rx.data.len() + payload.len() > MAX_BLOCK {
            self.rx = None;
            return Err(io("BSEND block larger than 65534 bytes"));
        }
        rx.data.extend_from_slice(payload);
        let seq_out = rx.seq_out;
        // the answer carries the PDU reference of the request
        self.conn.send_s7_ref(pdu_ref, 0x07, &params(TG_RES, seq_out, 0, 0), &[0x0A, 0x00, 0x00, 0x00])?;
        if eos == 0 {
            let rx = self.rx.take().expect("receive state");
            if rx.total != 0 && rx.total != rx.data.len() {
                return Err(io(format!("BSEND block: {} bytes announced, {} received", rx.total, rx.data.len())));
            }
            self.done.push_back(Block { r_id: rx.r_id, data: rx.data });
        }
        Ok(())
    }

    /// Reads frames until one is a response (returned) or the wait ends; PLC
    /// BSEND fragments in between are handled.
    fn pump(&mut self, wait: Duration, want_response: bool) -> Result<Option<(u8, u16)>> {
        let end = Instant::now() + wait;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let Some(f) = self.conn.recv_s7(left)? else { return Ok(None) };
            let Some(fr) = parse(&f)? else { continue };
            if fr.tg == TG_REQ {
                let (r, seq, id_seq, eos, d) = (fr.pdu_ref, fr.seq, fr.id_seq, fr.eos, fr.data.to_vec());
                self.on_request(r, seq, id_seq, eos, &d)?;
                if !want_response && !self.done.is_empty() {
                    return Ok(None);
                }
            } else if want_response {
                return Ok(Some((fr.seq, fr.err)));
            }
        }
    }

    /// The next block of a PLC BSEND, waiting up to `wait`. None: no
    /// complete block in that time.
    pub fn recv(&mut self, wait: Duration) -> Result<Option<Block>> {
        if let Some(b) = self.done.pop_front() {
            return Ok(Some(b));
        }
        self.pump(wait, false)?;
        Ok(self.done.pop_front())
    }

    /// Sends a block to the PLC BRCV with this R_ID (split into fragments
    /// of the PDU size; each one waits for the PLC acknowledgement).
    pub fn send(&mut self, r_id: u32, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > MAX_BLOCK {
            return Err(local(format!("BSEND block of {} bytes: 1..={MAX_BLOCK}", data.len())));
        }
        let pdu = self.conn.max_pdu as usize;
        if pdu <= OVERHEAD {
            return Err(local(format!("PDU {pdu} too small for BSEND")));
        }
        let mut max_slice = pdu - OVERHEAD;
        let mut off = 0;
        let mut seq_in = 0u8;
        let single = data.len() <= max_slice;
        let mut first = true;
        while off < data.len() {
            let n = (data.len() - off).min(max_slice);
            let last = off + n == data.len();
            let id_seq = if single { 0 } else { self.next_byte() };
            let extra = if first { 2 } else { 0 };
            let mut d = Vec::with_capacity(12 + extra + n);
            d.extend_from_slice(&[0xFF, 0x09]);
            d.extend_from_slice(&((n + 8 + extra) as u16).to_be_bytes());
            d.extend_from_slice(&[0x12, 0x06, 0x13, 0x00]);
            d.extend_from_slice(&r_id.to_be_bytes());
            if first {
                d.extend_from_slice(&(data.len() as u16).to_be_bytes());
            }
            d.extend_from_slice(&data[off..off + n]);
            self.conn.send_s7(0x07, &params(TG_REQ, seq_in, id_seq, if last { 0 } else { 1 }), &d)?;
            match self.pump(self.conn_timeout(), true)? {
                Some((seq, 0)) => seq_in = seq,
                Some((_, err)) => {
                    return Err(local(format!("the PLC refused the BSEND block (error {err:#06x}: no BRCV with R_ID {r_id} ready?)")))
                }
                None => return Err(io(format!("no BRCV acknowledgement from the PLC (R_ID {r_id})"))),
            }
            off += n;
            if first {
                first = false;
                max_slice += 2;
            }
        }
        Ok(())
    }

    fn conn_timeout(&self) -> Duration {
        self.conn.timeout()
    }
}
