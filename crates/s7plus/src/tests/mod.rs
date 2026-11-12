//! Tests of the crate: codec, tag descriptions, golden frames and the client
//! against the fake PLC. Tests that need the fake PLC model run with
//! `--features fake`.

mod cert;
mod codec;
mod golden;
mod tagdescr;

#[cfg(feature = "fake")]
mod fake_client;

/// Hex string to bytes (panics on bad input).
pub(crate) fn h(s: &str) -> Vec<u8> {
    crate::textfmt::unhex(s).expect("bad hex")
}

/// Strips COTP (3 bytes), the S7+ header (4) and trailer (4) of a
/// single-fragment frame.
pub(crate) fn s7data(frame: &str) -> Vec<u8> {
    let b = h(frame);
    b[7..b.len() - 4].to_vec()
}
