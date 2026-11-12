// Live test (manual, not part of npm test): s7 watch + s7 write against PLCSIM.
// node test/plcsim/live_watch.js [push|poll]   (PLC_HOST, default 192.168.0.1; TLS)
const path = require('node:path');
const R = path.join(__dirname, '..', '..'); // repository root
process.env.S7RS_NAPI = path.join(R, 'test/.bin/s7napi-test.node');
const helper = require(path.join(R, 'node_modules/node-red-node-test-helper'));
helper.init(require.resolve(path.join(R, 'node_modules/node-red')));
const NODES = ['endpoint', 'read', 'write', 'watch', 'info'].map((n) => require(path.join(R, `nodes/s7rs-${n}.js`)));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  await new Promise((r) => helper.startServer(r));
  const mode = process.argv[2] || 'push';
  const flow = [
    { id: 'ep', type: 's7rs-endpoint', protocol: 's7plus', host: process.env.PLC_HOST || '192.168.0.1', port: 102, tls: true, timeout: 3000 },
    { id: 'wa', type: 's7rs-watch', endpoint: 'ep', mode, interval: 100, output: 'each',
      tags: '"TestSub".tick\n"TestSub".toggle\n"TestSub".echo\n"TestSub".rSine\n"TestOpt".sText', wires: [['o']] },
    { id: 'w', type: 's7rs-write', endpoint: 'ep', tag: '', wires: [['ow']] },
    { id: 'o', type: 'helper' }, { id: 'ow', type: 'helper' },
  ];
  await helper.load(NODES, flow, {});
  const got = [];
  const t0 = Date.now();
  helper.getNode('o').on('input', (m) => got.push({ t: Date.now() - t0, topic: m.topic, v: m.payload, q: m.quality, ts: m.ts, e: m.error }));
  helper.getNode('ow').on('input', (m) => console.log('write ok', JSON.stringify(m.payload)));
  await sleep(1500);
  helper.getNode('w').receive({ payload: { '"TestSub".sp': 1001, '"TestOpt".sText': 'from node-red' } });
  await sleep(1500);
  const byTopic = {};
  for (const g of got) byTopic[g.topic] = (byTopic[g.topic] || 0) + 1;
  console.log('mode', mode, 'messages', got.length, JSON.stringify(byTopic));
  for (const g of got.filter((x) => !x.topic.endsWith('tick') && !x.topic.endsWith('rSine'))) console.log(JSON.stringify(g));
  console.log('ticks', got.filter((x) => x.topic.endsWith('tick')).slice(0, 4).map((x) => `${x.t}ms=${x.v} ts=${x.ts}`).join(' | '));
  await helper.unload();
  await new Promise((r) => helper.stopServer(r));
  process.exit(0);
})().catch((e) => { console.error(e); process.exit(1); });
