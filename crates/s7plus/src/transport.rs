//! TPKT/COTP framing over TCP and the TLS 1.3 tunnel inside COTP.
//!
//! Reads are buffered: a timeout never loses a partial TPKT, a partial TLS
//! record or a partial S7CommPlus fragment, so short timeouts (e.g. while
//! polling for notifications) are safe.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Instant;

use crate::error::{Error, Result};
use crate::textfmt::hex;

/// Maximum COTP TPDU size negotiated in the CR (1024).
pub(crate) const MAX_TPDU: usize = 1024;

/// Wraps a payload in a TPKT header.
pub(crate) fn tpkt(payload: &[u8]) -> Vec<u8> {
    let n = 4 + payload.len();
    let mut b = Vec::with_capacity(n);
    b.extend_from_slice(&[3, 0, (n >> 8) as u8, n as u8]);
    b.extend_from_slice(payload);
    b
}

/// COTP connection request with the remote TSAP string.
pub(crate) fn cotp_cr(tsap: &str) -> Vec<u8> {
    let mut p = vec![
        0xe0, 0, 0, 0, 1, 0, // CR, dst ref 0, src ref 1, class 0
        0xc0, 1, 0x0a, // TPDU size 1024
        0xc1, 2, 0x06, 0x00, // local TSAP 0x0600 (as TIA / HMI)
    ];
    p.push(0xc2);
    p.push(tsap.len() as u8);
    p.extend_from_slice(tsap.as_bytes());
    let mut out = vec![p.len() as u8];
    out.extend_from_slice(&p);
    out
}

/// Buffered TPKT reader/writer on a TCP stream with a deadline.
#[derive(Debug)]
pub(crate) struct Tpkt {
    pub tcp: TcpStream,
    rbuf: Vec<u8>,
    /// Pending DT payload (TLS mode: the byte stream carried by COTP).
    dt: Vec<u8>,
    deadline: Option<Instant>,
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "timeout")
}

impl Tpkt {
    pub fn new(tcp: TcpStream) -> Self {
        Tpkt { tcp, rbuf: Vec::new(), dt: Vec::new(), deadline: None }
    }

    pub fn set_deadline(&mut self, d: Option<Instant>) {
        self.deadline = d;
    }

    fn remaining(&self) -> io::Result<Option<std::time::Duration>> {
        match self.deadline {
            None => Ok(None),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return Err(timed_out());
                }
                Ok(Some((d - now).max(std::time::Duration::from_millis(1))))
            }
        }
    }

    fn fill(&mut self) -> io::Result<()> {
        let rem = self.remaining()?;
        self.tcp.set_read_timeout(rem)?;
        let mut buf = [0u8; 8192];
        loop {
            match self.tcp.read(&mut buf) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed")),
                Ok(n) => {
                    self.rbuf.extend_from_slice(&buf[..n]);
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(timed_out()),
                Err(e) => return Err(e),
            }
        }
    }

    /// Returns the payload of the next TPKT (from the COTP length byte).
    pub fn read_tpkt(&mut self) -> io::Result<Vec<u8>> {
        loop {
            if self.rbuf.len() >= 4 {
                let n = (self.rbuf[2] as usize) << 8 | self.rbuf[3] as usize;
                if self.rbuf[0] != 3 || n < 7 {
                    let h = hex(&self.rbuf[..4]);
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!("bad TPKT {h}")));
                }
                if self.rbuf.len() >= n {
                    let b = self.rbuf[4..n].to_vec();
                    self.rbuf.drain(..n);
                    return Ok(b);
                }
            }
            self.fill()?;
        }
    }

    pub fn write_all(&mut self, b: &[u8]) -> io::Result<()> {
        let rem = self.remaining()?;
        self.tcp.set_write_timeout(rem)?;
        self.tcp.write_all(b)
    }
}

/// io::Read/Write view of the COTP DT byte stream (rustls runs on it): one
/// write = one or more DT TPDUs (EOT on the last), read returns DT payloads.
pub(crate) struct CotpIo<'a> {
    pub tp: &'a mut Tpkt,
}

impl Read for CotpIo<'_> {
    fn read(&mut self, p: &mut [u8]) -> io::Result<usize> {
        while self.tp.dt.is_empty() {
            let b = self.tp.read_tpkt()?;
            if b.len() < 3 || b[1] != 0xf0 {
                continue; // not DT
            }
            self.tp.dt = b[3..].to_vec();
        }
        let n = p.len().min(self.tp.dt.len());
        p[..n].copy_from_slice(&self.tp.dt[..n]);
        self.tp.dt.drain(..n);
        Ok(n)
    }
}

impl Write for CotpIo<'_> {
    fn write(&mut self, p: &[u8]) -> io::Result<usize> {
        let max = MAX_TPDU - 3;
        let mut n = 0;
        let mut first = true;
        while first || n < p.len() {
            first = false;
            let k = (p.len() - n).min(max);
            let eot = if n + k == p.len() { 0x80 } else { 0 };
            let mut tp = vec![0x02, 0xf0, eot];
            tp.extend_from_slice(&p[n..n + k]);
            self.tp.write_all(&tpkt(&tp))?;
            n += k;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct TlsState {
    conn: rustls::Connection,
    plain: Vec<u8>,
}

/// The S7CommPlus link: plain COTP, or TLS inside COTP after InitSsl.
pub(crate) struct Link {
    pub tp: Tpkt,
    tls: Option<Box<TlsState>>,
}

/// Parses one S7+ fragment of the TLS plaintext stream:
/// 72 ver len16 data [72 ver 00 00]. Returns it rebuilt as a COTP DT TPDU
/// for the Reassembler, or None when more bytes are needed.
pub(crate) fn parse_s7_fragment(plain: &mut Vec<u8>, eof: bool) -> Result<Option<Vec<u8>>> {
    if plain.len() < 4 {
        return Ok(None);
    }
    if plain[0] != 0x72 {
        return Err(Error::proto(format!("TLS stream out of sync ({})", hex(&plain[..4]))));
    }
    let ver = plain[1];
    let n = (plain[2] as usize) << 8 | plain[3] as usize;
    if plain.len() < 4 + n {
        return Ok(None);
    }
    let mut take = 4 + n;
    if ver != 0xfe {
        if plain.len() < 4 + n + 4 && !eof {
            return Ok(None); // wait for the trailer or the next header
        }
        let t = &plain[4 + n..];
        if t.len() >= 4 && t[0] == 0x72 && t[1] == ver && t[2] == 0 && t[3] == 0 {
            take += 4;
        }
    }
    let mut out = Vec::with_capacity(3 + take);
    out.extend_from_slice(&[0x02, 0xf0, 0x80]);
    out.extend_from_slice(&plain[..take]);
    plain.drain(..take);
    Ok(Some(out))
}

impl Link {
    pub fn new(tcp: TcpStream) -> Self {
        Link { tp: Tpkt::new(tcp), tls: None }
    }

    pub fn is_tls(&self) -> bool {
        self.tls.is_some()
    }

    pub fn set_deadline(&mut self, d: Option<Instant>) {
        self.tp.set_deadline(d);
    }

    /// Runs the TLS handshake inside COTP.
    pub fn start_tls(&mut self, mut conn: rustls::Connection) -> Result<()> {
        let mut io = CotpIo { tp: &mut self.tp };
        while conn.is_handshaking() {
            conn.complete_io(&mut io).map_err(tls_io_err)?;
        }
        while conn.wants_write() {
            conn.write_tls(&mut io)?;
        }
        self.tls = Some(Box::new(TlsState { conn, plain: Vec::new() }));
        Ok(())
    }

    /// Fake PLC: true when the next COTP DT frame starts a TLS record (0x16).
    /// The frame stays in the read buffer for the next read.
    #[cfg(feature = "fake")]
    pub fn next_is_tls(&mut self) -> Result<bool> {
        let b = self.tp.read_tpkt()?;
        let tls = b.len() > 3 && b[1] == 0xf0 && b[3] == 0x16;
        let mut raw = tpkt(&b);
        raw.extend_from_slice(&self.tp.rbuf);
        self.tp.rbuf = raw;
        Ok(tls)
    }

    /// The PLC certificate (DER) of the TLS session.
    pub fn peer_certificate(&self) -> Option<Vec<u8>> {
        let st = self.tls.as_ref()?;
        st.conn.peer_certificates()?.first().map(|c| c.to_vec())
    }

    /// TLS exporter (RFC 8446 §7.5) with an empty context.
    pub fn export_keying_material(&self, label: &[u8], n: usize) -> Option<Vec<u8>> {
        let st = self.tls.as_ref()?;
        let mut out = vec![0u8; n];
        st.conn.export_keying_material(&mut out, label, None).ok()?;
        Some(out)
    }

    /// Returns one COTP TPDU (from the COTP length byte).
    pub fn read_tpdu(&mut self) -> Result<Vec<u8>> {
        let Some(st) = self.tls.as_mut() else {
            return Ok(self.tp.read_tpkt()?);
        };
        let mut eof = false;
        loop {
            if let Some(f) = parse_s7_fragment(&mut st.plain, eof)? {
                return Ok(f);
            }
            if eof {
                return Err(Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "TLS stream closed")));
            }
            let mut io = CotpIo { tp: &mut self.tp };
            let n = st.conn.read_tls(&mut io)?;
            if n == 0 {
                eof = true;
                continue;
            }
            let state = st.conn.process_new_packets();
            if let Err(e) = state {
                let _ = st.conn.write_tls(&mut io);
                return Err(e.into());
            }
            let mut buf = [0u8; 8192];
            loop {
                match st.conn.reader().read(&mut buf) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(k) => st.plain.extend_from_slice(&buf[..k]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
            while st.conn.wants_write() {
                st.conn.write_tls(&mut io)?;
            }
        }
    }

    /// Sends one S7+ fragment (header, data, trailer).
    pub fn write_fragment(&mut self, p: &[u8]) -> Result<()> {
        match self.tls.as_mut() {
            None => {
                let mut tp = vec![0x02, 0xf0, 0x80];
                tp.extend_from_slice(p);
                self.tp.write_all(&tpkt(&tp))?;
            }
            Some(st) => {
                st.conn.writer().write_all(p)?;
                let mut io = CotpIo { tp: &mut self.tp };
                while st.conn.wants_write() {
                    st.conn.write_tls(&mut io)?;
                }
            }
        }
        Ok(())
    }

    /// Writes raw TPKT bytes (COTP connection setup).
    pub fn write_raw(&mut self, b: &[u8]) -> Result<()> {
        Ok(self.tp.write_all(b)?)
    }

    /// Reads one raw TPKT payload (COTP connection setup).
    pub fn read_raw(&mut self) -> Result<Vec<u8>> {
        Ok(self.tp.read_tpkt()?)
    }

    /// Sends close_notify (TLS) and shuts the socket down.
    pub fn shutdown(&mut self) {
        if let Some(st) = self.tls.as_mut() {
            st.conn.send_close_notify();
            let mut io = CotpIo { tp: &mut self.tp };
            while st.conn.wants_write() {
                if st.conn.write_tls(&mut io).is_err() {
                    break;
                }
            }
        }
        let _ = self.tp.tcp.shutdown(Shutdown::Both);
    }
}

fn tls_io_err(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::InvalidData {
        return Error::Tls(e.to_string());
    }
    e.into()
}
