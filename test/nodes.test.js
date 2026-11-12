'use strict';
// End-to-end tests of the six nodes in a real Node-RED runtime
// (node-red-node-test-helper). The test addon (test/.bin/s7napi-test.node, cargo
// feature "fake") is the real addon plus in-process fake PLCs: a classic S7 CPU
// and an S7-1200 that speaks S7CommPlus. Build it with "npm run test:node".

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

process.env.S7RS_NAPI = process.env.S7RS_NAPI || path.join(__dirname, '.bin', 's7napi-test.node');

const helper = require('node-red-node-test-helper');
const { loadAddon, stopAll } = require('../lib/napi-bridge');

// S7RS_NODE_RED=node-red-4 runs the tests on Node-RED 4 (locked alias in devDependencies).
const RED_MODULE = process.env.S7RS_NODE_RED || 'node-red';
helper.init(require.resolve(RED_MODULE));
const fakes = loadAddon();

const NODES = ['endpoint', 'read', 'write', 'watch', 'info', 'alarms'].map((n) => require(`../nodes/s7rs-${n}.js`));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function until(cond, ms = 5000) {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    const iv = setInterval(() => {
      if (cond()) {
        clearInterval(iv);
        resolve();
      } else if (Date.now() - t0 > ms) {
        clearInterval(iv);
        reject(new Error('timeout'));
      }
    }, 10);
  });
}

// A flow: one endpoint, the given nodes, and one helper node "out-<id>" behind each.
// load.got[id] collects every message of node id from the start (watch nodes send
// their first values as soon as the subscription starts).
async function load(ep, nodes, credentials) {
  const flow = [{ id: 'ep', type: 's7rs-endpoint', timeout: 2000, ...ep }];
  for (const n of nodes) {
    flow.push({ endpoint: 'ep', wires: [[`out-${n.id}`]], ...n });
    flow.push({ id: `out-${n.id}`, type: 'helper' });
  }
  await helper.load(NODES, flow, credentials || {});
  load.got = {};
  for (const n of nodes) {
    const got = (load.got[n.id] = []);
    helper.getNode(`out-${n.id}`).on('input', (m) => got.push(m));
  }
  const epNode = helper.getNode('ep');
  await epNode.ready;
  await until(() => epNode.state.state === 'connected');
  return epNode;
}

// ask sends msg to node id and resolves with the message on its output, or
// rejects with the error the node reports.
function ask(id, msg) {
  const n = helper.getNode(id);
  const out = helper.getNode(`out-${id}`);
  return new Promise((resolve, reject) => {
    const onMsg = (m) => {
      cleanup();
      resolve(m);
    };
    const onErr = (call) => {
      cleanup();
      reject(call.args[0] instanceof Error ? call.args[0] : new Error(String(call.args[0])));
    };
    const cleanup = () => {
      out.removeListener('input', onMsg);
      n.removeListener('call:error', onErr);
    };
    out.on('input', onMsg);
    n.on('call:error', onErr);
    n.receive(msg);
  });
}

const collect = (id) => load.got[id];

test.before(() => new Promise((r) => helper.startServer(r)));
test.after(() => new Promise((r) => helper.stopServer(r)));
test.afterEach(async () => {
  await helper.unload();
  stopAll();
});

test('nodes register with Node-RED', async () => {
  await helper.load(NODES, [{ id: 'ep', type: 's7rs-endpoint', host: '127.0.0.1', port: 1 }]);
  for (const t of ['s7rs-endpoint', 's7rs-read', 's7rs-write', 's7rs-watch', 's7rs-alarms', 's7rs-info']) {
    assert.ok(helper._RED.nodes.getType(t), t);
  }
});

test('S7comm: read, write and watch', async () => {
  const [host, port] = fakes.startFakeS7(960).split(':');
  await load({ host, port, cpu: '1500' }, [
    { id: 'w', type: 's7rs-write', tag: '' },
    { id: 'r', type: 's7rs-read', tags: 'MW10:int\nMD12:real\nM20.3\nDB1.DBW0[3]:int\n# comment' },
    { id: 'r1', type: 's7rs-read', tags: '' },
    { id: 'wa', type: 's7rs-watch', tags: 'MW40:int\nDB1.DBB100:string[10]', interval: 50, onChange: true, output: 'each' },
  ]);
  const watched = collect('wa');

  const items = { 'MW10:int': -1234, 'MD12:real': 3.25, 'M20.3': true, 'DB1.DBW0[3]:int': [1, 2, 3] };
  const wm = await ask('w', { payload: items });
  assert.equal(wm.s7.results.length, 4);
  assert.deepEqual((await ask('r', {})).payload, items);

  // tag from msg.topic, single value out
  await ask('w', { topic: 'DB1.DBB100:string[10]', payload: 'hello' });
  assert.equal((await ask('r1', { topic: 'DB1.DBB100:string[10]' })).payload, 'hello');

  // a bad tag: null value and msg.s7.errors, the other tags still good
  const mixed = await ask('r1', { tags: ['MW10:int', 'DB2.DBW0'] });
  assert.equal(mixed.payload['MW10:int'], -1234);
  assert.equal(mixed.payload['DB2.DBW0'], null);
  assert.ok(mixed.s7.errors['DB2.DBW0']);

  await assert.rejects(ask('w', { topic: 'DB2.DBW0', payload: 1 }), /write failed/);

  // watch: each tag once with its current value, then only changes
  await until(() => watched.some((m) => m.payload === 'hello'));
  await sleep(200);
  const before = watched.length;
  await ask('w', { topic: 'MW40:int', payload: 42 });
  await until(() => watched.length > before);
  await sleep(200);
  assert.deepEqual(watched.slice(before).map((m) => [m.topic, m.payload]), [['MW40:int', 42]]);
});

test('S7comm: info actions and conditional watch rate', async () => {
  const [host, port] = fakes.startFakeS7(960).split(':');
  await load({ host, port, cpu: '1500' }, [
    { id: 'i', type: 's7rs-info', action: 'cpuState' },
    { id: 'w', type: 's7rs-write', tag: '' },
    { id: 'wa', type: 's7rs-watch', tags: 'MW50:int', interval: 5000, onChange: true, output: 'each', condTag: 'M30.0', condInterval: 30 },
  ]);
  const watched = collect('wa');

  assert.equal((await ask('i', {})).payload.state, 'RUN');
  assert.ok(Array.isArray((await ask('i', { action: 'identify' })).payload.modules));
  assert.ok((await ask('i', { action: 'szlList' })).payload.some((x) => x.id === '0x0011'));
  const status = (await ask('i', { action: 'status' })).payload;
  assert.equal(status.state, 'connected');
  assert.equal(status.protocol, 's7');
  await assert.rejects(ask('i', { action: 'stop' }), /unknown action/);

  // slow interval, fast while M30.0 is true
  await until(() => watched.length === 1);
  await ask('w', { payload: { 'M30.0': true } });
  await sleep(5100); // one slow cycle reads the condition
  const before = watched.length;
  await ask('w', { payload: { 'MW50:int': 7 } });
  await until(() => watched.length > before, 500); // fast rate now
  assert.equal(watched.at(-1).payload, 7);
});

test('S7CommPlus: absolute and symbolic tags, info, watch', async () => {
  const [host, port] = fakes.startFakeS7Plus().split(':');
  const ep = await load({ protocol: 's7plus', host, port }, [
    { id: 'w', type: 's7rs-write', tag: '' },
    { id: 'r', type: 's7rs-read', tags: '', output: 'object' },
    { id: 'i', type: 's7rs-info', action: 'browse' },
    { id: 'wa', type: 's7rs-watch', tags: '"Data_block_1".Int1', interval: 50, onChange: true, output: 'each' },
  ]);
  assert.match(ep.state.detail, /S7CommPlus V/);
  const watched = collect('wa');

  const items = {
    'MW10:int': -42,
    'MD12:real': 1.5,
    Speed: 1234,
    '"Data_block_1".Int1': 7,
    '"Data_block_1".Real1': 2.25,
    '"Data_block_1".Str': 'hello',
    '"Data_block_1".Arr': [1, 2, 3, 4, 5],
  };
  assert.equal((await ask('w', { payload: items })).s7.results.length, 7);
  assert.deepEqual((await ask('r', { tags: Object.keys(items) })).payload, items);

  // a bit write over byte-area access is refused; an unknown symbol is a bad tag
  await assert.rejects(ask('w', { topic: 'M20.1', payload: true }), /bit writes/);
  const bad = await ask('r', { tags: ['"Data_block_1".Nope', 'MW10:int'] });
  assert.equal(bad.payload['MW10:int'], -42);
  assert.ok(bad.s7.errors['"Data_block_1".Nope']);

  assert.ok((await ask('i', {})).payload.some((v) => v.name.endsWith('Int1')));
  assert.ok((await ask('i', { action: 'dbs' })).payload.some((d) => d.name === 'Data_block_1' && d.number === 1));
  assert.equal((await ask('i', { action: 'cpuState' })).payload.code, 8);
  assert.match((await ask('i', { action: 'info' })).payload.paom, /6ES7 212/);

  await until(() => watched.some((m) => m.payload === 7));
  await ask('w', { payload: { '"Data_block_1".Int1': 8 } });
  await until(() => watched.some((m) => m.payload === 8));
});

test('S7CommPlus: watch with a PLC subscription (FW V4 fake)', async () => {
  const addr = fakes.startFakeS7Plus(false, true);
  const [host, port] = addr.split(':');
  await load({ protocol: 's7plus', host, port }, [
    { id: 'w', type: 's7rs-write', tag: '' },
    { id: 'wa', type: 's7rs-watch', tags: '"Data_block_1".Int1\nSpeed\n"Data_block_1".Nope', mode: 'push', interval: 100, output: 'each' },
  ]);
  const watched = collect('wa');
  // the first notification has every tag; an unknown symbol is a bad tag
  await until(() => watched.some((m) => m.topic === 'Speed') && watched.some((m) => m.topic.endsWith('Nope')));
  assert.equal(watched.find((m) => m.topic.endsWith('Nope')).quality, 'bad');
  assert.equal(fakes.fakeS7PlusSubscriptions(addr), 1, 'no PLC subscription (polling?)');
  await ask('w', { payload: { Speed: 4321 } });
  await until(() => watched.some((m) => m.topic === 'Speed' && m.payload === 4321));
  // the PLC restarts: the node reconnects and subscribes again
  fakes.fakeS7PlusDropConnections(addr);
  await until(() => fakes.fakeS7PlusSubscriptions(addr) === 1, 15000);
  await ask('w', { payload: { Speed: 55 } });
  await until(() => watched.some((m) => m.topic === 'Speed' && m.payload === 55));
  // the node deletes its subscription on close
  await helper.unload();
  await until(() => fakes.fakeS7PlusSubscriptions(addr) === 0);
});

test('S7comm: watch mode push falls back to cyclic reads', async () => {
  const [host, port] = fakes.startFakeS7(960).split(':');
  await load({ host, port, cpu: '1500' }, [
    { id: 'w', type: 's7rs-write', tag: '' },
    { id: 'wa', type: 's7rs-watch', tags: 'MW60:int', mode: 'push', interval: 50, output: 'each' },
  ]);
  const watched = collect('wa');
  await until(() => watched.length === 1);
  await ask('w', { topic: 'MW60:int', payload: 9 });
  await until(() => watched.some((m) => m.payload === 9));
});

test('S7CommPlus: s7 alarms (FW V4 fake)', async () => {
  const addr = fakes.startFakeS7Plus(false, true);
  const [host, port] = addr.split(':');
  await load({ protocol: 's7plus', host, port }, [{ id: 'al', type: 's7rs-alarms' }]);
  const got = collect('al');
  // the fake has one active alarm at start
  await until(() => got.some((m) => m.initial));
  const first = got.find((m) => m.initial);
  assert.equal(first.state, 'coming');
  assert.equal(first.payload.texts[0].text, 'Motor overload');
  assert.equal(fakes.fakeS7PlusSubscriptions(addr), 1);
  // an alarm comes (with an associated value) and goes
  const id = '0x8a0e000500010000';
  fakes.fakeS7PlusRaiseAlarm(addr, id, true, [[1, 1, 0, 0], [0, 77]]);
  await until(() => got.some((m) => m.topic === id && m.state === 'coming'));
  const on = got.find((m) => m.topic === id && m.state === 'coming');
  assert.deepEqual(on.payload.coming.values[1], [0, 77]);
  assert.equal(on.initial, undefined);
  fakes.fakeS7PlusRaiseAlarm(addr, id, false);
  await until(() => got.some((m) => m.topic === id && m.state === 'going'));
  // the PLC restarts: the node subscribes again and lists the active alarms again
  const before = got.filter((m) => m.initial).length;
  fakes.fakeS7PlusDropConnections(addr);
  await until(() => got.filter((m) => m.initial).length > before, 15000);
  await helper.unload();
  await until(() => fakes.fakeS7PlusSubscriptions(addr) === 0);
});

// review of #17: an alarm that goes while the PLC is not reachable must not stay
// in the active count after the reconnect (the new list is empty)
test('S7CommPlus: s7 alarms forgets alarms that went during a reconnect', async () => {
  const addr = fakes.startFakeS7Plus(false, true);
  const [host, port] = addr.split(':');
  await load({ protocol: 's7plus', host, port }, [{ id: 'al', type: 's7rs-alarms' }]);
  const node = helper.getNode('al');
  let status = '';
  const orig = node.status.bind(node);
  node.status = (s) => {
    if (s && / active$/.test(s.text)) status = s.text; // not the connection state texts
    return orig(s);
  };
  const got = collect('al');
  await until(() => got.some((m) => m.initial)); // alarm A of the fake
  const a = got.find((m) => m.initial).topic;
  fakes.fakeS7PlusDropConnections(addr);
  fakes.fakeS7PlusRaiseAlarm(addr, a, false); // A goes while the node is not connected
  await until(() => fakes.fakeS7PlusSubscriptions(addr) === 1, 15000); // subscribed again
  await until(() => status === '0 active');
  const b = '0x8a0e000500010000';
  fakes.fakeS7PlusRaiseAlarm(addr, b, true, [[1, 1, 0, 0], [0, 1]]);
  await until(() => got.some((m) => m.topic === b && m.state === 'coming'));
  assert.equal(status, '1 active');
});

test('admin API: status and read-only actions', async () => {
  const [host, port] = fakes.startFakeS7(960).split(':');
  await load({ host, port, cpu: '1500' }, []);
  const http = helper.request();
  const st = await http.get('/s7rs/status/ep').expect(200);
  assert.equal(st.body.state, 'connected');
  const cpu = await http.get('/s7rs/call/ep/cpuState').expect(200);
  assert.equal(cpu.body.state, 'RUN');
  await http.get('/s7rs/call/ep/setClock').expect(400);
  await http.get('/s7rs/status/nope').expect(404);
});

test('S7CommPlus: check of the PLC certificate (TLS)', async () => {
  const addr = fakes.startFakeS7Plus(true);
  const [host, port] = addr.split(':');
  const pem = fakes.fakeS7PlusCertificate(addr);
  assert.match(pem, /^-----BEGIN CERTIFICATE-----\n/);

  // no check: the certificate action shows the certificate to pin
  await load({ protocol: 's7plus', host, port, tls: true }, [{ id: 'i', type: 's7rs-info', action: 'certificate' }]);
  const cert = (await ask('i', {})).payload;
  assert.equal(cert.pem, pem);
  assert.equal(cert.check, 'none');
  assert.match(cert.fingerprint, /^([0-9A-F]{2}:){31}[0-9A-F]{2}$/);
  assert.match(cert.subject, /CN=PLC-1\/Communication-1/);
  assert.deepEqual(cert.altNames, ['DNS:fake-plc', 'IP:127.0.0.1']);
  const http = helper.request();
  assert.equal((await http.get('/s7rs/call/ep/certificate').expect(200)).body.fingerprint, cert.fingerprint);
  await helper.unload();
  stopAll();

  // pinned fingerprint and trusted certificate: connected
  const ep = await load({ protocol: 's7plus', host, port, tls: true, certVerify: 'fingerprint', certFingerprint: cert.fingerprint.toLowerCase() }, []);
  assert.match(ep.state.detail, /TLS/);
  await helper.unload();
  stopAll();
  await load({ protocol: 's7plus', host, port, tls: true, certVerify: 'trusted', certTrusted: pem }, [{ id: 'i', type: 's7rs-info', action: 'info' }]);
  const info = (await ask('i', {})).payload;
  assert.equal(info.certificateCheck, 'trusted');
  assert.equal(info.certificateFingerprint, cert.fingerprint);
  await helper.unload();
  stopAll();

  // another fingerprint: the endpoint stays disconnected and names the fingerprint it got
  await helper.load(NODES, [
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host, port, tls: true, timeout: 2000, certVerify: 'fingerprint', certFingerprint: '00'.repeat(32) },
  ]);
  const bad = helper.getNode('ep');
  await bad.ready;
  await until(() => bad.state.state === 'faulted' && /fingerprint/.test(bad.state.detail));
  assert.ok(bad.state.detail.includes(cert.fingerprint), bad.state.detail);
  await helper.unload();
  stopAll();

  // a certificate check without TLS, or a bad fingerprint, is a configuration error
  for (const cfg of [{ certVerify: 'fingerprint', certFingerprint: cert.fingerprint }, { tls: true, certVerify: 'fingerprint', certFingerprint: 'AB:CD' }]) {
    await helper.load(NODES, [{ id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host, port, ...cfg }]);
    const n = helper.getNode('ep');
    await assert.rejects(n.ready, cfg.tls ? /SHA-256 fingerprint/ : /needs TLS/);
    await helper.unload();
    stopAll();
  }
});
