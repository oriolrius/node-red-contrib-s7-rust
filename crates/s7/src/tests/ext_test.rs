//! Extension tests: Userdata/job golden frames, blocks, multi-part SZL, PLC
//! control (fake CPU only), write-verify, nodes7 addresses, date/time types,
//! LOGO! presets and family limits.

use std::time::Duration;

use chrono::{TimeZone, Utc};

use super::{dial_fake, hex, unhex};
use crate::addr::parse_tag;
use crate::blocks::{block_file, encode_s7_timestamp, upload_filename, upload_param, upload_start_param, BlockType};
use crate::control::{plc_start_param, plc_stop_param};
use crate::cpu::{parse_family, Family};
use crate::fakecpu::{Block, FakeCpu, AREA_FLAGS as FAKE_FLAGS};
use crate::textfmt::Zone;
use crate::types::{decode, encode, native, Native, Type};
use crate::userdata::{userdata_next, userdata_req, GROUP_BLOCK, GROUP_CPU, UD_TS_OCTET};
use crate::wire::{cotp_connect_request, s7_frame, DialOptions, Item, AREA_DB, AREA_FLAGS};

/// Golden Userdata request frames (group 3 block functions, SZL, follow-up).
#[test]
fn userdata_golden_frames() {
    let frame = |(p, d): (Vec<u8>, Vec<u8>)| hex(&s7_frame(1, 0x07, &p, &d));
    let cases = [
        (
            "block count",
            userdata_req(GROUP_BLOCK, 0x01, UD_TS_OCTET, None),
            "0300001d02f080".to_string() + "32070000000100080004" + "0001120411430100" + "0a000000",
        ),
        (
            "list DB",
            userdata_req(GROUP_BLOCK, 0x02, UD_TS_OCTET, Some(&BlockType::DB.code())),
            "0300001f02f080".to_string() + "32070000000100080006" + "0001120411430200" + "ff0900023041",
        ),
        (
            "block info OB1",
            userdata_req(GROUP_BLOCK, 0x03, UD_TS_OCTET, Some(&block_file(BlockType::OB, 1, b'A'))),
            "0300002502f080".to_string() + "3207000000010008000c" + "0001120411430300" + "ff090008" + "3038303030303141",
        ),
        (
            "SZL 0x0011",
            userdata_req(GROUP_CPU, 0x01, UD_TS_OCTET, Some(&[0x00, 0x11, 0x00, 0x00])),
            "0300002102f080".to_string() + "32070000000100080008" + "0001120411440100" + "ff09000400110000",
        ),
        (
            "follow-up",
            userdata_next(GROUP_BLOCK, 0x02, 0x05),
            "0300002102f080".to_string() + "320700000001000c0004" + "000112081243020500000000" + "0a000000",
        ),
    ];
    for (name, pd, want) in cases {
        assert_eq!(frame(pd), want, "{name}");
    }
}

/// Golden job parameters: upload (0x1D/0x1E/0x1F), STOP (0x29), start (0x28).
#[test]
fn job_golden_params() {
    let pp = hex(b"P_PROGRAM");
    let cases = [
        (
            "upload start OB1",
            hex(&upload_start_param(&upload_filename(BlockType::OB, 1))),
            "1d0000000000000009".to_string() + "5f3038303030303141",
        ),
        (
            "upload DB",
            hex(&upload_start_param(&upload_filename(BlockType::DB, 12345))),
            "1d0000000000000009".to_string() + &hex(b"_0A12345A"),
        ),
        ("upload step", hex(&upload_param(0x1E, 7, 0)), "1e00000000000007".to_string()),
        ("upload abort", hex(&upload_param(0x1F, 7, 0xFFFF)), "1f00ffff00000007".to_string()),
        ("stop", hex(&plc_stop_param()), "290000000000".to_string() + "09" + &pp),
        ("hot start", hex(&plc_start_param(false)), "28000000000000fd0000".to_string() + "09" + &pp),
        ("cold start", hex(&plc_start_param(true)), "28000000000000fd00024320".to_string() + "09" + &pp),
    ];
    for (name, got, want) in cases {
        assert_eq!(got, want, "{name}");
    }
    assert_eq!(upload_filename(BlockType::OB, 1), "_0800001A");
}

fn fake_blocks(f: &FakeCpu) -> Vec<u8> {
    let body: Vec<u8> = (0..450).map(|i| (i * 7) as u8).collect();
    f.add_block(Block {
        typ: 0x08,
        number: 1,
        lang: 0x02,
        flags: 0x01,
        author: "ACME".into(),
        family: "DEMO".into(),
        name: "Main".into(),
        version: 0x12,
        checksum: 0xBEEF,
        local: 20,
        body: body.clone(),
    });
    f.add_block(Block { typ: 0x0C, number: 3, lang: 0x04, body: b"fc3".to_vec(), ..Default::default() });
    for n in 1..=60u16 {
        f.add_block(Block { typ: 0x0A, number: n, lang: 0x05, body: vec![n as u8, 0, 0, 0], ..Default::default() });
    }
    body
}

/// Block functions, multi-part lists and upload against the fake CPU.
#[test]
fn blocks_against_fake() {
    let (mut cn, f) = dial_fake(240);
    let body = fake_blocks(&f);
    f.set_ud_max(32); // force several data units

    let counts = cn.block_counts().unwrap();
    let get = |t: BlockType| counts.iter().find(|c| c.typ == t).map(|c| c.count).unwrap_or(99);
    assert_eq!(counts.len(), 7);
    assert_eq!((get(BlockType::OB), get(BlockType::DB), get(BlockType::FC), get(BlockType::FB)), (1, 60, 1, 0));
    let dbs = cn.list_blocks(BlockType::DB).unwrap();
    assert_eq!(dbs.len(), 60);
    assert_eq!(dbs[59].number, 60);
    assert_eq!(dbs[0].language, 0x05);
    assert!(f.follow_ups() >= 6, "follow-ups = {}", f.follow_ups()); // 240 B in units of 32 B
    assert!(cn.list_blocks(BlockType::FB).unwrap().is_empty());

    f.set_ud_max(0);
    let bi = cn.get_block_info(BlockType::OB, 1).unwrap();
    assert_eq!(bi.typ, BlockType::OB);
    assert_eq!(bi.number, 1);
    assert_eq!(bi.language, 2);
    assert!(bi.linked());
    assert_eq!((bi.author.as_str(), bi.family.as_str(), bi.name.as_str()), ("ACME", "DEMO", "Main"));
    assert_eq!(bi.version_string(), "1.2");
    assert_eq!(bi.checksum, 0xBEEF);
    assert_eq!((bi.load_size, bi.mc7_size, bi.local_data), (450, 450 - 36, 20));
    assert_eq!(bi.code_date, Utc.with_ymd_and_hms(2026, 11, 4, 12, 0, 0).unwrap());
    assert_eq!(&encode_s7_timestamp(&bi.code_date)[..], &bi.raw[22..28], "timestamp round trip");
    let e = cn.get_block_info(BlockType::FB, 9).unwrap_err();
    assert_eq!(e.item_code(), Some(0x0A), "missing block info err = {e}");

    f.set_upload_chunk(100); // 450 B in 5 steps
    assert_eq!(cn.upload(BlockType::OB, 1).unwrap(), body);
    let e = cn.upload(BlockType::FB, 1).unwrap_err();
    assert!(e.protocol().is_some(), "upload of missing block: {e}");
    let all = cn.upload_all().unwrap();
    assert_eq!(all.len(), 62);
    for b in &all {
        assert!(b.err.is_none(), "{}{}: {:?}", b.typ, b.number, b.err);
    }
    // The connection still works after all that.
    cn.read(&[Item::bytes(AREA_FLAGS, 0, 0, 1)]).unwrap();
}

/// SZL answers longer than one data unit are reassembled; SZL 0x0000 helper.
#[test]
fn szl_multi_part() {
    let (mut cn, f) = dial_fake(240);
    f.set_ud_max(10); // 8-byte header + 44 bytes of IDs → 6 units
    let ids = cn.szl_ids().unwrap();
    let list = f.szl_id_list();
    assert_eq!(ids, list);
    assert_eq!(f.follow_ups(), 5);
    let mods = cn.module_identification().unwrap(); // 2 × 28 B over several units
    assert_eq!(mods.len(), 2);
    assert_eq!(mods[1].version, "V3.0.2");
    let e = cn.read_szl(0x001C, 0).unwrap_err();
    assert_eq!(e.userdata_code(), Some(0xD401), "unsupported SZL err = {e}");
    assert!(e.to_string().starts_with("SZL 0x001c/0: userdata error 0xd401"), "{e}");
}

/// STOP / hot start / cold start: ONLY against the fake CPU.
#[test]
fn plc_control_fake() {
    let (mut cn, f) = dial_fake(240);
    assert_eq!(cn.cpu_state().unwrap().0, "RUN");
    assert!(!cn.stop_cpu().unwrap());
    assert_eq!(cn.cpu_state().unwrap().0, "STOP");
    assert!(cn.stop_cpu().unwrap(), "second stop: already");
    assert!(!cn.start_cpu(false).unwrap());
    assert_eq!(cn.cpu_state().unwrap().0, "RUN");
    assert!(cn.start_cpu(true).unwrap(), "cold start: already");
    let (stops, starts) = f.control();
    assert_eq!((stops, starts), (2, vec![false, true]));
}

/// Write with read-back and N re-writes (node-red "S7 Out" bingo/rewriteCount).
#[test]
fn write_verify() {
    let (mut cn, f) = dial_fake(240);
    let tg = parse_tag("MW200:int").unwrap();
    let bit = parse_tag("M202.3").unwrap();
    let items = [tg.item, bit.item];
    let vals = [tg.value("-77").unwrap(), bit.value("true").unwrap()];

    f.set_ignore_writes_n(2); // first write and first re-write are lost
    let rep = cn.write_verify(&items, &vals, 3, Duration::ZERO).unwrap();
    assert!(rep.bingo && rep.rewrites == 2, "{rep:?}");
    {
        let mut s = f.lock();
        let m = s.mem(FAKE_FLAGS, 0);
        assert!(m[200] == 0xff && m[201] == 0xb3 && m[202] & 0x08 != 0, "values not in memory");
    }

    f.set_ignore_writes(true);
    let v3 = tg.value("5").unwrap();
    let rep = cn.write_verify(&[tg.item], &[v3], 2, Duration::from_millis(1)).unwrap();
    assert!(!rep.bingo && rep.rewrites == 2 && rep.mismatched().len() == 1, "{rep:?}");
    assert!(rep.to_string().contains("NOT verified"), "{rep}");
    f.set_ignore_writes(false);
    let missing = Item::bytes(AREA_DB, 2, 0, 2); // DB2 does not exist: no re-write
    f.reset_reqs();
    let rep = cn.write_verify(&[missing], &[[1u8, 2]], 5, Duration::ZERO).unwrap();
    assert!(!rep.bingo && rep.rewrites == 0 && f.reqs() == 2, "item error: {rep:?} reqs={}", f.reqs());
}

#[test]
fn node_s7_addresses() {
    let cases: [(&str, &str, Type, bool, usize); 25] = [
        ("DB1,REAL0", "120a10020004000184000000", Type::Real, false, 1),
        ("DB5,X0.1", "120a10010001000584000001", Type::Bool, false, 1),
        ("DB2,S7.10", "120a1002000c000284000038", Type::String, false, 1),
        ("DB2,S0.4.3", "120a10020012000284000000", Type::String, false, 3),
        ("MI14", "120a10020002000083000070", Type::Int, false, 1),
        ("MRW20", "120a100200020000830000a0", Type::Word, true, 1),
        ("IDI22", "120a100200040000810000b0", Type::DInt, false, 1),
        ("DB1,DT0", "120a10020008000184000000", Type::DT, false, 1),
        ("DB1,DTZ10", "120a10020008000184000050", Type::DTZ, false, 1),
        ("DB2,DTL2", "120a1002000c000284000010", Type::DTL, false, 1),
        ("DB2,DTLZ12", "120a1002000c000284000060", Type::DTLZ, false, 1),
        ("DB57,RWORD4", "120a10020002003984000020", Type::Word, true, 1),
        ("DB13,RDI5", "120a10020004000d84000028", Type::DInt, true, 1),
        ("IC7", "120a10020001000081000038", Type::Char, false, 1),
        ("EC7", "120a10020001000081000038", Type::Char, false, 1),
        ("DB1,INT0.5", "120a1002000a000184000000", Type::Int, false, 5),
        ("MW10.3", "120a10020006000083000050", Type::Word, false, 3),
        ("QR36", "120a10020004000082000120", Type::Real, false, 1),
        ("PIW0", "120a10020002000080000000", Type::Word, false, 1),
        ("A2.1", "120a10010001000082000011", Type::Bool, false, 1),
        ("T5", "120a101d000100001d000005", Type::S5Time, false, 1),
        ("C3.2", "120a101c000200001c000003", Type::Counter, false, 2),
        // LOGO! / S7-200 V memory = DB1 (Siemens parser).
        ("VW20", "120a100200020001840000a0", Type::Word, false, 1),
        ("V1.3", "120a1001000100018400000b", Type::Bool, false, 1),
        ("VB1024", "120a10020001000184002000", Type::Byte, false, 1),
    ];
    for (input, item, typ, le, elems) in cases {
        let tg = parse_tag(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!((hex(&tg.item.encode()).as_str(), tg.typ, tg.little_endian, tg.elems), (item, typ, le, elems), "{input}");
    }
    for bad in ["DB1,X0.1.8", "DB1,FOO0", "MS0.5", "DB0,B0", "DB1,X0.8", "DB1,S0", "DB1,X0", "MDT0", "VB1.3"] {
        assert!(parse_tag(bad).is_err(), "{bad}: expected error");
    }
    // RWORD is little-endian: 0x1234 is stored 34 12.
    let tg = parse_tag("DB57,RWORD4").unwrap();
    assert_eq!(hex(&tg.value("0x1234").unwrap()), "3412");
}

#[test]
fn date_time_types() {
    let cases = [
        (Type::DTL, "2026-11-04 13:45:30.123456789", "07ea0b04040d2d1e075bcd15", "2026-11-04 13:45:30.123456789"),
        (Type::DTLZ, "2026-11-04 13:45:30.5 UTC", "07ea0b04040d2d1e1dcd6500", "2026-11-04 13:45:30.5 UTC"),
        (Type::DTLZ, "2026-11-04T14:45:30.5+01:00", "07ea0b04040d2d1e1dcd6500", "2026-11-04 13:45:30.5 UTC"),
        (Type::DTZ, "2026-11-04 13:45:30.120Z", "2611041345301204", "2026-11-04 13:45:30.12 UTC"),
        (Type::DT, "2026-11-04 13:45:30.125", "2611041345301254", "2026-11-04 13:45:30.125"),
        (Type::Date, "2026-11-04", "3490", "2026-11-04"),
        (Type::TOD, "13:45:30.250", "02f3c58a", "13:45:30.25"),
        (Type::Time, "-1h2m3.5s", "ffc72f14", "-1h2m3.5s"),
        (Type::Time, "T#1H", "0036ee80", "1h0m0s"),
        (Type::Time, "1500", "000005dc", "1.5s"),
        (Type::LTime, "1.5s", "0000000059682f00", "1.5s"),
        (Type::LTOD, "01:00:00", "0000034630b8a000", "01:00:00"),
        (Type::LDT, "2026-11-04 12:00:00", "18e4d5c6785e8000", "2026-11-04 12:00:00"),
        (Type::WChar, "é", "00e9", "\"é\""),
    ];
    for (t, input, h, back) in cases {
        let b = encode(t, input, 0).unwrap_or_else(|e| panic!("{t} {input}: {e}"));
        assert_eq!(hex(&b), h, "{t} {input}");
        assert_eq!(decode(t, &b).unwrap(), back, "{t} decode");
    }
    let ws = parse_tag("DB1.DBB0:wstring[4]").unwrap();
    assert_eq!(ws.item.count, 12);
    let b = ws.value("añ").unwrap();
    assert_eq!(hex(&b), "000400020061".to_string() + "00f1" + "00000000");
    assert_eq!(ws.format(&b).unwrap(), "\"añ\"");
    let dtl = parse_tag("MB0:dtl").unwrap();
    let nv = dtl.natives(&unhex("07ea0b04040d2d1e075bcd15")).unwrap();
    match &nv[0] {
        Native::Time(tm) => {
            use chrono::Timelike;
            assert_eq!(tm.nanosecond(), 123456789);
            assert_eq!(*tm.offset(), Zone::Local.convert(tm).offset().to_owned(), "local zone");
        }
        other => panic!("dtl native = {other:?}"),
    }
    assert_eq!(native(Type::Time, &[0xff, 0xc7, 0x2f, 0x14]).unwrap(), Native::I32(-3723500));
    assert!(encode(Type::Time, "30d", 0).is_err(), "TIME > 24 days accepted");
}

/// LOGO! 8 presets and the new family limits.
#[test]
fn logo_and_type_limits() {
    for (input, want) in [("logo", "c1020100c2020102"), ("LOGO!8", "c1020100c2020102"), ("logo-tsap", "c1021000c2022000")] {
        let f = parse_family(input).unwrap();
        let mut o = DialOptions::default();
        f.apply(&mut o, false, false, false);
        let cr = cotp_connect_request(&o);
        assert_eq!(hex(&cr[cr.len() - 8..]), want, "{input} TSAPs");
    }
    let cases = [
        (Family::S71200, "MB0:dtl", true),
        (Family::S7300, "MB0:dtl", false),
        (Family::Logo8, "DB1,DTL0", false),
        (Family::S71200, "MB0:ltime", false),
        (Family::S71500, "MB0:ldt", true),
        (Family::S71200, "MB0:wstring[4]", true),
        (Family::S7300, "MB0:wchar", false),
        (Family::S7300, "MB0:dtz", true),
        (Family::S71200, "DB1,DTZ0", false),
        (Family::Logo8, "VW0", true),
    ];
    for (f, tag, ok) in cases {
        let tg = parse_tag(tag).unwrap();
        assert_eq!(f.check(&tg).is_ok(), ok, "{f} {tag}");
    }
}
