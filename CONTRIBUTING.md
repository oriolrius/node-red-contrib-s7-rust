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

Node-RED 4 runs from the locked alias `node-red-4`: `S7RS_NODE_RED=node-red-4 npm run test:node`.

Fuzzing (nightly toolchain and `cargo install cargo-fuzz --locked`):

```sh
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run s7_reply -- -max_total_time=60
```

Try your build in a local Node-RED: `npm run build`, then in `~/.node-red`:
`npm install /path/to/node-red-contrib-s7-rust` and restart Node-RED.

## Pull requests

1. Fork the repository and create a branch from `main`.
2. Make the change with its tests, run `npm test` and `npm run lint`, and add a line under
   `[Unreleased]` in `CHANGELOG.md`.
3. Open a pull request against `main` and fill in the checklist of the template.
4. `main` takes changes only through pull requests. The required checks (CI, CodeQL, secret
   scanning, dependency audit and review) must pass and the branch must be up to date with
   `main`. Pull requests are squash-merged.
5. Each pull request needs one approval from a code owner who is not its author
   (`.github/CODEOWNERS`), given after the last push.

AI models: development and bug fixes are made with Claude Opus 5.5; the account `i40sys`
reviews pull requests with the Astra model (a different model from the one that wrote the
change). Each review says which model made it. Maintainers still own every merge.

## Rules

- Tests run against the in-process fake PLCs (cargo features `fakecpu` and `fake`). Add a test
  for each fix or feature. Code that parses bytes from the PLC gets a fuzz target in `fuzz/`.
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
