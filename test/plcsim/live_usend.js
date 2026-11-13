// Live test (manual, not part of npm test): s7 urcv + s7 usend against PLCSIM over
// the S7 connection S7_Connection_1 (PLC TSAP 10.01, partner TSAP 03.00, FB PartnerComm:
// USEND R_ID 3 with usendBuf (10 bytes) and usendInt, URCV R_ID 4 into urcvBuf (100 bytes)).
// node test/plcsim/live_usend.js   (PLC_HOST, default 192.168.0.1)
const path = require('node:path');
const R = path.join(__dirname, '..', '..'); // repository root
process.env.S7RS_NAPI = path.join(R, 'test/.bin/s7napi-test.node');
const helper = require(path.join(R, 'node_modules/node-red-node-test-helper'));
helper.init(require.resolve(path.join(R, 'node_modules/node-red')));
const NODES = ['endpoint', 'read', 'write', 'usend', 'urcv'].map((n) => require(path.join(R, `nodes/s7rs-${n}.js`)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const host = process.env.PLC_HOST || '192.168.0.1';
(async () => {
  await new Promise((r) => helper.startServer(r));
  await helper.load(NODES, [
    { id: 'pt', type: 's7rs-endpoint', protocol: 's7partner', host, port: 102, localTsap: '03.00', remoteTsap: '10.01', timeout: 3000 },
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host, port: 102, tls: true, timeout: 3000 },
    { id: 'rx', type: 's7rs-urcv', endpoint: 'pt', rid: '3', wires: [['orx']] },
    { id: 'tx', type: 's7rs-usend', endpoint: 'pt', rid: '4', wires: [['otx']] },
    { id: 'w', type: 's7rs-write', endpoint: 'ep', tag: '', wires: [[]] },
    { id: 'r', type: 's7rs-read', endpoint: 'ep', tags: '"PartnerData".urcvCount\n"PartnerData".urcvBuf\n"PartnerData".usendCount', output: 'object', wires: [['or']] },
    { id: 'orx', type: 'helper' }, { id: 'otx', type: 'helper' }, { id: 'or', type: 'helper' },
  ]);
  const got = [];
  helper.getNode('orx').on('input', (m) => got.push(m));
  helper.getNode('otx').on('input', (m) => console.log('usend ok', JSON.stringify(m.s7)));
  helper.getNode('tx').on('call:error', (c) => console.log('usend error', String(c.args[0])));
  const reads = [];
  helper.getNode('or').on('input', (m) => reads.push(m.payload));
  await sleep(2500);
  console.log(`status: partner ${helper.getNode('pt').state.state} (${helper.getNode('pt').state.detail})`);
  helper.getNode('r').receive({});
  await sleep(800);
  const before = reads.at(-1) || {};
  // PLC -> Node-RED: USEND R_ID 3 (usendBuf: 0xCD, the count, 2..9; usendInt: count * 3)
  helper.getNode('w').receive({ payload: { '"PartnerData".usendReq': true } });
  await sleep(1500);
  let okIn = false;
  for (const m of got) {
    const [a, b] = m.payload;
    console.log(`${m.ts} urcv R_ID ${m.rid}: areas ${m.areas.map((x) => `type ${x.type} ${x.data.length} bytes`).join(', ')}: ${a.toString('hex')} ${b.toString('hex')}`);
    const n = before['"PartnerData".usendCount'];
    okIn = a.length === 10 && a[0] === 0xcd && a[1] === (n & 0xff) && a[9] === 9 && b.readInt16BE(0) === n * 3;
  }
  // Node-RED -> PLC: 100 bytes to URCV R_ID 4 (RD_1 = Array[0..99] of Byte)
  const data = Buffer.from(Array.from({ length: 100 }, (_, i) => (i * 7 + Date.now()) % 256));
  helper.getNode('tx').receive({ payload: data });
  await sleep(1000);
  helper.getNode('r').receive({});
  await sleep(800);
  const v = reads.at(-1) || {};
  const buf = v['"PartnerData".urcvBuf'] || [];
  const okOut = v['"PartnerData".urcvCount'] === before['"PartnerData".urcvCount'] + 1 && data.every((x, i) => buf[i] === x);
  console.log(`PLC: urcvCount ${before['"PartnerData".urcvCount']} -> ${v['"PartnerData".urcvCount']}, urcvBuf ${okOut ? 'equals' : 'DIFFERS from'} the data sent`);
  const pass = got.length === 1 && okIn && okOut;
  console.log(pass ? 'RESULT ok' : 'RESULT FAILED');
  await helper.unload();
  await new Promise((r) => helper.stopServer(r));
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error(e); process.exit(1); });
