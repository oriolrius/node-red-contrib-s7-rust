'use strict';
// s7rs-urcv: receives the USENDs of the PLC over an S7 partner endpoint
// (S7 connection, USEND/URCV). One message per USEND.

const { text, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsUrcv(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    if (!ep) return;
    const want = String(config.rid ?? '').trim() === '' ? null : Number(config.rid);
    let count = 0;

    const onUsend = (ev) => {
      if (ev.sub !== node.id || ev.kind !== 'usend') return;
      if (want !== null && ev.rid !== want) return;
      count += 1;
      node.status({ fill: 'green', shape: 'dot', text: text(RED, 's7rs-urcv.status.count', { n: count }) });
      const payload = ev.areas.length === 1 ? ev.areas[0].data : ev.areas.map((a) => a.data);
      node.send({ topic: String(ev.rid), payload, areas: ev.areas, rid: ev.rid, ts: ev.ts });
    };
    ep.bridge.on('blocks', onUsend);

    ep.ready
      .then(() => ep.bridge.subscribeBlocks({ conn: ep.id, sub: node.id, kind: 'usend' }))
      .catch((err) => node.error(text(RED, 's7rs-urcv.errors.subscribe', { error: err.message })));

    node.on('close', (removed, done) => {
      ep.bridge.off('blocks', onUsend);
      ep.bridge
        .unsubscribe(node.id)
        .catch(() => {})
        .finally(() => done());
    });
  }
  RED.nodes.registerType('s7rs-urcv', S7rsUrcv);
};
