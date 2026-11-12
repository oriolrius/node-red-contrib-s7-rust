# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.4...HEAD
[1.0.4]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.3...v1.0.4
[1.0.3]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/oriolrius/node-red-contrib-s7-rust/releases/tag/v1.0.0
