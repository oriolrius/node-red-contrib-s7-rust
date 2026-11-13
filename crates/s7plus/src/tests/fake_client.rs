//! Client against the fake PLC.

use std::sync::Arc;
use std::time::Duration;

use crate::fake::{db1_var_count, FakeModel, FakePlc, TEST_DICT, TI_DB1, TI_DB2, TI_UDT};
use crate::ids::*;
use crate::legit::{ACCESS_FULL, ACCESS_HMI, ACCESS_NONE};
use crate::msg::ItemAddress;
use crate::object::decode_object;
use crate::plcvalue::*;
use crate::softdt::*;
use crate::subscribe::{decode_notification, SubOptions};
use crate::tags::{db_area, AreaItem, AREA_DB, AREA_FLAGS};
use crate::textfmt::{duration_string, DateTime};
use crate::tls::{CertVerify, NssKeyLog, SharedBuf, TlsOptions};
use crate::value::Value;
use crate::wire::{Reader, Writer};
use crate::{decode_alarm, AlarmSubOptions, Client, Error, Options, OP_STATE_RUN, OP_STATE_STOP};

fn dial(f: &FakePlc, mut o: Options) -> Client {
    o.addr = f.addr();
    if o.timeout.is_zero() || o.timeout == Duration::from_secs(5) {
        o.timeout = Duration::from_secs(2);
    }
    Client::connect(o).unwrap_or_else(|e| panic!("dial: {e}"))
}

#[test]
fn type_info_round_trip() {
    let md = FakeModel::new();
    for rid in [TI_DB1, TI_DB2, TI_UDT, RID_M_TYPE_INFO] {
        let o = &md.types[&rid];
        let mut w = Writer::new();
        o.encode(&mut w);
        let mut r = Reader::new(&w.b);
        let got = decode_object(&mut r).unwrap();
        assert!(r.err.is_none() && r.left() == 0, "{rid:#x}: {:?} left={}", r.err, r.left());
        assert_eq!(got.vartypes, o.vartypes, "{rid:#x}");
        assert_eq!(got.varnames, o.varnames, "{rid:#x}");
    }
}

#[test]
fn client_against_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    assert!(c.init_ssl_refused() && c.session() == 0x39c, "session = {:#x}", c.session());
    let echo = f.lock().echo.clone().unwrap();
    assert_eq!(echo.get(320).unwrap().v.as_str(), Some("1;6ES7 212-1BE31-0XB0;V3.0"), "session echo 320");

    // MW2:int, MD4:real, DB1.DBW10, M0.3, DB2.DBW0
    let items = [
        AreaItem::bytes(AREA_FLAGS, 0, 2, 2),
        AreaItem::bytes(AREA_FLAGS, 0, 4, 4),
        AreaItem::bytes(AREA_DB, 1, 10, 2),
        AreaItem::bit(AREA_FLAGS, 0, 0, 3),
        AreaItem::bytes(AREA_DB, 2, 0, 2),
    ];
    let data = vec![
        (-1234i16).to_be_bytes().to_vec(),
        3.25f32.to_be_bytes().to_vec(),
        4660u16.to_be_bytes().to_vec(),
        vec![1],
        1u16.to_be_bytes().to_vec(),
    ];
    let errs = c.write_items(&items, &data).unwrap();
    assert!(errs[0].is_none() && errs[1].is_none() && errs[2].is_none(), "write errors {errs:?}");
    assert!(matches!(errs[3], Some(Error::BitWrite)), "bit write error = {:?}", errs[3]);
    assert!(matches!(errs[4], Some(Error::Item(_))), "DB2 write error = {:?}", errs[4]);
    f.lock().mem.get_mut(&RID_M_AREA).unwrap()[0] = 0x08; // M0.3 set by the "program"
    let res = c.read_items(&items).unwrap();
    for (i, want) in data.iter().enumerate().take(4) {
        assert!(res[i].err.is_none(), "item {i}: {:?}", res[i].err);
        assert_eq!(&res[i].data, want, "item {i}");
    }
    assert_eq!(i16::from_be_bytes([res[0].data[0], res[0].data[1]]), -1234);
    assert!(matches!(res[4].err, Some(Error::Item(_))), "DB2 read error = {:?}", res[4].err);
}

/// Renders a decoded value in a form encode_plc accepts.
fn format_plc_input(t: &Tag, x: &PlcValue) -> String {
    match x {
        PlcValue::String(s) => s.clone(),
        PlcValue::Duration(d) => {
            if t.softdatatype == SDT_TIME_OF_DAY || t.softdatatype == SDT_LTOD {
                format_plc(t, x).trim().to_string()
            } else {
                duration_string(*d)
            }
        }
        _ => {
            let s = format_plc(t, x);
            match s.find(" (16#") {
                Some(i) if i > 0 => s[..i].to_string(),
                _ => s,
            }
        }
    }
}

#[test]
fn browse_and_symbolic_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let l = c.limits();
    assert!(l.tags_per_read_max == 50 && l.subscriptions_max == 33, "limits {l}");
    let dbs = c.list_datablocks(true).unwrap();
    assert!(
        dbs.len() == 2
            && dbs[0].name == "Data_block_1"
            && dbs[0].type_info_rid == TI_DB1
            && dbs[0].optimized
            && !dbs[1].optimized
            && dbs[1].number == 2
            && dbs[0].vars == db1_var_count(),
        "dbs {dbs:?}"
    );
    let q = f.lock().model.explores[0].clone();
    assert!(q.filter.is_some() && q.object == RID_PLC_PROGRAM, "DB explore without class filter: {q:?}");
    let vars = c.browse().unwrap();
    let by_name: std::collections::HashMap<&str, &crate::VarInfo> = vars.iter().map(|v| (v.name.as_str(), v)).collect();
    let checks = [
        ("\"Data_block_1\".Int1", "8A0E0001.A"),
        ("\"Data_block_1\".Arr[3]", "8A0E0001.F.2"),
        ("\"Data_block_1\".Mat[1,2]", "8A0E0001.10.5"),
        ("\"Data_block_1\".S.y", "8A0E0001.11.2"),
        ("\"Data_block_1\".SA[1].x", "8A0E0001.12.1.1.1"),
        ("\"Data_block_1\".Stamp", "8A0E0001.13"),
        ("\"Std_DB\".Bit", "8A0E0002.B"),
        ("Motor_On", "52.A"),
        ("Lamp", "51.A"),
    ];
    for (n, acc) in checks {
        let v = by_name.get(n).unwrap_or_else(|| panic!("{n} not browsed"));
        assert_eq!(v.addr.access_string(), acc, "{n}");
    }
    assert_eq!(by_name["\"Data_block_1\".SA[1].y"].opt_offset, 82 + 6 + 2);
    assert_eq!(by_name["\"Data_block_1\".Arr[3]"].opt_offset, 42 + 4);
    let bit = by_name["\"Std_DB\".Bit"];
    assert!(bit.non_opt_offset == 2 && bit.non_opt_bit == 3, "Std_DB.Bit {bit:?}");
    assert_eq!(by_name["\"Data_block_1\".Str"].max_len, 10);
    assert_eq!(vars.len(), 5 + 5 + 6 + 2 + 4 + 1 + 22 + 10 + 1 + 2 + 2 + 1, "variables"); // + BFlag

    // symbolic write + read of every type
    let vals: &[(&str, &str)] = &[
        ("\"Data_block_1\".Int1", "-7"),
        ("\"Data_block_1\".Real1", "1.5"),
        ("\"Data_block_1\".Flag", "true"),
        ("\"Data_block_1\".Str", "hello"),
        ("\"Data_block_1\".WStr", "añ€"),
        ("\"Data_block_1\".Arr[5]", "99"),
        ("\"Data_block_1\".Mat[1,2]", "-3"),
        ("\"Data_block_1\".S.x", "12"),
        ("\"Data_block_1\".SA[1].y", "2.5"),
        ("\"Data_block_1\".Stamp", "2024-05-06T07:08:09.25"),
        ("\"Data_block_1\".D", "2001-09-11"),
        ("\"Data_block_1\".Tod", "01:02:03.004"),
        ("\"Data_block_1\".T", "-2s"),
        ("\"Data_block_1\".S5", "10s"),
        ("\"Data_block_1\".LT", "1h"),
        ("\"Data_block_1\".LTod", "13:00:00"),
        ("\"Data_block_1\".Ldt", "2030-01-01T00:00:00"),
        ("\"Data_block_1\".Dt", "2020-02-03T04:05:06.007"),
        ("\"Data_block_1\".Ch", "Z"),
        ("\"Data_block_1\".WCh", "ß"),
        ("\"Data_block_1\".LR", "-0.125"),
        ("\"Data_block_1\".UL", "123"),
        ("\"Data_block_1\".LI", "-123"),
        ("\"Data_block_1\".LW", "65536"),
        ("\"Data_block_1\".US", "200"),
        ("\"Data_block_1\".UI", "60000"),
        ("\"Data_block_1\".UD", "70000"),
        ("\"Data_block_1\".SI", "-100"),
        ("\"Data_block_1\".B", "16"),
        ("\"Data_block_1\".W", "4660"),
        ("\"Data_block_1\".DW", "1"),
        ("\"Data_block_1\".DI", "-1"),
        ("\"Data_block_1\".Bools[9]", "true"),
        ("\"Std_DB\".A", "5"),
        ("Motor_On", "true"),
        ("Speed", "1500"),
    ];
    let mut tags: Vec<Tag> = vals.iter().map(|(n, _)| c.resolve_tag(n).unwrap_or_else(|e| panic!("resolve {n}: {e}"))).collect();
    let text: Vec<&str> = vals.iter().map(|(_, v)| *v).collect();
    c.set_chunk_sizes(7, 5); // force several requests
    f.lock().model.max_items = 0;
    let errs = c.write_tags(&tags, &text).unwrap();
    for (i, e) in errs.iter().enumerate() {
        assert!(e.is_none(), "write {}: {e:?}", tags[i].name);
    }
    let res = c.read_tags(&mut tags).unwrap();
    for (i, r) in res.iter().enumerate() {
        let t = &tags[i];
        let x = r.value.as_ref().unwrap_or_else(|| panic!("read {}: {:?}", t.name, r.err));
        let back = encode_plc(t, &format_plc_input(t, x));
        let orig = encode_plc(t, text[i]).unwrap();
        assert!(
            back.as_ref().ok().map(|b| b.to_string()) == Some(orig.to_string()),
            "{}: wrote {:?} read {} ({:?})",
            t.name,
            text[i],
            format_plc(t, x),
            back.err()
        );
    }
    assert!(f.lock().model.max_items <= 7, "chunking: {} items in one request", f.lock().model.max_items);
    // whole array read
    let arr = c.resolve_tag("\"Data_block_1\".Arr").unwrap();
    assert!(arr.array && arr.count == 5, "Arr tag {arr:?}");
    let mut at = vec![arr];
    let res = c.read_tags(&mut at).unwrap();
    assert_eq!(format_plc(&at[0], res[0].value.as_ref().unwrap()), "[0, 0, 0, 0, 99]");
    // DTL keeps the interface timestamp learned on read
    let mut dtl = vec![c.resolve_tag("\"Data_block_1\".Stamp").unwrap()];
    c.read_tags(&mut dtl).unwrap();
    assert_eq!(dtl[0].packed_ts, DEFAULT_DTL_TIMESTAMP);
    // errors
    for bad in [
        "\"Data_block_1\".Nope",
        "\"Data_block_1\".Arr[6]",
        "\"Data_block_1\".S",
        "\"Data_block_1\".Mat[1]",
        "\"Data_block_1\"",
        "NoSuchTag",
        "\"Unclosed",
    ] {
        assert!(c.resolve_tag(bad).is_err(), "{bad} resolved");
    }
    assert!(matches!(c.resolve_tag("NoSuchTag"), Err(Error::NotFound(_))), "not found error");
    // type mismatch on write is reported per item
    let mut wrong = tags[0].clone();
    wrong.softdatatype = SDT_LREAL;
    let errs = c.write_tags(&[wrong], &["1"]).unwrap();
    assert!(matches!(errs[0], Some(Error::Item(_))), "type mismatch write: {:?}", errs[0]);
    // classic addresses of standard DBs and tag tables
    let m = c.classic_addresses(&["\"Std_DB\".Bit", "Motor_On", "Speed", "\"Data_block_1\".Int1"]).unwrap();
    assert_eq!(m.get("\"Std_DB\".Bit").map(|s| s.as_str()), Some("DB2.DBX2.3"));
    assert_eq!(m.get("Motor_On").map(|s| s.as_str()), Some("M10.2"));
    assert_eq!(m.get("Speed").map(|s| s.as_str()), Some("MW12:int"));
    assert!(!m.contains_key("\"Data_block_1\".Int1"), "optimized DB mapped");
}

// A Bool of an optimized S7-1500 DB has the softdatatype BBool: it reads and
// writes like a Bool (PLCSIM FW S4.1: "writing BBool is not supported").
#[test]
fn bbool_write_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let mut t = vec![c.resolve_tag("\"Data_block_1\".BFlag").unwrap()];
    assert_eq!(t[0].softdatatype, SDT_BBOOL);
    for v in ["true", "false", "1"] {
        let errs = c.write_tags(&t, &[v]).unwrap();
        assert!(errs[0].is_none(), "write {v}: {:?}", errs[0]);
        let r = c.read_tags(&mut t).unwrap();
        assert_eq!(r[0].value, Some(PlcValue::Bool(v != "false")), "read after write {v}");
    }
    assert!(c.write_tags(&t, &["maybe"]).unwrap()[0].is_some(), "bad bool accepted");
}

// S7-1500 FW V4.x sets the optimized-access flag in standard DBs too (#10).
#[test]
fn fw4_standard_db_fake() {
    let f = FakePlc::start().unwrap();
    f.lock().model.use_fw4_flags();
    let c = dial(&f, Options::default());
    let dbs = c.list_datablocks(true).unwrap();
    assert!(dbs.len() == 2 && dbs[0].optimized && !dbs[1].optimized, "dbs {dbs:?}");
    let m = c.classic_addresses(&["\"Std_DB\".Bit", "\"Data_block_1\".Int1"]).unwrap();
    assert_eq!(m.get("\"Std_DB\".Bit").map(|s| s.as_str()), Some("DB2.DBX2.3"));
    assert!(!m.contains_key("\"Data_block_1\".Int1"), "optimized DB mapped");
}

#[test]
fn subscription_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let speed = c.resolve_tag("Speed").unwrap();
    let m300 = ItemAddress { area: RID_M_AREA, sub_area: ATTR_CTRL_AREA_VALUE_ACT, lid: vec![3, 300, 2], ..Default::default() };
    assert!(
        c.subscribe(std::slice::from_ref(&m300), SubOptions { attr1055: true, ..Default::default() }).is_err(),
        "attribute 1055 accepted (the FW V3 refuses it)"
    );
    let mut sub = c
        .subscribe(&[m300, speed.addr.clone()], SubOptions { cycle_ms: 10, credit_limit: 3, credit_step: 4, ..Default::default() })
        .unwrap();
    let n = sub.next(&c, Duration::from_secs(1)).unwrap();
    assert!(
        n.value(1).map(|v| v.to_string()) == Some("Blob(2) 0000".into()) && n.value(2).map(|v| v.to_string()) == Some("Int 0".into()),
        "initial {n}"
    );
    let errs = c.write_tags(&[speed], &["77"]).unwrap();
    assert!(errs[0].is_none());
    let mut got = false;
    let mut ticks = 0;
    for _ in 0..40 {
        let n = sub.next(&c, Duration::from_secs(1)).unwrap();
        ticks += 1;
        if n.value(2).map(|v| v.to_string()) == Some("Int 77".into()) {
            got = true;
            break;
        }
    }
    assert!(got, "no notification with the new value");
    // keep reading: without credit renewal the fake stops after 3 ticks
    for i in 0..12 {
        if let Err(e) = sub.next(&c, Duration::from_secs(1)) {
            panic!("after {} notifications: {e}", ticks + i);
        }
    }
    let sets = f.lock().model.credit_sets.clone();
    assert!(!sets.is_empty() && sets[0] == 7, "credit renewals {sets:?}");
    // a request between notifications still works (no-reply SetVariable)
    c.read_limits().unwrap();
    sub.close(&c).unwrap();
    assert_eq!(f.subscriptions(), 0, "subscriptions left");
}

// S7-1500 FW V4.x (PLCSIM S4.1): with cycle time 0 only the first
// notification comes; with a cycle time the changes come too.
#[test]
fn subscription_needs_cycle_fake() {
    let f = FakePlc::start().unwrap();
    f.lock().model.sub_needs_cycle = true;
    let c = dial(&f, Options::default());
    let speed = c.resolve_tag("Speed").unwrap();
    let wait_for = |cycle_ms: u32, value: &str| {
        let mut sub =
            c.subscribe(std::slice::from_ref(&speed.addr), SubOptions { cycle_ms, credit_limit: -1, ..Default::default() }).unwrap();
        let first = sub.next(&c, Duration::from_secs(1)).unwrap();
        assert!(first.value(1).is_some(), "cycle {cycle_ms}: no first notification: {first}");
        assert!(c.write_tags(std::slice::from_ref(&speed), &[value]).unwrap()[0].is_none());
        let want = format!("Int {value}");
        let t0 = std::time::Instant::now();
        let mut got = false;
        while t0.elapsed() < Duration::from_millis(300) {
            match sub.next(&c, Duration::from_millis(50)) {
                Ok(n) if n.value(1).map(|v| v.to_string()) == Some(want.clone()) => {
                    got = true;
                    break;
                }
                Ok(_) => {}
                Err(e) if e.is_timeout() => {}
                Err(e) => panic!("cycle {cycle_ms}: {e}"),
            }
        }
        sub.close(&c).unwrap();
        got
    };
    assert!(!wait_for(0, "11"), "cycle 0: a change came");
    assert!(wait_for(100, "12"), "cycle 100: no change came");
}

#[test]
fn subscription_run_with_callback() {
    let f = FakePlc::start().unwrap();
    let c = Arc::new(dial(&f, Options::default()));
    let speed = c.resolve_tag("Speed").unwrap();
    let mut sub = c.subscribe(std::slice::from_ref(&speed.addr), SubOptions { credit_limit: -1, ..Default::default() }).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (c2, s2) = (c.clone(), stop.clone());
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        sub.run(&c2, Duration::from_millis(20), &s2, |n| {
            if let Some(v) = n.value(1) {
                seen.push(v.to_string());
            }
        })
        .unwrap();
        sub.close(&c2).unwrap();
        seen
    });
    // the other thread keeps reading and writing meanwhile
    for v in ["11", "22"] {
        let errs = c.write_tags(std::slice::from_ref(&speed), &[v]).unwrap();
        assert!(errs[0].is_none());
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let seen = h.join().unwrap();
    assert!(seen.contains(&"Int 11".to_string()) && seen.contains(&"Int 22".to_string()), "seen {seen:?}");
}

#[test]
fn operating_state_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let st = c.operating_state().unwrap();
    assert!(st == 8 && crate::operating_state_name(st) == "RUN", "state {st}");
    c.set_plc_operating_state(OP_STATE_STOP).unwrap();
    assert_eq!(c.operating_state().unwrap(), 1, "after STOP");
    c.set_plc_operating_state(OP_STATE_RUN).unwrap();
    assert_eq!(f.lock().model.op_reqs, vec![1, 3]);
}

#[test]
fn legacy_legitimation_fake() {
    let f = FakePlc::start().unwrap();
    {
        let mut s = f.lock();
        s.paom = "1;6ES7 214-1AG40-0XB0 ;V4.5".into();
        s.model.protection = ACCESS_HMI;
        s.model.password = "s3cret".into();
    }
    let r = Client::connect(Options { addr: f.addr(), timeout: Duration::from_secs(2), password: "wrong".into(), ..Default::default() });
    assert!(matches!(r, Err(Error::AccessDenied(_))), "wrong password: {:?}", r.err());
    let c = dial(&f, Options { password: "s3cret".into(), ..Default::default() });
    {
        let s = f.lock();
        assert!(s.model.legit_ok && s.model.legit_kind == "legacy", "legitimation ok={} kind={}", s.model.legit_ok, s.model.legit_kind);
    }
    assert_eq!(c.protection_level().unwrap(), ACCESS_FULL);
    // FW V3 (no TLS, below the reference's minimum): refused locally
    let f2 = FakePlc::start().unwrap();
    f2.lock().model.protection = ACCESS_HMI;
    let r = Client::connect(Options { addr: f2.addr(), timeout: Duration::from_secs(2), password: "x".into(), ..Default::default() });
    match r {
        Err(e) => assert!(e.to_string().contains("not supported"), "FW V3 legitimation: {e}"),
        Ok(_) => panic!("FW V3 legitimation accepted"),
    }
}

#[test]
fn session_without_tls_refused_fake() {
    // PLCSIM Advanced V8 (FW S4.1) accepts InitSsl but refuses a session
    // without TLS with a negative return value and no session id.
    let f = FakePlc::start().unwrap();
    f.enable_tls().unwrap();
    {
        let mut s = f.lock();
        s.paom = "1;6ES7 SIM-01500-APLC;S4.1".into();
        s.require_tls = true;
    }
    let r = Client::connect(Options { addr: f.addr(), timeout: Duration::from_secs(2), ..Default::default() });
    let e = match r {
        Ok(_) => panic!("session without TLS accepted"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("without TLS") && e.contains("0xa201d6000214fdf8") && e.contains("enable TLS"), "error: {e}");
    let c = dial(&f, Options { tls: Some(TlsOptions::default()), ..Default::default() });
    assert!(c.info().tls);
}

#[test]
fn tls_and_new_legitimation_fake() {
    let f = FakePlc::start().unwrap();
    f.enable_tls().unwrap();
    {
        let mut s = f.lock();
        s.paom = "1;6ES7 510-1DJ01-0AB0;V3.1".into();
        s.model.protection = ACCESS_NONE;
        s.model.user = "operator".into();
        s.model.password = "pässword".into();
    }
    let keylog = SharedBuf::default();
    let tls = TlsOptions { key_log: Some(Arc::new(NssKeyLog::new(keylog.clone()))), ..Default::default() };
    let c = dial(&f, Options { tls: Some(tls), username: "operator".into(), password: "pässword".into(), ..Default::default() });
    let kl = String::from_utf8_lossy(&keylog.contents()).into_owned();
    assert!(
        !c.init_ssl_refused() && c.oms_secret().map(|s| s.len()) == Some(32) && kl.contains("CLIENT_TRAFFIC_SECRET_0"),
        "TLS session: refused={} secret={:?}",
        c.init_ssl_refused(),
        c.oms_secret().map(|s| s.len())
    );
    assert!(c.info().tls);
    {
        let s = f.lock();
        assert!(s.model.legit_ok && s.model.legit_kind == "new", "legitimation ok={} kind={}", s.model.legit_ok, s.model.legit_kind);
    }
    // a PDU larger than one fragment, both ways, through TLS
    let big = vec![0x5au8; 3000];
    let a = ItemAddress { area: RID_M_AREA, sub_area: ATTR_CTRL_AREA_VALUE_ACT, lid: vec![3, 1000, 3000], ..Default::default() };
    let errs = c.write(std::slice::from_ref(&a), &[Value::blob(big.clone())]).unwrap();
    assert_eq!(errs[0], 0, "write");
    let (vals, errs) = c.read(&[a]).unwrap();
    assert!(errs[0] == 0 && vals[0].v.as_bytes() == Some(&big[..]), "read back over TLS: {errs:?}");
    // Explore over TLS (integrity id): PLCSIM FW S4.1 needs the trailing 0 byte
    let dbs = c.list_datablocks(false).unwrap_or_else(|e| panic!("Explore over TLS: {e}"));
    assert!(!dbs.is_empty(), "no DBs over TLS");
    // TLS asked to a PLC without TLS
    let f2 = FakePlc::start().unwrap();
    let r = Client::connect(Options {
        addr: f2.addr(),
        timeout: Duration::from_secs(2),
        tls: Some(TlsOptions::default()),
        ..Default::default()
    });
    assert!(r.is_err(), "TLS against a PLC without TLS succeeded");
}

#[test]
fn alarms_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let al = c.active_alarms(false).unwrap();
    assert_eq!(al.len(), 1, "alarms {al:?}");
    let a = &al[0];
    let ok = a.cpu_alarm_id == (db_area(1) as u64) << 32 | 1 << 16
        && a.coming.as_ref().and_then(|c| c.timestamp).map(|t| t.civil().year) == Some(2023)
        && a.coming.as_ref().map(|c| c.assoc_values.len()) == Some(2)
        && a.hmi.map(|h| (h.priority, h.alarm_class)) == Some((5, 3))
        && a.texts.get(&(1033 << 16 | 2)).map(|s| s.as_str()) == Some("Motor overload")
        && a.seq_counter == 4;
    assert!(ok, "alarm {a} {:?}", a.hmi);
    let texts = c.alarm_texts(1033).unwrap();
    let at = texts.get(&a.cpu_alarm_id).expect("texts");
    assert!(
        at.texts.get(&1).map(|s| s.as_str()) == Some("Motor overload")
            && at.texts.get(&0).map(|s| s.as_str()) == Some("Check the motor")
            && at.unknown2 == 9,
        "texts {at:?}"
    );
    assert!(c.alarm_texts(1031).is_err(), "missing language accepted");
    let sub = c.subscribe_alarms(&AlarmSubOptions::default()).unwrap();
    sub.close(&c).unwrap();
    // all DAIs of the pool explored with request id 0 are not served by the
    // fake (error), like a PLC without the pool attribute
    assert!(c.active_alarms(true).is_err());
}

// Alarm notifications: a DAI per coming / going. S7-1500 FW V4.x (PLCSIM S4.1)
// sends them only to the S7CommPlusDriver layout.
#[test]
fn alarm_subscription_fake() {
    let wait_alarms = |c: &Client, sub: &mut crate::Subscription, ms: u64| {
        let mut got = Vec::new();
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_millis(ms) {
            match sub.next(c, Duration::from_millis(50)) {
                Ok(n) => got.extend(n.alarm_objects.iter().map(decode_alarm)),
                Err(e) if e.is_timeout() => {}
                Err(e) => panic!("{e}"),
            }
        }
        got
    };
    let id = (db_area(5) as u64) << 32 | 1 << 16;
    for (fw4, reference, want) in [(false, false, true), (true, false, false), (true, true, true)] {
        let f = FakePlc::start().unwrap();
        if fw4 {
            f.lock().model.alarm_needs_reference = true;
        }
        let c = dial(&f, Options::default());
        let mut sub = c.subscribe_alarms(&AlarmSubOptions { reference, languages: vec![1033], with_texts: true }).unwrap();
        f.raise_alarm(id, true, vec![vec![1, 1, 0, 0], vec![0, 77]]);
        f.raise_alarm(id, false, vec![]);
        let got = wait_alarms(&c, &mut sub, 300);
        let case = format!("fw4={fw4} reference={reference}");
        if !want {
            assert!(got.is_empty(), "{case}: {got:?}");
            sub.close(&c).unwrap();
            continue;
        }
        assert_eq!(got.len(), 2, "{case}: {got:?}");
        let (on, off) = (&got[0], &got[1]);
        assert!(on.cpu_alarm_id == id && on.all_states == 0x87 && on.going.is_none(), "{case}: coming {on}");
        assert_eq!(on.coming.as_ref().map(|x| x.assoc_values[1].clone()), Some(vec![0, 77]), "{case}");
        assert!(off.all_states == 0x86 && off.going.is_some() && off.coming.is_none(), "{case}: going {off}");
        assert!(c.active_alarms(false).unwrap().iter().all(|a| a.cpu_alarm_id != id), "{case}: still active");
        sub.close(&c).unwrap();
    }
    // the layout by PLC
    assert!(!AlarmSubOptions::for_plc("1;6ES7 212-1BE31-0XB0 ;V3.0").reference);
    assert!(AlarmSubOptions::for_plc("1;6ES7 SIM-01500-APLC;S4.1").reference);
    assert!(AlarmSubOptions::for_plc("1;6ES7 513-1AL02-0AB0 ;V2.9").reference);
}

#[test]
fn comments_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    c.dictionaries().register(TEST_DICT);
    let cm = c.get_comments(db_area(1)).unwrap();
    assert!(
        cm.object.get(&1033).map(|s| s.as_str()) == Some("Test DB") && cm.line_error.is_none() && cm.line_xml.contains("Speed setpoint"),
        "comments {cm:?}"
    );
}

#[test]
fn alarm_notification_part2() {
    let md = FakeModel::new();
    let mut w = Writer::new();
    w.u32(0x1000006c);
    w.u16(0x0400);
    w.u16(0);
    w.u16(0);
    w.byte(0);
    w.vlq32(3);
    w.byte(0); // no items
    w.u32(0x1000006d);
    w.u16(0);
    w.byte(0x81);
    md.dais[0].encode(&mut w);
    w.u32(0);
    let n = decode_notification(&w.b).unwrap();
    assert!(n.alarm_sub_id == 0x1000006d && n.alarm_objects.len() == 1, "{n}");
    let a = decode_alarm(&n.alarm_objects[0]);
    assert!(a.texts.get(&(1033 << 16 | 2)).map(|s| s.as_str()) == Some("Motor overload") && a.coming.is_some(), "alarm {a}");
}

#[test]
fn logger_traces_frames() {
    let f = FakePlc::start().unwrap();
    let lines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let l2 = lines.clone();
    let logger = move |lvl: crate::Level, msg: &str| l2.lock().unwrap().push(format!("{lvl:?} {msg}"));
    let c = dial(&f, Options { logger: Some(Arc::new(logger)), ..Default::default() });
    c.read_limits().unwrap();
    drop(c);
    let lines = lines.lock().unwrap();
    assert!(lines.iter().any(|l| l.starts_with("Trace s7plus: > V2 Request GetMultiVariables")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("Info s7plus: session 0x39c")), "{lines:?}");
    let _ = DateTime::default();
}

fn tls_dial(f: &FakePlc, verify: CertVerify, name: &str) -> Result<Client, Error> {
    Client::connect(Options {
        addr: f.addr(),
        timeout: Duration::from_secs(2),
        tls: Some(TlsOptions { verify, server_name: name.into(), ..Default::default() }),
        ..Default::default()
    })
}

fn refused(r: Result<Client, Error>, what: &str) -> Error {
    match r {
        Ok(_) => panic!("{what}"),
        Err(e) => e,
    }
}

#[test]
fn certificate_verification_fake() {
    let f = FakePlc::start().unwrap();
    f.enable_tls().unwrap();
    let der = f.tls_cert().unwrap();
    let fp = crate::tls::fingerprint_text(&crate::tls::fingerprint(&der));
    // no check: any certificate; the session keeps the PLC certificate
    let c = tls_dial(&f, CertVerify::None, "").unwrap();
    assert_eq!(c.info().peer_cert.as_deref(), Some(&der[..]));
    // pinned fingerprint
    assert!(tls_dial(&f, CertVerify::fingerprints(&fp).unwrap(), "").is_ok());
    let e = refused(tls_dial(&f, CertVerify::fingerprints(&"00".repeat(32)).unwrap(), ""), "wrong fingerprint accepted");
    let t = e.to_string();
    assert!(
        e.refused_certificate() == Some(&der[..]) && t.contains(&fp) && t.contains("does not match the fingerprint"),
        "unexpected certificate error"
    );
    // the self-signed certificate as trusted certificate (SAN fake-plc, 127.0.0.1)
    let pem = crate::x509::pem(&der);
    assert!(tls_dial(&f, CertVerify::trusted_pem(&pem).unwrap(), "").is_ok());
    assert!(tls_dial(&f, CertVerify::trusted_pem(&pem).unwrap(), "fake-plc").is_ok());
    let e = refused(tls_dial(&f, CertVerify::trusted_pem(&pem).unwrap(), "other-plc"), "wrong name accepted").to_string();
    assert!(e.contains("not valid for this name") && e.contains("other-plc"), "unexpected certificate error");
    // another certificate as trusted certificate
    let other = FakePlc::start().unwrap();
    other.enable_tls().unwrap();
    let pem2 = crate::x509::pem(&other.tls_cert().unwrap());
    let e = refused(tls_dial(&f, CertVerify::trusted_pem(&pem2).unwrap(), ""), "untrusted accepted").to_string();
    assert!(e.contains("not signed by a trusted certificate") && e.contains("CN=PLC-1/Communication-1"), "unexpected certificate error");
}

/// A caller TLS configuration and a certificate check cannot be combined
/// (the check would be ignored).
#[test]
fn certificate_check_with_caller_config_refused() {
    let f = FakePlc::start().unwrap();
    f.enable_tls().unwrap();
    let p = crate::tls::provider();
    let cfg = rustls::ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(crate::tls::NoCertVerification(p)))
        .with_no_client_auth();
    let tls = TlsOptions { config: Some(Arc::new(cfg)), verify: CertVerify::fingerprints(&"00".repeat(32)).unwrap(), ..Default::default() };
    let r = Client::connect(Options { addr: f.addr(), timeout: Duration::from_secs(2), tls: Some(tls), ..Default::default() });
    let e = refused(r, "wrong fingerprint accepted with a caller config");
    assert!(matches!(&e, Error::Certificate(t) if t.contains("not both")), "unexpected error");
}

#[test]
fn certificate_signed_by_project_ca_fake() {
    let f = FakePlc::start().unwrap();
    let ca = f.enable_tls_signed(&["127.0.0.1"]).unwrap();
    let ca_pem = crate::x509::pem(&ca);
    assert!(tls_dial(&f, CertVerify::trusted_pem(&ca_pem).unwrap(), "").is_ok(), "chain to the project CA");
    let info = crate::x509::parse(&f.tls_cert().unwrap()).unwrap();
    assert!(!info.self_signed && info.issuer == "CN=Fake project CA" && info.alt_names == ["IP:127.0.0.1"], "{info:?}");
    assert!(crate::x509::parse(&ca).unwrap().ca);
    // the PLC certificate itself as trusted certificate (editor "Use this certificate"):
    // accepted without the CA; the name is still checked
    let leaf_pem = crate::x509::pem(&f.tls_cert().unwrap());
    assert!(tls_dial(&f, CertVerify::trusted_pem(&leaf_pem).unwrap(), "").is_ok(), "CA-signed PLC certificate trusted by itself");
    let e = refused(tls_dial(&f, CertVerify::trusted_pem(&leaf_pem).unwrap(), "plc.local"), "accepted").to_string();
    assert!(e.contains("not valid for this name"), "unexpected error");
    // the name check uses the address when no name is set: a DNS name fails
    let e = refused(tls_dial(&f, CertVerify::trusted_pem(&ca_pem).unwrap(), "plc.local"), "accepted").to_string();
    assert!(e.contains("not valid for this name"), "unexpected certificate error");
}

#[test]
fn alarm_acknowledge_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    let mut sub = c.subscribe_alarms(&AlarmSubOptions::for_plc(&c.info().paom())).unwrap();
    let (need, no_need) = (0x8a0e_0005_0001_0000u64, 0x8a0e_0005_0002_0000u64);
    f.raise_alarm_ack(need, true, vec![], true);
    f.raise_alarm_ack(no_need, true, vec![], false);
    let mut seen = 0;
    while seen < 2 {
        let n = sub.next(&c, Duration::from_secs(2)).unwrap();
        seen += n.alarm_objects.iter().filter(|o| o.class == CLASS_DAI).count();
    }
    let states = |id: u64| c.active_alarms(false).unwrap().iter().find(|a| a.cpu_alarm_id == id).map(|a| a.all_states);
    assert_eq!(states(need), Some(0x85), "waits for acknowledgement");
    c.acknowledge_alarms(&[need, no_need]).unwrap();
    let n = sub.next(&c, Duration::from_secs(2)).unwrap();
    let res = n.alarm_objects.iter().find_map(crate::decode_ack_job).expect("AckJob in the notification");
    assert_eq!(
        res,
        vec![crate::AckResult { cpu_alarm_id: need, result: 0 }, crate::AckResult { cpu_alarm_id: no_need, result: 0x8005u16 as i16 }]
    );
    assert_eq!(states(need), Some(0x87), "acknowledged");
    assert!(crate::decode_ack_job(&crate::object::Object::new(1, CLASS_DAI)).is_none());
    assert!(c.acknowledge_alarms(&[]).is_err() && c.acknowledge_alarms(&[0]).is_err());
}

#[test]
fn links_and_resolve_fake() {
    let f = FakePlc::start().unwrap();
    let c = dial(&f, Options::default());
    // the fake starts with one active alarm (DAI 0x10000006)
    assert_eq!(c.get_links(RID_ALARM_SUBSYSTEM, ATTR_SUBSYS_UPD_RELEVANT_DAI).unwrap(), vec![0x1000_0006]);
    assert!(matches!(c.get_links(RID_ALARM_SUBSYSTEM, 2665), Err(Error::Return { .. })));
    assert_eq!(c.resolve_object("ASRoot").unwrap(), Some(1));
    assert_eq!(c.resolve_object("ASRoot.PLCProgram").unwrap(), Some(3));
    assert_eq!(c.resolve_object(&crate::program_object_path("Data_block_1")).unwrap(), Some(db_area(1)));
    assert_eq!(c.resolve_object(&crate::program_object_path("1Odd")).unwrap(), Some(db_area(7)));
    assert_eq!(c.resolve_object("ASRoot.PLCProgram.Nope").unwrap(), None);
    assert!(c.resolve_object("").is_err());
    assert_eq!(crate::program_object_path("a.b[1]-c$"), "ASRoot.PLCProgram.a$.b$[1$]$-c$$");
}
