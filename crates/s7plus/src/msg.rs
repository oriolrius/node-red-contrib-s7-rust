//! Messages and item addresses.

use std::fmt;

use crate::error::{Error, Result};
use crate::ids::*;
use crate::textfmt::hex;
use crate::value::Value;
use crate::wire::{Reader, Writer};

/// One reassembled S7CommPlus PDU (without the 0x72 header/trailer).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    pub version: u8,
    pub opcode: u8,
    pub function: u16,
    pub seq: u16,
    /// Requests only.
    pub session: u32,
    pub tflags: u8,
    /// After the transport flags.
    pub body: Vec<u8>,
}

impl Message {
    /// Parses the data part of a PDU (starting at the opcode).
    pub fn parse(version: u8, data: &[u8]) -> Result<Message> {
        let mut r = Reader::new(data);
        let mut m = Message { version, opcode: r.byte(), ..Default::default() };
        match m.opcode {
            OP_REQUEST => {
                r.u16();
                m.function = r.u16();
                r.u16();
                m.seq = r.u16();
                m.session = r.u32();
                m.tflags = r.byte();
            }
            OP_RESPONSE | OP_RESPONSE2 => {
                r.u16();
                m.function = r.u16();
                r.u16();
                m.seq = r.u16();
                if m.function != FN_INIT_SSL {
                    m.tflags = r.byte();
                }
            }
            OP_NOTIFICATION => {
                // notifications: subscription object id + body, kept raw
            }
            op => return Err(Error::proto(format!("unknown opcode {op:#x}"))),
        }
        r.result()?;
        m.body = data[r.pos..].to_vec();
        Ok(m)
    }

    /// Returns the data part (opcode … body) of a request.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.byte(self.opcode);
        w.u16(0);
        w.u16(self.function);
        w.u16(0);
        w.u16(self.seq);
        if self.opcode == OP_REQUEST {
            w.u32(self.session);
        }
        if !(self.opcode != OP_REQUEST && self.function == FN_INIT_SSL) {
            w.byte(self.tflags);
        }
        w.raw(&self.body);
        w.b
    }

    /// Short description for logs ("Request GetMultiVariables seq=3 …").
    pub fn summary(&self) -> String {
        let op = match self.opcode {
            OP_REQUEST => "Request",
            OP_RESPONSE => "Response",
            OP_RESPONSE2 => "Response2",
            OP_NOTIFICATION => "Notification",
            _ => "?",
        };
        if self.opcode == OP_NOTIFICATION {
            return format!("V{} {op} len={}", self.version, self.body.len());
        }
        format!(
            "V{} {op} {} seq={} session={:#x} tflags={:#x} len={}",
            self.version,
            crate::ids::fn_name(self.function),
            self.seq,
            self.session,
            self.tflags,
            self.body.len()
        )
    }

    /// Summary plus the hex dump of the encoded message.
    pub fn dump(&self) -> String {
        format!("{} {}", self.summary(), hex(&self.encode_any()))
    }

    fn encode_any(&self) -> Vec<u8> {
        if self.opcode == OP_NOTIFICATION {
            let mut v = vec![self.opcode];
            v.extend_from_slice(&self.body);
            return v;
        }
        self.encode()
    }
}

/// A variable address: symbol CRC, access area (an object id, e.g. 82 = M
/// area or 0x8a0e0000+n = DB n), sub-area (attribute, e.g. 3736
/// ControllerArea.ValueActual or 2550 DB.ValueActual) and the LID path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ItemAddress {
    pub symbol_crc: u32,
    pub area: u32,
    pub sub_area: u32,
    pub lid: Vec<u32>,
}

impl ItemAddress {
    /// Number of VLQ fields the address occupies (for the field count).
    pub fn fields(&self) -> u32 {
        4 + self.lid.len() as u32
    }

    /// Appends the address.
    pub fn encode(&self, w: &mut Writer) {
        w.vlq32(self.symbol_crc);
        w.vlq32(self.area);
        w.vlq32(self.lid.len() as u32 + 1);
        w.vlq32(self.sub_area);
        for l in &self.lid {
            w.vlq32(*l);
        }
    }

    /// Reads an address.
    pub fn decode(r: &mut Reader) -> ItemAddress {
        let mut a = ItemAddress { symbol_crc: r.vlq32(), area: r.vlq32(), ..Default::default() };
        let n = r.vlq32();
        if n == 0 || n as usize > r.left() {
            r.fail();
            return a;
        }
        a.sub_area = r.vlq32();
        for _ in 1..n {
            a.lid.push(r.vlq32());
        }
        a
    }

    /// The address with one more LID.
    pub fn child(&self, lid: u32) -> ItemAddress {
        let mut a = self.clone();
        a.lid.push(lid);
        a
    }

    /// Renders the address like the reference ("8A0E0001.A.1").
    pub fn access_string(&self) -> String {
        let mut s = format!("{:X}", self.area);
        for l in &self.lid {
            s.push_str(&format!(".{l:X}"));
        }
        s
    }

    /// Parses "8A0E0001.A.1" (hex ids separated by dots).
    pub fn parse_access_string(s: &str) -> Result<ItemAddress> {
        let mut a = ItemAddress::default();
        for (i, p) in s.split('.').enumerate() {
            let v = u32::from_str_radix(p, 16).map_err(|_| Error::proto(format!("bad access string {s:?}")))?;
            if i == 0 {
                a.area = v;
            } else {
                a.lid.push(v);
            }
        }
        a.sub_area = sub_area_for(a.area);
        Ok(a)
    }
}

pub(crate) fn sub_area_for(area: u32) -> u32 {
    if area >= 0x8a0e_0000 {
        ATTR_DB_VALUE_ACTUAL
    } else {
        ATTR_CTRL_AREA_VALUE_ACT
    }
}

impl fmt::Display for ItemAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "addr{{crc={:#x} area={} sub={} lid={:?}}}", self.symbol_crc, id_name(self.area), id_name(self.sub_area), self.lid)
    }
}

/// Appends the fixed object qualifier used by requests.
pub(crate) fn encode_object_qualifier(w: &mut Writer) {
    w.u32(ATTR_OBJECT_QUALIFIER);
    w.vlq32(ATTR_PARENT_RID);
    Value::rid(0).encode(w);
    w.vlq32(ATTR_COMPOSITION_AID);
    Value::aid(0).encode(w);
    w.vlq32(ATTR_KEY_QUALIFIER);
    Value::udint(0).encode(w);
    w.byte(0);
}
