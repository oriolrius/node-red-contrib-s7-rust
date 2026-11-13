// Live test (manual, not part of npm test): s7 alarms + s7 write against PLCSIM.
// alReq has the alarm class "Acknowledgement" (README): the script acknowledges it
// with msg.ack and also tries alAuto (no acknowledgement: the PLC refuses it).
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
  const al = helper.getNode('al');
  let status = '';
  const orig = al.status.bind(al);
  al.status = (s) => {
    if (s && s.text) status = s.text;
    return orig(s);
  };
  helper.getNode('o').on('input', (m) =>
    console.log(`${Date.now() - t0}ms`, m.state, m.topic, m.initial ? 'initial' : '',
      m.payload.ackResult !== undefined ? `ackResult=${m.payload.ackResult}` : `acknowledged=${m.payload.acknowledged}`,
      JSON.stringify(m.payload.coming?.values || m.payload.going?.values || []), m.payload.coming?.time || m.payload.going?.time || '', `| status: ${status}`));
  al.on('call:error', (c) => console.log('node error', String(c.args[0])));
  await sleep(2000);
  console.log(`${Date.now() - t0}ms write alarmReq=true alarmVal=1234`);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmVal': 1234, '"TestSub".alarmReq': true } });
  await sleep(2000);
  const alReq = '0x8a0e000500010000';
  const alAuto = '0x8a0e000500020000';
  console.log(`${Date.now() - t0}ms status: ${status}; msg.ack = [alReq, alAuto]`);
  al.receive({ ack: [alReq, alAuto] });
  await sleep(1500);
  console.log(`${Date.now() - t0}ms status: ${status}`);
  console.log(`${Date.now() - t0}ms write alarmReq=false`);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmReq': false } });
  await sleep(1500);
  // an alarm that goes before its acknowledgement still waits for it
  console.log(`${Date.now() - t0}ms write alarmReq=true, then false (no acknowledgement)`);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmReq': true } });
  await sleep(1000);
  helper.getNode('w').receive({ payload: { '"TestSub".alarmReq': false } });
  await sleep(1000);
  console.log(`${Date.now() - t0}ms status: ${status}; msg.ack = "all"`);
  al.receive({ ack: 'all' });
  await sleep(1500);
  console.log(`${Date.now() - t0}ms status: ${status}`);
  await helper.unload();
  await new Promise((r) => helper.stopServer(r));
  process.exit(0);
})().catch((e) => { console.error(e); process.exit(1); });
