//! Golden frames from a TIA Portal capture (TIA Portal V21 <-> S7-1200 FW V3).

use super::{h, s7data};
use crate::alarms::alarm_subscription_object;
use crate::client::tia_echo;
use crate::explore::ExploreReq;
use crate::ids::*;
use crate::msg::{encode_object_qualifier, Message};
use crate::object::decode_object;
use crate::subscribe::{decode_notification, decode_notification_layout, SubOptions};
use crate::textfmt::hex;
use crate::value::Value;
use crate::wire::{Reader, Writer};
use crate::AlarmSubOptions;

const TIA_CREATE_OBJECT_RES: &str = "32000004ca00000002360002871c8715a100000120821f0000a38169001500a3823200170000013a823b00048200823c00048140823d00048480c040823e00048480c040823f00151b313b36455337203231322d31424533312d30584230203b56332e308240001505323b35343482410003000300a2000000";
const TIA_SET_MULTI_VARS_REQ: &str = "3100000542000000030000039c340000039c010182320100170000013a823b00048200823c00048140823d00048480c040823e00048480c040823f001500824000151a313b36455337203231322d31424533312d305842303b56332e308241000300030000000004e88969001200000000896a001300896b0004000000000000";

#[test]
fn golden_session_setup() {
    let m = Message::parse(1, &h(TIA_CREATE_OBJECT_RES)).unwrap();
    let mut r = Reader::new(&m.body);
    assert_eq!(r.vlq64(), 0, "return");
    let n = r.byte();
    let ids: Vec<u32> = (0..n).map(|_| r.vlq32()).collect();
    assert_eq!(ids, vec![0x39c, 0x395]);
    let obj = decode_object(&mut r).unwrap();
    assert!(r.err.is_none());
    let st = obj.attr(ATTR_SESS_VERSION).unwrap().v.as_struct().unwrap().clone();
    assert_eq!(st.get(319).unwrap().v.as_str(), Some("1;6ES7 212-1BE31-0XB0 ;V3.0"));
    // Re-encoding the object gives the same bytes.
    let mut w = Writer::new();
    obj.encode(&mut w);
    assert!(hex(&h(TIA_CREATE_OBJECT_RES)).contains(&hex(&w.b)), "object re-encode differs: {}", hex(&w.b));

    // Our SetMultiVariables (session echo) must equal the TIA request byte for byte.
    let mut body = Writer::new();
    body.u32(0x39c);
    body.vlq32(1);
    body.vlq32(1);
    body.vlq32(ATTR_SESS_VERSION);
    body.vlq32(1);
    Value::structv(tia_echo(&st)).encode(&mut body);
    body.byte(0);
    encode_object_qualifier(&mut body);
    body.u32(0);
    let req = Message {
        opcode: OP_REQUEST,
        function: FN_SET_MULTI_VARIABLES,
        seq: 3,
        session: 0x39c,
        tflags: 0x34,
        body: body.b,
        ..Default::default()
    };
    assert_eq!(hex(&req.encode()), TIA_SET_MULTI_VARS_REQ, "SetMultiVariables differs from TIA");
}

#[test]
fn golden_explore_requests() {
    // seq 27: Explore 56, recursive, parents (TIA flags), no attributes.
    let mut w = Writer::new();
    ExploreReq { object: 56, recursive: true, parents: true, ..Default::default() }.encode(&mut w);
    w.u32(0);
    let m = Message { opcode: OP_REQUEST, function: FN_EXPLORE, seq: 0x1b, session: 0x39c, tflags: 0x34, body: w.b, ..Default::default() };
    assert_eq!(
        hex(&m.encode()),
        hex(&s7data("02f0807202001c31000004bb0000001b0000039c34000000380001000100000000000072020000")),
        "explore 56"
    );
    // seq 30: Explore PLCProgram with 56 attributes.
    let frame = "02f0807202008c31000004bb0000001e0000039c340000000300010001003893619360936f93159313be09bf0381699359932f935b935cbe7d935f937b93779c16bd19bc359427944094428f679735902a900a902790158f6a9024900d8f7d9010903a903d9032901a903790079020c039a12f9c33931b9364bb25bc37be08bb62bb61a43ea350a41ebb22a421a3620000000072020000";
    let d = s7data(frame);
    let mut r = Reader::new(&d[14..]);
    r.u32();
    r.vlq32();
    r.bytes(4);
    let n = r.vlq32();
    let attrs: Vec<u32> = (0..n).map(|_| r.vlq32()).collect();
    assert_eq!(attrs.len(), 56);
    assert_eq!(attrs[7], ATTR_OBJECT_VARIABLE_TYPE_NAME);
    let mut w = Writer::new();
    ExploreReq { object: RID_PLC_PROGRAM, recursive: true, parents: true, attrs, ..Default::default() }.encode(&mut w);
    w.u32(0);
    let m = Message { opcode: OP_REQUEST, function: FN_EXPLORE, seq: 0x1e, session: 0x39c, tflags: 0x34, body: w.b, ..Default::default() };
    assert_eq!(hex(&m.encode()), hex(&d), "explore 3 differs");
}

#[test]
fn golden_alarm_subscription_request() {
    // seq 47: TIA creates the alarm subscription 0x7fffc005 + ref 0x7fffc006.
    let want = s7data("02f080720200c731000004ca0000002f0000039c340000039500040000000000a17fffc00587690000a38169001517537562736372697074696f6e5f32313437343637323639a3883a000202a3876a00030000a3876b000900a38810000202a38811000101a3881820040388808480000000a38819000400a3881b000200a3881c000200a3881d0007ffffa3881e0003ffffa17fffc00694660000a38169001500a3876d000203a3946310030a0000000000000000000000000000000000000000a4946400000008a2a20000000072020000");
    let o = SubOptions { route_mode: 2, credit_limit: -1, ..Default::default() };
    let obj = alarm_subscription_object(&o, &AlarmSubOptions::default(), 0x7fff_c005, 0x7fff_c006);
    let mut w = Writer::new();
    w.u32(0x395);
    Value::udint(0).encode(&mut w);
    w.u32(0);
    obj.encode(&mut w);
    w.u32(0);
    let m = Message {
        opcode: OP_REQUEST,
        function: FN_CREATE_OBJECT,
        seq: 0x2f,
        session: 0x39c,
        tflags: 0x34,
        body: w.b,
        ..Default::default()
    };
    assert_eq!(hex(&m.encode()), hex(&want), "alarm subscription differs");
    // and the TIA object decodes back with the relation 2660 -> 8
    let mut r = Reader::new(&want[14 + 4 + 3 + 4..]);
    let o2 = decode_object(&mut r).unwrap();
    assert!(r.err.is_none());
    assert_eq!(o2.children.len(), 1);
    assert_eq!(o2.children[0].relation(ATTR_ALARM_SUB_REF_SUBSYSTEM), Some(RID_ALARM_SUBSYSTEM));
}

#[test]
#[allow(clippy::type_complexity)]
fn golden_notifications() {
    let cases: [(&str, u32, u32, &[(u32, &str)]); 3] = [
        ("02f08072020012331000006b0400000000000001000000000072020000", 0x1000006b, 1, &[]),
        ("02f0807202001a331000006b04000000000000039200000006000808000000000072020000", 0x1000006b, 3, &[(6, "DInt 8")]),
        ("02f0807202001a331000006e04000000000000019200000002000401000000000072020000", 0x1000006e, 1, &[(2, "UDInt 1")]),
    ];
    for (frame, sub, seq, vals) in cases {
        let m = Message::parse(2, &s7data(frame)).unwrap();
        let n = decode_notification(&m.body).unwrap_or_else(|e| panic!("{frame}: {e}"));
        assert_eq!((n.subscription_id, n.seq, n.values.len()), (sub, seq, vals.len()), "{n}");
        for (r, s) in vals {
            assert_eq!(n.value(*r).unwrap().to_string(), *s);
        }
    }
    // a struct value notification (CPU state struct 3705)
    let body = h("1000006b04000000000000029200000005001700000e799c740008029c700008019c710008019c720008000000000000");
    let n = decode_notification(&body).unwrap();
    assert_eq!(n.values.len(), 1);
    // newer firmware: change counter byte, and 0 + 8-byte timestamp
    for b in [
        "1000006b0400000000000001".to_string() + "05" + "9b0300080700" + "00000000",
        "1000006b0400000000000001".to_string() + "00000600000000000a" + "9b0300080700" + "00000000",
    ] {
        match decode_notification(&h(&b)) {
            Ok(n) if n.value(3).map(|v| v.to_string()) == Some("DInt 7".into()) => {}
            other => {
                let layouts: Vec<String> = (0..3).map(|l| format!("{:?}", decode_notification_layout(&h(&b), l).err())).collect();
                panic!("{b}: {other:?} {layouts:?}");
            }
        }
    }
}

// Notifications of PLCSIM Advanced V8 (CPU 1513-1 PN, FW S4.1, TLS), subscription
// with cycle 100 ms on "TestSub".tick (ref 1, DInt), toggle (2, Bool), slow (3, Int),
// sp (4, Int), echo (5, Int). The first one has all values; later ones only the
// changed values; after a write of sp = 42 the PLC copies it to echo.
#[test]
fn golden_notifications_fw4() {
    // (body, sequence number, credit tick, values by reference id)
    type Case<'a> = (&'a str, u32, u8, &'a [(u32, &'a str)]);
    let cases: [Case; 3] = [
        (
            "70000fd9040000000000010000066010db092b560192000000050007030992000000040007030992000000030007001192000000020001019200000001000886520000000000",
            0,
            1,
            &[(5, "Int 777"), (4, "Int 777"), (3, "Int 17"), (2, "Bool true"), (1, "DInt 850")],
        ),
        ("70000fd9040000000000020100066010db0ab210019200000001000886530000000000", 1, 2, &[(1, "DInt 851")]),
        (
            "70000fd90400000000000b0a00066010db186da80192000000050007002a92000000040007002a920000000200010092000000010008865c0000000000",
            10,
            11,
            &[(5, "Int 42"), (4, "Int 42"), (2, "Bool false"), (1, "DInt 860")],
        ),
    ];
    let mut last_ts = None;
    for (body, seq, credit_tick, vals) in cases {
        let n = decode_notification(&h(body)).unwrap_or_else(|e| panic!("{body}: {e}"));
        assert_eq!((n.subscription_id, n.seq, n.credit_tick, n.change_counter), (0x7000_0fd9, seq, credit_tick, 1), "{n}");
        assert!(n.item_errors.is_empty(), "{n}");
        let got: Vec<(u32, String)> = n.values.iter().map(|(r, v)| (*r, v.to_string())).collect();
        let want: Vec<(u32, String)> = vals.iter().map(|(r, s)| (*r, s.to_string())).collect();
        assert_eq!(got, want);
        let ts = n.timestamp.expect("FW V4 sends a timestamp");
        if let Some(prev) = last_ts {
            assert!(ts > prev, "timestamps go up");
        }
        last_ts = Some(ts);
    }
}
