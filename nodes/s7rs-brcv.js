'use strict';
// s7rs-brcv: receives the blocks of a PLC BSEND over an S7 partner endpoint
// (S7 connection, BSEND/BRCV). One message per block.

const { text, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsBrcv(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    if (!ep) return;
    const want = String(config.rid ?? '').trim() === '' ? null : Number(config.rid);
    let count = 0;

    const onBlocks = (ev) => {
      if (ev.sub !== node.id) return;
      if (want !== null && ev.rid !== want) return;
      count += 1;
      node.status({ fill: 'green', shape: 'dot', text: text(RED, 's7rs-brcv.status.count', { n: count }) });
      node.send({ topic: String(ev.rid), payload: ev.data, rid: ev.rid, ts: ev.ts });
    };
    ep.bridge.on('blocks', onBlocks);

    ep.ready
      .then(() => ep.bridge.subscribeBlocks({ conn: ep.id, sub: node.id }))
      .catch((err) => node.error(text(RED, 's7rs-brcv.errors.subscribe', { error: err.message })));

    node.on('close', (removed, done) => {
      ep.bridge.off('blocks', onBlocks);
      ep.bridge
        .unsubscribe(node.id)
        .catch(() => {})
        .finally(() => done());
    });
  }
  RED.nodes.registerType('s7rs-brcv', S7rsBrcv);
};
