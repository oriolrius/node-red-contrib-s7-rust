// Live test (manual, not part of npm test): the check of the PLC certificate
// against PLCSIM (TLS). Reads the certificate, then connects with each check:
// pinned fingerprint, trusted certificate, a wrong fingerprint and a wrong name.
// node test/plcsim/live_cert.js   (PLC_HOST, default 192.168.0.1)
const path = require('node:path');
const R = path.join(__dirname, '..', '..'); // repository root
process.env.S7RS_NAPI = path.join(R, 'test/.bin/s7napi-test.node');
const helper = require(path.join(R, 'node_modules/node-red-node-test-helper'));
helper.init(require.resolve(path.join(R, 'node_modules/node-red')));
const { stopAll } = require(path.join(R, 'lib/napi-bridge'));
const NODES = ['endpoint', 'read', 'info'].map((n) => require(path.join(R, `nodes/s7rs-${n}.js`)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const host = process.env.PLC_HOST || '192.168.0.1';

// Loads an endpoint with the check, reads "TestSub".tick; returns the state.
async function run(check) {
  await helper.load(NODES, [
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host, port: 102, tls: true, timeout: 3000, ...check },
    { id: 'r', type: 's7rs-read', endpoint: 'ep', tags: '"TestSub".tick', wires: [['o']] },
    { id: 'i', type: 's7rs-info', endpoint: 'ep', action: 'certificate', wires: [['oi']] },
    { id: 'o', type: 'helper' }, { id: 'oi', type: 'helper' },
  ]);
  const ep = helper.getNode('ep');
  const out = {};
  helper.getNode('o').on('input', (m) => (out.tick = m.payload));
  helper.getNode('oi').on('input', (m) => (out.cert = m.payload));
  await ep.ready.catch((e) => (out.openError = e.message));
  for (let i = 0; i < 50 && !['connected', 'faulted'].includes(ep.state.state); i++) await sleep(100);
  out.state = ep.state.state;
  out.detail = ep.state.detail;
  if (out.state === 'connected') {
    helper.getNode('r').receive({});
    helper.getNode('i').receive({});
    await sleep(500);
  }
  await helper.unload();
  stopAll();
  return out;
}

(async () => {
  await new Promise((r) => helper.startServer(r));
  const none = await run({});
  const c = none.cert;
  console.log('none        :', none.state, 'tick', none.tick);
  console.log('certificate :', c.subject, '|', c.issuer === c.subject ? 'self-signed' : c.issuer, '|', c.notBefore, '-', c.notAfter, '|', c.altNames.join(', '));
  console.log('fingerprint :', c.fingerprint);
  const fp = await run({ certVerify: 'fingerprint', certFingerprint: c.fingerprint });
  console.log('fingerprint :', fp.state, 'tick', fp.tick, 'check', fp.cert && fp.cert.check);
  const tr = await run({ certVerify: 'trusted', certTrusted: c.pem });
  console.log('trusted     :', tr.state, 'tick', tr.tick, 'check', tr.cert && tr.cert.check);
  const bad = await run({ certVerify: 'fingerprint', certFingerprint: '00'.repeat(32) });
  console.log('wrong pin   :', bad.state, '-', bad.detail);
  const name = await run({ certVerify: 'trusted', certTrusted: c.pem, certName: 'other-plc' });
  console.log('wrong name  :', name.state, '-', name.detail);
  const ok = fp.tick !== undefined && tr.tick !== undefined && bad.state === 'faulted' && name.state === 'faulted';
  console.log(ok ? 'RESULT ok' : 'RESULT FAILED');
  await new Promise((r) => helper.stopServer(r));
  process.exit(ok ? 0 : 1);
})().catch((e) => { console.error(e); process.exit(1); });
