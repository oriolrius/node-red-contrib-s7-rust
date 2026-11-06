'use strict';
// s7rs-watch: a Rust thread reads the tags at a fixed (or conditional) rate and
// the node sends only the values that changed.

const { text, parseTags, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsWatch(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    const tags = parseTags(config.tags);
    const mode = config.output || 'each'; // each | object
    const last = {};
    const quality = {};
    if (!ep) return;
    if (!tags.length) {
      node.status({ fill: 'red', shape: 'ring', text: text(RED, 's7rs-watch.status.noTags') });
      return;
    }

    const onData = (ev) => {
      if (ev.sub !== node.id) return;
      if (mode === 'each') {
        for (const v of ev.values) {
          const m = { topic: v.tag, payload: v.value ?? null, quality: v.quality, ts: ev.ts };
          if (v.error) m.error = v.error;
          node.send(m);
        }
        return;
      }
      for (const v of ev.values) {
        last[v.tag] = v.value ?? null;
        quality[v.tag] = v.quality;
      }
      node.send({
        topic: config.topic || config.name || 's7',
        payload: { ...last },
        changed: ev.values.map((v) => v.tag),
        quality: { ...quality },
        s7: { values: ev.values, ms: ev.ms },
        ts: ev.ts,
      });
    };
    ep.bridge.on('data', onData);

    const params = {
      conn: ep.id,
      sub: node.id,
      tags,
      intervalMs: Number(config.interval) || 1000,
      onChange: config.onChange !== false,
      // conditional rate: while condTag is true (or non-zero), read every condInterval ms
      condTag: String(config.condTag || '').trim() || undefined,
      condIntervalMs: Number(config.condInterval) || 100,
    };
    ep.ready
      .then(() => ep.bridge.subscribe(params))
      .then((res) => {
        for (const b of res.invalid || []) node.warn(`${b.tag}: ${b.error}`);
      })
      .catch((err) => node.error(text(RED, 's7rs-watch.errors.subscribe', { error: err.message })));

    node.on('close', (removed, done) => {
      ep.bridge.off('data', onData);
      ep.bridge
        .unsubscribe(node.id)
        .catch(() => {})
        .finally(() => done());
    });
  }
  RED.nodes.registerType('s7rs-watch', S7rsWatch);
};
