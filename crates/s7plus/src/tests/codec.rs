//! Codec tests.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use super::h;
use crate::blob::{compress_with_dictionary, Dictionaries};
use crate::browse::{array_access_id, array_indexes, db_optimized};
use crate::legit::*;
use crate::plcvalue::*;
use crate::softdt::*;
use crate::stream::{Pdu, Reassembler};
use crate::textfmt::{hex, DateTime, HOUR, MILLISECOND, MINUTE, SECOND};
use crate::transport::{parse_s7_fragment, CotpIo, Tpkt};
use crate::typeinfo::*;
use crate::value::*;
use crate::wire::{Reader, Writer};
use crate::{parse_text_library, Error};

pub(crate) const TEST_DICT: &[u8] = br#"<InterfaceLineComments><Part Kind="Comments"><Comment Path="1"><DictEntry Language="en-US">"#;

#[test]
fn vlq_round_trip() {
    for v in [0u32, 1, 0x7f, 0x80, 0x3fff, 0x4000, 306, 0x8a0e0001, u32::MAX] {
        let mut w = Writer::new();
        w.vlq32(v);
        let mut r = Reader::new(&w.b);
        assert!(r.vlq32() == v && r.left() == 0, "VLQ32 {v} -> {}", hex(&w.b));
    }
    for v in [0u64, 0x7f, 0x80, 1 << 56, 0x00ff_ffff_ffff_ffff, 0x0100_0000_0000_0000, u64::MAX, 0xa2019a000145ff89] {
        let mut w = Writer::new();
        w.vlq64(v);
        let mut r = Reader::new(&w.b);
        assert!(r.vlq64() == v && r.left() == 0, "VLQ64 {v:#x} -> {}", hex(&w.b));
    }
    for v in [0i32, 1, -1, 63, 64, -64, -65, -255, 1234, -1234567, i32::MAX, i32::MIN] {
        let mut w = Writer::new();
        w.svlq32(v);
        let mut r = Reader::new(&w.b);
        assert!(r.svlq32() == v && r.left() == 0, "SVLQ32 {v} -> {}", hex(&w.b));
    }
    for v in [0i64, -1, 1 << 40, -(1 << 40), i64::MAX >> 8, i64::MIN >> 8] {
        let mut w = Writer::new();
        w.svlq64(v);
        let mut r = Reader::new(&w.b);
        assert!(r.svlq64() == v && r.left() == 0, "SVLQ64 {v} -> {}", hex(&w.b));
    }
    // Encodings documented by the PLC: -255 = fe 01, -1234567 = ff b4 d2 79.
    for (v, b) in [(-255i32, "fe01"), (-1234567, "ffb4d279")] {
        assert_eq!(Reader::new(&h(b)).svlq32(), v, "decode {b}");
        let mut w = Writer::new();
        w.svlq32(v);
        assert_eq!(hex(&w.b), b, "encode {v}");
    }
}

#[test]
fn value_round_trip() {
    let mut st = Struct::new(314);
    st.set(315, Value::udint(256));
    st.set(319, Value::wstring("x"));
    let mut sparse = BTreeMap::new();
    sparse.insert(1, V::Str("a".into()));
    sparse.insert(7, V::Str("b".into()));
    let vals = vec![
        Value::bool(true),
        Value::int(-5),
        Value::uint(65535),
        Value::dint(-1234567),
        Value::udint(8396864),
        Value::real(3.25),
        Value::lreal(2.5),
        Value::lint(-1 << 40),
        Value::ulint(1 << 60),
        Value::wstring("héllo"),
        Value::blob(vec![1, 2, 3]),
        Value::rid(0x9003_0000),
        Value::aid(2550),
        Value::timestamp(0x1234567890),
        Value::usint_array(&[0xde, 0xad]),
        Value::structv(st),
        Value { flags: FLAG_SPARSE_ARRAY, typ: T_WSTRING, v: V::Sparse(sparse) },
    ];
    for v in vals {
        let b = v.to_bytes();
        let mut r = Reader::new(&b);
        let got = Value::decode(&mut r);
        assert!(r.err.is_none() && r.left() == 0 && got.to_string() == v.to_string(), "{v} -> {} -> {got}", hex(&b));
        assert_eq!(got, v);
    }
}

#[test]
fn reassembler_fragments() {
    let mut re = Reassembler::new();
    // One PDU split into two COTP TPDUs (no EOT on the first) and a PDU split
    // into two S7+ fragments (trailer only on the last).
    assert!(re.push(&h("02f0007201000331")).unwrap().is_empty(), "PDU before EOT");
    let p2 = re.push(&h("02f080aabb72010000")).unwrap();
    assert_eq!(p2.len(), 1);
    assert_eq!(hex(&p2[0].data), "31aabb");
    assert!(re.push(&h("02f0807202000231aa")).unwrap().is_empty(), "fragment without trailer returned");
    let p4 = re.push(&h("02f08072020001bb72020000")).unwrap();
    assert_eq!(p4.len(), 1);
    assert_eq!(hex(&p4[0].data), "31aabb");
}

#[test]
fn offset_info_round_trip() {
    // all offset-info kinds survive encode/decode
    for k in 0..16u8 {
        let oi = OffsetInfo {
            kind: k,
            info1: 3,
            info2: 5,
            opt_addr: 7,
            non_opt_addr: 9,
            lower_bound: -2,
            count: 4,
            m_lower: [-1, 2, 0, 0, 0, 0],
            m_count: [2, 2, 0, 0, 0, 0],
            non_opt_size: 11,
            opt_size: 12,
            relation_id: 0x9100_0099,
            info4: [1, 2, 3, 4],
            retain_off: 13,
            volatile_off: 14,
            classic_size: 15,
            retain_size: 16,
            volatile_sz: 17,
        };
        let mut w = Writer::new();
        encode_offset_info(&mut w, &oi);
        let got = decode_offset_info(&mut Reader::new(&w.b), k);
        let mut w2 = Writer::new();
        encode_offset_info(&mut w2, &got);
        assert_eq!(w.b, w2.b, "offset info kind {k} not stable");
    }
    // the old struct-element form swaps optimized / standard offsets
    let got = decode_offset_info(&mut Reader::new(&[1, 0, 2, 0]), OI_STRUCT_ELEM_STD);
    assert!(got.non_opt_addr == 1 && got.opt_addr == 2, "StructElemStd = {got:?}");
}

#[test]
fn array_access_ids() {
    let o = OffsetInfo { kind: OI_ARRAY_MDIM, count: 6, m_lower: [1, -1, 0, 0, 0, 0], m_count: [3, 2, 0, 0, 0, 0], ..Default::default() };
    let (names, ids) = array_indexes(&o, SDT_DINT);
    assert_eq!(names.join(" "), "[-1,1] [-1,2] [-1,3] [0,1] [0,2] [0,3]");
    for (i, n) in names.iter().enumerate() {
        assert_eq!(array_access_id(&o, SDT_DINT, n), Ok(ids[i]), "{n}");
    }
    // BBool: the lowest dimension is padded to 8 ids
    let b = OffsetInfo { kind: OI_ARRAY_MDIM, count: 6, m_count: [3, 2, 0, 0, 0, 0], ..Default::default() };
    let (_, bid) = array_indexes(&b, SDT_BBOOL);
    assert_eq!(bid, vec![0, 1, 2, 8, 9, 10]);
    assert_eq!(array_access_id(&b, SDT_BBOOL, "[1,2]"), Ok(10));
    assert!(array_access_id(&o, SDT_DINT, "[1,1]").is_err(), "out of bounds accepted");
}

#[test]
fn plc_value_conversions() {
    let cases: &[(u8, &str, &str)] = &[
        (SDT_BOOL, "true", "true"),
        (SDT_BYTE, "0x7f", "127 (16#7F)"),
        (SDT_CHAR, "A", "\"A\""),
        (SDT_WORD, "4660", "4660 (16#1234)"),
        (SDT_INT, "-5", "-5"),
        (SDT_DWORD, "1", "1 (16#1)"),
        (SDT_DINT, "-1234567", "-1234567"),
        (SDT_REAL, "3.25", "3.25"),
        (SDT_DATE, "2024-02-29", "2024-02-29"),
        (SDT_TIME_OF_DAY, "12:34:56.789", "12:34:56.789000000"),
        (SDT_TIME, "T#1h2m3s", "1h2m3s"),
        (SDT_TIME, "-500", "-500ms"),
        (SDT_S5TIME, "2h46m30s", "2h46m30s"),
        (SDT_S5TIME, "9.99s", "9.99s"),
        (SDT_DATE_AND_TIME, "2023-10-05T08:09:10.123", "2023-10-05T08:09:10.123"),
        (SDT_LREAL, "1e300", "1e+300"),
        (SDT_ULINT, "18446744073709551615", "18446744073709551615"),
        (SDT_LINT, "-9000000000", "-9000000000"),
        (SDT_LWORD, "1", "1 (16#1)"),
        (SDT_USINT, "255", "255"),
        (SDT_UINT, "65535", "65535"),
        (SDT_UDINT, "4000000000", "4000000000"),
        (SDT_SINT, "-128", "-128"),
        (SDT_WCHAR, "€", "\"€\""),
        (SDT_LTIME, "1.5s", "1.5s"),
        (SDT_LTOD, "23:59:59.000000001", "23:59:59.000000001"),
        (SDT_LDT, "2024-01-02T03:04:05.123456789", "2024-01-02T03:04:05.123456789"),
        (SDT_DTL, "2024-01-02T03:04:05.5", "2024-01-02T03:04:05.5"),
    ];
    for (sdt, input, want) in cases {
        let tag = Tag::of_type(*sdt);
        let v = encode_plc(&tag, input).unwrap_or_else(|e| panic!("{} {input:?}: {e}", tag.name));
        // through the wire codec
        let b = v.to_bytes();
        let v2 = Value::decode(&mut Reader::new(&b));
        let x = decode_plc(&tag, &v2).unwrap_or_else(|e| panic!("{}: {e}", tag.name));
        assert_eq!(format_plc(&tag, &x), *want, "{} {input:?}", tag.name);
    }
    // strings keep max length and header
    let st = Tag { name: "s".into(), softdatatype: SDT_STRING, max_len: 10, ..Default::default() };
    let v = encode_plc(&st, "héllo").unwrap();
    let b = bytes_of(&v).unwrap();
    assert!(b.len() == 12 && b[0] == 10 && b[1] == 5 && b[3] == 0xe9, "STRING bytes {}", hex(&b));
    assert!(encode_plc(&st, "this is too long").is_err(), "long string accepted");
    let ws = Tag { name: "w".into(), softdatatype: SDT_WSTRING, max_len: 8, ..Default::default() };
    let v = encode_plc(&ws, "añ€").unwrap();
    assert_eq!(decode_plc(&ws, &v).unwrap(), PlcValue::String("añ€".into()));
    // S5TIME and DT byte layouts (reference examples)
    assert_eq!(s5time_to_duration(2457), 9990 * MILLISECOND);
    assert_eq!(s5time_to_duration(14745), 2 * HOUR + 46 * MINUTE + 30 * SECOND);
    let dt = time_to_dt(&DateTime::from_civil(1999, 12, 31, 23, 59, 58, 987_000_000)).unwrap();
    assert_eq!(hex(&dt), "9912312359589876");
    // arrays
    let at = Tag { name: "a".into(), softdatatype: SDT_INT, array: true, count: 3, ..Default::default() };
    let v = encode_plc(&at, "1, -2, 3").unwrap();
    assert!(v.flags == FLAG_ARRAY && v.typ == T_INT, "array {v}");
    let x = decode_plc(&at, &v).unwrap();
    assert_eq!(format_plc(&at, &x), "[1, -2, 3]");
    assert!(encode_plc(&at, "1,2").is_err(), "short array accepted");
    // JSON mapping
    assert_eq!(x.to_json(), "[1,-2,3]");
    let t = Tag::of_type(SDT_LDT);
    let x = decode_plc(&t, &encode_plc(&t, "2024-01-02T03:04:05.5").unwrap()).unwrap();
    assert_eq!(x.to_json(), "\"2024-01-02T03:04:05.5Z\"");
    let t = Tag::of_type(SDT_TIME);
    assert_eq!(decode_plc(&t, &encode_plc(&t, "1.5s").unwrap()).unwrap().to_json(), "1500");
}

#[test]
fn legitimation_crypto() {
    let modes = [
        ("1;6ES7 212-1BE31-0XB0 ;V3.0", LegitMode::Unsupported), // CPU 1212C FW V3.0: below the reference's minimum
        ("1;6ES7 214-1AG40-0XB0 ;V4.5", LegitMode::Legacy),
        ("1;6ES7 214-1AG40-0XB0 ;V4.7", LegitMode::New),
        ("1;6ES7 510-1DJ01-0AB0;V2.9", LegitMode::Legacy),
        ("1;6ES7 510-1DJ01-0AB0;V3.1", LegitMode::New),
        ("1;6ES7 672-7FC01-0YA0;V21.9", LegitMode::Legacy),
        ("1;6ES7 212-1HG50-0XB0;V1.0", LegitMode::New),
        ("1;6ES7 510-1DJ01-0AB0;V2.8", LegitMode::Unsupported),
        ("1;6ES7 315-2EH14-0AB0 ;V3.2", LegitMode::Unsupported),
        ("1;6ES7 214-1AG40-0XB0 ;V4.6 ", LegitMode::Legacy),
    ];
    for (p, want) in modes {
        let got = legitimation_mode_for(p).map(|x| x.0).unwrap_or(LegitMode::Unsupported);
        assert_eq!(got, want, "{p:?}");
    }
    // legacy: SHA1(password) XOR challenge
    let r = legacy_response("", &[0u8; 20]).unwrap();
    assert_eq!(hex(&r), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    // TIA's 303/304 pair of the capture is not SHA1("") XOR challenge.
    let tia = legacy_response("", &h("2339421d3412e7bdeb11e921d8b67df5c199c33f")).unwrap();
    assert_ne!(hex(&tia), "864d95aa1a2cd154ec5c5d9651d80b6f9038322f");
    // new: AES-256-CBC, key SHA256(secret), IV challenge[:16]
    let secret = [7u8; 32];
    let ch = [9u8; 20];
    let p = legitimation_payload("pw", "user");
    let (ct, next) = new_legitimation_response(&p, &secret, &ch).unwrap();
    let key = sha256(&secret);
    assert_eq!(next, key.to_vec(), "key roll");
    assert_eq!(ct.len() % 16, 0, "padding");
    let pt = aes256_cbc_decrypt(&key, &ch[..16], &ct);
    assert_eq!(&pt[..p.len()], &p[..], "decrypted payload differs");
    let v = Value::decode(&mut Reader::new(&legitimation_payload("pw", "")));
    let st = v.v.as_struct().unwrap();
    assert_eq!(st.get(crate::ids::ATTR_LEGIT_PAYLOAD_TYPE).unwrap().v, V::U32(LEGIT_LEGACY));
    assert_eq!(st.get(crate::ids::ATTR_LEGIT_PAYLOAD_PASSWORD).unwrap().v, V::Bytes(sha1(b"pw").to_vec()));
}

#[test]
fn aes256_cbc_nist_vectors() {
    // NIST SP 800-38A, F.2.5 / F.2.6 (CBC-AES256), first two blocks.
    let key: [u8; 32] = h("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4").try_into().unwrap();
    let iv = h("000102030405060708090a0b0c0d0e0f");
    let pt = h("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
    let ct = h("f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d");
    let enc = aes256_cbc_encrypt(&key, &iv, &pt);
    assert_eq!(enc.len(), 48, "PKCS#7 adds one full block");
    assert_eq!(hex(&enc[..32]), hex(&ct));
    assert_eq!(hex(&aes256_cbc_decrypt(&key, &iv, &ct)), hex(&pt));
}

fn tcp_pair() -> (TcpStream, TcpStream) {
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let a = TcpStream::connect(ln.local_addr().unwrap()).unwrap();
    let (b, _) = ln.accept().unwrap();
    (a, b)
}

#[test]
fn cotp_framing() {
    let (a, b) = tcp_pair();
    let mut ta = Tpkt::new(a);
    let mut tb = Tpkt::new(b);
    let data: Vec<u8> = b"0123456789".repeat(250); // 2500 bytes -> 3 TPDUs
    CotpIo { tp: &mut ta }.write_all(&data).unwrap();
    let mut tpdus = Vec::new();
    loop {
        let tp = tb.read_tpkt().unwrap();
        let eot = tp[2] & 0x80 != 0;
        tpdus.push(tp);
        if eot {
            break;
        }
    }
    assert!(tpdus.len() == 3 && tpdus[0][2] == 0 && tpdus[0].len() == 1024, "tpdus {} first={}", tpdus.len(), tpdus[0].len());
    // Read side reassembles the stream
    CotpIo { tp: &mut ta }.write_all(&data).unwrap();
    let mut got = vec![0u8; data.len()];
    CotpIo { tp: &mut tb }.read_exact(&mut got).unwrap();
    assert_eq!(got, data, "read back");
    // S7+ fragments inside the TLS plaintext stream
    let mut stream = h("7202000231aa72020001bb7202000072fe0002cafe");
    let mut re = Reassembler::new();
    let mut pdus: Vec<Pdu> = Vec::new();
    for _ in 0..3 {
        let tp = parse_s7_fragment(&mut stream, true).unwrap().unwrap();
        pdus.extend(re.push(&tp).unwrap());
    }
    assert!(pdus.len() == 2 && hex(&pdus[0].data) == "31aabb" && pdus[1].version == 0xfe, "pdus {pdus:?}");
    // a fragment waits for the 4 bytes that tell whether a trailer follows
    let mut partial = h("7202000231aa7202");
    assert!(parse_s7_fragment(&mut partial, false).unwrap().is_none());
}

#[test]
fn decompress_blob_and_dictionaries() {
    let d = Dictionaries::new();
    let id = d.register(TEST_DICT);
    let z = compress_with_dictionary(b"<InterfaceLineComments>hi</InterfaceLineComments>", TEST_DICT);
    let mut blob = vec![1, 2, 3, 4];
    blob.extend_from_slice(&z);
    let out = d.decompress_blob(&blob, 4).unwrap();
    assert_eq!(out, b"<InterfaceLineComments>hi</InterfaceLineComments>");
    // unknown dictionary
    let other = b"some other dictionary for the test".to_vec();
    let z2 = compress_with_dictionary(b"x", &other);
    match d.decompress_blob(&z2, 0) {
        Err(e) => assert!(e.to_string().contains("no zlib dictionary"), "{e}"),
        Ok(_) => panic!("missing dictionary accepted"),
    }
    // .cs extraction checks the declared Adler-32 (and tolerates a typo)
    let cs_bytes = |b: &[u8]| b.iter().map(|c| format!("0x{c:02x}, ")).collect::<String>();
    let cs = format!(
        "private const uint DICTID_A_1 = 0x{id:08x};\nprivate static readonly byte[] dict_A_1 = new byte[]\n{{\n{}}};\n\
         private const uint DICTID_B_1 = 0x1234;\nprivate static readonly byte[] dict_B_1 = new byte[] {{{}}};\n",
        cs_bytes(TEST_DICT),
        cs_bytes(&other)
    );
    let d2 = Dictionaries::new();
    let n = d2.load_from_cs_source(&cs);
    assert_eq!(n, 2);
    assert_eq!(d2.warnings().len(), 1, "{:?}", d2.warnings());
    assert!(d2.decompress_blob(&z2, 0).is_ok(), "after loading");
    // and from a file
    let p = std::env::temp_dir().join(format!("s7plus-dict-{}.cs", std::process::id()));
    std::fs::write(&p, cs).unwrap();
    let d3 = Dictionaries::new();
    assert_eq!(d3.load(p.to_str().unwrap()).unwrap(), 2);
    let _ = std::fs::remove_file(&p);
    assert_eq!(crate::blob::adler32(b"Wikipedia"), 0x11E60398);
}

#[test]
fn parse_text_library_bounds() {
    assert!(parse_text_library(&[0u8; 10], &[], &[], &[]).is_err(), "short table accepted");
    let mut t1 = vec![0u8; 28];
    t1[16] = 1;
    assert!(parse_text_library(&t1, &[5, 0, 0, 0], &[], &[]).is_err(), "truncated table 2 accepted");
}

#[test]
fn formatting_helpers() {
    use crate::textfmt::*;
    assert_eq!(format_float(1e6, false), "1e+06");
    assert_eq!(format_float(123456.0, false), "123456");
    assert_eq!(format_float(0.0001, false), "0.0001");
    assert_eq!(format_float(0.00001, false), "1e-05");
    assert_eq!(format_float(-0.125, false), "-0.125");
    assert_eq!(format_float(0.1f32 as f64, true), "0.1");
    assert_eq!(duration_string(1_500_000), "1.5ms");
    assert_eq!(duration_string(-2 * SECOND), "-2s");
    assert_eq!(duration_string(HOUR), "1h0m0s");
    assert_eq!(duration_string(1500), "1.5µs");
    assert_eq!(parse_duration("1h2m3.5s"), Some(HOUR + 2 * MINUTE + 3 * SECOND + SECOND / 2));
    assert_eq!(parse_duration("1.5us"), Some(1500));
    assert_eq!(parse_duration("x"), None);
    assert_eq!(parse_int("0x10", 8), Some(16));
    assert_eq!(parse_int("-129", 8), None);
    assert_eq!(parse_uint("0b101", 8), Some(5));
    assert_eq!(parse_uint("010", 8), Some(8));
    assert_eq!(quote("a\"b\n"), "\"a\\\"b\\n\"");
    let t = DateTime::from_civil(2024, 2, 29, 1, 2, 3, 400_000_000);
    assert_eq!(t.format_plc(), "2024-02-29T01:02:03.4");
    assert_eq!(DateTime::parse("2024-02-29 01:02:03.4"), Some(t));
    assert_eq!(DateTime::parse("2023-02-29"), None);
    assert_eq!(t.civil().weekday, 4); // Thursday
    assert_eq!(DateTime::from_civil(1969, 12, 31, 0, 0, 0, 0).unix_secs, -86400);
    let e = Error::Item(0x13);
    assert_eq!(e.to_string(), "s7plus: item error 0x13");
}

// Vartype elements of two DBs from PLCSIM Advanced V8 (CPU 1513-1 PN, FW S4.1):
// "TestAbs" (standard) and "TestOpt" (optimized) both have flags 0x8ac0, so
// the optimized-access flag is set in both. Only the standard DB has the
// classic bit (0x08) in the bit-offset info (issue #10).
#[test]
fn db_optimized_fw4() {
    let el = |sdt: u8, bit_info: u8, opt: u32, non: u32| Vartype {
        softdatatype: sdt,
        flags: 0x8ac0,
        bit_info,
        offset: OffsetInfo { kind: OI_STD, opt_addr: opt, non_opt_addr: non, ..Default::default() },
        ..Default::default()
    };
    let std_db = VartypeList {
        first_id: 9,
        elems: vec![el(SDT_BYTE, 0x08, 0, 0), el(SDT_INT, 0x08, 2, 2), el(SDT_REAL, 0x08, 8, 8), el(SDT_BOOL, 0x08, 12, 12)],
    };
    let opt_db = VartypeList {
        first_id: 9,
        elems: vec![el(SDT_BBOOL, 0, 28, 0), el(SDT_INT, 0, 24, 2), el(SDT_LREAL, 0, 8, 12), el(SDT_WORD, 0, 26, 20)],
    };
    assert!(std_db.elems.iter().all(|e| e.optimized() && e.classic()));
    assert!(!db_optimized(&std_db), "standard DB on FW V4.x reported as optimized");
    assert!(db_optimized(&opt_db), "optimized DB on FW V4.x not reported as optimized");
    // Older firmware: no optimized-access flag in a standard DB, classic only on Bools.
    let mut old_std = std_db.clone();
    for e in &mut old_std.elems {
        e.flags &= !VT_OPTIMIZED;
        if e.softdatatype != SDT_BOOL {
            e.bit_info = 0;
        }
    }
    assert!(!db_optimized(&old_std));
    assert!(!db_optimized(&VartypeList { first_id: 9, elems: vec![] }), "empty DB");
}
