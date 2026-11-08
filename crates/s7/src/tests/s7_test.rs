//! Golden frames: our builders reproduce the exact byte layout of the
//! S7comm frames. No PLC.

use super::hex;
use crate::addr::parse_tag;
use crate::types::{decode, encode, s5time_to_ms, Type};
use crate::wire::{cotp_connect_request, s7_frame, setup_comm_params, DialOptions, Item, AREA_DB, AREA_FLAGS};

#[test]
fn connect_request_tsap() {
    // S7-1200 CPU: rack 0, slot 1, link type 0x03 (Other), local TSAP 0x0100.
    let o = DialOptions { link_type: 0x03, rack: 0, slot: 1, local_tsap: 0x0100, ..Default::default() };
    // dest TSAP = [LinkType, (Rack<<5)|Slot] = 03 01
    assert_eq!(hex(&cotp_connect_request(&o)), "11e00000000100c0010ac1020100c2020301");
}

#[test]
fn connect_request_rack_slot() {
    // rack 2, slot 4 -> (2<<5)|4 = 0x44.
    let o = DialOptions { link_type: 0x02, rack: 2, slot: 4, local_tsap: 0x0100, ..Default::default() };
    let cr = cotp_connect_request(&o);
    assert_eq!(&cr[cr.len() - 2..], &[0x02, 0x44]);
}

#[test]
fn setup_comm_params_960() {
    // F0 00 00 01 00 01 <pdu BE>. For 960 -> 03C0.
    assert_eq!(hex(&setup_comm_params(960)), "f000000100010".to_string() + "3c0");
}

#[test]
fn read_item_layout() {
    // DB1.DBW0 -> 12 0a 10 02 0002 0001 84 000000
    assert_eq!(hex(&Item::bytes(AREA_DB, 1, 0, 2).encode()), "120a100200020001".to_string() + "84" + "000000");
}

#[test]
fn read_item_bit() {
    // M0.3 -> 12 0a 10 01 0001 0000 83 000003  (bit address = 0*8+3)
    assert_eq!(hex(&Item::bit(AREA_FLAGS, 0, 0, 3).encode()), "120a100100010000".to_string() + "83" + "000003");
}

#[test]
fn read_item_bit_address() {
    // M10.5 byte addressing -> bit addr = 10*8+5 = 85 = 0x000055
    let b = Item::bit(AREA_FLAGS, 0, 10, 5).encode();
    assert_eq!(&b[9..12], &[0x00, 0x00, 0x55]);
}

#[test]
fn write_data_header() {
    // Byte item: 00 04 <len in bits>; bit item: 00 03 0001.
    assert_eq!(hex(&Item::bytes(AREA_FLAGS, 0, 100, 2).data_header(2)), "00040010");
    assert_eq!(hex(&Item::bit(AREA_FLAGS, 0, 0, 1).data_header(1)), "00030001");
}

#[test]
fn write_frame() {
    let it = Item::bytes(AREA_FLAGS, 0, 100, 2);
    let mut param = vec![0x05, 0x01];
    param.extend_from_slice(&it.encode());
    let mut data = it.data_header(2).to_vec();
    data.extend_from_slice(&[0xFB, 0x2E]); // -1234
                                           // TPKT 03 00 0025 | COTP 02 f0 80 | S7 32 01 0000 0001 000e 0006 | param 05 01 + item | data
    let want = "030000250".to_string() + "2f080" + "320100000001000e0006" + "0501" + "120a10020002000083000320" + "00040010fb2e";
    assert_eq!(hex(&s7_frame(1, 0x01, &param, &data)), want);
}

#[test]
fn timer_item() {
    // T5, 2 timers: TS 0x1d, count 2, area 0x1d, address = timer number.
    assert_eq!(hex(&Item::timers(5, 2).encode()), "120a101d000200001d000005");
}

#[test]
fn parse_tag_items() {
    let cases = [
        ("DB1.DBX0.3", "120a10010001000184000003"),
        ("DB1.DBW2", "120a10020002000184000010"),
        ("DB10.D4", "120a1002000400 0a84000020"),
        ("MW10:int", "120a10020002000083000050"),
        ("MD20:real", "120a100200040000830000a0"),
        ("I0.1", "120a10010001000081000001"),
        ("QB0", "120a10020001000082000000"),
        ("EW4", "120a10020002000081000020"),
        ("DB1.DBW0[4]", "120a10020008000184000000"),
        ("DB1.DBB0:string[10]", "120a1002000c000184000000"),
        ("MB0:lreal", "120a10020008000083000000"),
        ("C3", "120a101c000100001c000003"),
    ];
    for (input, want) in cases {
        let tg = parse_tag(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!(hex(&tg.item.encode()), want.replace(' ', ""), "{input}");
    }
    for bad in ["M0.8", "DB1.DBW0.1", "X12", "MW0:bool", "DB1.DBX0.1[2]"] {
        assert!(parse_tag(bad).is_err(), "{bad}: expected error");
    }
}

#[test]
fn types_round_trip() {
    let cases = [
        (Type::Int, "-1234", "fb2e", "-1234"),
        (Type::DInt, "-100000", "fffe7960", "-100000"),
        (Type::Real, "3.25", "40500000", "3.25"),
        (Type::LReal, "-0.5", "bfe0000000000000", "-0.5"),
        (Type::LInt, "-2", "fffffffffffffffe", "-2"),
        (Type::Word, "0x1234", "1234", "4660 (0x1234)"),
        (Type::Counter, "123", "0123", "123"),
        (Type::BCD, "1234", "1234", "1234"),
        (Type::LBCD, "87654321", "87654321", "87654321"),
        (Type::DT, "2026-11-03 21:30:15", "2611032130150003", "2026-11-03 21:30:15.000"),
    ];
    for (t, input, h, back) in cases {
        let b = encode(t, input, 0).unwrap_or_else(|e| panic!("{t} {input}: {e}"));
        assert_eq!(hex(&b), h, "{t} {input}");
        assert_eq!(decode(t, &b).unwrap(), back, "{t} decode");
    }
    // STRING[10] "abc": max 10, len 3.
    let b = encode(Type::String, "abc", 10).unwrap();
    assert_eq!(hex(&b[..5]), "0a03616263");
    assert_eq!(b.len(), 12);
    // S5TIME: base 1 (100 ms) * 123 = 12300 ms.
    assert_eq!(s5time_to_ms(&[0x11, 0x23]).unwrap(), 12300);
}

/// The 24-bit address field holds byte * 8 + bit: an offset past 0x1FFFFF
/// would wrap to another address. Timer/counter numbers and counts are 16-bit.
/// (Found by the s7_tag_value fuzz target.)
#[test]
fn address_limits() {
    use crate::addr::{parse_siemens, MAX_BYTE_OFFSET};
    use crate::nodes7addr::parse_node_s7;
    for ok in ["DB1.DBB2097151", "DB1.DBX2097151.7", "MB2097148:dint", "T65535", "C0[32767]", "T5[100]"] {
        assert!(parse_tag(ok).is_ok(), "{ok} must parse");
    }
    for ok in ["DB1,B2097151", "MB2097151", "T5.32767", "C65535"] {
        assert!(parse_node_s7(ok).is_ok(), "{ok} must parse");
    }
    let t = parse_tag("DB1.DBB2097151").unwrap();
    assert_eq!(t.item.start as i64, MAX_BYTE_OFFSET);
    assert_eq!(hex(&t.item.encode()[9..]), "fffff8");
    for bad in [
        "DB1.DBB2097152",
        "DB1.DBX2097152.0",
        "MB2097150:dint",
        "DB1.DBB2097000[200]",
        "MB99999999",
        "M99999999.1",
        "T65536",
        "C99999999",
        "T5[32768]",
        "T65535[2]",
        "T5[70000]",
    ] {
        let e = parse_siemens(bad).expect_err(bad).to_string();
        assert!(e.contains("out of range"), "{bad}: {e}");
    }
    assert!(parse_tag("M0.8").is_err(), "bit 8");
    for bad in ["DB1,B2097152", "DB1,X2097152.0", "DB1,REAL2097150", "MB99999999", "T5.70000", "T65536", "C5.40000"] {
        let e = parse_node_s7(bad).expect_err(bad).to_string();
        assert!(e.contains("out of range"), "{bad}: {e}");
    }
}
