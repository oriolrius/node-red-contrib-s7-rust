//! Write with read-back and N re-writes (node-red-contrib-s7 "S7 Out": bingo =
//! every value read back equals the value written; rewriteCount).

use std::fmt;
use std::time::Duration;

use crate::error::{local, Error, Result};
use crate::wire::{Conn, Item};

/// Outcome of one item of `write_verify`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerifyItem {
    /// CPU answer to the last write of this item (None = OK)
    pub write_err: Option<Error>,
    /// last read-back (empty when it failed)
    pub read: Vec<u8>,
    /// CPU answer to the last read-back
    pub read_err: Option<Error>,
    /// read-back equals the written value
    pub matched: bool,
}

/// Outcome of `write_verify`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WriteReport {
    pub items: Vec<VerifyItem>,
    pub bingo: bool,
    pub rewrites: usize,
}

impl WriteReport {
    /// Indexes of the items that do not match.
    pub fn mismatched(&self) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, it)| !it.matched).map(|(i, _)| i).collect()
    }
}

impl fmt::Display for WriteReport {
    /// "bingo …, rewrites 1" or "NOT verified: 2 of 3 items differ, rewrites 2".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bingo {
            write!(f, "bingo (all values read back equal), rewrites {}", self.rewrites)
        } else {
            write!(f, "NOT verified: {} of {} items differ, rewrites {}", self.mismatched().len(), self.items.len(), self.rewrites)
        }
    }
}

fn same_value(it: &Item, want: &[u8], got: &[u8]) -> bool {
    if it.is_bit {
        return !got.is_empty() && !want.is_empty() && want[0] & 1 == got[0] & 1;
    }
    want == got
}

impl Conn {
    /// Writes values[i] to items[i], reads them back and, while some item does
    /// not match, writes those items again (up to `retries` times, waiting
    /// `interval` before each read-back). Items whose write fails with a CPU
    /// item error (e.g. 0x0A, object does not exist) are not written again: the
    /// answer will not change. Transport errors end the call.
    pub fn write_verify<V: AsRef<[u8]>>(
        &mut self,
        items: &[Item],
        values: &[V],
        retries: usize,
        interval: Duration,
    ) -> Result<WriteReport> {
        if items.len() != values.len() {
            return Err(local("items and values differ in length"));
        }
        let mut rep = WriteReport { items: vec![VerifyItem::default(); items.len()], ..Default::default() };
        let mut todo: Vec<usize> = (0..items.len()).collect();
        loop {
            let wi: Vec<Item> = todo.iter().map(|&i| items[i]).collect();
            let wv: Vec<&[u8]> = todo.iter().map(|&i| values[i].as_ref()).collect();
            let errs = self.write(&wi, &wv)?;
            for (k, &i) in todo.iter().enumerate() {
                rep.items[i].write_err = errs[k].clone();
            }
            if !interval.is_zero() {
                std::thread::sleep(interval);
            }
            let res = self.read(items)?;
            rep.bingo = true;
            todo.clear();
            for (i, r) in res.into_iter().enumerate() {
                let v = &mut rep.items[i];
                match r {
                    Ok(d) => {
                        v.read = d;
                        v.read_err = None;
                    }
                    Err(e) => {
                        v.read = Vec::new();
                        v.read_err = Some(e);
                    }
                }
                v.matched = v.read_err.is_none() && v.write_err.is_none() && same_value(&items[i], values[i].as_ref(), &v.read);
                if v.matched {
                    continue;
                }
                rep.bingo = false;
                if v.write_err.as_ref().and_then(|e| e.item_code()).is_none() {
                    todo.push(i);
                }
            }
            if rep.bingo || todo.is_empty() || rep.rewrites >= retries {
                return Ok(rep);
            }
            rep.rewrites += 1;
        }
    }
}
