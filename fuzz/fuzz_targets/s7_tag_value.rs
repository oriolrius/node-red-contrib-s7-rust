//! S7comm: tag text (three address syntaxes) and the value decoded from PLC bytes.
//! Input: <tag> 0x00 <bytes>.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (tag, bytes) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    s7::fuzzing::tag_value(&String::from_utf8_lossy(tag), bytes);
});
