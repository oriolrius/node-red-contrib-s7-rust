//! PDU reassembly from COTP DT TPDUs.

use crate::error::{Error, Result};
use crate::textfmt::hex;

/// A complete S7CommPlus PDU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pdu {
    pub version: u8,
    pub data: Vec<u8>,
}

/// Rebuilds S7CommPlus PDUs from COTP DT payloads of one direction. A PDU is
/// 0x72 <ver> <len16> <data> and, on its last fragment, the trailer
/// 0x72 <ver> 00 00. System events (ver 0xfe) have no trailer.
#[derive(Debug, Default, Clone)]
pub struct Reassembler {
    cotp: Vec<u8>,
    data: Vec<u8>,
    ver: u8,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one COTP TPDU (starting at the COTP length byte). Returns the
    /// complete PDUs.
    pub fn push(&mut self, tpdu: &[u8]) -> Result<Vec<Pdu>> {
        if tpdu.len() < 3 || tpdu[1] != 0xf0 {
            return Ok(Vec::new()); // not DT
        }
        self.cotp.extend_from_slice(&tpdu[3..]);
        if tpdu[2] & 0x80 == 0 {
            return Ok(Vec::new());
        }
        let b = std::mem::take(&mut self.cotp);
        if b.len() < 4 || b[0] != 0x72 {
            return Err(Error::proto(format!("not an S7CommPlus PDU: {}", hex(&b[..b.len().min(8)]))));
        }
        let ver = b[1];
        let n = (b[2] as usize) << 8 | b[3] as usize;
        if 4 + n > b.len() {
            return Err(Error::proto(format!("PDU length {} > {}", n, b.len() - 4)));
        }
        self.data.extend_from_slice(&b[4..4 + n]);
        let rest = &b[4 + n..];
        if ver == 0xfe || (rest.len() >= 4 && rest[0] == 0x72) {
            let p = Pdu { version: ver, data: std::mem::take(&mut self.data) };
            return Ok(vec![p]);
        }
        self.ver = ver;
        Ok(Vec::new())
    }
}
