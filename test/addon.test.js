'use strict';
// Tests of the Node-API boundary (crate s7napi): the exact shapes and the value
// conversions in both directions, against the in-process fake PLCs of the test
// addon (test/.bin/s7napi-test.node, built by "npm run test:node").

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

process.env.S7RS_NAPI = process.env.S7RS_NAPI || path.join(__dirname, '.bin', 's7napi-test.node');
const { loadAddon } = require('../lib/napi-bridge');

const addon = loadAddon();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(cond, ms = 5000) {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await sleep(10);
  }
}

// One native instance per test, with every callback recorded.
async function open(protocol, addr) {
  const [host, port] = addr.split(':');
  const ev = { status: [], data: [], log: [] };
  const native = new addon.S7Native(
    (conn, state, detail) => ev.status.push([conn, state, detail]),
    (e) => ev.data.push(e),
    (conn, level, text) => ev.log.push([conn, level, text]),
  );
  const conn = `c-${protocol}`;
  const res = native.open({ conn, protocol, host, port: Number(port), timeoutMs: 2000, cpu: '1500', logLevel: 'debug' });
  await until(() => ev.status.some((s) => s[1] === 'connected'));
  return { native, conn, ev, res };
}

test('S7comm: value conversions, columns and errors', async () => {
  const { native, conn, ev, res } = await open('s7', addon.startFakeS7(960));
  try {
    assert.equal(res.protocol, 's7');
    assert.equal(typeof res.addr, 'string');
    assert.equal(ev.status[0][0], conn);

    const iso = new Date('2026-11-06T10:20:30.456Z');
    const tags = [
      'MW10:int', 'MD12:real', 'M20.3', 'DB1.DBW0[3]:int', 'DB1.DBB100:string[40]',
      'DB1.DBB200:string[10]', 'DB1.DBB20[2]', 'MW30:int', 'MW32:int', 'MW34:int',
    ];
    const values = [-1234, 3.25, true, [1, 2, 3], iso, 123n, Buffer.from([7, 8]), { a: 1 }, undefined, 2.5];
    const w = await native.write(conn, tags, values);
    assert.equal(typeof w.nativeMs, 'number');
    const errIdx = w.errs.map((e) => e[0]);
    // the object and the missing value do not convert: not sent, reported by index
    assert.deepEqual(errIdx.slice(0, 2), [7, 8]);
    assert.match(w.errs[0][1], /^MW30:int: objects are not S7 values/);
    assert.match(w.errs[1][1], /^MW32:int: value is missing/);

    const r = await native.read(conn, tags.slice(0, 7).concat(['DB2.DBW0']));
    assert.deepEqual(r.vals.slice(0, 7), [-1234, 3.25, true, [1, 2, 3], iso.toISOString(), '123', [7, 8]]);
    assert.equal(r.vals[7], null, 'bad tag: null value');
    assert.equal(r.errs.length, 1);
    assert.equal(r.errs[0][0], 7);
    assert.equal(typeof r.errs[0][1], 'string');
    assert.equal(typeof r.nativeMs, 'number');

    // subscribe: bad tags come back as [{tag, error}]; first event has all values (no idx)
    const bad = await native.subscribe({ conn, sub: 's1', tags: ['MW40:int', 'MW42:int', 'nonsense'], intervalMs: 30, onChange: true });
    assert.equal(bad.length, 1);
    assert.equal(bad[0].tag, 'nonsense');
    assert.equal(typeof bad[0].error, 'string');
    await until(() => ev.data.length >= 1);
    const first = ev.data[0];
    assert.equal(first.sub, 's1');
    assert.equal(first.conn, conn);
    assert.equal(first.vals.length, 3);
    assert.equal(first.idx, undefined);
    assert.equal(first.vals[2], null);
    assert.equal(first.errs[0][0], 2);
    assert.ok(!Number.isNaN(Date.parse(first.ts)));
    assert.equal(typeof first.ms, 'number');
    // a change of one tag: only that tag, with idx
    const n = ev.data.length;
    await native.write(conn, ['MW42:int'], [77]);
    await until(() => ev.data.length > n);
    const ch = ev.data[n];
    assert.deepEqual(ch.vals, [77]);
    assert.deepEqual(ch.idx, [1]);
    assert.deepEqual(ch.errs, []);
    native.unsubscribe('s1');

    // status is an object, call returns JSON text
    const st = await native.status(conn);
    assert.equal(st.conn, conn);
    assert.equal(st.state, 'connected');
    const id = await native.call(conn, 'identify');
    assert.equal(typeof id, 'string');
    assert.equal(typeof JSON.parse(id), 'object');
    await assert.rejects(native.call(conn, 'noSuchAction'), /noSuchAction/);
    assert.throws(() => native.call(conn, 'szl', '{bad json'), /bad args/); // checked before the promise
    assert.throws(() => native.read('nope', ['MW0']), /is not open/);
    assert.ok(ev.log.length > 0 && ev.log.every((l) => l[0] === conn && typeof l[2] === 'string'));
  } finally {
    native.stop();
  }
  await until(() => ev.status.some((s) => s[1] === 'disconnected'));
});

test('S7CommPlus: status, browse and read through the addon', async () => {
  const { native, conn } = await open('s7plus', addon.startFakeS7Plus());
  try {
    const st = await native.status(conn);
    assert.equal(st.protocol, 's7plus');
    const browse = JSON.parse(await native.call(conn, 'browse'));
    assert.ok(browse);
    const r = await native.read(conn, ['"Data_block_1".Static_1', 'MW0:int']);
    assert.equal(r.vals.length, 2);
    assert.equal(typeof r.nativeMs, 'number');
  } finally {
    native.stop();
  }
});
