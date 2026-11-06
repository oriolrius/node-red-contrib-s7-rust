//! Primitive field codec: big-endian integers and the
//! S7CommPlus variable-length quantities (VLQ).

use crate::error::Error;

/// Decodes S7CommPlus primitive fields from a byte slice. A short
/// buffer sets `err` and the getters return 0.
#[derive(Debug)]
pub struct Reader<'a> {
    pub b: &'a [u8],
    pub pos: usize,
    pub err: Option<Error>,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0, err: None }
    }

    pub(crate) fn fail(&mut self) {
        if self.err.is_none() {
            self.err = Some(Error::Short);
        }
    }

    pub(crate) fn set_err(&mut self, e: Error) {
        self.err = Some(e);
    }

    /// True while no error happened.
    pub fn ok(&self) -> bool {
        self.err.is_none()
    }

    /// Takes the error (if any) as a result.
    pub fn result(&mut self) -> crate::Result<()> {
        match self.err.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Number of unread bytes.
    pub fn left(&self) -> usize {
        self.b.len().saturating_sub(self.pos)
    }

    /// The unread bytes.
    pub fn rest(&self) -> &'a [u8] {
        &self.b[self.pos.min(self.b.len())..]
    }

    pub fn byte(&mut self) -> u8 {
        if self.pos >= self.b.len() {
            self.fail();
            return 0;
        }
        let b = self.b[self.pos];
        self.pos += 1;
        b
    }

    pub fn peek(&self) -> u8 {
        if self.pos >= self.b.len() {
            return 0;
        }
        self.b[self.pos]
    }

    /// Returns n bytes, or None (and sets the error) when the buffer is short.
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n > self.b.len() {
            self.fail();
            self.pos = self.b.len();
            return None;
        }
        let b = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Some(b)
    }

    pub fn u16(&mut self) -> u16 {
        self.bytes(2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0)
    }

    pub fn u32(&mut self) -> u32 {
        self.bytes(4).map(|b| u32::from_be_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    pub fn u64(&mut self) -> u64 {
        self.bytes(8).map(|b| u64::from_be_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    pub(crate) fn le_u16(&mut self) -> u16 {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
    }

    pub(crate) fn le_u32(&mut self) -> u32 {
        self.bytes(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    /// Unsigned VLQ (7 bits per octet, MSB = more).
    pub fn vlq32(&mut self) -> u32 {
        let mut v: u32 = 0;
        for _ in 0..5 {
            let o = self.byte();
            v = (v << 7) | (o & 0x7f) as u32;
            if o & 0x80 == 0 {
                break;
            }
        }
        v
    }

    /// Signed VLQ: bit 6 of the first octet is the sign.
    pub fn svlq32(&mut self) -> i32 {
        let mut v: i32 = 0;
        for i in 0..5 {
            let mut o = self.byte();
            if i == 0 && o & 0x40 != 0 {
                o &= !0x40;
                v = -64;
            } else {
                v = v.wrapping_shl(7);
            }
            v = v.wrapping_add((o & 0x7f) as i32);
            if o & 0x80 == 0 {
                break;
            }
        }
        v
    }

    /// Unsigned 64-bit VLQ: after 8 continued octets the 9th has 8 bits.
    pub fn vlq64(&mut self) -> u64 {
        let mut v: u64 = 0;
        let mut cont = false;
        for _ in 0..8 {
            let o = self.byte();
            v = (v << 7) | (o & 0x7f) as u64;
            cont = o & 0x80 != 0;
            if !cont {
                break;
            }
        }
        if cont {
            v = (v << 8) | self.byte() as u64;
        }
        v
    }

    /// Signed 64-bit VLQ.
    pub fn svlq64(&mut self) -> i64 {
        let mut v: i64 = 0;
        let mut cont = false;
        for i in 0..8 {
            let mut o = self.byte();
            if i == 0 && o & 0x40 != 0 {
                o &= !0x40;
                v = -64;
            } else {
                v = v.wrapping_shl(7);
            }
            v = v.wrapping_add((o & 0x7f) as i64);
            cont = o & 0x80 != 0;
            if !cont {
                break;
            }
        }
        if cont {
            v = v.wrapping_shl(8) | self.byte() as i64;
        }
        v
    }
}

/// Encodes S7CommPlus primitive fields.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    pub b: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { b: Vec::new() }
    }
    pub fn byte(&mut self, b: u8) {
        self.b.push(b);
    }
    pub fn raw(&mut self, b: &[u8]) {
        self.b.extend_from_slice(b);
    }
    pub fn u16(&mut self, v: u16) {
        self.raw(&v.to_be_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.raw(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.raw(&v.to_be_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    pub(crate) fn le_u16(&mut self, v: u16) {
        self.raw(&v.to_le_bytes());
    }
    pub(crate) fn le_u32(&mut self, v: u32) {
        self.raw(&v.to_le_bytes());
    }

    /// Unsigned VLQ in the shortest form.
    pub fn vlq32(&mut self, v: u32) {
        self.vlq64(v as u64);
    }

    /// Unsigned 64-bit VLQ (9th octet carries 8 bits).
    pub fn vlq64(&mut self, mut v: u64) {
        let mut tmp = [0u8; 9];
        if v > 0x00ff_ffff_ffff_ffff {
            tmp[8] = v as u8;
            v >>= 8;
            for i in (0..=7).rev() {
                tmp[i] = (v & 0x7f) as u8 | 0x80;
                v >>= 7;
            }
            self.raw(&tmp);
            return;
        }
        let mut n = 0;
        loop {
            tmp[8 - n] = (v & 0x7f) as u8;
            if n > 0 {
                tmp[8 - n] |= 0x80;
            }
            n += 1;
            v >>= 7;
            if v == 0 {
                break;
            }
        }
        let start = 9 - n;
        self.b.extend_from_slice(&tmp[start..]);
    }

    /// Signed VLQ (compact form, sign in bit 6 of the first octet).
    pub fn svlq32(&mut self, v: i32) {
        self.svlq64(v as i64);
    }

    /// Signed 64-bit VLQ in the compact form.
    pub fn svlq64(&mut self, mut v: i64) {
        let mut tmp = [0u8; 10];
        let mut n = 0;
        loop {
            tmp[9 - n] = (v & 0x7f) as u8;
            if n > 0 {
                tmp[9 - n] |= 0x80;
            }
            n += 1;
            let rest = v >> 6;
            if rest == 0 || rest == -1 {
                break;
            }
            v >>= 7;
        }
        let start = 10 - n;
        self.b.extend_from_slice(&tmp[start..]);
    }
}
