//! Against the in-process fake CPU.

use super::dial_fake;
use crate::addr::parse_tag;
use crate::error::Error;
use crate::wire::{Item, AREA_DB, AREA_FLAGS};

#[test]
fn round_trip_against_fake_cpu() {
    let (mut cn, f) = dial_fake(240);
    assert_eq!(cn.max_pdu, 240);
    f.set_mem(AREA_DB, 1, 0, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let res = cn.read(&[Item::bytes(AREA_DB, 1, 0, 4)]).unwrap();
    assert_eq!(res[0].as_ref().unwrap(), &vec![0xDE, 0xAD, 0xBE, 0xEF]);
    assert!(f.errors().is_empty());
}

#[test]
fn write_then_read() {
    let (mut cn, _f) = dial_fake(240);
    let tags: Vec<_> = ["MW100:int", "MD104:real", "M110.5", "DB1.DBB20:string[10]"].iter().map(|s| parse_tag(s).unwrap()).collect();
    let vals: Vec<Vec<u8>> = tags.iter().zip(["-1234", "3.25", "true", "hello"]).map(|(t, v)| t.value(v).unwrap()).collect();
    let items: Vec<Item> = tags.iter().map(|t| t.item).collect();
    let errs = cn.write(&items, &vals).unwrap();
    assert!(errs.iter().all(|e| e.is_none()), "{errs:?}");
    let res = cn.read(&items).unwrap();
    let want = ["-1234", "3.25", "true", "\"hello\""];
    for (i, t) in tags.iter().enumerate() {
        assert_eq!(t.format(res[i].as_ref().unwrap()).unwrap(), want[i], "{}", t.text);
    }
}

#[test]
fn per_item_errors() {
    let (mut cn, _f) = dial_fake(240);
    let res = cn.read(&[Item::bytes(AREA_FLAGS, 0, 0, 1), Item::bytes(AREA_DB, 2, 0, 2), Item::bytes(AREA_FLAGS, 0, 1, 1)]).unwrap();
    assert!(res[0].is_ok() && res[2].is_ok());
    assert_eq!(res[1].as_ref().unwrap_err(), &Error::Item(0x0A));
}

#[test]
fn pdu_splitting() {
    let (mut cn, f) = dial_fake(240);
    {
        let mut s = f.lock();
        for (i, b) in s.mem(AREA_DB, 1).iter_mut().enumerate() {
            *b = i as u8;
        }
    }
    // 1000 bytes > 240-28: must be split into several requests and reassembled.
    let res = cn.read(&[Item::bytes(AREA_DB, 1, 0, 1000)]).unwrap();
    for (i, b) in res[0].as_ref().unwrap().iter().enumerate() {
        assert_eq!(*b, i as u8, "byte {i}");
    }
    // Many small items: must be split across requests that fit the PDU.
    let items: Vec<Item> = (0..40).map(|i| Item::bytes(AREA_FLAGS, 0, i * 2, 2)).collect();
    f.reset_reqs();
    cn.read(&items).unwrap();
    assert!(f.reqs() >= 3, "40 items in {} requests; expected splitting", f.reqs());
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn szl_module_identification() {
    let (mut cn, _f) = dial_fake(240);
    let mods = cn.module_identification().unwrap();
    assert_eq!(mods.len(), 2);
    assert_eq!(mods[0].order_no, "6ES7 212-1BE31-0XB0");
    assert_eq!(mods[1].version, "V3.0.2");
}
