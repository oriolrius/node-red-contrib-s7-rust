# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `s7 alarms`: acknowledges alarms. An input message with `msg.ack` (an alarm id `"0x…"`, a list of
  ids, or `"all"` / `true`: every alarm that waits for acknowledgement) acknowledges the coming
  state, as the TIA Portal alarm display does. The node sends `state: "acknowledged"` or
  `"ackFailed"` (`payload.ackResult`: the PLC refuses an alarm whose class needs no
  acknowledgement) per alarm. Alarm messages have `payload.acknowledged`. An alarm that went
  before its acknowledgement stays in the node list until it is acknowledged (the PLC keeps it
  too); the status shows the active alarms and how many wait for acknowledgement.
- `s7 info`: action `acknowledge {ids, all}` (writes; the admin API refuses it).
- Rust crate `s7plus`: `Client::acknowledge_alarms` (CreateObject of an AckJob, class 3636, under
  the alarm subsystem; at most 100 alarms per job, like TIA Portal), `decode_ack_job`,
  `AckResult`; arrays of structs (dynamic array, flags 0x20) are encoded and decoded with the
  struct id once after the datatype, then the count and the members of each element. The fake
  PLC has alarms with acknowledgement (`raise_alarm_ack`) and answers AckJobs.
- `test/plcsim`: `set-alarm-class.ps1` and `alarm-classes.xlsx` give `alReq` the alarm class
  "Acknowledgement"; `live_alarms.js` acknowledges it. Tested on PLCSIM Advanced V8 (S7-1500,
  FW S4.1): acknowledged in about 15 ms; an alarm of a class without acknowledgement gives
  `ackFailed` (0x8005).
- BSEND / BRCV (S7 communication over an S7 connection): the endpoint protocol "S7 partner" and
  two nodes. `s7 brcv` sends a message for each block of a PLC `BSEND` (`msg.payload` Buffer,
  `msg.rid`; filter by R_ID); `s7 bsend` sends `msg.payload` (Buffer, byte array or string) to a
  PLC `BRCV` with the R_ID of the node or `msg.rid`. Blocks up to 65534 bytes, in fragments of
  the PDU size. Node-RED connects actively with the TSAPs of the S7 connection and connects again
  after a loss. Blocks that come before the first `s7 brcv` node starts go to that node.
- Rust crate `s7`: `partner::Partner` (connect, `recv`, `send`) and a fake PLC
  (`partner_fake`). The acknowledgement of a fragmented block carries the IDSeq of the fragments
  as its sequence and the PDU reference of the request, as an S7-1500 does (snap7 uses a new
  sequence; PLCSIM then stops after the first fragment).
- `test/plcsim`: FB `PartnerComm`, DBs `PartnerData` / `PartnerComm_DB`, `add-s7-connection.ps1`
  (TIA Portal Openness: S7 connection to an unspecified partner), `download-hw-sw.ps1` (download
  with the hardware configuration; the TIA Portal MCP loads the software only) and
  `live_partner.js`. Tested on PLCSIM Advanced V8 (S7-1500, FW S4.1): 16 and 1800 bytes both
  ways.
- `s7 info` (S7CommPlus), read only, also in the admin API:
  - `resolve {name}`: the object id (RID) of a DB, FB, FC, OB or technology object by its name,
    asked to the PLC (GetVariablesAddress 0x0590 with the object path
    `ASRoot.PLCProgram.<name>`, as TIA Portal does for technology objects); `{path}` takes any
    object path (`"ASRoot"`, `"ASRoot.PLCProgram"`). `rid` is null when the PLC has no such
    object.
  - `links {rid, aid}`: the objects that a relation of an object points to (GetLink 0x0524), for
    example rid 8, aid 2667: the active alarm instances.
- Rust crate `s7plus`: `Client::resolve_object`, `Client::get_links`, `program_object_path`. The
  request and response layouts were found on PLCSIM Advanced V8 (S7-1500, FW S4.1); the fake PLC
  answers both with the PLCSIM bytes.

## [1.2.2] - 2026-11-12

### Fixed

- `s7 endpoint`, PLC certificate check *trusted certificate or project CA*: the PLC certificate
  itself as trusted certificate (what *Use this certificate* copies) was refused when the CA of
  the TIA project signed it ("is not signed by a trusted certificate"); it worked only for a
  self-signed PLC certificate. Now the exact PLC certificate is accepted without its CA; the
  validity dates and the name are still checked. Risk: none; before, the check refused the
  connection, it did not accept a wrong certificate. Found on PLCSIM Advanced V8 with the
  global security settings of TIA Portal V21 (certificate signed by the project CA).

## [1.2.1] - 2026-11-12

### Fixed

- `s7 endpoint` editor, *Read from the PLC*: when the check of the deployed endpoint refused the
  PLC certificate, the button showed only the error, so a new PLC certificate could not be taken
  over with *Use this certificate*. It now shows the refused certificate with a warning. `s7 info`
  action `certificate` returns it with `refused: true` and `error`; a trusted certificate has
  `refused: false`.
- Rust crate `s7plus`: a refused certificate gives `Error::CertificateRefused { reason, cert }`
  (before: `Error::Certificate`, which now means a wrong check setting only) and
  `Error::refused_certificate()`.

## [1.2.0] - 2026-11-12

### Added

- `s7 endpoint`: check of the PLC certificate (S7CommPlus with TLS), option "PLC certificate":
  *pinned fingerprint* (SHA-256, one or more) or *trusted certificate or project CA* (PEM: the
  chain, the validity and the name, by default the host; option "Certificate name"). The
  default is no check, as before. When the check fails, the endpoint stays disconnected with
  an error that names the reason and the fingerprint it got. A check without TLS, a bad
  fingerprint or a bad PEM stops the endpoint with a configuration error.
- `s7 info`: action `certificate` (S7CommPlus with TLS): subject, issuer, validity, alternative
  names, SHA-256 fingerprint, PEM and the check of the endpoint. The editor of the endpoint reads
  it ("Read from the PLC") and copies the fingerprint and the PEM to the fields. Action `info`:
  new fields `certificateCheck`, `certificateFingerprint`.
- Rust crate `s7plus`: `TlsOptions::verify` (`CertVerify`: `None`, `Fingerprint`, `Trusted`),
  `Error::Certificate` (also for `verify` together with a caller `config`, which has its own
  verifier), `SessionInfo::peer_cert`, a small X.509 reader (`x509::parse`,
  `x509::pem`); the fake PLC can use a certificate signed by a CA (`enable_tls_signed`).
- `test/plcsim/live_cert.js`: the check against PLCSIM. Tested on PLCSIM Advanced V8 (S7-1500,
  FW S4.1): the PLC certificate is self-signed (CN `PLC-1/Communication-1`, IP address in the
  alternative names, CA:FALSE); both checks accept it, a wrong fingerprint or name is refused.

## [1.1.0] - 2026-11-12

### Added

- `s7 watch`: mode "PLC subscription" (S7CommPlus). The node subscribes to the tags with the
  interval as cycle time, and the PLC sends only the values that changed (no read request per
  cycle). `msg.ts` is the PLC time of the notification when the PLC sends one. After a reconnect
  the node subscribes again; when the PLC refuses the subscription (or for S7comm) it reads
  cyclically. Tested on PLCSIM Advanced V8 (FW S4.1) with TLS: first message with all tags, then
  the changes every cycle, and a write is in the next notification. The default stays "read
  cyclically".
- `s7 alarms` node (S7CommPlus): one message per PLC alarm event (`msg.state` coming / going,
  `msg.topic` the alarm id, `msg.payload` times, associated values, priority, class, texts). At
  start, and again after a reconnect, it sends the active alarms (`msg.initial`). The status shows
  the number of active alarms. Example flow "05 - S7CommPlus alarms". Tested on PLCSIM Advanced
  V8 (FW S4.1): a Program_Alarm comes about 20 ms after its signal is written.
- Alarm subscriptions: `AlarmSubOptions::for_plc` selects the layout. S7-1500 FW V4.x accepts the
  TIA Portal layout but sends no alarms with it; it needs the S7CommPlusDriver layout (the S7-1200
  keeps the TIA Portal layout). The `s7 info` action `alarms` also returns `state`, `priority`,
  `alarmClass` and the associated values. The fake PLC sends alarm notifications
  (`FakePlc::raise_alarm`, `FakeModel::alarm_needs_reference`); golden tests decode PLCSIM alarm
  notifications.
- `test/plcsim/`: the PLCSIM Advanced test program (SIMATIC SD documents of the DBs, the alarm FB
  and OB1), how to load it with TIA Portal (Openness MCP), and two manual live-test scripts
  (`s7 watch` push/poll, `s7 alarms`). `npm test` does not use them.

### Fixed

- S7CommPlus: a Bool member of an optimized S7-1500 DB (softdatatype BBool) could not be written
  ("writing BBool is not supported"); reads worked. It is now written like a Bool. Found on
  PLCSIM Advanced V8 (FW S4.1); the fake PLC has a BBool member (`"Data_block_1".BFlag`).
- S7CommPlus subscriptions on S7-1500 FW V4.x: with cycle time 0 the PLC sends only the first
  notification. `SubOptions::cycle_ms` documents it; the fake PLC models it
  (`FakeModel::sub_needs_cycle`), and golden tests decode FW V4.x notifications (change counter,
  8-byte timestamp).
- Addon: a data event with every tag in another order than the tag list now carries the index
  list (before, the values went to the wrong tags; only possible with PLC notifications).

## [1.0.5] - 2026-11-12

### Fixed

- S7CommPlus over TLS: symbol resolution, `dbs` and `browse` failed on S7-1500 FW V4.x
  (tested on PLCSIM Advanced, FW S4.1) with `Explore returned 0xa201be00017eff79`. With an
  integrity id, the Explore request now ends with one more 0 byte after the fill, like
  S7CommPlusDriver. Symbolic reads such as `"DB".member` work over TLS now. The fake PLC
  refuses the old request in TLS sessions, and the TLS test explores the DB list.
- S7CommPlus: a PLC that refuses a session without TLS (PLCSIM Advanced V8 with FW S4.1, or
  "Only allow secure PG/PC and HMI communication") gave "CreateObject returned no session id".
  The session setup now detects the negative return value (`0xa201d6000214fdf8`) and, when
  the PLC supports TLS, says: "the PLC refused the session without TLS … enable TLS on the
  endpoint" (#11). The fake PLC models this PLC, and goes on in plain text when a client
  skips TLS after InitSsl.
- S7CommPlus: `dbs` reported a standard DB as `optimized: true` on S7-1500 FW V4.x (PLCSIM
  Advanced, FW S4.1), and the symbols of that DB got no classic absolute address. This
  firmware sets the optimized-access flag in standard DBs too. A DB with classic elements
  (bit-offset info 0x08) is now a standard DB (#10). The fake PLC can send FW V4.x flags.

## [1.0.4] - 2026-11-08

### Fixed

- Tag addresses out of the S7 address range are now an error. Before, a byte offset past
  2097151 (`DB1.DBB2097152`) wrapped in the 24-bit address field and reached another address
  (`DB1.DBB0`), a timer/counter number past 65535 lost its high bits, and a count past
  32767 (`T5[70000]`) was cut (and a very large one made the connection thread panic). All three
  address syntaxes check the range.

### Added

- Fuzzing: cargo-fuzz targets for S7comm replies (COTP, S7 header, ReadVar data,
  Userdata), tag addresses and values, block information and clock, S7CommPlus PDU
  reassembly, messages, values, objects, notifications, alarm texts and comment blobs
  (`fuzz/`, CI workflow `fuzz.yml`).

### Changed

- GitHub releases start with a summary for users (`.github/release-notes/`), then the
  CHANGELOG section.
- CI: cargo-zigbuild installed from a hash-pinned requirements file; Node-RED 4 tests use the
  locked alias `node-red-4` (no unpinned `npm install`); `node-red-dev` pinned; SECURITY.md
  links the private reporting form.

## [1.0.3] - 2026-11-06

### Changed

- Node-API layer on `napi` 3 / `napi-derive` 3 (native API, no compat mode): results are
  built by `ToNapiValue` types, callbacks are weak thread-safe functions, `status` is
  converted with the `serde-json` feature instead of `JSON.parse`. The JS API and the
  values are the same (byte-identical output against the fake PLCs).
- `aes` 0.9 (`cipher` 0.5) for the S7CommPlus legitimation.

### Added

- Tests: NIST SP 800-38A known-answer test for AES-256-CBC; `test/addon.test.js` for the
  shapes and value conversions at the Node-API boundary.

## [1.0.2] - 2026-11-06

### Security

- Development dependencies only (the published package has no dependencies): npm `overrides`
  for `multer` 2.5.0 (GHSA high), `body-parser` 1.20.9 (high) and the `npm` CLI bundled by
  Node-RED 11.21.2, which brings `undici` 6.29.1 (high and medium). No change to the nodes or
  the binaries.

## [1.0.1] - 2026-11-06

### Changed

- Release built and published by GitHub Actions with npm Trusted Publishing (OIDC) and
  provenance. No code change.

## [1.0.0] - 2026-11-06

First release: the next generation of `@oriolrius/node-red-contrib-s7`.

### Added

- S7comm client in Rust (crate `s7`): S7-200/300/400/1200/1500 and LOGO! 8, Siemens and nodes7
  address syntax, all S7 data types, request optimization with gap spanning, reconnection,
  SZL, clock, block information.
- S7CommPlus client in Rust (crate `s7plus`): S7-1200/1500 symbolic access to optimized DBs and
  PLC tags, absolute area access, TLS 1.3, legitimation (password), browse, alarms.
- Node-API addon (crate `s7napi`): one thread per connection, columnar results, change
  detection on the raw PLC bytes.
- Nodes `s7 endpoint`, `s7 read`, `s7 write`, `s7 watch` (conditional rate) and `s7 info`.
- Admin API `GET /s7rs/status/:id` and `GET /s7rs/call/:id/:action` (read-only actions).
- Prebuilt binaries for Linux x64: glibc ≥ 2.28 and musl (Alpine).
- Example flows, editor help, en-US message catalogs.
- CI: tests on Node-RED 4 and 5, gitleaks, GitGuardian, RustSec and npm audit, CodeQL,
  OpenSSF Scorecard, Dependabot. License LGPL-3.0-only.

[Unreleased]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.2.2...HEAD
[1.2.2]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.2.1...v1.2.2
[1.2.1]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.2.0...v1.2.1
[1.2.0]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.5...v1.1.0
[1.0.5]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.4...v1.0.5
[1.0.4]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.3...v1.0.4
[1.0.3]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/oriolrius/node-red-contrib-s7-rust/releases/tag/v1.0.0
