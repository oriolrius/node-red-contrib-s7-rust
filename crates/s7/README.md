# s7

S7comm client (classic Siemens S7 over ISO-on-TCP, port 102): S7-200/300/400/1200/1500 and
LOGO! 8. Std threads, no async runtime, no global state, no `unsafe`.

- Addresses: Siemens (`DB1.DBW2:int`, `MD12:real`, `M0.1`, `MW120[4]`) and nodes7 (`DB1,REAL4`).
- All S7 data types, little-endian option, arrays, strings.
- Request optimization (gap spanning, PDU-sized chunks), reconnect and retry.
- SZL, CPU state, module identification, clock, block lists and block info.
- `Endpoint`: tag cache, JSON-friendly values, subscriptions with change detection.
- Feature `fakecpu`: an in-process fake CPU for tests.

```sh
cargo test -p s7
```

Used by the Node-API addon `s7napi`. Not published to crates.io.
