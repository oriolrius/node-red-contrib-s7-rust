//! S7comm: block information and CPU clock payloads.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| s7::fuzzing::block_and_clock(data));
