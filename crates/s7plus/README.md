# s7plus

S7CommPlus client for S7-1200/1500. Std threads, no async runtime, no global state.

- Session setup, TLS 1.3 (rustls + ring), legitimation (legacy and new password methods).
- Symbolic access: type information, symbol resolution (`"DB".a.b[1,2]`, PLC tags), browse.
- Absolute area access (I/Q/M/DB) in the same requests as symbols.
- Data block list, system limits, CPU state, alarms, subscriptions.
- Feature `fake`: an in-process fake S7-1200 for tests.

```sh
cargo test -p s7plus --features fake
```

Used by the Node-API addon `s7napi`. Not published to crates.io.
