//! S7 partner (BSEND / BRCV) against the fake PLC.
use std::time::{Duration, Instant};

use crate::partner::{Partner, MAX_BLOCK};
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
        if let Some(b) = p.recv(Duration::from_millis(100)).unwrap() {
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
