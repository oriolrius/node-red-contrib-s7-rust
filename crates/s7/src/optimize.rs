//! Gap spanning.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::wire::{Conn, Item, ItemResult, PDU_OVERHEAD};

impl Conn {
    /// Reads items like `read`, but first merges items of the same area/DB whose
    /// byte ranges are closer than `max_gap` bytes into one range (gap spanning).
    /// Bit items become byte reads and the bit is extracted afterwards.
    /// Timers/counters are read as they are. If a merged range fails (for
    /// example one part is outside the DB), its items are retried one by one so
    /// that one bad address does not hide the others.
    pub fn read_optimized(&mut self, items: &[Item], max_gap: i64) -> Result<Vec<ItemResult>> {
        struct Ref {
            idx: usize,
            start: i64,
            n: i64,
        }
        let mut groups: BTreeMap<(u8, u16), Vec<Ref>> = BTreeMap::new();
        let mut passthru = Vec::new();
        for (i, it) in items.iter().enumerate() {
            if it.is_timer_counter() {
                passthru.push(i);
                continue;
            }
            groups.entry((it.area, it.db)).or_default().push(Ref { idx: i, start: it.start as i64, n: it.size() as i64 });
        }

        let limit = self.max_pdu as i64 - PDU_OVERHEAD as i64;
        struct Span {
            item: Item,
            refs: Vec<Ref>,
        }
        let mut spans: Vec<Span> = Vec::new();
        for ((area, db), mut refs) in groups {
            refs.sort_by_key(|r| r.start);
            let mut cur: Vec<Ref> = Vec::new();
            let (mut cur_start, mut cur_end) = (-1i64, -1i64);
            let mut flush = |cur: Vec<Ref>, s: i64, e: i64| {
                if s >= 0 {
                    spans.push(Span { item: Item::bytes(area, db, s as u32, (e - s) as u16), refs: cur });
                }
            };
            for r in refs {
                let end = r.start + r.n;
                if cur_start >= 0 && r.start <= cur_end + max_gap && end.max(cur_end) - cur_start <= limit {
                    cur_end = cur_end.max(end);
                    cur.push(r);
                    continue;
                }
                flush(std::mem::take(&mut cur), cur_start, cur_end);
                cur_start = r.start;
                cur_end = end;
                cur.push(r);
            }
            flush(cur, cur_start, cur_end);
        }

        let mut req: Vec<Item> = spans.iter().map(|s| s.item).collect();
        req.extend(passthru.iter().map(|&i| items[i]));
        let mut res = self.read(&req)?;

        let mut out: Vec<ItemResult> = vec![Ok(Vec::new()); items.len()];
        let mut retry = Vec::new();
        for (k, s) in spans.iter().enumerate() {
            match &res[k] {
                Err(e) => {
                    if s.refs.len() == 1 {
                        out[s.refs[0].idx] = Err(e.clone());
                    } else {
                        retry.extend(s.refs.iter().map(|r| r.idx));
                    }
                }
                Ok(data) => {
                    let base = s.item.start as i64;
                    for r in &s.refs {
                        let it = &items[r.idx];
                        let off = (r.start - base) as usize;
                        let chunk = &data[off..off + r.n as usize];
                        out[r.idx] = Ok(if it.is_bit { vec![(chunk[0] >> it.bit) & 1] } else { chunk.to_vec() });
                    }
                }
            }
        }
        for (j, &i) in passthru.iter().enumerate() {
            out[i] = std::mem::replace(&mut res[spans.len() + j], Ok(Vec::new()));
        }
        if !retry.is_empty() {
            let single: Vec<Item> = retry.iter().map(|&i| items[i]).collect();
            let r2 = self.read(&single)?;
            for (j, r) in r2.into_iter().enumerate() {
                out[retry[j]] = r;
            }
        }
        Ok(out)
    }
}
