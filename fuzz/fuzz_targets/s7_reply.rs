//! S7comm: one TPKT body from the CPU (COTP, S7 header, ReadVar data, Userdata).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| s7::fuzzing::reply(data));
