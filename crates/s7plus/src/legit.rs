//! Legitimation (password login): legacy SHA1-XOR and the new AES-256-CBC
//! payload with the TLS-exported OMS secret.

use std::fmt;

use aes::cipher::{BlockEncrypt, KeyInit};

use crate::client::Client;
use crate::error::{Error, Result};
use crate::ids::*;
use crate::plcvalue::bytes_of;
use crate::value::{Struct, Value};
use crate::wire::Writer;

// Access levels (EffectiveProtectionLevel 1842 of the session).
pub const ACCESS_FULL: u32 = 1;
pub const ACCESS_READ: u32 = 2;
pub const ACCESS_HMI: u32 = 3;
pub const ACCESS_NONE: u32 = 4;

// Legitimation payload types.
pub const LEGIT_LEGACY: u32 = 1;
pub const LEGIT_NEW: u32 = 2;

/// The legitimation method chosen from the PAOM string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegitMode {
    Unsupported,
    /// SHA1(password) XOR challenge -> 304.
    Legacy,
    /// AES-256-CBC payload -> 1846 (needs TLS).
    New,
}

impl fmt::Display for LegitMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LegitMode::Unsupported => "unsupported",
            LegitMode::Legacy => "legacy",
            LegitMode::New => "new",
        })
    }
}

/// Equivalent of the regexp `(?i)^[^;]*;[^;]*[17]\s?(\d{3}).*;[VS](\d{1,2})\.(\d+)$`:
/// returns (device digits, major, minor).
fn parse_paom(p: &str) -> Option<(String, u32, u32)> {
    let first = p.find(';')?;
    let last = p.rfind(';')?;
    if last <= first {
        return None;
    }
    let tail = &p[last + 1..];
    let tb = tail.as_bytes();
    if tb.is_empty() || !matches!(tb[0], b'V' | b'v' | b'S' | b's') {
        return None;
    }
    let (maj, min) = tail[1..].split_once('.')?;
    if maj.is_empty() || maj.len() > 2 || !maj.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if min.is_empty() || !min.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let middle = &p[first + 1..last];
    let run = &middle[..middle.find(';').unwrap_or(middle.len())];
    let rb: Vec<char> = run.chars().collect();
    let mut dev = None;
    for i in (0..rb.len()).rev() {
        if rb[i] != '1' && rb[i] != '7' {
            continue;
        }
        let mut k = i + 1;
        if k < rb.len() && rb[k].is_whitespace() {
            k += 1;
        }
        if k + 3 <= rb.len() && rb[k..k + 3].iter().all(|c| c.is_ascii_digit()) {
            dev = Some(rb[k..k + 3].iter().collect::<String>());
            break;
        }
    }
    Some((dev?, maj.parse().ok()?, min.parse().ok()?))
}

/// Applies the reference's firmware rules to the ServerSessionVersion PAOM
/// string ("1;6ES7 212-1BE31-0XB0 ;V3.0"): S7-1500 V2.9..V3.0 legacy, from
/// V3.1 new; S7-1200 V4.3..V4.6 legacy, >= V4.7 new, G2 (…50-0XB0) new;
/// software controllers >= V21.9 legacy. Returns the mode and the device digits.
pub fn legitimation_mode_for(paom: &str) -> Result<(LegitMode, String)> {
    let paom = paom.trim();
    let (dev, maj, minr) = parse_paom(paom).ok_or_else(|| Error::proto(format!("cannot parse firmware from {paom:?}")))?;
    let fw = maj * 100 + minr;
    let mode = match dev.as_bytes()[0] {
        b'5' => {
            if fw < 209 {
                LegitMode::Unsupported
            } else if fw < 301 {
                LegitMode::Legacy
            } else {
                LegitMode::New
            }
        }
        b'2' if paom.contains("50-0XB0") => LegitMode::New,
        b'2' => {
            if fw < 403 {
                LegitMode::Unsupported
            } else if fw < 407 {
                LegitMode::Legacy
            } else {
                LegitMode::New
            }
        }
        b'6' => {
            if fw < 2109 {
                LegitMode::Unsupported
            } else {
                LegitMode::Legacy
            }
        }
        _ => LegitMode::Unsupported,
    };
    Ok((mode, dev))
}

pub(crate) fn sha1(b: &[u8]) -> [u8; 20] {
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, b);
    d.as_ref().try_into().unwrap()
}

pub(crate) fn sha256(b: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, b);
    d.as_ref().try_into().unwrap()
}

/// The legacy answer: SHA1(password) XOR challenge.
pub fn legacy_response(password: &str, challenge: &[u8]) -> Result<Vec<u8>> {
    let h = sha1(password.as_bytes());
    if challenge.len() != h.len() {
        return Err(Error::proto(format!("challenge of {} bytes, want 20", challenge.len())));
    }
    Ok(h.iter().zip(challenge).map(|(a, b)| a ^ b).collect())
}

/// The struct 40400 (type, username, password): with a username the new
/// login (plain password), without it the legacy payload (SHA1 of the password).
pub fn legitimation_payload(password: &str, username: &str) -> Vec<u8> {
    let mut st = Struct::new(ATTR_LEGIT_PAYLOAD_STRUCT);
    if !username.is_empty() {
        st.set(ATTR_LEGIT_PAYLOAD_TYPE, Value::udint(LEGIT_NEW));
        st.set(ATTR_LEGIT_PAYLOAD_USER, Value::blob(username.as_bytes()));
        st.set(ATTR_LEGIT_PAYLOAD_PASSWORD, Value::blob(password.as_bytes()));
    } else {
        st.set(ATTR_LEGIT_PAYLOAD_TYPE, Value::udint(LEGIT_LEGACY));
        st.set(ATTR_LEGIT_PAYLOAD_USER, Value::blob(Vec::new()));
        st.set(ATTR_LEGIT_PAYLOAD_PASSWORD, Value::blob(sha1(password.as_bytes()).to_vec()));
    }
    let mut w = Writer::new();
    Value::structv(st).encode(&mut w);
    w.b
}

/// AES-256-CBC encryption with PKCS#7 padding.
pub(crate) fn aes256_cbc_encrypt(key: &[u8; 32], iv: &[u8], pt: &[u8]) -> Vec<u8> {
    let c = aes::Aes256::new(key.into());
    let pad = 16 - pt.len() % 16;
    let mut data = pt.to_vec();
    data.resize(data.len() + pad, pad as u8);
    let mut prev: [u8; 16] = iv[..16].try_into().unwrap();
    for chunk in data.chunks_mut(16) {
        for i in 0..16 {
            chunk[i] ^= prev[i];
        }
        let blk = aes::Block::from_mut_slice(chunk);
        c.encrypt_block(blk);
        prev.copy_from_slice(chunk);
    }
    data
}

/// AES-256-CBC decryption (no padding removal); used by the fake PLC.
#[allow(dead_code)]
pub(crate) fn aes256_cbc_decrypt(key: &[u8; 32], iv: &[u8], ct: &[u8]) -> Vec<u8> {
    use aes::cipher::BlockDecrypt;
    let c = aes::Aes256::new(key.into());
    let mut prev: [u8; 16] = iv[..16].try_into().unwrap();
    let mut out = ct.to_vec();
    for chunk in out.chunks_mut(16) {
        let cur: [u8; 16] = chunk.try_into().unwrap_or([0; 16]);
        let blk = aes::Block::from_mut_slice(chunk);
        c.decrypt_block(blk);
        for i in 0..16 {
            chunk[i] ^= prev[i];
        }
        prev = cur;
    }
    out
}

/// Encrypts the payload with AES-256-CBC (PKCS#7): key = SHA256(secret),
/// IV = first 16 bytes of the challenge. Returns the cipher text and the
/// rolled secret (the key) for the next legitimation.
pub fn new_legitimation_response(payload: &[u8], secret: &[u8], challenge: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    if challenge.len() < 16 {
        return Err(Error::proto("challenge shorter than 16 bytes"));
    }
    let key = sha256(secret);
    let ct = aes256_cbc_encrypt(&key, &challenge[..16], payload);
    Ok((ct, key.to_vec()))
}

impl Client {
    /// Reads the effective protection level of the session.
    pub fn protection_level(&self) -> Result<u32> {
        let v = self.get_var_sub_streamed(self.session(), ATTR_EFF_PROT_LEVEL)?;
        v.v.as_u32().ok_or_else(|| Error::proto(format!("protection level is {v}")))
    }

    /// Reads the 20-byte legitimation challenge (session attribute 303).
    pub fn challenge(&self) -> Result<Vec<u8>> {
        let v = self.get_var_sub_streamed(self.session(), ATTR_SESS_REQUEST)?;
        bytes_of(&v).ok_or_else(|| Error::proto(format!("challenge is {v}")))
    }

    /// Logs in with a password (and a username for the new login), like the
    /// reference: only when the protection level is above full access.
    pub fn legitimate(&self, password: &str, username: &str) -> Result<()> {
        let paom = self.info().paom();
        let (mode, _) = legitimation_mode_for(&paom)?;
        if mode == LegitMode::Unsupported {
            return Err(Error::proto(format!("legitimation not supported for {paom:?}")));
        }
        let lvl = self.protection_level()?;
        if lvl <= ACCESS_FULL || password.is_empty() {
            return Ok(());
        }
        let ch = self.challenge()?;
        let session = self.session();
        let e = if mode == LegitMode::Legacy {
            let resp = legacy_response(password, &ch)?;
            self.set_variable(session, ATTR_SESS_RESPONSE, &Value::usint_array(&resp))
        } else {
            let mut c = self.conn();
            let secret =
                c.oms_secret.clone().ok_or_else(|| Error::proto("the new legitimation needs a TLS session (OMS exporter secret)"))?;
            let (ct, next) = new_legitimation_response(&legitimation_payload(password, username), &secret, &ch)?;
            c.oms_secret = Some(next);
            c.set_variable(session, ATTR_LEGITIMATE, &Value::blob(ct))
        };
        match e {
            Err(Error::Return { ret, .. }) => Err(Error::AccessDenied(ret)),
            other => other,
        }
    }
}
