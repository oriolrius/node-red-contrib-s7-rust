//! Well-known object, class and attribute ids, opcodes and function codes.

pub const RID_PLC_PROGRAM: u32 = 3;
pub const RID_CPU_EXEC_UNIT: u32 = 52;
pub const RID_I_AREA: u32 = 80;
pub const RID_Q_AREA: u32 = 81;
pub const RID_M_AREA: u32 = 82;
pub const RID_S7_COUNTERS: u32 = 83;
pub const RID_S7_TIMERS: u32 = 84;
pub const RID_ROOT: u32 = 201;
pub const RID_GET_NEW_ON_SERVER: u32 = 211;
pub const CLASS_SUBSCRIPTIONS: u32 = 255;
pub const CLASS_SERVER_SESS_CONT: u32 = 284;
pub const OBJ_SERVER_SESS_CONT: u32 = 285;
pub const CLASS_SERVER_SESSION: u32 = 287;
pub const OBJ_NULL_SERVER_SESSION: u32 = 288;
pub const ATTR_SESS_CLIENT_RID: u32 = 300;
pub const ATTR_SESS_REQUEST: u32 = 303;
pub const ATTR_SESS_RESPONSE: u32 = 304;
pub const ATTR_SESS_VERSION: u32 = 306;
pub const ATTR_SYSTEM_LIMITS: u32 = 1037;
pub const ATTR_OBJECT_QUALIFIER: u32 = 1256;
pub const ATTR_PARENT_RID: u32 = 1257;
pub const ATTR_COMPOSITION_AID: u32 = 1258;
pub const ATTR_KEY_QUALIFIER: u32 = 1259;
pub const ATTR_EFF_PROT_LEVEL: u32 = 1842;
pub const ATTR_ACT_PROT_LEVEL: u32 = 1843;
pub const ATTR_LEGITIMATE: u32 = 1846;
pub const ATTR_OPERATING_STATE_REQ: u32 = 2167;
pub const ATTR_DB_VALUE_ACTUAL: u32 = 2550;
pub const ATTR_CTRL_AREA_VALUE_ACT: u32 = 3736;
pub const ATTR_RETURN_VALUE: u32 = 40305;

// Object model.
pub const RID_ALARM_SUBSYSTEM: u32 = 8;
pub const ATTR_OBJECT_VARIABLE_TYPE_NAME: u32 = 233;
pub const ATTR_PARENT_OBJECT: u32 = 229;
pub const CLASS_TYPE_INFO: u32 = 511;
pub const CLASS_OMS_TYPE_INFO_CONTAINER: u32 = 534;
pub const OBJ_OMS_TYPE_INFO_CONTAINER: u32 = 537;
pub const CLASS_TEXT_LIBRARY: u32 = 606;
pub const ATTR_TEXT_LIB_OFFSET_AREA: u32 = 608;
pub const ATTR_TEXT_LIB_STRING_AREA: u32 = 609;
pub const CLASS_SUBSCRIPTION: u32 = 1001;
pub const ATTR_SUB_MISSED_SENDINGS: u32 = 1002;
pub const ATTR_SUB_SUBSYSTEM_ERROR: u32 = 1003;
pub const ATTR_SUB_REF_TRIGGER_MODE: u32 = 1005;
pub const ATTR_SUB_ROUTE_MODE: u32 = 1040;
pub const ATTR_SUB_ACTIVE: u32 = 1041;
pub const ATTR_SUB_REFERENCE_LIST: u32 = 1048;
pub const ATTR_SUB_CYCLE_TIME: u32 = 1049;
pub const ATTR_SUB_DELAY_TIME: u32 = 1050;
pub const ATTR_SUB_DISABLED: u32 = 1051;
pub const ATTR_SUB_COUNT: u32 = 1052;
pub const ATTR_SUB_CREDIT_LIMIT: u32 = 1053;
pub const ATTR_SUB_TICKS: u32 = 1054;
pub const ATTR_FREE_ITEMS: u32 = 1081;
pub const ATTR_SUB_FUNCTION_CLASS_ID: u32 = 1082;
pub const ATTR_FILTER: u32 = 1246;
pub const ATTR_FILTER_OPERATION: u32 = 1247;
pub const ATTR_FILTER_ADDR_COUNT: u32 = 1249;
pub const ATTR_FILTER_ADDRESS: u32 = 1250;
pub const ATTR_FILTER_VALUE: u32 = 1251;
pub const ATTR_TI_TCOM_SIZE: u32 = 1502;
pub const CLASS_PLC_PROGRAM: u32 = 2520;
pub const ATTR_BLOCK_NUMBER: u32 = 2521;
pub const ATTR_DATA_IF_DESCRIPTION: u32 = 2544;
pub const ATTR_DATA_IF_LINE_COMMENTS: u32 = 2546;
pub const ATTR_DB_VALUE_INITIAL: u32 = 2548;
pub const CLASS_DB: u32 = 2574;
pub const ATTR_ALARM_SUB_REF_DOMAIN: u32 = 2659;
pub const ATTR_ALARM_SUB_REF_SUBSYSTEM: u32 = 2660;
pub const CLASS_ALARM_SUBSCRIPTION_REF: u32 = 2662;
pub const ATTR_SUBSYS_UPD_RELEVANT_DAI: u32 = 2667;
pub const ATTR_DAI_CPU_ALARM_ID: u32 = 2670;
pub const ATTR_DAI_ALL_STATES_INFO: u32 = 2671;
pub const ATTR_DAI_ALARM_DOMAIN: u32 = 2672;
pub const ATTR_DAI_COMING: u32 = 2673;
pub const ATTR_DAI_GOING: u32 = 2677;
pub const CLASS_DAI: u32 = 2681;
pub const ATTR_DAI_ALARM_TEXTS: u32 = 2715;
pub const ATTR_AS_CGS_ALL_STATES_INFO: u32 = 3474;
pub const ATTR_AS_CGS_TIMESTAMP: u32 = 3475;
pub const ATTR_AS_CGS_ASSOC_VALUES: u32 = 3476;
pub const ATTR_AS_CGS_ACK_TIMESTAMP: u32 = 3646;
// Alarm acknowledgement: an AckJob (class 3636, base AlarmingJob 3640)
// created under the alarm subsystem (8); its AcknowledgementList holds
// AS_AlarmAcknfyElem structs (7604). Names from the OMS+ object model.
pub const CLASS_ACK_JOB: u32 = 3636;
pub const ATTR_ALARMING_JOB_APPLICATION: u32 = 3637;
pub const ATTR_ALARMING_JOB_HOST: u32 = 3638;
pub const ATTR_ALARMING_JOB_USER: u32 = 3639;
pub const ATTR_ALARMING_JOB_STATE: u32 = 3641;
pub const ATTR_ALARMING_JOB_TIMESTAMP: u32 = 3642;
pub const ATTR_ACK_JOB_LIST: u32 = 3647;
pub const STRUCT_ALARM_ACKNFY_ELEM: u32 = 7604;
pub const ATTR_ACKNFY_ALL_STATES_INFO: u32 = 7596;
pub const ATTR_ACKNFY_ACK_RESULT: u32 = 7597;
pub const ATTR_ACKNFY_CPU_ALARM_ID: u32 = 7598;
pub const ATTR_DAI_MESSAGE_TYPE: u32 = 4079;
pub const ATTR_AS_OBJECT_ES_COMMENT: u32 = 4288;
/// S7-1200 FW V3: block comment, sparse WString per language.
pub const ATTR_AS_OBJECT_COMMENT_V3: u32 = 4287;
pub const ATTR_ALARM_SUB_REF_DOMAIN2: u32 = 7731;
pub const ATTR_DAI_HMI_INFO: u32 = 7813;
pub const CLASS_MULTIPLE_STAI: u32 = 7854;
pub const ATTR_MULTIPLE_STAIS: u32 = 7859;
pub const ATTR_DAI_SEQUENCE_COUNTER: u32 = 7917;
pub const ATTR_ALARM_SUB_REF_SEND_TEXTS: u32 = 8173;
pub const ATTR_ALARM_SUB_REF_TEXT_LANGS: u32 = 8181;
pub const ATTR_LEGIT_PAYLOAD_STRUCT: u32 = 40400;
pub const ATTR_LEGIT_PAYLOAD_TYPE: u32 = 40401;
pub const ATTR_LEGIT_PAYLOAD_USER: u32 = 40402;
pub const ATTR_LEGIT_PAYLOAD_PASSWORD: u32 = 40403;
pub const TI_DTL: u32 = 0x0200_0043;
pub const RID_I_TYPE_INFO: u32 = 0x9001_0000;
pub const RID_Q_TYPE_INFO: u32 = 0x9002_0000;
pub const RID_M_TYPE_INFO: u32 = 0x9003_0000;
pub const RID_TIMERS_TYPE_INFO: u32 = 0x9005_0000;
pub const RID_COUNTERS_TYPE_INFO: u32 = 0x9006_0000;
pub const RID_AS_ALARMS: u32 = 0x8a7e_0000;
pub const RID_TEXT_LIB_BASE: u32 = 0x8a36_0000;

// Opcodes.
pub const OP_REQUEST: u8 = 0x31;
pub const OP_RESPONSE: u8 = 0x32;
pub const OP_NOTIFICATION: u8 = 0x33;
pub const OP_RESPONSE2: u8 = 0x02;

// Function codes.
pub const FN_ERROR: u16 = 0x04b1;
pub const FN_EXPLORE: u16 = 0x04bb;
pub const FN_CREATE_OBJECT: u16 = 0x04ca;
pub const FN_DELETE_OBJECT: u16 = 0x04d4;
pub const FN_SET_VARIABLE: u16 = 0x04f2;
pub const FN_GET_VARIABLE: u16 = 0x04fc;
pub const FN_ADD_LINK: u16 = 0x0506;
pub const FN_REMOVE_LINK: u16 = 0x051a;
pub const FN_GET_LINK: u16 = 0x0524;
pub const FN_SET_MULTI_VARIABLES: u16 = 0x0542;
pub const FN_GET_MULTI_VARIABLES: u16 = 0x054c;
pub const FN_BEGIN_SEQUENCE: u16 = 0x0556;
pub const FN_END_SEQUENCE: u16 = 0x0560;
pub const FN_INVOKE: u16 = 0x056b;
pub const FN_SET_VAR_SUB_STREAMED: u16 = 0x057c;
pub const FN_GET_VAR_SUB_STREAMED: u16 = 0x0586;
pub const FN_GET_VARIABLES_ADDR: u16 = 0x0590;
pub const FN_ABORT: u16 = 0x059a;
pub const FN_ERROR2: u16 = 0x05a9;
pub const FN_INIT_SSL: u16 = 0x05b3;

fn id_name_str(id: u32) -> Option<&'static str> {
    Some(match id {
        3 => "PLCProgram",
        8 => "AlarmSubsystem",
        52 => "CPUexecUnit",
        80 => "IArea",
        81 => "QArea",
        82 => "MArea",
        83 => "S7Counters",
        84 => "S7Timers",
        201 => "Root",
        211 => "GetNewRIDOnServer",
        229 => "ObjectVariableTypeParentObject",
        233 => "VariableTypeName",
        255 => "ClassSubscriptions",
        284 => "ClassServerSessionContainer",
        285 => "ObjectServerSessionContainer",
        287 => "ClassServerSession",
        288 => "ObjectNullServerSession",
        299 => "ServerSessionClientComment?",
        300 => "ServerSessionClientRID",
        303 => "ServerSessionRequest",
        304 => "ServerSessionResponse",
        306 => "ServerSessionVersion",
        319 => "SessionVersionSystemPAOMString",
        511 => "ClassTypeInfo",
        534 => "ClassOMSTypeInfoContainer",
        537 => "ObjectOMSTypeInfoContainer",
        606 => "ClassTextLibrary",
        608 => "TextLibraryOffsetArea",
        609 => "TextLibraryStringArea",
        1001 => "ClassSubscription",
        1002 => "SubscriptionMissedSendings",
        1003 => "SubscriptionSubsystemError",
        1005 => "SubscriptionRefTriggerAndTransmitMode",
        1037 => "SystemLimits",
        1040 => "SubscriptionRouteMode",
        1041 => "SubscriptionActive",
        1048 => "SubscriptionReferenceList",
        1049 => "SubscriptionCycleTime",
        1050 => "SubscriptionDelayTime",
        1051 => "SubscriptionDisabled",
        1052 => "SubscriptionCount",
        1053 => "SubscriptionCreditLimit",
        1054 => "SubscriptionTicks",
        1081 => "FreeItems",
        1082 => "SubscriptionFunctionClassId",
        1246 => "Filter",
        1247 => "FilterOperation",
        1249 => "AddressCount",
        1250 => "Address",
        1251 => "FilterValue",
        1256 => "ObjectQualifier",
        1257 => "ParentRID",
        1258 => "CompositionAID",
        1259 => "KeyQualifier",
        1502 => "TI_TComSize",
        1842 => "EffectiveProtectionLevel",
        1843 => "ActiveProtectionLevel",
        1846 => "Legitimate",
        2167 => "CPUexecUnit.operatingStateReq",
        2520 => "PLCProgram_Class",
        2521 => "Block.BlockNumber",
        2544 => "DataInterface.InterfaceDescription",
        2546 => "DataInterface.LineComments",
        2548 => "DB.ValueInitial",
        2550 => "DB.ValueActual",
        2551 => "DB.InitialChanged",
        2574 => "DB_Class",
        2659 => "AlarmSubscriptionRef.AlarmDomain",
        2660 => "AlarmSubscriptionRef.itsAlarmSubsystem",
        2662 => "AlarmSubscriptionRef_Class",
        2667 => "AlarmSubsystem.itsUpdateRelevantDAI",
        2670 => "DAI.CPUAlarmID",
        2671 => "DAI.AllStatesInfo",
        2672 => "DAI.AlarmDomain",
        2673 => "DAI.Coming",
        2677 => "DAI.Going",
        2681 => "DAI_Class",
        2715 => "DAI.AlarmTexts",
        3474 => "AS_CGS.AllStatesInfo",
        3475 => "AS_CGS.Timestamp",
        3476 => "AS_CGS.AssociatedValues",
        3636 => "AckJob_Class",
        3637 => "AlarmingJob.Application",
        3638 => "AlarmingJob.Host",
        3639 => "AlarmingJob.User",
        3641 => "AlarmingJob.AlarmJobState",
        3642 => "AlarmingJob.JobTimestamp",
        3647 => "AckJob.AcknowledgementList",
        7596 => "AS_AlarmAcknfyElem.AllStatesInfo",
        7597 => "AS_AlarmAcknfyElem.AckResult",
        7598 => "AS_AlarmAcknfyElem.CPUAlarmID",
        7604 => "AS_AlarmAcknfyElem",
        3646 => "AS_CGS.AckTimestamp",
        3735 => "ControllerArea.ValueInitial",
        3736 => "ControllerArea.ValueActual",
        3737 => "ControllerArea.RuntimeModified",
        4079 => "DAI.MessageType",
        4288 => "ASObjectES.Comment",
        7731 => "AlarmSubscriptionRef.AlarmDomain2",
        7813 => "DAI.HmiInfo",
        7854 => "MultipleSTAI_Class",
        7859 => "MultipleSTAI.STAIs",
        7917 => "DAI.SequenceCounter",
        8173 => "AlarmSubscriptionRef.SendAlarmTexts",
        8181 => "AlarmSubscriptionRef.AlarmTextLanguages",
        40305 => "ReturnValue",
        40400 => "LegitimationPayloadStruct",
        40401 => "LegitimationPayloadType",
        40402 => "LegitimationPayloadUsername",
        40403 => "LegitimationPayloadPassword",
        _ => return None,
    })
}

/// Renders an id with its symbolic name when known.
pub fn id_name(id: u32) -> String {
    if let Some(n) = id_name_str(id) {
        return format!("{id}({n})");
    }
    if id >= 0x10000 {
        return format!("{id:#x}");
    }
    id.to_string()
}

/// Returns the name of a function code.
pub fn fn_name(f: u16) -> String {
    let n = match f {
        0x04b1 => "Error",
        0x04bb => "Explore",
        0x04ca => "CreateObject",
        0x04d4 => "DeleteObject",
        0x04f2 => "SetVariable",
        0x04fc => "GetVariable",
        0x0506 => "AddLink",
        0x051a => "RemoveLink",
        0x0524 => "GetLink",
        0x0542 => "SetMultiVariables",
        0x054c => "GetMultiVariables",
        0x0556 => "BeginSequence",
        0x0560 => "EndSequence",
        0x056b => "Invoke",
        0x057c => "SetVarSubStreamed",
        0x0586 => "GetVarSubStreamed",
        0x0590 => "GetVariablesAddress",
        0x059a => "Abort",
        0x05a9 => "Error2",
        0x05b3 => "InitSsl",
        _ => return format!("fn{f:#06x}"),
    };
    n.to_string()
}
