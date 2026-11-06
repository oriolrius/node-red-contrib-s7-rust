# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/oriolrius/node-red-contrib-s7-rust/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/oriolrius/node-red-contrib-s7-rust/releases/tag/v1.0.0
