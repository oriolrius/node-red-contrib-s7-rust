# AGENTS.md

Instructions for coding agents in this repository.

- Product: Node-RED nodes for Siemens S7 (`node-red-contrib-s7-rust`). Rust crates in
  `crates/`, Node-RED package at the root (`nodes/`, `lib/`, `examples/`).
- Linux x64 only. Do not add Windows, macOS or ARM code paths or build targets.
- Node types use the prefix `s7rs-`; admin routes `/s7rs/...`; env var `S7RS_NAPI`.
- Keep `version` the same in `package.json` and `[workspace.package]` of `Cargo.toml`
  (`scripts/check-pack.js` checks it).
- Test with `npm test` (fake PLCs). Never write to a real PLC from tests; never call CPU
  STOP/START or block upload from the addon.
- Editor strings and help live in `nodes/locales/en-US/`. Runtime messages use `text(RED, key)`
  from `lib/common.js`.
- Release: see `docs/RELEASING.md`. Do not publish to npm or push tags without the owner's request.
