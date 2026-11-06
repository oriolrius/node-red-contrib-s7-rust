'use strict';
// Helpers shared by the node files.

const fs = require('node:fs');
const path = require('node:path');

// English catalogs, for runtimes whose RED._ has no catalogs (the test helper
// returns the key).
const fallback = {};
for (const f of fs.readdirSync(path.join(__dirname, '..', 'nodes', 'locales', 'en-US'))) {
  if (f.endsWith('.json')) Object.assign(fallback, require(path.join(__dirname, '..', 'nodes', 'locales', 'en-US', f)));
}

// text: RED._(key, vars), or the English text when the runtime gives the key back.
function text(RED, key, vars) {
  const s = RED._(key, vars);
  if (s !== key) return s;
  const v = key.split('.').reduce((o, k) => (o && typeof o === 'object' ? o[k] : undefined), fallback);
  if (typeof v !== 'string') return key;
  return v.replace(/__(\w+)__/g, (m, k) => (vars && k in vars ? String(vars[k]) : m));
}

function intOrUndef(v) {
  if (v === undefined || v === null || String(v).trim() === '') return undefined;
  const n = Number(v);
  return Number.isFinite(n) ? Math.trunc(n) : undefined;
}

// parseTags: one address per line; empty lines and # or // comments are skipped.
function parseTags(text) {
  return String(text || '')
    .split(/\r?\n/)
    .map((s) => s.trim())
    .filter((s) => s && !s.startsWith('#') && !s.startsWith('//'));
}

// toWireValue: the forms the addon accepts for writes.
function toWireValue(v) {
  if (v instanceof Date) return v.toISOString();
  if (typeof v === 'bigint') return v.toString();
  if (Buffer.isBuffer(v)) return Array.from(v);
  return v;
}

function isPlainObject(p) {
  return p !== null && typeof p === 'object' && !Array.isArray(p) && !(p instanceof Date) && !Buffer.isBuffer(p);
}

function elapsedMs(t0) {
  return Number(process.hrtime.bigint() - t0) / 1e6;
}

// bindStatus shows the connection state of the endpoint on a flow node.
function bindStatus(node, ep) {
  const show = (s) => {
    const st = s.state;
    if (st === 'connected') node.status({ fill: 'green', shape: 'dot', text: s.detail || 'connected' });
    else if (st === 'connecting') node.status({ fill: 'yellow', shape: 'ring', text: 'connecting' });
    else if (st === 'faulted') node.status({ fill: 'red', shape: 'ring', text: (s.detail || 'faulted').slice(0, 60) });
    else node.status({ fill: 'grey', shape: 'ring', text: st || 'disconnected' });
  };
  show(ep.state);
  ep.on('s7status', show);
  node.on('close', () => ep.off('s7status', show));
}

// getEndpoint returns the deployed s7rs-endpoint config node, or null.
function getEndpoint(RED, node, config) {
  const ep = RED.nodes.getNode(config.endpoint);
  if (!ep || !ep.bridge) {
    node.status({ fill: 'red', shape: 'ring', text: 'no endpoint' });
    return null;
  }
  bindStatus(node, ep);
  return ep;
}

module.exports = { text, intOrUndef, parseTags, toWireValue, isPlainObject, elapsedMs, bindStatus, getEndpoint };
