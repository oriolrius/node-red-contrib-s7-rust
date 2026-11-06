//! Endpoint core: JSON-friendly reads, writes from values,
//! subscriptions.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::addr::parse_tag;
use crate::endpoint::{DataEvent, Endpoint, EndpointConfig, Subscription};
use crate::fakecpu::FakeCpu;
use crate::log::{Level, Log};
use crate::poll::Quality;
use crate::value::Val;

fn open(addr: &str, cpu: &str) -> Endpoint {
    let (host, port) = addr.rsplit_once(':').unwrap();
    let cfg = EndpointConfig {
        host: host.into(),
        port: port.parse().unwrap(),
        cpu: cpu.into(),
        timeout: Duration::from_secs(2),
        ..Default::default()
    };
    Endpoint::open("c1", &cfg, &Log::discard()).unwrap().0
}

#[test]
fn read_write_values() {
    let f = FakeCpu::new(480);
    f.add_db(100, 65536);
    let addr = f.start();
    let ep = open(&addr, "1500");
    let s = |x: &str| x.to_string();
    let errs = ep
        .write_values(&[
            (s("MW10:int"), Val::Int(-1234)),
            (s("MD12:real"), Val::Float(3.25)),
            (s("M20.3"), Val::Bool(true)),
            (s("DB1.DBB100:string[10]"), Val::Str(s("hello"))),
            (s("DB2.DBW0"), Val::Int(1)),
            (s("MW30[3]:int"), Val::List(vec![Val::Int(1), Val::Int(-2), Val::Int(3)])),
            (s("MW40"), Val::Null),
        ])
        .unwrap();
    assert!(errs[..4].iter().all(|e| e.is_none()), "{errs:?}");
    assert!(errs[4].is_some() && errs[5].is_none() && errs[6].as_deref() == Some("MW40: value is missing"), "{errs:?}");

    // 3000 consecutive INTs: gap spanning + PDU splitting
    let many: Vec<String> = (0..3000).map(|i| format!("DB100.DBW{}:int", i * 2)).collect();
    let items: Vec<(String, Val)> = many.iter().enumerate().map(|(i, t)| (t.clone(), Val::Int(i as i64))).collect();
    assert!(ep.write_values(&items).unwrap().iter().all(|e| e.is_none()));

    let mut tags = vec![s("MW10:int"), s("MD12:real"), s("M20.3"), s("DB1.DBB100:string[10]"), s("DB2.DBW0"), s("XX9"), s("MW30[3]:int")];
    tags.extend(many.iter().cloned());
    let r = ep.read_tags(&tags).unwrap();
    assert_eq!(r[0].value, Some(Val::Int(-1234)));
    assert_eq!(r[1].value, Some(Val::Float(3.25)));
    assert_eq!(r[2].value, Some(Val::Bool(true)));
    assert_eq!(r[3].value, Some(Val::Str(s("hello"))));
    assert!(r[4].quality == Quality::Bad && r[5].error.is_some());
    assert_eq!(r[6].value, Some(Val::List(vec![Val::Int(1), Val::Int(-2), Val::Int(3)])));
    for (i, o) in r[7..].iter().enumerate() {
        assert_eq!(o.value, Some(Val::Int(i as i64)), "tag {i}");
    }
    assert_eq!(ep.client.max_pdu(), 480);
    let st = ep.status();
    assert_eq!(st.pdu, 480);
    assert!(st.requests > 0);
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn json_values_for_transport() {
    let t = parse_tag("MD0:real").unwrap();
    assert_eq!(t.json_value(&t.value("3.1").unwrap()).unwrap(), Val::Float(3.1));
    let t = parse_tag("MB0:lint").unwrap();
    assert_eq!(t.json_value(&i64::MAX.to_be_bytes()).unwrap(), Val::Str(i64::MAX.to_string()));
    assert_eq!(t.json_value(&(-5i64).to_be_bytes()).unwrap(), Val::Int(-5));
    let t = parse_tag("MB0:dtlz").unwrap();
    let b = t.value("2026-11-04 13:45:30.5 UTC").unwrap();
    assert_eq!(t.json_value(&b).unwrap(), Val::Str("2026-11-04T13:45:30.5Z".into()));
    let t = parse_tag("MB0:raw").unwrap();
    assert_eq!(t.json_value(&[1, 0xab]).unwrap(), Val::Str("01 ab".into()));
    let t = parse_tag("MD0:real").unwrap();
    assert_eq!(t.json_value(&f32::NAN.to_be_bytes()).unwrap(), Val::Str("NaN".into()));
    assert_eq!(Val::List(vec![Val::Int(1), Val::Str("a\"".into())]).to_json(), "[1,\"a\\\"\"]");
    assert_eq!(Val::List(vec![Val::Float(1.5), Val::Bool(true)]).to_text().unwrap(), "1.5,true");
}

#[test]
fn subscription_on_change_and_supervisor() {
    let f = FakeCpu::new(240);
    let addr = f.start();
    let ep = Arc::new(open(&addr, "1200"));
    let states = Arc::new(Mutex::new(Vec::new()));
    let s2 = states.clone();
    ep.set_on_status(Some(Arc::new(move |_id: &str, st, _d: &str| s2.lock().unwrap().push(st))));
    let sup = ep.supervise(Duration::from_secs(1));
    let events = Arc::new(Mutex::new(Vec::<DataEvent>::new()));
    let e2 = events.clone();
    let (sub, bad) = Subscription::start(
        ep.clone(),
        "s1",
        vec!["MW0:int".into(), "XX1".into()],
        Duration::from_millis(20),
        true,
        Arc::new(move |ev| e2.lock().unwrap().push(ev)),
    )
    .unwrap();
    assert_eq!(bad.len(), 1);
    std::thread::sleep(Duration::from_millis(150));
    let n0 = events.lock().unwrap().len();
    assert_eq!(n0, 1, "on-change: only the first cycle reports (both tags)");
    assert_eq!(events.lock().unwrap()[0].values.len(), 2);
    f.set_mem(crate::wire::AREA_FLAGS, 0, 0, &[0, 9]);
    std::thread::sleep(Duration::from_millis(150));
    {
        let ev = events.lock().unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[1].values.len(), 1);
        assert_eq!(ev[1].values[0].value, Some(Val::Int(9)));
    }
    sub.stop();
    sup.stop();
    assert!(states.lock().unwrap().contains(&crate::client::State::Connected));
}

#[test]
fn trace_log_has_frames() {
    let f = FakeCpu::new(240);
    let addr = f.start();
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let l2 = lines.clone();
    let log = Log::from_fn(Level::Trace, move |lv, msg, kv| {
        l2.lock().unwrap().push(format!("{lv} {msg} {}", kv.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")))
    });
    let c = crate::client::Client::new(super::opts(&addr)).with_log(log);
    c.run(|cn| cn.read(&[crate::wire::Item::bytes(crate::wire::AREA_FLAGS, 0, 0, 1)])).unwrap();
    let l = lines.lock().unwrap();
    assert!(l.iter().any(|s| s.starts_with("TRACE frame dir=tx hex=03 00 00 1f 02 f0 80 32 01")), "{l:?}");
    assert!(l.iter().any(|s| s.starts_with("INFO connection state") && s.contains("state=connected")), "{l:?}");
    assert!(l.iter().any(|s| s.starts_with("DEBUG request")), "{l:?}");
}
