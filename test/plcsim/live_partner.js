// Live test (manual, not part of npm test): s7 brcv + s7 bsend against PLCSIM over
// the S7 connection S7_Connection_1 (PLC TSAP 10.01, partner TSAP 03.00, FB PartnerComm).
// node test/plcsim/live_partner.js [bytes]   (PLC_HOST, default 192.168.0.1; bytes up to 2000)
const path = require('node:path');
const R = path.join(__dirname, '..', '..'); // repository root
process.env.S7RS_NAPI = path.join(R, 'test/.bin/s7napi-test.node');
const helper = require(path.join(R, 'node_modules/node-red-node-test-helper'));
helper.init(require.resolve(path.join(R, 'node_modules/node-red')));
const NODES = ['endpoint', 'read', 'write', 'bsend', 'brcv'].map((n) => require(path.join(R, `nodes/s7rs-${n}.js`)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const host = process.env.PLC_HOST || '192.168.0.1';
const n = Math.min(Number(process.argv[2]) || 1800, 2000);
(async () => {
  await new Promise((r) => helper.startServer(r));
  await helper.load(NODES, [
    { id: 'pt', type: 's7rs-endpoint', protocol: 's7partner', host, port: 102, localTsap: '03.00', remoteTsap: '10.01', timeout: 3000 },
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host, port: 102, tls: true, timeout: 3000 },
    { id: 'rx', type: 's7rs-brcv', endpoint: 'pt', rid: '1', wires: [['orx']] },
    { id: 'tx', type: 's7rs-bsend', endpoint: 'pt', rid: '2', wires: [['otx']] },
    { id: 'w', type: 's7rs-write', endpoint: 'ep', tag: '', wires: [[]] },
    { id: 'r', type: 's7rs-read', endpoint: 'ep', tags: '"PartnerData".recvCount\n"PartnerData".recvBuf\n"PartnerData".sendCount', output: 'object', wires: [['or']] },
    { id: 'orx', type: 'helper' }, { id: 'otx', type: 'helper' }, { id: 'or', type: 'helper' },
  ]);
  const t0 = Date.now();
  const got = [];
  helper.getNode('orx').on('input', (m) => got.push(m));
  helper.getNode('otx').on('input', (m) => console.log(`${Date.now() - t0}ms bsend ok`, JSON.stringify(m.s7)));
  helper.getNode('tx').on('call:error', (c) => console.log('bsend error', String(c.args[0])));
  const reads = [];
  helper.getNode('or').on('input', (m) => reads.push(m.payload));
  await sleep(2500);
  console.log(`status: partner ${helper.getNode('pt').state.state} (${helper.getNode('pt').state.detail})`);
  // PLC -> Node-RED: the PLC program sends n bytes (BSEND R_ID 1)
  helper.getNode('w').receive({ payload: { '"PartnerData".sendSize': n, '"PartnerData".sendReq': true } });
  await sleep(2000);
  for (const m of got) {
    const p = m.payload;
    let ok = p.length === n && p[0] === 0xab;
    for (let i = 2; i < p.length && ok; i++) ok = p[i] === i % 251;
    console.log(`${m.ts} brcv R_ID ${m.rid}: ${p.length} bytes, pattern ${ok ? 'ok' : 'WRONG'}`);
  }
  // Node-RED -> PLC: n bytes to BRCV R_ID 2
  const data = Buffer.from(Array.from({ length: n }, (_, i) => (i * 7) % 256));
  helper.getNode('tx').receive({ payload: data });
  await sleep(1500);
  helper.getNode('r').receive({});
  await sleep(800);
  const v = reads.at(-1) || {};
  const buf = v['"PartnerData".recvBuf'] || [];
  let ok = true;
  for (let i = 0; i < n; i++) if (buf[i] !== data[i]) { ok = false; break; }
  console.log(`PLC: recvCount ${v['"PartnerData".recvCount']}, sendCount ${v['"PartnerData".sendCount']}, recvBuf[0..${n - 1}] ${ok ? 'equals' : 'DIFFERS from'} the block sent`);
  const pass = got.length === 1 && got[0].payload.length === n && ok;
  console.log(pass ? 'RESULT ok' : 'RESULT FAILED');
  await helper.unload();
  await new Promise((r) => helper.stopServer(r));
  process.exit(pass ? 0 : 1);
})().catch((e) => { console.error(e); process.exit(1); });
