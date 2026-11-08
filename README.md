# node-red-contrib-s7-rust

[![npm](https://img.shields.io/npm/v/node-red-contrib-s7-rust.svg)](https://www.npmjs.com/package/node-red-contrib-s7-rust)
[![CI](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/ci.yml)
[![Security](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/security.yml/badge.svg)](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/security.yml)
[![CodeQL](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/codeql.yml/badge.svg)](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/oriolrius/node-red-contrib-s7-rust/badge)](https://scorecard.dev/viewer/?uri=github.com/oriolrius/node-red-contrib-s7-rust)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/16456/badge)](https://www.bestpractices.dev/projects/16456)
[![Fuzz](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/fuzz.yml/badge.svg)](https://github.com/oriolrius/node-red-contrib-s7-rust/actions/workflows/fuzz.yml)
[![License](https://img.shields.io/badge/license-LGPL--3.0-blue.svg)](LICENSE)

Siemens S7 nodes for [Node-RED](https://nodered.org). The two S7 protocols are written in Rust and
run **inside** the Node-RED process as a Node-API addon:

- **S7comm** (classic, ISO-on-TCP port 102): S7-200, S7-300, S7-400, S7-1200, S7-1500, LOGO! 8.
  Absolute addresses, Siemens and nodes7 syntax.
- **S7CommPlus** (S7-1200/1500): **symbolic access** to optimized DBs and PLC tags
  (`"DB_Data".speed`, `Motor_On`), absolute I/Q/M/DB addresses, optional TLS 1.3 and password.

There is no external process, no compiler at install time and no runtime npm dependency. The
package ships prebuilt binaries for **Linux x64** (glibc and musl/Alpine).

This is the next generation of
[@oriolrius/node-red-contrib-s7](https://www.npmjs.com/package/@oriolrius/node-red-contrib-s7).
See [Migration](#migration-from-oriolriusnode-red-contrib-s7).

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [Nodes](#nodes)
- [Protocol coverage](#protocol-coverage)
- [Prepare the PLC](#prepare-the-plc)
- [Addresses](#addresses)
- [Values](#values)
- [Examples](#examples)
- [Safety](#safety)
- [Performance](#performance)
- [Limits](#limits)
- [Migration from @oriolrius/node-red-contrib-s7](#migration-from-oriolriusnode-red-contrib-s7)
- [Build from source](#build-from-source)
- [License](#license)

## Requirements

| | |
|---|---|
| OS / CPU | Linux x86-64 only: glibc ≥ 2.28 (Debian 10+, Ubuntu 20.04+, RHEL 8+) or musl (Alpine, the official `nodered/node-red` Docker image) |
| Node.js | ≥ 18 (Node-RED 5 needs ≥ 22.9) |
| Node-RED | ≥ 4.0 (tested on 4 and 5) |

Windows, macOS and ARM are not supported. npm refuses to install the package on them
(`os` and `cpu` fields).

## Install

In the Node-RED editor: **Menu → Manage palette → Install**, search
`node-red-contrib-s7-rust`.

Or on the command line, in your Node-RED user directory (usually `~/.node-red`):

```sh
npm install node-red-contrib-s7-rust
```

Then restart Node-RED. The nodes are in the **network** category of the palette.

## Nodes

| Node | Function |
|---|---|
| `s7 endpoint` (config) | One PLC connection: protocol, host, port. S7comm: CPU family, rack/slot or TSAPs, PDU, gap, byte order. S7CommPlus: remote TSAP, TLS, user, password (stored as a credential). |
| `s7 read` | Reads tags when a message arrives. Output: one value, `{tag: value}` or an array. |
| `s7 write` | Writes the tag of the node, the tag in `msg.topic`, or `msg.payload = {tag: value, …}` in one request. |
| `s7 watch` | A Rust thread reads the tags at an interval and sends only the changes. Optional conditional rate ("Fast while" a tag is true). |
| `s7 info` | Diagnostics. Both protocols: `status`, `cpuState`. S7comm: `identify`, `szl`, `szlList`, `clock`, `setClock`, `syncClock`, `blocks`, `listBlocks`, `blockInfo`. S7CommPlus: `info`, `dbs`, `browse`, `alarms`, `limits`. |

Every node has full help in the editor sidebar. Bad tags never stop the other tags: they give
`null` and an entry in `msg.s7.errors`, or `msg.quality = "bad"` in a watch.

The endpoint also adds two admin API routes for tools and the editor (Node-RED admin
authentication applies, permission `s7rs-endpoint.read`):

- `GET /s7rs/status/:endpointId`: connection state, request count, errors, latency percentiles.
- `GET /s7rs/call/:endpointId/:action`: a read-only `s7 info` action (for example `browse`).

## Protocol coverage

This map shows each protocol function and where it is available. It is strict: a function has
✅ only when a node uses it. The Rust tests run every ✅ and 🔒 function against the fake PLCs.

| Mark | Meaning |
|---|---|
| ✅ | Available in the nodes |
| 🔒 | In the Rust crate only. The nodes do not expose it (safety, or not needed) |
| ❌ | Not implemented |

### S7comm (crate `s7`)

| Function | Protocol detail | Status | Node / action |
|---|---|---|---|
| ISO-on-TCP connection | TPKT (RFC 1006) + COTP CR/CC, TCP port 102 | ✅ | `s7 endpoint` |
| Rack/slot addressing | remote TSAP `[link type, rack<<5 \| slot]`, link type PG/OP/S7 basic | ✅ | `s7 endpoint` |
| Explicit TSAPs | local and remote TSAP (S7-200, LOGO! 8) | ✅ | `s7 endpoint` |
| Setup Communication | job 0xF0, PDU size negotiation | ✅ | `s7 endpoint` |
| Read Var | job 0x04, multi-item, split at the PDU size, near tags merged ("gap") | ✅ | `s7 read`, `s7 watch` |
| Write Var | job 0x05, multi-item, split at the PDU size | ✅ | `s7 write` |
| Memory areas | I (0x81), Q (0x82), M (0x83), DB (0x84), peripheral PI/PQ (0x80), T (0x1D), C (0x1C), V (= DB1) | ✅ | all |
| Read SZL | userdata group 4 (CPU functions), multi-part replies | ✅ | `s7 info`: `szl`, `szlList`, `identify`, `cpuState` |
| Read clock | userdata group 7 (time functions) | ✅ | `s7 info`: `clock` |
| Set clock | userdata group 7 (time functions) | ✅ | `s7 info`: `setClock`, `syncClock` (refused by the admin API) |
| List blocks | userdata group 3 (block functions): count per type, list of one type | ✅ | `s7 info`: `blocks`, `listBlocks` |
| Block info | userdata group 3 (block functions) | ✅ | `s7 info`: `blockInfo` |
| Block upload | jobs 0x1D, 0x1E, 0x1F | 🔒 | — |
| PLC STOP | job 0x29, PI service `P_PROGRAM` | 🔒 | — |
| PLC hot / cold start | job 0x28, PI service `P_PROGRAM` | 🔒 | — |
| Block download, delete | jobs 0x1A, 0x1B, 0x1C, PI service `_DELE` | ❌ | — |
| Compress, copy RAM to ROM | PI services `_GARB`, `_MODU` | ❌ | — |
| Cyclic data | userdata group 2 | ❌ | — (`s7 watch` uses polling) |
| Alarms and messages | userdata group 4 alarm subfunctions, NOTIFY, ALARM_S/ALARM_8 | ❌ | — |
| Protection level password | userdata group 5 (security) | ❌ | — |
| Programmer commands, force | userdata group 1 | ❌ | — |
| BSEND/BRCV, USEND/URCV | userdata group 6 | ❌ | — |
| S7 routing to other subnets | COTP with routing parameters | ❌ | — |

### S7CommPlus (crate `s7plus`)

| Function | Protocol detail | Status | Node / action |
|---|---|---|---|
| Connection | TPKT + COTP, remote TSAP `SIMATIC-ROOT-HMI`, protocol version 1 for the session creation, version 2 after it | ✅ | `s7 endpoint` |
| Session | CreateObject 0x04CA (ClientSession), session setup with SetMultiVariables, DeleteObject 0x04D4 at close | ✅ | `s7 endpoint` |
| TLS 1.3 | InitSsl 0x05B3, TLS inside COTP | ✅ | `s7 endpoint` (option "TLS") |
| PLC certificate verification | — | ❌ | — (TLS protects against passive capture only) |
| Legitimation (access level password) | challenge from GetVarSubStreamed 0x0586, legacy and new response | ✅ | `s7 endpoint` (password) |
| Read | GetMultiVariables 0x054C, symbolic and absolute addresses | ✅ | `s7 read`, `s7 watch` |
| Write | SetMultiVariables 0x0542, symbolic and absolute addresses | ✅ | `s7 write` |
| Absolute areas | I, Q, M, DB, timers and counters | ✅ | all |
| Type information, DB list, browse | Explore 0x04BB, type info cache | ✅ | `s7 info`: `dbs`, `browse`; symbol resolution in all nodes |
| Symbol to absolute address | from the type information (non-optimized DBs) | ✅ | used inside the nodes |
| CPU operating state | read of the PLC program object | ✅ | `s7 info`: `cpuState` |
| Communication limits | read of the PLC limit attributes | ✅ | `s7 info`: `limits`, `info` |
| Active alarms | alarm objects and text libraries | ✅ | `s7 info`: `alarms` |
| Alarm subscription | CreateObject subscription, notifications | 🔒 | — |
| Variable subscription | CreateObject subscription, notifications | 🔒 | — (`s7 watch` uses polling) |
| Set CPU operating state (RUN/STOP) | SetVariable 0x04F2 | 🔒 | — |
| Block upload / download, program changes | BeginSequence 0x0556, EndSequence 0x0560, Invoke 0x056B | ❌ | — |
| Links | AddLink 0x0506, RemoveLink 0x051A, GetLink 0x0524 | ❌ | — |
| GetVariablesAddress | 0x0590 | ❌ | — |

## Prepare the PLC

**S7comm on S7-1200/1500**

1. In TIA Portal, CPU properties → Protection & Security: enable **Permit access with PUT/GET
   communication from remote partner**.
2. In each DB you read: Attributes → clear **Optimized block access**. S7comm uses absolute
   addresses (`DB1.DBW2`).
3. Rack 0, slot 1 (the defaults for these families).

**S7comm on S7-300/400**: rack 0, slot 2 (default). **LOGO! 8**: select *LOGO! 8* or
*LOGO! 8 (TSAP)* and set up a server connection in LOGO!Soft Comfort.

**S7CommPlus (S7-1200/1500)**: no PUT/GET, optimized DBs are fine. Use symbols. If the CPU has an
access level password, set it in the endpoint. TLS needs S7-1500 FW ≥ 2.9 or S7-1200 FW ≥ 4.5.

## Addresses

One address per line in the node, or in `msg.tags` / `msg.topic`.

**Siemens syntax** (S7comm and S7CommPlus)

```text
DB1.DBX0.3  DB1.DBB0  DB1.DBW2  DB1.DBD4      also DB1.X0.3, DB1.B0, DB1.W2, DB1.D4
M0.0 MB0 MW0 MD0    I0.0 IB0 IW0 ID0 (E = I)   Q0.0 QB0 QW0 QD0 (A = Q)
PIB0 PIW0 PQW0 (peripheral)   T0   C0 (Z = C)
V10.3 VB10 VW20 VD30          LOGO! 8 / S7-200 V memory (= DB1)
```

Optional suffixes: `[n]` for an array of n elements, and `:type`:

| Group | Types |
|---|---|
| Bit and integer | `bool`, `byte`, `char`, `word`, `int`, `dword`, `dint`, `lword`, `lint`, `bcd`, `lbcd` |
| Real | `real`, `lreal` |
| Text | `string[N]`, `wstring[N]`, `wchar` |
| Date and time | `dt`, `dtz`, `dtl`, `dtlz`, `date`, `tod`, `time`, `ltime`, `ltod`, `ldt` |

Examples: `DB1.DBW2:int`, `MD12:real`, `MW120[4]`, `DB1.DBB10:string[20]`, `DB2.DBB0:dtl`.
Without a type, the size gives one: `X` bool, `B` byte, `W` word, `D` dword.

**nodes7 syntax** (S7comm and S7CommPlus): `DB1,REAL4`, `DB5,X0.1`, `DB2,S7.10`, `MI14`, `MR38`,
`DB1,INT0.5` (5 INTs). Types with an `R` prefix (`RWORD`, `RDI`) are little-endian.

**Symbols** (S7CommPlus only): `"DB_Data".speed`, `"DB_Data".values[2]`, `"DB_Data".mat[1,2]`,
`"DB_Data".st.x`, PLC tag names (`Motor_On`), or any name with the `sym:` prefix. Use
`s7 info` → `browse` to list all symbols. Symbols and absolute addresses mix in one request.

## Values

| PLC type | JavaScript |
|---|---|
| BOOL | boolean |
| integers up to 32 bit, REAL, LREAL | number |
| LINT, LWORD | number, or string outside ±2^53 |
| STRING, WSTRING, CHAR | string |
| DT, DTL, DATE, LDT and the `z` (UTC) variants | ISO 8601 string |
| TIME, LTIME, TOD (S7comm) | number: TIME in ms, LTIME in ns, TOD in ms of the day |
| arrays (`[n]`) | array |

For writes, the node also accepts `Date`, `BigInt`, `Buffer` (for byte arrays) and numeric strings.

## Examples

Import them in the editor: **Menu → Import → Examples → node-red-contrib-s7-rust**.

| Example | Shows |
|---|---|
| 01 - Read and write (S7comm) | cyclic read to an object, single and multi-tag writes |
| 02 - Watch with conditional rate | change-only watch, faster while a tag is true |
| 03 - S7CommPlus symbolic access | symbols and absolute addresses in one read, browse |
| 04 - PLC diagnostics | `status`, `cpuState`, `identify` |

All examples use the address `192.168.0.1`. Change it in the endpoint.

## Safety

- The nodes **cannot** stop or start the CPU and **cannot** upload blocks. The Rust crates have
  these functions, but the addon does not expose them.
- `setClock` and `syncClock` change the PLC clock. The admin API refuses them.
- `s7 write` writes to the PLC at once. Test on a simulator (for example PLCSIM Advanced) before
  you write to a machine.

## Performance

The protocol work (frame building, request optimization, value decoding, change detection)
is in Rust. Node-RED gets ready-made JavaScript values, and a watch sends nothing when nothing
changed. Reads of tags that are near each other go in one request ("gap" setting).

Measured in-process against the fake CPU of the test suite (p50, Linux x64):

| Operation | 1 tag | 1 000 tags | 10 000 tags | 100 000 tags |
|---|---|---|---|---|
| read | 0.12 ms | 0.70 ms | 5.8 ms | 64 ms |

On a real network the PLC response time is the limit (for example ~2 ms per S7comm read on an
S7-1200 on the same LAN).

## Limits

- The addon runs in the Node-RED process. A fatal error in native code stops Node-RED. The code
  is safe Rust; the Node-API layer is the only FFI.
- S7CommPlus watch uses polling, not PLC subscriptions.
- S7CommPlus: the PLC certificate is not verified (TLS protects against passive capture only).

## Migration from @oriolrius/node-red-contrib-s7

The two packages can be installed together: the node types are different.

| @oriolrius/node-red-contrib-s7 | This package |
|---|---|
| `s7 endpoint` | `s7 endpoint` (type `s7rs-endpoint`) |
| `s7 in`, mode "Single variable" | `s7 watch` with one tag, or `s7 read` |
| `s7 in`, mode "All variables, one per message" | `s7 watch`, output "one message per tag" |
| `s7 in`, mode "All variables" | `s7 watch`, output "one message with all tags" |
| "Emit only when value changes (diff)" | `s7 watch`, "send only values that changed" (on by default) |
| `s7 out` | `s7 write` |
| `s7 control` | no direct equivalent: the watch interval is set in the node, the tags in the node or in `msg.tags` of `s7 read` |
| variable table (`name` → address) | addresses directly in the nodes; use `msg.topic` / object keys as names |

Addresses in nodes7 syntax (`DB1,REAL4`) work without change.

## Build from source

You need Rust (stable), Node.js ≥ 18 and, for the release binaries, `cargo-zigbuild`
(`pip install cargo-zigbuild`, or `uv`; the script uses `uvx` when it is not on `PATH`).

```sh
npm install                 # dev dependencies (Node-RED and the test helper)
npm run build               # addon for this host -> bin/linux-x64-<gnu|musl>/s7napi.node
npm test                    # Rust unit tests + Node-RED tests against fake PLCs
npm run build:release       # both published binaries (glibc 2.28 and musl)
npm run lint                # rustfmt + clippy
npm run validate            # node-red-dev scorecard checks
```

Repository layout:

```text
crates/s7        S7comm client (Rust)
crates/s7plus    S7CommPlus client (Rust)
crates/s7napi    Node-API addon
lib/             JavaScript bridge to the addon, shared helpers
nodes/           Node-RED nodes (.js runtime, .html editor), locales/ (labels, help), icons/
examples/        example flows for the Node-RED import menu
test/            Node-RED tests (node-red-node-test-helper)
scripts/         build and packaging checks
docs/            release process
```

`S7RS_NAPI=/path/to/s7napi.node` makes the nodes load another addon build.

## License

[GNU Lesser General Public License v3.0 only](LICENSE) (`LGPL-3.0-only`). The LGPL adds
permissions to the [GNU GPL v3.0](COPYING). See [NOTICE](NOTICE).

You can use the package in closed-source flows and applications. If you distribute a changed
version of the Rust crates or of the addon, you must publish the source code of the changes
under the same license.

Siemens, SIMATIC, S7-1200, S7-1500, LOGO! and TIA Portal are trademarks of Siemens AG. This
project is not affiliated with or endorsed by Siemens AG.
