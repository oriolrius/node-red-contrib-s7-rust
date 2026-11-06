//! Feature tests: TSAP override, families, byte order, gap
//! spanning, clock, reconnect, scan engine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::TimeDelta;

use super::{dial_fake, fixed_clock, hex, opts, start_fake};
use crate::addr::parse_tag;
use crate::client::{Client, State};
use crate::clock::{decode_clock, encode_clock};
use crate::cpu::{parse_family, Family};
use crate::error::Error;
use crate::poll::{Mode, PollConfig, PollValue, Poller, Quality, ScanGroup};
use crate::textfmt::Zone;
use crate::wire::{cotp_connect_request, DialOptions, Item, AREA_DB, AREA_FLAGS};

/// A5: an explicit remote TSAP replaces the rack/slot form.
#[test]
fn remote_tsap_override() {
    let cr = cotp_connect_request(&DialOptions {
        link_type: 3,
        rack: 0,
        slot: 1,
        local_tsap: 0x4D57,
        remote_tsap: 0x4D57,
        ..Default::default()
    });
    assert_eq!(hex(&cr[cr.len() - 8..]), "c1024d57c2024d57");
}

/// A6: family presets and limits.
#[test]
fn families() {
    let mut o = DialOptions::default();
    Family::S7300.apply(&mut o, false, false, false);
    assert!(o.slot == 2 && o.link_type == 1, "S7-300 preset = {o:?}");
    let mut o = DialOptions::default();
    Family::S7200.apply(&mut o, false, false, false);
    assert_eq!((o.local_tsap, o.remote_tsap), (0x4D57, 0x4D57));
    let mut o = DialOptions { slot: 4, ..Default::default() };
    Family::S71500.apply(&mut o, false, true, false); // explicit slot wins
    assert_eq!(o.slot, 4);
    let cases = [
        (Family::S71200, "T0", false),
        (Family::S7300, "T0", true),
        (Family::S71200, "MD0:lreal", true),
        (Family::S7300, "MD0:lreal", false),
        (Family::S71200, "MB0:lint", false),
        (Family::S71500, "MB0:lint", true),
        (Family::S71200, "MB0:dt", false),
        (Family::S71500, "MB0:dt", true),
        (Family::S71200, "MB0:string[220]", false),
    ];
    for (f, tag, ok) in cases {
        let tg = parse_tag(tag).unwrap();
        assert_eq!(f.check(&tg).is_ok(), ok, "{f} {tag}");
    }
    assert_eq!(parse_family("S7-1500").unwrap(), Family::S71500);
}

/// C8: little-endian byte order swaps each value.
#[test]
fn byte_order() {
    let mut tg = parse_tag("MD0:dint").unwrap();
    tg.little_endian = true;
    let b = tg.value("1").unwrap();
    assert_eq!(hex(&b), "01000000");
    assert_eq!(tg.format(&b).unwrap(), "1");
    let mut arr = parse_tag("MW0[2]:int").unwrap();
    arr.little_endian = true;
    assert_eq!(hex(&arr.value("1,2").unwrap()), "01000200");
    let mut s = parse_tag("MB0:string[4]").unwrap();
    s.little_endian = true;
    assert_eq!(hex(&s.value("ab").unwrap()[..4]), "04026162", "string must not swap");
}

/// B8: gap spanning merges nearby items, extracts bits, and isolates errors.
#[test]
fn read_optimized() {
    let (mut cn, f) = dial_fake(240);
    {
        let mut s = f.lock();
        let m = s.mem(AREA_FLAGS, 0);
        m[10] = 0x12;
        m[11] = 0x34;
        m[14] = 0x08;
        m[20] = 0xAB;
    }
    let items = [
        Item::bytes(AREA_FLAGS, 0, 10, 2), // MW10
        Item::bit(AREA_FLAGS, 0, 14, 3),   // M14.3 = 1
        Item::bytes(AREA_FLAGS, 0, 20, 1), // MB20
        Item::bytes(AREA_DB, 2, 0, 2),     // DB2 does not exist
        Item::bytes(AREA_DB, 1, 1990, 20), // DB1 ends at 2000 → error
        Item::bytes(AREA_DB, 1, 0, 2),     // DB1.DBW0 fine, merged with the failing one
    ];
    f.reset_reqs();
    let res = cn.read_optimized(&items, 16).unwrap();
    assert_eq!(hex(res[0].as_ref().unwrap()), "1234");
    assert_eq!(res[1].as_ref().unwrap()[0], 1);
    assert_eq!(res[2].as_ref().unwrap()[0], 0xAB);
    assert!(res[3].is_err() && res[4].is_err(), "expected errors for DB2 and DB1 out of range");
    assert!(res[5].is_ok(), "DB1.DBW0 must survive the failing neighbour: {:?}", res[5]);
}

/// H3: clock frames and round trip.
#[test]
fn clock() {
    let tm = fixed_clock(); // Tuesday
    assert_eq!(hex(&encode_clock(&tm)), "00202611032130151233");
    let (mut cn, f) = dial_fake(240);
    let got = cn.read_clock(Zone::Local).unwrap();
    assert_eq!(got, tm);
    // Threshold larger than the offset: no sync.
    let (_, synced) = cn.sync_clock(&(tm + TimeDelta::seconds(2)), TimeDelta::seconds(10), Zone::Local).unwrap();
    assert!(!synced, "sync under threshold");
    // Over threshold: set to now.
    let (off, synced) = cn.sync_clock(&Zone::Local.now(), TimeDelta::seconds(1), Zone::Local).unwrap();
    assert!(synced && off != TimeDelta::zero(), "sync over threshold: off={off} synced={synced}");
    let now = decode_clock(&f.clock(), Zone::Local).unwrap();
    let d = Zone::Local.now().signed_duration_since(now);
    assert!(d.abs() < TimeDelta::seconds(2), "clock after sync off by {d}");
}

/// A9 + E3 + E4: reconnect after a broken link; state and stats.
#[test]
fn client_reconnect() {
    let (f, addr) = start_fake(240);
    f.set_drop_after(1); // first connection dies after one job
    let mut o = opts(&addr);
    o.timeout = Duration::from_secs(1);
    let c = Client::new(o);
    for i in 0..3 {
        c.run(|cn| cn.read(&[Item::bytes(AREA_FLAGS, 0, 0, 2)])).unwrap_or_else(|e| panic!("read {i}: {e}"));
    }
    assert_eq!((c.reconnects(), f.conns()), (1, 2));
    assert_eq!(c.status().0, State::Connected);
    let s = c.stats().snapshot();
    assert!(s.requests >= 5 && s.errors >= 1 && !s.max.is_zero(), "stats = {s:?}");
    c.close();
    // Unreachable device → faulted.
    let bad =
        Client::new(DialOptions { addr: "127.0.0.1:1".into(), timeout: Duration::from_millis(200), ..Default::default() }).with_attempts(1);
    assert!(bad.run(|_| Ok(())).is_err(), "expected dial error");
    let (st, d) = bad.status();
    assert!(st == State::Faulted && !d.is_empty(), "state = {st} {d:?}");
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timeout waiting for {what}");
}

fn value(p: &Poller, tag: &str) -> PollValue {
    p.values().into_iter().find(|v| v.tag == tag).unwrap_or_default()
}

fn dial(addr: &str) -> DialOptions {
    let mut o = opts(addr);
    o.timeout = Duration::from_secs(1);
    o
}

/// F1 + F3 + D2 + E4: scan engine with two connections, read-after-write, metrics.
#[test]
fn poller_fixed_and_read_after_write() {
    let (f, addr) = start_fake(240);
    f.set_mem(AREA_FLAGS, 0, 0, &[7]);
    let p = Arc::new(
        Poller::new(PollConfig {
            dial: dial(&addr),
            family: Some(Family::S71200),
            concurrency: 2,
            read_after_write: true,
            groups: vec![
                ScanGroup {
                    name: "fast".into(),
                    rate: Duration::from_millis(50),
                    tags: vec!["MB0".into(), "MW2:int".into()],
                    ..Default::default()
                },
                ScanGroup {
                    name: "slow".into(),
                    rate: Duration::from_millis(200),
                    tags: vec!["MD4:real".into()],
                    max_gap: -1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .unwrap(),
    );
    let h = p.start();
    wait_for("first scan", || value(&p, "MB0").value.starts_with("7 "));
    p.write("MW2:int", "-5").unwrap();
    let v = value(&p, "MW2:int");
    assert!(v.value == "-5" && v.quality == Quality::Good, "read-after-write value = {v:?}");
    wait_for("several scans", || p.metrics()[0].cycles >= 4);
    let m = &p.metrics()[0];
    assert!(
        m.target_interval_ms == 50.0 && m.achieved_interval_ms > 0.0 && m.request_count == 1 && m.monitored_points == 2,
        "metrics = {m:?}"
    );
    assert_eq!(f.conns(), 2, "connections (concurrency)");
    h.stop();
    assert!(p.write("MB0", "1").is_err(), "write after stop");
}

/// D3: optimistic write shows the value at once and reverts if not confirmed.
#[test]
fn poller_optimistic_revert() {
    let (f, addr) = start_fake(240);
    f.set_ignore_writes(true); // the "device" acknowledges but keeps the old value
    let p = Arc::new(
        Poller::new(PollConfig {
            dial: dial(&addr),
            optimistic_write: true,
            optimistic_timeout: Duration::from_millis(300),
            groups: vec![ScanGroup {
                name: "g".into(),
                rate: Duration::from_millis(50),
                tags: vec!["MW10:int".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap(),
    );
    let _h = p.start();
    wait_for("first scan", || value(&p, "MW10:int").value == "0");
    p.write("MW10:int", "99").unwrap();
    let v = value(&p, "MW10:int");
    assert!(v.value == "99" && v.pending, "optimistic value = {v:?}");
    wait_for("revert", || {
        let v = value(&p, "MW10:int");
        v.value == "0" && !v.pending
    });
}

/// F2: conditional and on-demand rates.
#[test]
fn poller_modes() {
    let (f, addr) = start_fake(240);
    let p = Arc::new(
        Poller::new(PollConfig {
            dial: dial(&addr),
            groups: vec![
                ScanGroup {
                    name: "cond".into(),
                    mode: Mode::Conditional,
                    rate: Duration::from_secs(3600),
                    cond_tag: "M0.0".into(),
                    cond_rate: Duration::from_millis(40),
                    tags: vec!["MB1".into()],
                    ..Default::default()
                },
                ScanGroup { name: "trigger".into(), rate: Duration::from_millis(30), tags: vec!["M0.0".into()], ..Default::default() },
                ScanGroup {
                    name: "demand".into(),
                    mode: Mode::OnDemand,
                    rate: Duration::from_millis(40),
                    idle_rate: Duration::ZERO,
                    tags: vec!["MB2".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
        .unwrap(),
    );
    let _h = p.start();
    std::thread::sleep(Duration::from_millis(300));
    let ms = p.metrics();
    let (cond0, demand0) = (ms[0].cycles, ms[2].cycles);
    assert!(cond0 <= 1 && demand0 == 0, "idle cycles: cond={cond0} demand={demand0}");
    f.set_mem(AREA_FLAGS, 0, 0, &[1]); // condition true
    p.watch("MB2"); // a client displays MB2
    wait_for("fast rates", || {
        let ms = p.metrics();
        ms[0].cycles >= cond0 + 3 && ms[2].cycles >= 3
    });
}

/// A device answer (item, protocol or userdata error) must not trigger a reconnect.
#[test]
fn no_retry_on_device_errors() {
    for e in [Error::Item(0x0A), Error::Proto(0x81, 0x04), Error::Userdata(0x8104), Error::Userdata(0xD401).context("SZL 0x001c/0")] {
        assert!(!e.retryable(), "{e} must not be retryable");
    }
    assert!(Error::from(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)).retryable(), "EOF must be retryable");
    // and a client does not reconnect on them
    let (f, addr) = start_fake(240);
    let c = Client::new(opts(&addr));
    let r = c.run(|cn| cn.read_szl(0x001C, 0));
    assert_eq!(r.unwrap_err().userdata_code(), Some(0xD401));
    assert_eq!((c.reconnects(), f.conns()), (0, 1));
}

/// F2: a condition tag that no other group reads is watched at the fast rate.
#[test]
fn poller_condition_watched_fast() {
    let (f, addr) = start_fake(240);
    let p = Arc::new(
        Poller::new(PollConfig {
            dial: dial(&addr),
            groups: vec![ScanGroup {
                name: "cond".into(),
                mode: Mode::Conditional,
                rate: Duration::from_secs(3600),
                cond_tag: "M0.0".into(),
                cond_rate: Duration::from_millis(40),
                cond_one_shot: true,
                tags: vec!["MB1".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap(),
    );
    let _h = p.start();
    std::thread::sleep(Duration::from_millis(200));
    let before = p.metrics()[0].cycles;
    f.set_mem(AREA_FLAGS, 0, 0, &[1]);
    wait_for("conditional fast rate", || p.metrics()[0].cycles >= before + 3);
    assert_eq!(p.metrics()[1].name, "cond.condition");
}

/// D4: write priority — queued writes run before queued reads, and the update
/// hook sees every value.
#[test]
fn poller_write_priority_and_hook() {
    let (f, addr) = start_fake(240);
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    let p = Arc::new(
        Poller::new(PollConfig {
            dial: dial(&addr),
            write_priority: 2,
            optimistic_write: true,
            groups: vec![ScanGroup {
                name: "g".into(),
                rate: Duration::from_millis(20),
                tags: vec!["MW20:int".into()],
                ..Default::default()
            }],
            on_update: Some(Arc::new(move |t, v: &PollValue| s2.lock().unwrap().push(format!("{}={}", t.text, v.value)))),
            ..Default::default()
        })
        .unwrap(),
    );
    let _h = p.start();
    let t0 = Instant::now();
    for i in 0..5 {
        p.write("MW20:int", &i.to_string()).unwrap();
    }
    assert!(t0.elapsed() < Duration::from_secs(2));
    wait_for("value 4", || value(&p, "MW20:int").value == "4" && !value(&p, "MW20:int").pending);
    assert_eq!(&f.mem(AREA_FLAGS, 0)[20..22], &[0, 4]);
    assert!(seen.lock().unwrap().iter().any(|s| s == "MW20:int=4"));
}
