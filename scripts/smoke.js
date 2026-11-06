'use strict';
// Smoke test of a packed or built tree: the prebuilt addon for this host loads and
// the nodes register. Used in CI inside glibc and musl containers.
delete process.env.S7RS_NAPI;
const { addonPath, loadAddon, NapiBridge } = require('../lib/napi-bridge');

const p = addonPath();
const a = loadAddon();
if (typeof a.S7Native !== 'function') throw new Error(`${p}: no S7Native export`);
const b = new NapiBridge();
b.start();
b.stop();

const types = [];
const RED = {
  _: (k) => k,
  log: { error() {}, warn() {}, info() {}, debug() {}, trace() {} },
  nodes: { registerType: (t) => types.push(t), getNode: () => null, createNode() {} },
  httpAdmin: { get() {} },
  auth: { needsPermission: () => (req, res, next) => next() },
};
for (const n of ['endpoint', 'read', 'write', 'watch', 'info']) require(`../nodes/s7rs-${n}.js`)(RED);
if (types.length !== 5) throw new Error(`registered ${types.length} node types`);
console.log(`smoke ok: ${process.version} ${p}`);
