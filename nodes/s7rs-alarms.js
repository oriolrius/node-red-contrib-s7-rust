'use strict';
// s7rs-alarms: a PLC alarm subscription (S7CommPlus). One message per alarm
// event (coming / going); the active alarms first, and again after a reconnect.

const { text, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsAlarms(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    if (!ep) return;
    const active = new Map(); // id -> alarm

    const showCount = () =>
      node.status({ fill: active.size ? 'yellow' : 'green', shape: 'dot', text: text(RED, 's7rs-alarms.status.active', { n: active.size }) });

    const onAlarms = (ev) => {
      if (ev.sub !== node.id) return;
      // a new subscription (start or reconnect): the alarms that went meanwhile are gone
      if (ev.snapshot) active.clear();
      for (const a of ev.alarms) {
        if (a.state === 'coming') active.set(a.id, a);
        else active.delete(a.id);
        const msg = { topic: a.id, payload: a, state: a.state, ts: ev.ts };
        if (a.initial) msg.initial = true;
        node.send(msg);
      }
      showCount();
    };
    ep.bridge.on('alarms', onAlarms);

    ep.ready
      .then(() =>
        ep.bridge.subscribeAlarms({ conn: ep.id, sub: node.id, initial: config.initial !== false, layout: config.layout || 'auto' }),
      )
      .catch((err) => node.error(text(RED, 's7rs-alarms.errors.subscribe', { error: err.message })));

    node.on('close', (removed, done) => {
      ep.bridge.off('alarms', onAlarms);
      ep.bridge
        .unsubscribe(node.id)
        .catch(() => {})
        .finally(() => done());
    });
  }
  RED.nodes.registerType('s7rs-alarms', S7rsAlarms);
};
