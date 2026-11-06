# s7napi

Node-API addon (napi-rs, N-API 8) that runs the `s7` and `s7plus` clients inside the Node.js
process. One OS thread per PLC connection, one per subscription. Results are columnar
(`vals`, `errs`, `idx`); `lib/napi-bridge.js` builds the JavaScript objects.

Feature `fake` adds `startFakeS7()` and `startFakeS7Plus()` for the JavaScript tests.

Build with `npm run build` (this host) or `npm run build:release` (glibc 2.28 + musl) from the
repository root. Linux x64 only.
