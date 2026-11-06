'use strict';
// s7rs-write: writes S7 tags (one tag, or {tag: value} in one request).

const { text, toWireValue, isPlainObject, elapsedMs, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsWrite(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    const cfgTag = String(config.tag || '').trim();

    node.on('input', async (msg, send, done) => {
      if (!ep) return done(new Error(text(RED, 's7rs-write.errors.noEndpoint')));
      const p = msg.payload;
      let items;
      if (cfgTag) items = [{ tag: cfgTag, value: toWireValue(p) }];
      else if (isPlainObject(p)) items = Object.entries(p).map(([tag, value]) => ({ tag, value: toWireValue(value) }));
      else if (typeof msg.topic === 'string' && msg.topic) items = [{ tag: msg.topic, value: toWireValue(p) }];
      if (!items || !items.length) return done(new Error(text(RED, 's7rs-write.errors.nothing')));
      try {
        await ep.ready;
        const t0 = process.hrtime.bigint();
        const { results } = await ep.bridge.write(ep.id, items);
        msg.s7 = { results, ms: elapsedMs(t0) };
        const bad = results.filter((r) => r.error);
        if (bad.length) {
          return done(new Error(text(RED, 's7rs-write.errors.failed', { tags: bad.map((r) => (r.error.startsWith(`${r.tag}: `) ? r.error : `${r.tag}: ${r.error}`)).join('; ') })));
        }
        send(msg);
        done();
      } catch (err) {
        done(err);
      }
    });
  }
  RED.nodes.registerType('s7rs-write', S7rsWrite);
};
