//! Tag descriptions captured from a CPU 1212C FW V3.0.2.

use super::h;
use crate::browse::VarInfo;
use crate::ids::*;
use crate::object::{decode_object, Object, ELEM_START_OBJECT};
use crate::softdt::*;
use crate::symabs::classic_for;
use crate::textfmt::hex;
use crate::typeinfo::*;
use crate::wire::{Reader, Writer};

const TD_DB_OPT: &str = "a7050162000101809000000900000400750000a8a704026279000a02809000000a000004015ca8a7040177000b04809000000b000004024ca8a7040169000705809000000c000004044ea8a704026469000807809000000d0000040610a8a7040172000e08809000000e0000040a14a8a704026c72000f30809000000f0000040e08a8a70401630002038090000010000004165da8a7040277630002038090000011000004175ea8a704017300191380900000120a0004185fa8a7040277730019138090000013080004246ba8a704016400030980900000140000042e50a8a70403746f6400040a80900000150000043018a8a704017400080b8090000016000004341ca8a7040364746c0017804380900000179080804300043820a8a706036172721007058090000018000004445201050202a8a707036d617408080780900000190000044e2c04040200030002a8a704027374001711809000001a899084800a00046644a8a200000000";
const TD_DB_STD: &str = "a7050162000101819000000900000400000000a8a704026279000a02819000000b0000040101a8a7040177000b04819000000d0000040202a8a7040169000705819000000f0000040404a8a70402646900080781900000110000040606a8a7040172000e0881900000130000040a0aa8a704026c72000f3081900000150000040e0ea8a704016300020381900000170000041616a8a70402776300020381900000190000041717a8a7040173001913819000001b0a00041818a8a704027773001913819000001d0800042424a8a7040164000309819000001f0000042e2ea8a70403746f6400040a81900000210000043030a8a704017400080b81900000230000043434a8a7040364746c0017804381900000259080804300043838a8a706036172721007058190000027000004444401050202a8a707036d617408080781900000290000044e4e04040200030002a8a704027374001711819000002b899084800b00046666a8a200000000";
const TD_M_AREA: &str = "a70509546573745f426f6f6c0001018190000009000004822c822c0000a8a70408546573745f496e74000705819000000b000004822e822ea8a70409546573745f5265616c000e08819000000d00000482308230a8a200000000";
const TD_STRUCT: &str = "a70001780007058090000009000004a8a7000179000e08809000000a000200a8a200000000";

fn td_object(rid: u32, tail: &str) -> Object {
    let mut w = Writer::new();
    w.byte(ELEM_START_OBJECT);
    w.u32(rid);
    w.vlq32(CLASS_TYPE_INFO);
    w.vlq32(0x20);
    w.vlq32(0);
    w.raw(&h(tail));
    let mut r = Reader::new(&w.b);
    let o = decode_object(&mut r).unwrap();
    assert!(r.err.is_none(), "decode {rid:#x}: {:?}", r.err);
    assert!(o.raw.is_empty(), "decode {rid:#x}: raw tail left: {}", hex(&o.raw));
    // round trip: encode emits the same tag descriptions
    let mut w2 = Writer::new();
    o.encode(&mut w2);
    assert!(w2.b == w.b[..w.b.len() - 4] || w2.b == w.b, "re-encode {rid:#x}:\n got {}\nwant {}", hex(&w2.b), hex(&w.b));
    o
}

#[test]
fn tag_descr_db_opt() {
    let o = td_object(0x9200_000a, TD_DB_OPT);
    let want = ["b", "by", "w", "i", "di", "r", "lr", "c", "wc", "s", "ws", "d", "tod", "t", "dtl", "arr", "mat", "st"];
    assert_eq!(o.varnames, want);
    let el = &o.vartypes.as_ref().unwrap().elems;
    for (i, n) in want.iter().enumerate() {
        assert!(el[i].lid == 9 + i as u32 && el[i].optimized(), "{n}: lid {} optimized {}", el[i].lid, el[i].optimized());
    }
    assert!(el[0].softdatatype == SDT_BOOL && el[0].offset.opt_addr == 117, "b: {:?}", el[0]);
    let s = &el[9];
    assert!(s.softdatatype == SDT_STRING && s.offset.kind == OI_STRING && s.offset.info1 == 10 && s.offset.non_opt_addr == 24, "s: {s:?}");
    let a = &el[15].offset;
    assert!(a.kind == OI_ARRAY_1DIM && a.lower_bound == 1 && a.count == 5, "arr: {a:?}");
    let m = &el[16].offset;
    assert!(m.kind == OI_ARRAY_MDIM && m.count == 6 && m.m_count[0] == 3 && m.m_count[1] == 2, "mat: {m:?}");
    let st = &el[17].offset;
    assert!(st.kind == OI_STRUCT && st.relation_id == 0x9201_000a && st.non_opt_addr == 102, "st: {st:?}");
    assert!(el[14].softdatatype == SDT_DTL && el[14].offset.non_opt_addr == 56, "dtl: {:?}", el[14]);
}

#[test]
fn tag_descr_db_std_and_m() {
    let o = td_object(0x9200_000b, TD_DB_STD);
    for (i, e) in o.vartypes.as_ref().unwrap().elems.iter().enumerate() {
        assert!(e.lid == 9 + 2 * i as u32 && !e.optimized(), "{}: lid {} optimized {}", o.varnames[i], e.lid, e.optimized());
        assert_eq!(e.offset.non_opt_addr, e.offset.opt_addr, "{}", o.varnames[i]);
    }
    let m = td_object(RID_M_TYPE_INFO, TD_M_AREA);
    let me = &m.vartypes.as_ref().unwrap().elems;
    assert!(
        m.varnames[0] == "Test_Bool"
            && me[0].offset.non_opt_addr == 300
            && m.varnames[2] == "Test_Real"
            && me[2].offset.non_opt_addr == 304
            && me[2].lid == 13,
        "M: {:?} {me:?}",
        m.varnames
    );
    let s = td_object(0x9201_000a, TD_STRUCT);
    let se = &s.vartypes.as_ref().unwrap().elems;
    assert!(
        s.varnames[1] == "y" && se[1].offset.non_opt_addr == 2 && se[1].offset.opt_addr == 0 && se[0].offset.kind == OI_STRUCT_ELEM_STD,
        "struct: {:?} {se:?}",
        s.varnames
    );
}

#[test]
fn classic_for_addresses() {
    let v = |sdt: u8, off: u32, bit: u8, max_len: u16| VarInfo {
        softdatatype: sdt,
        non_opt_offset: off,
        non_opt_bit: bit,
        max_len,
        ..Default::default()
    };
    let cases = [
        ("DB11.DB", v(SDT_BOOL, 0, 3, 0), "DB11.DBX0.3"),
        ("DB11.DB", v(SDT_INT, 4, 0, 0), "DB11.DBW4:int"),
        ("DB11.DB", v(SDT_REAL, 104, 0, 0), "DB11.DBD104:real"),
        ("DB11.DB", v(SDT_LREAL, 14, 0, 0), "DB11.DBB14:lreal"),
        ("DB11.DB", v(SDT_STRING, 24, 0, 10), "DB11,S24.10"),
        ("DB11.DB", v(SDT_DTL, 56, 0, 0), "DB11,DTL56"),
        ("M", v(SDT_BOOL, 300, 0, 0), "M300.0"),
        ("M", v(SDT_INT, 302, 0, 0), "MW302:int"),
        ("M", v(SDT_STRING, 10, 0, 0), ""),
    ];
    for (prefix, var, want) in cases {
        assert_eq!(classic_for(prefix, &var), want, "{prefix} {var:?}");
    }
}

// The Bools of tag descriptions always have the classic bit; the block
// attribute decides (review of #10: FW V3 optimized DBs stay optimized).
#[test]
fn tag_descr_db_optimized() {
    assert!(crate::browse::db_optimized(&td_object(0x9200_000a, TD_DB_OPT)), "FW V3 optimized DB");
    assert!(!crate::browse::db_optimized(&td_object(0x9200_000b, TD_DB_STD)), "FW V3 standard DB");
}

// The FW V3 captures as the fake's DB1 (optimized) and DB2 (standard): `dbs`
// and the classic address map follow the block attribute.
#[cfg(feature = "fake")]
#[test]
fn tag_descr_dbs_fake() {
    use crate::fake::{FakePlc, TI_DB1, TI_DB2};
    use crate::{Client, Options};
    let f = FakePlc::start().unwrap();
    {
        let mut st = f.lock();
        st.model.types.insert(TI_DB1, td_object(TI_DB1, TD_DB_OPT));
        st.model.types.insert(TI_DB2, td_object(TI_DB2, TD_DB_STD));
    }
    let c = Client::connect(Options { addr: f.addr(), timeout: std::time::Duration::from_secs(2), ..Default::default() }).unwrap();
    let dbs = c.list_datablocks(true).unwrap();
    assert!(dbs.len() == 2 && dbs[0].optimized && !dbs[1].optimized, "dbs {dbs:?}");
    let m = c.classic_addresses(&["\"Data_block_1\".i", "\"Data_block_1\".b", "\"Std_DB\".i", "\"Std_DB\".b"]).unwrap();
    assert!(!m.contains_key("\"Data_block_1\".i") && !m.contains_key("\"Data_block_1\".b"), "optimized DB mapped: {m:?}");
    assert_eq!(m.get("\"Std_DB\".i").map(|s| s.as_str()), Some("DB2.DBW4:int"), "{m:?}");
    assert_eq!(m.get("\"Std_DB\".b").map(|s| s.as_str()), Some("DB2.DBX0.0"), "{m:?}");
}
