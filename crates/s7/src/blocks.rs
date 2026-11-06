//! Block functions (Userdata group 3: count, list of type, block info) and
//! block upload (jobs 0x1D/0x1E/0x1F) [std; nodes7, snap7].

use std::fmt;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};

use crate::error::{io, local, Result};
use crate::log::hex_spaced;
use crate::userdata::GROUP_BLOCK;
use crate::wire::Conn;

/// Block sub-type code used by the block functions and in upload file names
/// (OB = 0x08 → "08", DB = 0x0A → "0A"…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockType(pub u8);

impl BlockType {
    pub const OB: BlockType = BlockType(0x08);
    pub const DB: BlockType = BlockType(0x0A);
    pub const SDB: BlockType = BlockType(0x0B);
    pub const FC: BlockType = BlockType(0x0C);
    pub const SFC: BlockType = BlockType(0x0D);
    pub const FB: BlockType = BlockType(0x0E);
    pub const SFB: BlockType = BlockType(0x0F);

    /// Name ("OB", "DB"…), None for unknown codes.
    pub fn name(self) -> Option<&'static str> {
        Some(match self.0 {
            0x08 => "OB",
            0x0A => "DB",
            0x0B => "SDB",
            0x0C => "FC",
            0x0D => "SFC",
            0x0E => "FB",
            0x0F => "SFB",
            _ => return None,
        })
    }

    /// The 2-character ASCII form on the wire ("08", "0A"…).
    pub fn code(self) -> Vec<u8> {
        format!("{:02X}", self.0).into_bytes()
    }
}

/// The block types in the usual order.
pub const BLOCK_TYPES: [BlockType; 7] =
    [BlockType::OB, BlockType::DB, BlockType::SDB, BlockType::FC, BlockType::SFC, BlockType::FB, BlockType::SFB];

impl fmt::Display for BlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => f.write_str(n),
            None => write!(f, "0x{:02X}", self.0),
        }
    }
}

/// Accepts "OB", "db", "0x0A", "10".
pub fn parse_block_type(s: &str) -> Result<BlockType> {
    let u = s.trim().to_uppercase();
    if let Some(t) = BLOCK_TYPES.iter().find(|t| t.name() == Some(u.as_str())) {
        return Ok(*t);
    }
    if let Ok(n) = crate::textfmt::parse_uint(&s.to_lowercase(), 8) {
        return Ok(BlockType(n as u8));
    }
    Err(local(format!("unknown block type {s:?} (OB, DB, SDB, FC, SFC, FB, SFB)")))
}

/// Text for a block language code [std, Wireshark s7comm].
pub fn language_name(l: u8) -> String {
    match l {
        0x00 => "not defined",
        0x01 => "AWL/STL",
        0x02 => "KOP/LAD",
        0x03 => "FUP/FBD",
        0x04 => "SCL",
        0x05 => "DB",
        0x06 => "GRAPH",
        0x07 => "SDB",
        0x08 => "CPU-DB",
        0x11 => "SDB (after overall reset)",
        0x12 => "SDB (routing)",
        0x29 => "encrypted",
        _ => return format!("0x{l:02x}"),
    }
    .to_string()
}

/// One entry of "list blocks" (Userdata group 3, sub 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockCount {
    pub typ: BlockType,
    pub count: usize,
}

/// One block in "list blocks of type".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEntry {
    pub number: u16,
    /// 0x22 on most CPUs; meaning not documented
    pub flags: u8,
    pub language: u8,
}

/// The block header the CPU returns for "block info" (Userdata group 3, sub 3).
/// Offsets follow the Wireshark s7comm dissector and snap7's TResDataBlockInfo.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockInfo {
    /// sub-block type (byte 11)
    pub typ: BlockType,
    pub number: u16,
    pub language: u8,
    /// 0x01 linked, 0x08 standard block, 0x40 non-retain
    pub flags: u8,
    pub security: u32,
    /// load memory length
    pub load_size: u32,
    pub sbb_length: u16,
    pub add_length: u16,
    /// local data (L stack) length
    pub local_data: u16,
    /// MC7 code length
    pub mc7_size: u16,
    pub author: String,
    pub family: String,
    /// "header name"
    pub name: String,
    /// high nibble major, low nibble minor
    pub version: u8,
    pub checksum: u16,
    /// code (last modified) timestamp
    pub code_date: DateTime<Utc>,
    /// interface timestamp
    pub interface_date: DateTime<Utc>,
    pub raw: Vec<u8>,
}

impl BlockInfo {
    /// "major.minor".
    pub fn version_string(&self) -> String {
        format!("{}.{}", self.version >> 4, self.version & 0x0f)
    }
    pub fn linked(&self) -> bool {
        self.flags & 0x01 != 0
    }
    pub fn standard(&self) -> bool {
        self.flags & 0x08 != 0
    }
    pub fn non_retain(&self) -> bool {
        self.flags & 0x40 != 0
    }
}

/// Day 0 of the 6-byte S7 timestamp (ms since midnight + days since 1984-01-01).
fn s7_epoch() -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(1984, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc()
}

pub fn decode_s7_timestamp(b: &[u8]) -> DateTime<Utc> {
    let ms = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let days = u16::from_be_bytes([b[4], b[5]]);
    s7_epoch() + TimeDelta::days(days as i64) + TimeDelta::milliseconds(ms as i64)
}

pub fn encode_s7_timestamp(t: &DateTime<Utc>) -> [u8; 6] {
    let day = t.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc();
    let ms = (*t - day).num_milliseconds() as u32;
    let days = (day - s7_epoch()).num_days() as u16;
    let m = ms.to_be_bytes();
    let d = days.to_be_bytes();
    [m[0], m[1], m[2], m[3], d[0], d[1]]
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().trim_end_matches(['\0', ' ']).to_string()
}

/// Decodes the 78-byte block info payload.
pub fn parse_block_info(d: &[u8]) -> Result<BlockInfo> {
    if d.len() < 70 {
        return Err(io(format!("block info: {} bytes, need 70", d.len())));
    }
    let be16 = |i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
    let be32 = |i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    Ok(BlockInfo {
        flags: d[9],
        language: d[10],
        typ: BlockType(d[11]),
        number: be16(12),
        load_size: be32(14),
        security: be32(18),
        code_date: decode_s7_timestamp(&d[22..]),
        interface_date: decode_s7_timestamp(&d[28..]),
        sbb_length: be16(34),
        add_length: be16(36),
        local_data: be16(38),
        mc7_size: be16(40),
        author: cstr(&d[42..50]),
        family: cstr(&d[50..58]),
        name: cstr(&d[58..66]),
        version: d[66],
        checksum: be16(68),
        raw: d.to_vec(),
    })
}

/// The 8-character block address of block info / upload: <type 2 hex>
/// <number 5 decimal> <filesystem A|P|B>.
pub fn block_file(t: BlockType, num: u16, fs: u8) -> Vec<u8> {
    format!("{:02X}{:05}{}", t.0, num, fs as char).into_bytes()
}

/// The name the upload job uses: "_" (complete block; "$" would be header
/// only) + type + number + filesystem, e.g. "_0800001A" = OB1.
pub fn upload_filename(t: BlockType, num: u16) -> String {
    format!("_{}", String::from_utf8(block_file(t, num, b'A')).unwrap())
}

pub fn upload_start_param(name: &str) -> Vec<u8> {
    let mut p = vec![0x1D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, name.len() as u8];
    p.extend_from_slice(name.as_bytes());
    p
}

pub fn upload_param(func: u8, id: u32, err_code: u16) -> Vec<u8> {
    let mut p = vec![func, 0x00, (err_code >> 8) as u8, err_code as u8];
    p.extend_from_slice(&id.to_be_bytes());
    p
}

/// One result of `upload_all`.
#[derive(Debug, Clone)]
pub struct UploadedBlock {
    pub typ: BlockType,
    pub number: u16,
    pub data: Vec<u8>,
    pub err: Option<crate::error::Error>,
}

impl Conn {
    /// How many blocks of each type the CPU holds. Reply entries are 4 bytes:
    /// 0x30 <type> <count BE>.
    pub fn block_counts(&mut self) -> Result<Vec<BlockCount>> {
        let d: Vec<u8> = self.userdata(GROUP_BLOCK, 0x01, None).map_err(|e| e.context("block count"))?.concat();
        if !d.len().is_multiple_of(4) {
            return Err(io(format!("block count: reply length {} is not a multiple of 4", d.len())));
        }
        d.chunks_exact(4)
            .map(|c| {
                let t = std::str::from_utf8(&c[..2])
                    .ok()
                    .filter(|s| s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .ok_or_else(|| io(format!("block count: bad type {}", hex_spaced(&c[..2]))))?;
                Ok(BlockCount { typ: BlockType(t), count: u16::from_be_bytes([c[2], c[3]]) as usize })
            })
            .collect()
    }

    /// Lists the blocks of one type (Userdata group 3, sub 2). Long lists come
    /// in several data units. Entries are 4 bytes: <number BE> <flags> <language>.
    pub fn list_blocks(&mut self, t: BlockType) -> Result<Vec<BlockEntry>> {
        let d: Vec<u8> = self.userdata(GROUP_BLOCK, 0x02, Some(&t.code())).map_err(|e| e.context(format!("list {t} blocks")))?.concat();
        if !d.len().is_multiple_of(4) {
            return Err(io(format!("list {t} blocks: reply length {} is not a multiple of 4", d.len())));
        }
        Ok(d.chunks_exact(4).map(|c| BlockEntry { number: u16::from_be_bytes([c[0], c[1]]), flags: c[2], language: c[3] }).collect())
    }

    /// Reads the header of one block from the active filesystem ('A').
    pub fn get_block_info(&mut self, t: BlockType, num: u16) -> Result<BlockInfo> {
        let parts =
            self.userdata(GROUP_BLOCK, 0x03, Some(&block_file(t, num, b'A'))).map_err(|e| e.context(format!("block info {t}{num}")))?;
        parse_block_info(&parts.concat())
    }

    /// Copies one block from the CPU (complete block, MC7 with header): start
    /// upload → upload while "more data" → end upload. On a failure after start
    /// it still sends end upload (error code 0xFFFF) to free the CPU job.
    pub fn upload(&mut self, t: BlockType, num: u16) -> Result<Vec<u8>> {
        let name = upload_filename(t, num);
        let r = self.round_trip_owned(0x01, &upload_start_param(&name), &[]).map_err(|e| e.context(format!("start upload {name}")))?;
        let p = &r.param;
        if p.len() < 9 || p[0] != 0x1D {
            return Err(io(format!("start upload {name}: bad reply param {}", hex_spaced(p))));
        }
        // 1D <status> <2 unknown> <upload id BE32> <len> <block length, ASCII decimal>
        let id = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
        let mut total: i64 = -1;
        let n = p[8] as usize;
        if n > 0 && 9 + n <= p.len() {
            if let Some(v) = crate::textfmt::parse_dec_i64(String::from_utf8_lossy(&p[9..9 + n]).trim()) {
                total = v;
            }
        }
        let mut out = Vec::new();
        let mut i = 0;
        let fail = |cn: &mut Conn, e: crate::error::Error| -> crate::error::Error {
            let _ = cn.round_trip(0x01, &upload_param(0x1F, id, 0xFFFF), &[]);
            e.context(format!("upload {name}"))
        };
        loop {
            let r = match self.round_trip_owned(0x01, &upload_param(0x1E, id, 0), &[]) {
                Ok(r) => r,
                Err(e) => return Err(fail(self, e)),
            };
            if r.param.len() < 2 || r.param[0] != 0x1E {
                let e = io(format!("bad upload reply param {}", hex_spaced(&r.param)));
                return Err(fail(self, e));
            }
            let st = r.param[1];
            if st & 0x02 != 0 {
                return Err(fail(self, io(format!("upload status 0x{st:02x} (error)"))));
            }
            if r.data.len() >= 4 {
                let mut n = u16::from_be_bytes([r.data[0], r.data[1]]) as usize;
                if 4 + n > r.data.len() {
                    n = r.data.len() - 4;
                }
                out.extend_from_slice(&r.data[4..4 + n]);
            }
            if st & 0x01 == 0 {
                break;
            }
            if i > 4096 {
                return Err(fail(self, io("too many upload steps")));
            }
            i += 1;
        }
        self.round_trip(0x01, &upload_param(0x1F, id, 0), &[]).map_err(|e| e.context(format!("end upload {name}")))?;
        if total >= 0 && total as usize != out.len() {
            return Err(local(format!("upload {name}: CPU announced {total} bytes, got {}", out.len())));
        }
        Ok(out)
    }

    /// Lists every block type (OB, FC, FB, DB, SDB) and uploads each block. A
    /// block that fails is reported in its `err`; a failing list (type not
    /// supported) is skipped, but if no type can be listed the last list error
    /// is returned. A transport error ends the call.
    pub fn upload_all(&mut self) -> Result<Vec<UploadedBlock>> {
        let mut out = Vec::new();
        let mut list_err = None;
        let mut listed = 0;
        for t in [BlockType::OB, BlockType::FC, BlockType::FB, BlockType::DB, BlockType::SDB] {
            let list = match self.list_blocks(t) {
                Ok(l) => l,
                Err(e) if e.retryable() => return Err(e),
                Err(e) => {
                    list_err = Some(e);
                    continue;
                }
            };
            listed += 1;
            for b in list {
                match self.upload(t, b.number) {
                    Ok(data) => out.push(UploadedBlock { typ: t, number: b.number, data, err: None }),
                    Err(e) if e.retryable() => return Err(e),
                    Err(e) => out.push(UploadedBlock { typ: t, number: b.number, data: Vec::new(), err: Some(e) }),
                }
            }
        }
        if listed == 0 {
            return Err(list_err.unwrap_or_else(|| io("no block type listed")));
        }
        Ok(out)
    }
}
