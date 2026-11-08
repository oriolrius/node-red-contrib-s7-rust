# Releasing

The package goes to three places: GitHub (source, releases), npm (the package) and the
Node-RED Flow Library (flows.nodered.org, the palette manager catalogue).

## One-time setup

1. **GitHub**: create the public repository `oriolrius/node-red-contrib-s7-rust` and push `main`.
   - Settings → Environments: create the environment `npm` (optional: required reviewers).
   - Settings → Code security: enable private vulnerability reporting (see `SECURITY.md`),
     Dependabot alerts and security updates, secret scanning and push protection.
   - Settings → Secrets → Actions: add `GITGUARDIAN_API_KEY` (GitGuardian API key, scope `scan`).
     Without it the GitGuardian job writes a warning and skips the scan.
2. **npm Trusted Publishing** (no token in GitHub):
   - npmjs.com → package `node-red-contrib-s7-rust` → Settings → Trusted Publisher →
     GitHub Actions: owner `oriolrius`, repository `node-red-contrib-s7-rust`, workflow
     `release.yml`, environment `npm`.
   - npm configures a Trusted Publisher only for a package that exists. For the first version,
     publish once from a local checkout (`npm login`, then the steps in *Manual publish*), then
     configure the Trusted Publisher and use tags for all later versions.
3. **Flow Library**: after the first npm publish, open <https://flows.nodered.org/add/node>
   (sign in with GitHub) and submit the package name. The library does not index npm by itself.

## CI workflows

| Workflow | Runs | Checks |
|---|---|---|
| `ci.yml` | push, pull request | rustfmt, clippy, Rust tests, Node-RED tests (Node.js 20/22/24, Node-RED 4/5), release binaries, smoke tests in glibc and musl containers, `npm pack`, scorecard |
| `security.yml` | push, pull request, weekly | gitleaks, GitGuardian (`ggshield`), `cargo audit` (RustSec), `npm audit` of runtime dependencies, dependency review of pull requests |
| `codeql.yml` | push, pull request, weekly | CodeQL for Rust, JavaScript and the workflows |
| `scorecard.yml` | push, weekly | OpenSSF Scorecard (public score, code scanning alerts) |
| `fuzz.yml` | push, pull request (parser changes), weekly | cargo-fuzz on the PLC reply and tag address parsers: 60 s per target, 10 min weekly |
| `release.yml` | tag `vX.Y.Z` | tests, binaries, npm publish with provenance, GitHub release |

Dependabot (`.github/dependabot.yml`) opens weekly updates for Cargo and npm, and monthly
updates for the pinned GitHub Actions (the workflows pin each action to a commit SHA) and for
`.github/requirements/zigbuild.txt` (cargo-zigbuild, pinned by hash: regenerate it with
`uv pip compile --generate-hashes --python-version 3.12 .github/requirements/zigbuild.in -o .github/requirements/zigbuild.txt`).

Rulesets: `main` takes changes only through pull requests (squash merge, one approval of a
code owner other than the author after the last push, review threads resolved) whose required checks are green and up to date (CI, CodeQL, secrets, audit,
dependency review); `main` and the `v*` tags cannot be deleted or force-pushed; release tags
cannot move.

## Each release

1. Update `CHANGELOG.md`: move `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`, fix the links.
   Write the release summary `.github/release-notes/vX.Y.Z.md` (see AGENTS.md). The GitHub
   release shows the summary first, then the CHANGELOG section and the compare link. Preview:
   `sh scripts/release-notes.sh X.Y.Z`. The `Release` workflow stops before it publishes when
   the summary or the CHANGELOG section is missing.
2. Set the same version in `package.json` and in `[workspace.package]` of `Cargo.toml`, then
   `cargo update -w` (updates `Cargo.lock`).
3. Check locally:
   ```sh
   npm test && npm run lint
   npm run build:release && npm pack --dry-run   # prepack checks both binaries and the versions
   npm run validate                               # scorecard (node-red-dev)
   ```
4. `main` takes changes only through pull requests with green CI (squash merge). Open a PR
   `chore(release): X.Y.Z`, merge it, then tag the merge commit and push the tag:
   ```sh
   git switch main && git pull --ff-only
   git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z
   ```
   The `Release` workflow tests, builds the binaries, smoke-tests them in glibc and musl
   containers, publishes to npm with provenance and creates the GitHub release.
5. Flow Library: on the package page, use **request refresh** (visible when signed in), or
   submit the name again at <https://flows.nodered.org/add/node>. Check the scorecard there.

## Manual publish (first version only)

```sh
npm ci
npm run build:release
npm publish --access public --provenance=false   # provenance needs CI (OIDC)
```

## Scorecard checklist

The Flow Library scorecard checks these points. The package passes all of them except P04 (a warning):

| Check | Where |
|---|---|
| License | `license: LGPL-3.0-only`, `LICENSE` (LGPL-3.0) and `COPYING` (GPL-3.0) |
| Bugs URL | `bugs.url` in `package.json` |
| Package name follows the guidelines | `node-red-contrib-s7-rust`: valid. Warning P04: the Node-RED guidelines ask new packages for a scoped name. It is a warning only, not a failure. |
| Node-RED keyword | `keywords` contains `node-red` |
| Supported Node-RED version | `node-red.version: >=4.0.0` |
| Node.js version | `engines.node: >=18` |
| Unique package name and node types | node types `s7rs-*` |
| Nodes have examples | `examples/*.json` use every non-config node |
| Dependencies | no runtime dependencies |
