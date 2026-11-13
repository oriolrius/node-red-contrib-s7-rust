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
- Every release has a summary for GitHub readers in `.github/release-notes/vX.Y.Z.md`, not only
  the CHANGELOG entry. The `Release` workflow puts it first in the GitHub release, then the
  CHANGELOG section (the details) and the compare link (`scripts/release-notes.sh`), and stops
  when it is missing. Write it for a user who did not follow the work:
  - one or two sentences: what the release does and who should upgrade;
  - `## Highlights`: the changes that matter to users, in plain words (what was wrong, what
    happens now), with the risk for fixes (for example "a write could reach another address");
  - `## Upgrade notes`: actions or behaviour changes (new errors, removed options), or "None";
  - no internal detail that only matters to maintainers (that stays in the CHANGELOG).
- `main` takes changes only through pull requests with green required checks (squash merge)
  and one approval of a code owner who is not the author, after the last push.
- Development and bug fixes: Claude Opus 5.5 (as `oriolrius`). Review: the account `i40sys`
  with the Astra model. The authoring agent never approves a pull request, not with `i40sys`
  and not with any other account.

## Rust toolchain

- CI and releases use the Rust version in `rust-toolchain.toml` (also in `ci.yml` and
  `release.yml`), not `stable`: a new stable release adds clippy lints and fails the required
  check of every open pull request for code they do not touch (Rust 1.100, 2026-11-12).
- Move to a new Rust version in its own pull request (the three files together, plus the lint
  fixes). The weekly `Rust next stable` workflow (not required) runs the latest stable and shows
  when that pull request is due.

## Waiting on pull requests

- A watch on a pull request wakes the agent only on changes. After each wake, and before ending
  a turn that waits on checks, run `gh pr checks <n> --required` and look for checks that do not
  move.
- A required check that stays `pending` for more than 15 minutes is stuck when its workflow run
  is already completed (`gh run view <run> --json status,jobs`): GitHub sometimes keeps a job
  `in_progress` after the run ends (seen with "Dependency review"). Re-run it at once
  (`gh run rerun <run>`); do not wait for a notification that will not come.
- A required check that fails for code outside the pull request (a new lint, a broken main) is
  not a reason to wait or to fix it inside each pull request: fix it on `main` in its own small
  pull request first, then rebase the open pull requests on `main`.
- Open pull requests that edit the same files (CHANGELOG, README) merge one at a time; rebase
  the next one on `main` after each merge.

