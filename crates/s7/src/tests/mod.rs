//! Tests of the crate: codec, fake CPU server, extensions, features and the
//! endpoint core.

mod endpoint_test;
mod ext_test;
mod features_test;
mod s7_server_test;
mod s7_test;

use std::time::Duration;

use crate::clock::encode_clock;
use crate::fakecpu::FakeCpu;
use crate::textfmt::{make_date, Zone};
use crate::wire::{Conn, DialOptions};

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// 2026-11-03 21:30:15.123 local time (a Tuesday).
pub(crate) fn fixed_clock() -> chrono::DateTime<chrono::FixedOffset> {
    make_date(2026, 11, 3, 21, 30, 15, 123_000_000, Zone::Local)
}

pub(crate) fn start_fake(pdu: u16) -> (FakeCpu, String) {
    let f = FakeCpu::new(pdu);
    f.set_clock(&encode_clock(&fixed_clock()));
    let addr = f.start();
    (f, addr)
}

pub(crate) fn opts(addr: &str) -> DialOptions {
    DialOptions { addr: addr.to_string(), rack: 0, slot: 1, link_type: 0x03, timeout: Duration::from_secs(2), ..Default::default() }
}

pub(crate) fn dial_fake(pdu: u16) -> (Conn, FakeCpu) {
    let (f, addr) = start_fake(pdu);
    let cn = Conn::dial(&opts(&addr)).expect("dial");
    (cn, f)
}

mod partner_test;
