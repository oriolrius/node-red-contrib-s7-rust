# PLCSIM test program

The test program of the live tests against PLCSIM Advanced: the blocks, how to load them,
and two Node-RED scripts. `npm test` does not use them (it uses the fake PLCs). They write
to the PLC, so use them only with a simulated or test CPU.

## Setup

| | |
|---|---|
| Simulator | PLCSIM Advanced V8, one instance (`plcsim01`), PLCSIM Virtual Ethernet Adapter |
| CPU | CPU 1513-1 PN (6ES7513-1AM03-0AB0), FW V3.0 in TIA Portal V21; PLCSIM reports FW S4.1 |
| Address | 192.168.0.1 |
| Security | the CPU accepts only TLS sessions ("the PLC refused the session without TLS"); turn on TLS in the endpoint. TIA Portal asks for a password for the confidential PLC configuration data before the first compile. |
| PUT/GET | not needed (S7CommPlus); S7comm reads of `TestAbs` need it |

## Blocks (`blocks/`, SIMATIC SD documents, UTF-8 with BOM)

| Block | Purpose |
|---|---|
| DB1 `TestAbs` | standard DB (absolute addresses): `b0`, `b1`, `i2`, `di4`, `r8`, `x12` with start values |
| DB2 `TestOpt` | optimized DB: Bool, Int, DInt, Real, LReal, Word, String[32] with start values |
| DB3 `SimTimers` | `t100 : TON_TIME`, a timer that runs all the time |
| DB4 `TestSub` | values that change: `tick` (+1 every 100 ms), `toggle` (every 1 s), `slow` (every 5 s), `rSine`; `sp` / `echo` (the PLC copies `sp` to `echo`: a write shows in the next read or notification); `alarmReq`, `alarmVal` (alarm `alReq`) |
| FB1 `AlarmGen` | two Program_Alarm: `alReq` on `"TestSub".alarmReq` with SD_1 = `alarmVal`, alarm class "Acknowledgement" (needs acknowledgement, see below); `alAuto` comes and goes every 5 s with SD_1 = `tick`, default class (no acknowledgement) |
| DB5 `AlarmGen_DB` | instance DB of `AlarmGen` |
| OB1 `Main` | one SCL network: the timer, the values of `TestSub`, the `sp` -> `echo` copy, the call of `AlarmGen_DB` |

The alarms have no texts (the TIA Portal Openness API cannot write instance texts); add them in
the TIA Portal alarm editor to test the texts.

Notes:

- An S7-1500 IEC timer updates `Q` and `ET` when the program reads them, so the pulse pattern
  `t(IN := NOT t.Q, ...)` does not work in SCL: `tick` comes from `ET` of a timer that runs all
  the time.
- A Bool of an optimized DB has the softdatatype BBool on the wire.

## Load the program

With TIA Portal V21 and the TIA Portal Openness MCP server (or by hand in TIA Portal):

1. Open or create the project with the CPU above.
2. `ImportBlocksFromDocuments` with this `blocks/` directory (option `Override`). If the import of
   `AlarmGen` fails, import it as an SCL external source and generate the block
   (`ImportPlcExternalSource` + `GenerateBlocksFromExternalSource`), then create the instance DB
   (`CreateInstanceDB`).
3. Alarm class of `alReq` (the block source has no alarm class): with the project open in TIA
   Portal, `powershell -ExecutionPolicy Bypass -File test/plcsim/set-alarm-class.ps1 -ProjectPath
   <path>.ap21` (imports `alarm-classes.xlsx` with TIA Portal Openness; accept the Openness access
   dialog), or set it in TIA Portal (FB `AlarmGen`, alarm `alReq`, alarm class "Acknowledgement").
4. `CompileAndDiagnosePlc`, `SaveProject`.
5. `DownloadToPlc` with `pgPcInterface = "PLCSIM"`: the automatic pick of the PG/PC interface can
   take another adapter (for example Hyper-V) and fail with "Connect to module failed".

To export the program again after a change: `ExportBlocksAsDocuments` to this directory.

## Scripts

Build the test addon first (`npm run test:node` builds `test/.bin/s7napi-test.node`). The scripts
load the real nodes in a Node-RED runtime (node-red-node-test-helper). `PLC_HOST` sets the PLC
address (default 192.168.0.1).

| Script | What it does |
|---|---|
| `node test/plcsim/live_watch.js [push\|poll]` | `s7 watch` on `TestSub` and `TestOpt.sText` (interval 100 ms), writes `sp = 1001` and a String with `s7 write`, prints the messages |
| `node test/plcsim/live_alarms.js [auto\|tia\|reference]` | `s7 alarms`, writes `alarmReq = true` / `false` with `s7 write`, acknowledges `alReq` (and `alAuto`: refused) with `msg.ack`, then lets `alReq` go before its acknowledgement and acknowledges it with `"all"`; prints the alarm events and the node status |
| `node test/plcsim/live_cert.js` | reads the PLC certificate (`s7 info` `certificate`), then connects with a pinned fingerprint, with the certificate as trusted certificate, with a wrong fingerprint and with a wrong certificate name; prints the state of each and `RESULT ok` |

Expected: push mode sends all tags first, then `tick` / `rSine` every 100 ms and the writes about
100 ms later; `alReq` comes about 20 ms after the write of `alarmReq = true` with the value
`[4, 210]` (1234) and goes after `false`; `alAuto` comes and goes every 5 s. The PLC certificate
of PLCSIM is self-signed (CN `PLC-1/Communication-1`, `IP:192.168.0.1`). With "Use global security
settings for certificate manager" (TIA Portal, CPU properties, Protection & Security, Certificate
manager) the project CA ("Siemens TIA Project- <project>- (EC)-n") signs it; export the CA in
Security settings, Security features, Certificate manager, to test *trusted certificate or
project CA* with it. TIA Portal Openness V21 has no API for the global certificate manager, and
a download after a change of the PLC certificate needs the trust dialog of TIA Portal.
