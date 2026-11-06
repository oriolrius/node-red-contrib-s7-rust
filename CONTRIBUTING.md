# Contributing

Thank you for your help. Bug reports, PLC compatibility reports and pull requests are welcome.

## Set up

Linux x64 with Rust (stable) and Node.js ≥ 18:

```sh
git clone https://github.com/oriolrius/node-red-contrib-s7-rust.git
cd node-red-contrib-s7-rust
npm install
npm test        # Rust unit tests + Node-RED tests against fake PLCs
npm run lint    # rustfmt + clippy (warnings are errors)
```

Try your build in a local Node-RED: `npm run build`, then in `~/.node-red`:
`npm install /path/to/node-red-contrib-s7-rust` and restart Node-RED.

## Rules

- Tests run against the in-process fake PLCs (cargo features `fakecpu` and `fake`). Add a test
  for each fix or feature.
- Never run tests that write against a production PLC. Use a simulator.
- The addon must not expose CPU STOP/START or block upload.
- Keep the editor help (`nodes/locales/en-US/*.html`) and the README in step with the code.
- Commit messages: [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `docs:`, `test:`, `ci:`, `chore:`).
- Add an entry under `[Unreleased]` in `CHANGELOG.md`.

## PLC reports

Open an issue with the CPU (order number and firmware: `s7 info` → `identify` or `info`), the
protocol, the addresses, and a log with the endpoint log level `debug` (or `trace` for frames).
Remove IP addresses and passwords that you do not want to publish.
