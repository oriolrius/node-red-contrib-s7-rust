//! S7 partner (BSEND / BRCV) against the fake PLC.
use std::time::{Duration, Instant};

use crate::partner::{Area, Block, Message, Partner, Usend, MAX_BLOCK};
use crate::partner_fake::{FakePartnerPlc, PARTNER_TSAP, PLC_TSAP};
use crate::wire::DialOptions;

fn opts(addr: String) -> DialOptions {
    DialOptions { addr, local_tsap: PARTNER_TSAP, remote_tsap: PLC_TSAP, timeout: Duration::from_secs(2), ..Default::default() }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[test]
fn bsend_and_brcv_with_fragments() {
    let f = FakePartnerPlc::start(240);
    let mut p = Partner::connect(&opts(f.addr())).unwrap();
    assert_eq!(p.max_pdu(), 240);
    // partner -> PLC: one fragment, then 1800 bytes in 9 fragments
    p.send(2, &[1, 2, 3]).unwrap();
    p.send(7, &pattern(1800)).unwrap();
    let got = f.received();
    assert_eq!(got.len(), 2, "{:?}", f.errors());
    assert_eq!((got[0].r_id, got[0].data.clone()), (2, vec![1, 2, 3]));
    assert_eq!((got[1].r_id, got[1].data.len()), (7, 1800));
    assert_eq!(got[1].data, pattern(1800));
    // PLC -> partner: one fragment, then a fragmented block
    assert!(f.bsend(1, vec![0xab, 0xcd]));
    assert!(f.bsend(1, pattern(1500)));
    let mut blocks = Vec::new();
    let t0 = Instant::now();
    while blocks.len() < 2 && t0.elapsed() < Duration::from_secs(5) {
        if let Some(Message::Block(b)) = p.recv(Duration::from_millis(100)).unwrap() {
            blocks.push(b);
        }
    }
    assert_eq!(blocks.len(), 2, "{:?}", f.errors());
    assert_eq!((blocks[0].r_id, blocks[0].data.clone()), (1, vec![0xab, 0xcd]));
    assert_eq!(blocks[1].data, pattern(1500));
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn brcv_refusal_and_limits() {
    let f = FakePartnerPlc::start(480);
    let mut p = Partner::connect(&opts(f.addr())).unwrap();
    f.refuse(Some(9));
    let e = p.send(9, &[1]).unwrap_err().to_string();
    assert!(e.contains("refused") && e.contains("R_ID 9"), "{e}");
    // the connection stays usable
    p.send(3, &[4]).unwrap();
    assert_eq!(f.received().len(), 1);
    assert!(p.send(3, &[]).is_err());
    assert!(p.send(3, &vec![0; MAX_BLOCK + 1]).is_err());
    assert!(p.recv(Duration::from_millis(50)).unwrap().is_none());
}

#[test]
fn wrong_tsap_is_refused() {
    let f = FakePartnerPlc::start(480);
    let mut o = opts(f.addr());
    o.remote_tsap = 0x1002;
    assert!(Partner::connect(&o).is_err());
}

#[test]
fn pending_blocks_are_bounded() {
    use crate::partner::MAX_PENDING;
    let f = FakePartnerPlc::start(480);
    let mut p = Partner::connect(&opts(f.addr())).unwrap();
    // the PLC sends while only send() runs (no recv): the queue keeps the newest blocks
    for i in 0..(MAX_PENDING + 20) {
        assert!(f.bsend(1, vec![(i % 256) as u8]));
    }
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..40 {
        p.send(2, &[0]).unwrap();
    }
    let t0 = Instant::now();
    while p.dropped() < 20 && t0.elapsed() < Duration::from_secs(10) {
        p.send(2, &[0]).unwrap();
    }
    assert!(p.dropped() >= 20, "dropped {}", p.dropped());
    let mut n = 0;
    while p.recv(Duration::from_millis(50)).unwrap().is_some() {
        n += 1;
    }
    assert!(n <= MAX_PENDING, "{n}");
}

fn recv_all(p: &mut Partner, n: usize) -> Vec<Message> {
    let mut out = Vec::new();
    let t0 = Instant::now();
    while out.len() < n && t0.elapsed() < Duration::from_secs(5) {
        if let Some(m) = p.recv(Duration::from_millis(100)).unwrap() {
            out.push(m);
        }
    }
    out
}

#[test]
fn usend_and_urcv() {
    let f = FakePartnerPlc::start(240);
    let mut p = Partner::connect(&opts(f.addr())).unwrap();
    // PLC -> partner: the frame of PLCSIM (10 bytes + an Int), and a BSEND in between
    let a10 = Area { tsize: 0x04, data: vec![0xcd, 0, 2, 3, 4, 5, 6, 7, 8, 9] };
    let int = Area { tsize: 0x05, data: vec![0x00, 0x03] };
    assert!(f.usend(3, vec![a10.clone(), int.clone()]));
    assert!(f.bsend(1, vec![7]));
    let odd = Area { tsize: 0x04, data: vec![1, 2, 3] };
    assert!(f.usend(5, vec![odd.clone(), Area { tsize: 0x09, data: vec![9] }]));
    let got = recv_all(&mut p, 3);
    assert_eq!(
        got,
        vec![
            Message::Usend(Usend { r_id: 3, areas: vec![a10.clone(), int] }),
            Message::Block(Block { r_id: 1, data: vec![7] }),
            Message::Usend(Usend { r_id: 5, areas: vec![odd.clone(), Area { tsize: 0x09, data: vec![9] }] }),
        ],
        "{:?}",
        f.errors()
    );
    // partner -> PLC
    p.usend(4, std::slice::from_ref(&a10)).unwrap();
    p.usend(6, &[odd.clone(), a10.clone()]).unwrap();
    p.send(2, &[1]).unwrap(); // a BSEND after it: the USENDs were sent before
    let u = f.usends();
    assert_eq!(u, vec![Usend { r_id: 4, areas: vec![a10.clone()] }, Usend { r_id: 6, areas: vec![odd, a10.clone()] }]);
    // limits
    assert!(p.usend(4, &[]).is_err());
    assert!(p.usend(4, &vec![a10.clone(); 5]).is_err());
    assert!(p.usend(4, &[Area { tsize: 0x04, data: vec![] }]).is_err());
    let e = p.usend(4, &[Area { tsize: 0x04, data: vec![0; 230] }]).unwrap_err().to_string();
    assert!(e.contains("PDU"), "{e}");
    p.usend(4, &[Area { tsize: 0x04, data: vec![0; 240 - 28] }]).unwrap();
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn malformed_usend_is_an_error() {
    use crate::partner::parse_usend;
    let frame = |p: &str, d: &str| {
        let h = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let (p, d) = (h(p), h(d));
        let mut f = vec![0x02, 0xF0, 0x80, 0x32, 0x07, 0, 0, 0, 0];
        f.extend_from_slice(&(p.len() as u16).to_be_bytes());
        f.extend_from_slice(&(d.len() as u16).to_be_bytes());
        f.extend(p);
        f.extend(d);
        f
    };
    let p = "000112061300".to_string() + "00000003";
    // PLCSIM bytes
    let ok = frame(&p, "ff02000200040050cd000203040506070809000500100000" /* PLCSIM, usendInt 0 */);
    assert_eq!(parse_usend(&ok).unwrap().unwrap().areas.len(), 2);
    for bad in [
        "ff020000",             // no areas
        "ff020005",             // 5 areas
        "ff0200010004",         // truncated area header
        "ff02000100040050cd00", // 10 bytes announced, 2 present
        "ff0200010004000801ee", // extra bytes after the areas
        "00020001000400080100", // not FF
    ] {
        assert!(parse_usend(&frame(&p, bad)).is_err(), "accepted {bad}");
    }
    // a BSEND frame is not a USEND
    assert!(parse_usend(&frame("000112081246010000000000", "0a000000")).unwrap().is_none());
}
