//! S7CommPlus: value, object and item address encodings.
#![no_main]
use libfuzzer_sys::fuzz_target;
use s7plus::wire::Reader;
use s7plus::{ItemAddress, Object, Value};

fuzz_target!(|data: &[u8]| {
    let _ = Value::decode(&mut Reader::new(data));
    let _ = s7plus::value::decode_value(&mut Reader::new(data));
    let _ = Object::decode(&mut Reader::new(data));
    let _ = ItemAddress::decode(&mut Reader::new(data));
});
