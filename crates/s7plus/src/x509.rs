//! A small X.509 reader for the PLC certificate: subject, issuer, validity
//! and subject alternative names. It only reads fields for display; the
//! verification is done by rustls (webpki).

use std::net::{Ipv4Addr, Ipv6Addr};

use crate::tls::{fingerprint, fingerprint_text};

/// Fields of a certificate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertInfo {
    /// "CN=PLC-1/Communication-1, OU=..., O=Siemens, C=DE"
    pub subject: String,
    pub issuer: String,
    /// ISO 8601 UTC ("2026-11-11T18:59:11Z").
    pub not_before: String,
    pub not_after: String,
    /// "IP:192.168.0.1", "DNS:plc1"
    pub alt_names: Vec<String>,
    /// SHA-256 of the DER, "AB:9F:..."
    pub fingerprint: String,
    /// The subject is the issuer.
    pub self_signed: bool,
    /// basicConstraints CA:TRUE.
    pub ca: bool,
}

struct Tlv<'a> {
    tag: u8,
    body: &'a [u8],
}

/// Reads one DER TLV; returns it and the rest.
fn tlv(b: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let tag = *b.first()?;
    let l0 = *b.get(1)? as usize;
    let (len, hdr) = if l0 < 0x80 {
        (l0, 2)
    } else {
        let n = l0 & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = len << 8 | *b.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let end = hdr.checked_add(len)?;
    if b.len() < end {
        return None;
    }
    Some((Tlv { tag, body: &b[hdr..end] }, &b[end..]))
}

fn items(mut b: &[u8]) -> Vec<Tlv<'_>> {
    let mut out = Vec::new();
    while let Some((t, rest)) = tlv(b) {
        out.push(t);
        b = rest;
    }
    out
}

fn oid_text(b: &[u8]) -> String {
    let mut parts = Vec::new();
    if let Some(f) = b.first() {
        parts.push((f / 40) as u64);
        parts.push((f % 40) as u64);
    }
    let mut v = 0u64;
    for &x in b.iter().skip(1) {
        v = v << 7 | (x & 0x7f) as u64;
        if x & 0x80 == 0 {
            parts.push(v);
            v = 0;
        }
    }
    parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".")
}

fn name_text(b: &[u8]) -> String {
    let mut out = Vec::new();
    for set in items(b) {
        for atv in items(set.body) {
            let f = items(atv.body);
            if f.len() < 2 {
                continue;
            }
            let key = match oid_text(f[0].body).as_str() {
                "2.5.4.3" => "CN".to_string(),
                "2.5.4.6" => "C".into(),
                "2.5.4.7" => "L".into(),
                "2.5.4.8" => "ST".into(),
                "2.5.4.10" => "O".into(),
                "2.5.4.11" => "OU".into(),
                "2.5.4.5" => "serialNumber".into(),
                o => o.to_string(),
            };
            let val = match f[1].tag {
                0x1e => {
                    // BMPString (UTF-16 BE)
                    let u: Vec<u16> = f[1].body.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                    String::from_utf16_lossy(&u)
                }
                _ => String::from_utf8_lossy(f[1].body).into_owned(),
            };
            out.push(format!("{key}={val}"));
        }
    }
    out.reverse(); // most specific first, like OpenSSL's RFC 2253 output
    out.join(", ")
}

fn time_text(t: &Tlv<'_>) -> String {
    let s = String::from_utf8_lossy(t.body);
    if !s.is_ascii() {
        return s.into_owned();
    }
    let s = s.trim_end_matches('Z');
    let full = match t.tag {
        0x17 if s.len() >= 12 => {
            // UTCTime YYMMDDHHMMSS: 50..99 = 19xx
            let yy: u32 = s[..2].parse().unwrap_or(0);
            format!("{}{}", if yy >= 50 { 19 } else { 20 }, s)
        }
        _ => s.to_string(),
    };
    if full.len() < 14 {
        return full;
    }
    format!("{}-{}-{}T{}:{}:{}Z", &full[..4], &full[4..6], &full[6..8], &full[8..10], &full[10..12], &full[12..14])
}

fn alt_names(b: &[u8]) -> Vec<String> {
    let Some((seq, _)) = tlv(b) else { return Vec::new() };
    items(seq.body)
        .iter()
        .filter_map(|n| match n.tag {
            0x82 => Some(format!("DNS:{}", String::from_utf8_lossy(n.body))),
            0x86 => Some(format!("URI:{}", String::from_utf8_lossy(n.body))),
            0x87 if n.body.len() == 4 => Some(format!("IP:{}", Ipv4Addr::new(n.body[0], n.body[1], n.body[2], n.body[3]))),
            0x87 if n.body.len() == 16 => {
                let mut a = [0u8; 16];
                a.copy_from_slice(n.body);
                Some(format!("IP:{}", Ipv6Addr::from(a)))
            }
            _ => None,
        })
        .collect()
}

/// Reads the fields of a DER certificate (None when it is not one).
pub fn parse(der: &[u8]) -> Option<CertInfo> {
    let (cert, _) = tlv(der)?;
    let (tbs, _) = tlv(cert.body)?;
    let mut f = items(tbs.body).into_iter();
    let mut t = f.next()?;
    if t.tag == 0xa0 {
        t = f.next()?; // version
    }
    let _serial = t;
    let _sig = f.next()?;
    let issuer = f.next()?;
    let validity = f.next()?;
    let subject = f.next()?;
    let _spki = f.next()?;
    let v = items(validity.body);
    let mut info = CertInfo {
        subject: name_text(subject.body),
        issuer: name_text(issuer.body),
        not_before: v.first().map(time_text).unwrap_or_default(),
        not_after: v.get(1).map(time_text).unwrap_or_default(),
        fingerprint: fingerprint_text(&fingerprint(der)),
        self_signed: subject.body == issuer.body,
        ..Default::default()
    };
    for x in f {
        if x.tag != 0xa3 {
            continue;
        }
        let Some((exts, _)) = tlv(x.body) else { continue };
        for ext in items(exts.body) {
            let e = items(ext.body);
            let (Some(id), Some(val)) = (e.first(), e.last()) else { continue };
            match oid_text(id.body).as_str() {
                "2.5.29.17" => info.alt_names = alt_names(val.body),
                "2.5.29.19" => {
                    // BasicConstraints ::= SEQUENCE { cA BOOLEAN DEFAULT FALSE, ... }
                    if let Some((bc, _)) = tlv(val.body) {
                        info.ca = items(bc.body).first().is_some_and(|b| b.tag == 0x01 && b.body.first().is_some_and(|x| *x != 0));
                    }
                }
                _ => {}
            }
        }
    }
    Some(info)
}

/// notBefore and notAfter of a DER certificate as Unix seconds.
pub fn validity(der: &[u8]) -> Option<(u64, u64)> {
    let i = parse(der)?;
    Some((iso_secs(&i.not_before)?, iso_secs(&i.not_after)?))
}

/// "YYYY-MM-DDTHH:MM:SSZ" to Unix seconds.
fn iso_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 20 || !s.is_ascii() {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // days from civil (H. Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A DER certificate as PEM.
pub fn pem(der: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for c in der.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                b64.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                b64.push('=');
            }
        }
    }
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap_or(""));
        out.push('\n');
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}
