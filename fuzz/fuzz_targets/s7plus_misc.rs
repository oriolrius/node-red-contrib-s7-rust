//! S7CommPlus: notifications, alarm info, text libraries, access strings, comment blobs.
#![no_main]
use libfuzzer_sys::fuzz_target;
use s7plus::{decode_notification, parse_text_library, Dictionaries, ItemAddress};

fuzz_target!(|data: &[u8]| {
    let _ = decode_notification(data);
    let _ = s7plus::alarms::decode_hmi_info(data);
    let _ = ItemAddress::parse_access_string(&String::from_utf8_lossy(data));
    let _ = Dictionaries::new().decompress_blob(data, 0);
    // four parts: <len8> each for the first three, the rest is the string table
    let mut parts = Vec::new();
    let mut rest = data;
    for _ in 0..3 {
        let Some((&n, tail)) = rest.split_first() else { break };
        let n = (n as usize).min(tail.len());
        parts.push(&tail[..n]);
        rest = &tail[n..];
    }
    if parts.len() == 3 {
        let _ = parse_text_library(parts[0], parts[1], parts[2], rest);
    }
});
