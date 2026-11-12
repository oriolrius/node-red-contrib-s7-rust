'use strict';
// The S7 clients in Rust, inside the Node process (Node-API addon s7napi, source in
// crates/): classic S7comm (crate s7) and S7CommPlus (crate s7plus). No child
// process, no IPC, no JSON on the hot path. Linux x64 only (glibc or musl).
//
// API (Promise based, events 'status', 'data', 'log'):
//   open(params) close(conn) read(conn, tags) write(conn, [{tag, value}])
//   subscribe(params) subscribeAlarms(params) unsubscribe(sub) status(conn) call(conn, action, args) stop()
// acquire()/release(): one shared instance per Node-RED runtime.

const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const path = require('node:path');

function isMusl() {
  const r = process.report && process.report.getReport && process.report.getReport();
  return !(r && r.header && r.header.glibcVersionRuntime);
}

// addonPath: S7RS_NAPI (tests, custom builds) or the prebuilt binary of this host.
function addonPath() {
  if (process.env.S7RS_NAPI) return process.env.S7RS_NAPI;
  if (process.platform !== 'linux' || process.arch !== 'x64') {
    throw new Error(`node-red-contrib-s7-rust supports Linux x64 only (this host: ${process.platform}-${process.arch})`);
  }
  const dir = `linux-x64-${isMusl() ? 'musl' : 'gnu'}`;
  const p = path.join(__dirname, '..', 'bin', dir, 's7napi.node');
  if (!fs.existsSync(p)) throw new Error(`s7napi addon not found (${p}); run "npm run build" or set S7RS_NAPI`);
  return p;
}

let addon = null;
function loadAddon() {
  if (!addon) addon = require(addonPath());
  return addon;
}

// rows builds the {tag, value, quality, error?} objects from the addon's columns.
// Creating them here (V8, one shape) is much cheaper than one Node-API call per
// property in Rust.
function rows(tags, vals, errs, idx) {
  const n = vals.length;
  const out = new Array(n);
  for (let k = 0; k < n; k++) out[k] = { tag: idx ? tags[idx[k]] : tags[k], value: vals[k], quality: 'good' };
  for (let j = 0; j < errs.length; j++) {
    const o = out[errs[j][0]];
    o.quality = 'bad';
    o.error = errs[j][1];
  }
  return out;
}

class NapiBridge extends EventEmitter {
  constructor() {
    super();
    this.setMaxListeners(0);
    this.native = null;
    this.subTags = new Map(); // sub -> tags (the addon sends columns, not tag names)
  }

  start() {
    if (this.native) return;
    const { S7Native } = loadAddon();
    this.native = new S7Native(
      (conn, state, detail) => this.emit('status', { event: 'status', conn, state, detail }),
      (ev) => {
        if (ev.alarms !== undefined) {
          if (!this.subTags.has(ev.sub)) return; // unsubscribed meanwhile
          const a = JSON.parse(ev.alarms);
          this.emit('alarms', { event: 'alarms', sub: ev.sub, conn: ev.conn, alarms: a.alarms, snapshot: a.snapshot, ts: ev.ts });
          return;
        }
        const tags = this.subTags.get(ev.sub);
        if (!tags) return; // unsubscribed meanwhile
        this.emit('data', { event: 'data', sub: ev.sub, conn: ev.conn, values: rows(tags, ev.vals, ev.errs, ev.idx), ts: ev.ts, ms: ev.ms });
      },
      (conn, level, text) => this.emit('log', level, `${conn}: ${text}`),
    );
  }

  pid() {
    return undefined; // in-process: its CPU is part of the Node process
  }

  async open(p) {
    this.start();
    return this.native.open(p);
  }

  async close(conn) {
    if (this.native) this.native.close(conn);
  }

  async read(conn, tags) {
    const r = await this.native.read(conn, tags);
    return { values: rows(tags, r.vals, r.errs), nativeMs: r.nativeMs };
  }

  async write(conn, items) {
    const n = items.length;
    const tags = new Array(n);
    const vals = new Array(n);
    for (let i = 0; i < n; i++) {
      tags[i] = items[i].tag;
      vals[i] = items[i].value;
    }
    const r = await this.native.write(conn, tags, vals);
    const results = new Array(n);
    for (let i = 0; i < n; i++) results[i] = { tag: tags[i] };
    for (let j = 0; j < r.errs.length; j++) results[r.errs[j][0]].error = r.errs[j][1];
    return { results, nativeMs: r.nativeMs };
  }

  // subscribe({conn, sub, tags, intervalMs, onChange, condTag, condIntervalMs, push}) -> {invalid}
  async subscribe(p) {
    this.start();
    this.subTags.set(p.sub, p.tags);
    return { invalid: await this.native.subscribe(p) };
  }

  // subscribeAlarms({conn, sub, initial, layout}): 'alarms' events {sub, conn, alarms, snapshot, ts};
  // snapshot: a new PLC subscription (start or reconnect), the receiver forgets the alarms it knew
  async subscribeAlarms(p) {
    this.start();
    this.subTags.set(p.sub, []);
    this.native.subscribeAlarms(p);
  }

  async unsubscribe(sub) {
    this.subTags.delete(sub);
    if (this.native) this.native.unsubscribe(sub);
  }

  async status(conn) {
    return this.native.status(conn);
  }

  // call(conn, action, args) -> parsed JSON. Actions:
  //   s7:     identify, cpuState, szl {id, index}, szlList, clock, setClock {time?},
  //           syncClock {thresholdMs}, blocks, listBlocks {type}, blockInfo {type, number}
  //   s7plus: info, cpuState, limits, dbs, browse, alarms {all}
  async call(conn, action, args) {
    const text = await this.native.call(conn, action, args === undefined ? undefined : JSON.stringify(args));
    return JSON.parse(text);
  }

  stop() {
    if (this.native) this.native.stop();
    this.subTags.clear();
  }
}

// ---- one shared instance per runtime -----------------------------------------

const GRACE_MS = 3000; // a deploy closes and recreates the nodes: keep the instance
let shared = null;
let refs = 0;
let stopTimer = null;

function acquire() {
  clearTimeout(stopTimer);
  stopTimer = null;
  if (!shared) {
    shared = new NapiBridge();
    shared.start();
  }
  refs++;
  return shared;
}

function release() {
  if (refs > 0) refs--;
  if (refs === 0 && shared && !stopTimer) {
    stopTimer = setTimeout(() => {
      stopTimer = null;
      if (refs === 0 && shared) {
        shared.stop();
        shared = null;
      }
    }, GRACE_MS);
    stopTimer.unref();
  }
}

// stopAll stops the shared instance at once (tests; in Node-RED, release does it).
function stopAll() {
  clearTimeout(stopTimer);
  stopTimer = null;
  refs = 0;
  if (shared) {
    shared.stop();
    shared = null;
  }
}

module.exports = { NapiBridge, acquire, release, stopAll, addonPath, loadAddon };
