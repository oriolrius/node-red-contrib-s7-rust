'use strict';
// s7rs-read: reads S7 tags when a message arrives.

const { text, parseTags, elapsedMs, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsRead(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    const cfgTags = parseTags(config.tags);
    const output = config.output || 'auto'; // auto | object | array

    node.on('input', async (msg, send, done) => {
      if (!ep) return done(new Error(text(RED, 's7rs-read.errors.noEndpoint')));
      let tags = cfgTags;
      if (!tags.length) {
        if (Array.isArray(msg.tags)) tags = msg.tags.map(String);
        else if (Array.isArray(msg.payload)) tags = msg.payload.map(String);
        else if (typeof msg.topic === 'string' && msg.topic) tags = [msg.topic];
      }
      if (!tags.length) return done(new Error(text(RED, 's7rs-read.errors.noTags')));
      try {
        await ep.ready;
        const t0 = process.hrtime.bigint();
        const { values } = await ep.bridge.read(ep.id, tags);
        msg.s7 = { values, ms: elapsedMs(t0) };
        if (output === 'array') msg.payload = values.map((v) => v.value ?? null);
        else if (output === 'auto' && tags.length === 1) {
          msg.payload = values[0].value ?? null;
          msg.topic = msg.topic || tags[0];
        } else msg.payload = Object.fromEntries(values.map((v) => [v.tag, v.value ?? null]));
        const bad = values.filter((v) => v.quality !== 'good');
        if (bad.length) {
          msg.s7.errors = Object.fromEntries(bad.map((v) => [v.tag, v.error]));
          node.warn(text(RED, 's7rs-read.errors.badTags', { tags: bad.map((v) => `${v.tag}: ${v.error}`).join('; ') }));
        }
        send(msg);
        done();
      } catch (err) {
        done(err);
      }
    });
  }
  RED.nodes.registerType('s7rs-read', S7rsRead);
};
