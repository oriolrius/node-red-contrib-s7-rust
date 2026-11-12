// Live test (manual, not part of npm test): s7 alarms + s7 write against PLCSIM.
// node test/plcsim/live_alarms.js [auto|tia|reference]   (PLC_HOST, default 192.168.0.1; TLS)
const path = require('node:path');
const R = path.join(__dirname, '..', '..'); // repository root
process.env.S7RS_NAPI = path.join(R, 'test/.bin/s7napi-test.node');
const helper = require(path.join(R, 'node_modules/node-red-node-test-helper'));
helper.init(require.resolve(path.join(R, 'node_modules/node-red')));
const NODES = ['endpoint', 'write', 'alarms'].map((n) => require(path.join(R, `nodes/s7rs-${n}.js`)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  await new Promise((r) => helper.startServer(r));
  const flow = [
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host: process.env.PLC_HOST || '192.168.0.1', port: 102, tls: true, timeout: 3000 },
    { id: 'al', type: 's7rs-alarms', endpoint: 'ep', layout: process.argv[2] || 'auto', wires: [['o']] },
    { id: 'w', type: 's7rs-write', endpoint: 'ep', tag: '', wires: [[]] },
    { id: 'o', type: 'helper' },
  ];
  await helper.load(NODES, flow, {});
  const t0 = Date.now();
  helper.getNode('o').on('input', (m) => console.log(`${Date.now() - t0}ms`, m.state, m.topic, m.initial ? 'initial' : '', JSON.stringify(m.payload.coming?.values || m.payload.going?.values || []), m.payload.coming?.time || m.payload.going?.time));
  await sleep(2000);
  console.log(`${Date.now() - t0}ms write alarmReq=true alarmVal=1234`);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmVal': 1234, '"TestSub".alarmReq': true } });
  await sleep(3000);
  console.log(`${Date.now() - t0}ms write alarmReq=false`);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmReq': false } });
  await sleep(6000);
  console.log('status', JSON.stringify(helper.getNode('al').status?.lastCall?.args || ''));
  await helper.unload();
  await new Promise((r) => helper.stopServer(r));
  process.exit(0);
})().catch((e) => { console.error(e); process.exit(1); });
