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
//!
//! The same connection carries USEND / URCV (unacknowledged): a userdata
//! frame whose parameters are the R_ID (`00 01 12 06 13 xx <R_ID>`) and whose
//! data has one to four areas (`FF 02 <count u16>`, then per area
//! `00 <transport size> <length> <bytes>`, odd lengths padded except the
//! last). [`Partner::recv`] returns them as [`Message::Usend`];
//! [`Partner::usend`] sends one. A USEND fits in one PDU; the PLC URCV takes it
//! only when the areas match its RD_1..RD_4 (an S7-1500 needs the exact length
//! of an `Array of Byte`), and it gives no answer either way.

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

/// One area (SD_i / RD_i) of a USEND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    /// S7 transport size: 0x03 bit, 0x04 byte/word/dword, 0x05 integer,
    /// 0x07 real, 0x09 octet string.
    pub tsize: u8,
    pub data: Vec<u8>,
}

/// A USEND of the PLC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usend {
    pub r_id: u32,
    pub areas: Vec<Area>,
}

/// What the PLC sent: a BSEND block or a USEND.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Block(Block),
    Usend(Usend),
}

/// Areas of a USEND (SD_1..SD_4).
pub const MAX_AREAS: usize = 4;

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
    done: VecDeque<Message>,
    dropped: u64,
}

/// Received blocks kept until [`Partner::recv`] takes them; older ones are
/// dropped (see [`Partner::dropped`]).
pub const MAX_PENDING: usize = 256;

/// Length in bytes of a USEND area: bits for the transport sizes bit, byte
/// and integer, bytes for the others.
fn area_bytes(tsize: u8, len: u16) -> usize {
    match tsize {
        0x03..=0x05 => (len as usize).div_ceil(8),
        _ => len as usize,
    }
}

/// A USEND indication (parameters `00 01 12 06 13 xx <R_ID>`); None for
/// other frames. The data must hold exactly the announced areas.
pub(crate) fn parse_usend(f: &[u8]) -> Result<Option<Usend>> {
    if f.len() < 13 || f[0] != 0x02 || f[1] != 0xF0 || f[3] != 0x32 || f[4] != 0x07 {
        return Ok(None);
    }
    let plen = u16::from_be_bytes([f[9], f[10]]) as usize;
    let dlen = u16::from_be_bytes([f[11], f[12]]) as usize;
    let Some(p) = f.get(13..13 + plen) else { return Ok(None) };
    if p.len() != 10 || p[0..5] != [0x00, 0x01, 0x12, 0x06, 0x13] {
        return Ok(None);
    }
    let r_id = u32::from_be_bytes([p[6], p[7], p[8], p[9]]);
    let d = f.get(13 + plen..13 + plen + dlen).ok_or_else(|| io("short USEND data"))?;
    if d.len() < 4 || d[0] != 0xFF {
        return Err(io(format!("bad USEND data header ({} bytes)", d.len())));
    }
    let n = u16::from_be_bytes([d[2], d[3]]) as usize;
    if n == 0 || n > MAX_AREAS {
        return Err(io(format!("USEND with {n} areas (1..={MAX_AREAS})")));
    }
    let mut areas = Vec::with_capacity(n);
    let mut o = 4;
    for i in 0..n {
        let h = d.get(o..o + 4).ok_or_else(|| io(format!("USEND area {} of {n}: truncated header", i + 1)))?;
        let tsize = h[1];
        let len = area_bytes(tsize, u16::from_be_bytes([h[2], h[3]]));
        o += 4;
        let data = d.get(o..o + len).ok_or_else(|| io(format!("USEND area {} of {n}: {len} bytes announced", i + 1)))?;
        areas.push(Area { tsize, data: data.to_vec() });
        o += len;
        if len % 2 == 1 && i + 1 < n {
            o += 1; // fill byte
        }
    }
    if o != d.len() {
        return Err(io(format!("USEND: {} bytes after the areas", d.len().saturating_sub(o))));
    }
    Ok(Some(Usend { r_id, areas }))
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
        Ok(Partner { conn: Conn::dial(o)?, next: 0, rx: None, done: VecDeque::new(), dropped: 0 })
    }

    /// Received blocks dropped because more than [`MAX_PENDING`] waited.
    pub fn dropped(&self) -> u64 {
        self.dropped
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
        if d.len() < 4 + len {
            self.rx = None;
            return Err(io(format!("BSEND fragment: {} bytes announced, {} present", len, d.len().saturating_sub(4))));
        }
        let r_id = u32::from_be_bytes([d[8], d[9], d[10], d[11]]);
        let first = self.rx.is_none() || seq_in == 0;
        let (payload, total) = if first {
            if d.len() < 14 || len < 10 {
                return Err(io("short first BSEND fragment"));
            }
            (&d[14..4 + len], u16::from_be_bytes([d[12], d[13]]) as usize)
        } else {
            if len < 8 {
                return Err(io("short BSEND fragment"));
            }
            (&d[12..4 + len], 0)
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
            self.push(Message::Block(Block { r_id: rx.r_id, data: rx.data }));
        }
        Ok(())
    }

    fn push(&mut self, m: Message) {
        if self.done.len() >= MAX_PENDING {
            // nobody calls recv(): keep the newest messages
            self.done.pop_front();
            self.dropped += 1;
        }
        self.done.push_back(m);
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
            if let Some(u) = parse_usend(&f)? {
                self.push(Message::Usend(u));
                if !want_response {
                    return Ok(None);
                }
                continue;
            }
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

    /// The next block of a PLC BSEND or USEND, waiting up to `wait`. None:
    /// nothing complete in that time.
    pub fn recv(&mut self, wait: Duration) -> Result<Option<Message>> {
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

    /// Sends a USEND with these areas (SD_1..SD_4) to the PLC URCV with this
    /// R_ID. Unacknowledged: the PLC gives no answer, also when no URCV takes
    /// it. The areas are sent with their transport size (0x04: bytes).
    pub fn usend(&mut self, r_id: u32, areas: &[Area]) -> Result<()> {
        if areas.is_empty() || areas.len() > MAX_AREAS {
            return Err(local(format!("USEND with {} areas: 1..={MAX_AREAS}", areas.len())));
        }
        let mut d = vec![0xFF, 0x02];
        d.extend_from_slice(&(areas.len() as u16).to_be_bytes());
        for (i, a) in areas.iter().enumerate() {
            if a.data.is_empty() {
                return Err(local(format!("USEND area {} is empty", i + 1)));
            }
            let len = match a.tsize {
                0x03..=0x05 => a.data.len().checked_mul(8),
                _ => Some(a.data.len()),
            }
            .filter(|n| *n <= u16::MAX as usize)
            .ok_or_else(|| local(format!("USEND area {} too long ({} bytes)", i + 1, a.data.len())))?;
            d.extend_from_slice(&[0x00, a.tsize]);
            d.extend_from_slice(&(len as u16).to_be_bytes());
            d.extend_from_slice(&a.data);
            if a.data.len() % 2 == 1 && i + 1 < areas.len() {
                d.push(0x00);
            }
        }
        let pdu = self.conn.max_pdu as usize;
        if 10 + 10 + d.len() > pdu {
            return Err(local(format!(
                "USEND of {} bytes does not fit in the PDU ({pdu}): at most {} bytes of data",
                d.len(),
                pdu.saturating_sub(24 + 4 * areas.len())
            )));
        }
        let mut p = vec![0x00, 0x01, 0x12, 0x06, 0x13, 0x00];
        p.extend_from_slice(&r_id.to_be_bytes());
        self.conn.send_s7(0x07, &p, &d)
    }

    fn conn_timeout(&self) -> Duration {
        self.conn.timeout()
    }
}
