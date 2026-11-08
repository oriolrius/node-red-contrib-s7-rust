//! S7CommPlus: COTP payloads -> PDU reassembly -> message -> objects and values.
#![no_main]
use libfuzzer_sys::fuzz_target;
use s7plus::stream::Reassembler;
use s7plus::wire::Reader;
use s7plus::{Message, Object, Value};

fn message(version: u8, data: &[u8]) {
    if let Ok(m) = Message::parse(version, data) {
        let _ = Object::decode(&mut Reader::new(&m.body));
        let _ = Value::decode(&mut Reader::new(&m.body));
    }
}

fuzz_target!(|data: &[u8]| {
    // the input as a stream of TPDUs: <len8> <payload> ...
    let mut re = Reassembler::new();
    let mut rest = data;
    while let Some((&n, tail)) = rest.split_first() {
        let n = (n as usize).min(tail.len());
        if let Ok(pdus) = re.push(&tail[..n]) {
            for p in pdus {
                message(p.version, &p.data);
            }
        }
        rest = &tail[n..];
    }
    if let Some((&v, d)) = data.split_first() {
        message(v, d);
    }
});
