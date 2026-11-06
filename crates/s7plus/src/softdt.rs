//! Softdatatypes: PLC data types as seen in the type information.

use crate::typeinfo::Vartype;

pub const SDT_VOID: u8 = 0;
pub const SDT_BOOL: u8 = 1;
pub const SDT_BYTE: u8 = 2;
pub const SDT_CHAR: u8 = 3;
pub const SDT_WORD: u8 = 4;
pub const SDT_INT: u8 = 5;
pub const SDT_DWORD: u8 = 6;
pub const SDT_DINT: u8 = 7;
pub const SDT_REAL: u8 = 8;
pub const SDT_DATE: u8 = 9;
pub const SDT_TIME_OF_DAY: u8 = 10;
pub const SDT_TIME: u8 = 11;
pub const SDT_S5TIME: u8 = 12;
pub const SDT_DATE_AND_TIME: u8 = 14;
pub const SDT_STRUCT: u8 = 17;
pub const SDT_STRING: u8 = 19;
pub const SDT_POINTER: u8 = 20;
pub const SDT_ANY: u8 = 22;
pub const SDT_BLOCK_FB: u8 = 23;
pub const SDT_BLOCK_FC: u8 = 24;
pub const SDT_COUNTER: u8 = 28;
pub const SDT_TIMER: u8 = 29;
pub const SDT_BBOOL: u8 = 40;
pub const SDT_LREAL: u8 = 48;
pub const SDT_ULINT: u8 = 49;
pub const SDT_LINT: u8 = 50;
pub const SDT_LWORD: u8 = 51;
pub const SDT_USINT: u8 = 52;
pub const SDT_UINT: u8 = 53;
pub const SDT_UDINT: u8 = 54;
pub const SDT_SINT: u8 = 55;
pub const SDT_WCHAR: u8 = 61;
pub const SDT_WSTRING: u8 = 62;
pub const SDT_VARIANT: u8 = 63;
pub const SDT_LTIME: u8 = 64;
pub const SDT_LTOD: u8 = 65;
pub const SDT_LDT: u8 = 66;
pub const SDT_DTL: u8 = 67;
pub const SDT_REMOTE: u8 = 96;
pub const SDT_AOM_IDENT: u8 = 128;
pub const SDT_EVENT_ANY: u8 = 129;
pub const SDT_EVENT_ATT: u8 = 130;
pub const SDT_EVENT_HW_INT: u8 = 134;
pub const SDT_HW_ANY: u8 = 144;
pub const SDT_HW_IE_PORT: u8 = 156;
pub const SDT_OB_ANY: u8 = 160;
pub const SDT_OB_ATT: u8 = 164;
pub const SDT_CONN_ANY: u8 = 168;
pub const SDT_CONN_RID: u8 = 171;
pub const SDT_PORT: u8 = 173;
pub const SDT_RTM: u8 = 174;
pub const SDT_PIP: u8 = 175;
pub const SDT_OB_PCYCLE: u8 = 192;
pub const SDT_OB_STARTUP: u8 = 197;
pub const SDT_DB_ANY: u8 = 208;
pub const SDT_DB_WWW: u8 = 209;
pub const SDT_DB_DYN: u8 = 210;

fn sdt_name(t: u8) -> Option<&'static str> {
    Some(match t {
        0 => "Void",
        1 => "Bool",
        2 => "Byte",
        3 => "Char",
        4 => "Word",
        5 => "Int",
        6 => "DWord",
        7 => "DInt",
        8 => "Real",
        9 => "Date",
        10 => "Time_Of_Day",
        11 => "Time",
        12 => "S5Time",
        13 => "S5Count",
        14 => "Date_And_Time",
        15 => "InternetTime",
        16 => "Array",
        17 => "Struct",
        18 => "EndStruct",
        19 => "String",
        20 => "Pointer",
        21 => "MultiFB",
        22 => "Any",
        23 => "Block_FB",
        24 => "Block_FC",
        25 => "Block_DB",
        26 => "Block_SDB",
        27 => "MultiSFB",
        28 => "Counter",
        29 => "Timer",
        30 => "IEC_Counter",
        31 => "IEC_Timer",
        32 => "Block_SFB",
        33 => "Block_SFC",
        34 => "Block_CB",
        35 => "Block_SCB",
        36 => "Block_OB",
        37 => "Block_UDT",
        38 => "Offset",
        39 => "Block_SDT",
        40 => "BBool",
        41 => "Block_Ext",
        48 => "LReal",
        49 => "ULInt",
        50 => "LInt",
        51 => "LWord",
        52 => "USInt",
        53 => "UInt",
        54 => "UDInt",
        55 => "SInt",
        56 => "BCD8",
        57 => "BCD16",
        58 => "BCD32",
        59 => "BCD64",
        60 => "ARef",
        61 => "WChar",
        62 => "WString",
        63 => "Variant",
        64 => "LTime",
        65 => "LTOD",
        66 => "LDT",
        67 => "DTL",
        68 => "IEC_LTimer",
        69 => "IEC_SCounter",
        70 => "IEC_DCounter",
        71 => "IEC_LCounter",
        72 => "IEC_UCounter",
        73 => "IEC_USCounter",
        74 => "IEC_UDCounter",
        75 => "IEC_ULCounter",
        96 => "Remote",
        97 => "ErrorStruct",
        98 => "NRef",
        99 => "VRef",
        100 => "FBTRef",
        101 => "CRef",
        102 => "VARef",
        128 => "AOM_Ident",
        129 => "Event_Any",
        130 => "Event_Att",
        131 => "Folder",
        132 => "AOM_AID",
        133 => "AOM_Link",
        134 => "Event_HwInt",
        144 => "Hw_Any",
        145 => "Hw_IoSystem",
        146 => "Hw_DpMaster",
        147 => "Hw_Device",
        148 => "Hw_DpSlave",
        149 => "Hw_Io",
        150 => "Hw_Module",
        151 => "Hw_SubModule",
        152 => "Hw_Hsc",
        153 => "Hw_Pwm",
        154 => "Hw_Pto",
        155 => "Hw_Interface",
        156 => "Hw_IEPort",
        160 => "OB_Any",
        161 => "OB_Delay",
        162 => "OB_Tod",
        163 => "OB_Cyclic",
        164 => "OB_Att",
        168 => "Conn_Any",
        169 => "Conn_Prg",
        170 => "Conn_Ouc",
        171 => "Conn_R_ID",
        172 => "HW_NR",
        173 => "Port",
        174 => "RTM",
        175 => "PIP",
        176 => "C_Alarm",
        177 => "C_Alarm_S",
        178 => "C_Alarm_8",
        179 => "C_Alarm_8p",
        180 => "C_Alarm_T",
        181 => "C_AR_Send",
        182 => "C_Notify",
        183 => "C_Notify_8p",
        192 => "OB_PCycle",
        193 => "OB_HwInt",
        194 => "OB_Comm",
        195 => "OB_Diag",
        196 => "OB_TimeError",
        197 => "OB_Startup",
        200 => "OPCUA_LocTxtEncM",
        201 => "OPCUA_StrActLen",
        208 => "DB_Any",
        209 => "DB_WWW",
        210 => "DB_Dyn",
        253 => "Para",
        254 => "Label",
        255 => "Undefined",
        _ => return None,
    })
}

/// The PLC type name of a softdatatype.
pub fn softdatatype_name(t: u8) -> String {
    match sdt_name(t) {
        Some(n) => n.to_string(),
        None => format!("sdt{t}"),
    }
}

/// Finds a softdatatype by its (case-insensitive) name.
pub fn softdatatype_by_name(s: &str) -> Option<u8> {
    (0..=255u8).find(|t| sdt_name(*t).is_some_and(|n| n.eq_ignore_ascii_case(s)))
}

/// Size of one array element in the PLC image.
pub(crate) fn elem_size(v: &Vartype) -> u32 {
    match v.softdatatype {
        SDT_BOOL | SDT_BYTE | SDT_CHAR | SDT_USINT | SDT_SINT | SDT_BBOOL => 1,
        SDT_WORD | SDT_INT | SDT_DATE | SDT_S5TIME | SDT_UINT | SDT_WCHAR | SDT_BLOCK_FB | SDT_BLOCK_FC | SDT_COUNTER | SDT_TIMER
        | SDT_PORT | SDT_RTM | SDT_PIP | SDT_OB_PCYCLE | SDT_OB_STARTUP | SDT_DB_ANY | SDT_DB_WWW | SDT_DB_DYN | SDT_CONN_ANY | 169
        | 170 => 2,
        SDT_DWORD | SDT_DINT | SDT_REAL | SDT_TIME_OF_DAY | SDT_TIME | SDT_UDINT | SDT_AOM_IDENT | SDT_EVENT_ANY | SDT_EVENT_ATT
        | SDT_EVENT_HW_INT | SDT_CONN_RID => 4,
        SDT_POINTER => 6,
        SDT_DATE_AND_TIME | SDT_LREAL | SDT_ULINT | SDT_LINT | SDT_LWORD | SDT_LTIME | SDT_LTOD | SDT_LDT => 8,
        SDT_ANY | SDT_REMOTE => 10,
        SDT_DTL => 12,
        SDT_STRING => v.offset.info1 as u32 + 2,
        SDT_WSTRING => (v.offset.info1 as u32 + 2) * 2,
        t if (144..=164).contains(&t) => 2,
        _ => 0,
    }
}
