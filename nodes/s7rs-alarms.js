'use strict';
// s7rs-alarms: a PLC alarm subscription (S7CommPlus). One message per alarm
// event (coming / going / acknowledged); the active alarms first, and again
// after a reconnect. An input msg.ack acknowledges alarms (a write to the PLC).

const { text, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsAlarms(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    if (!ep) return;
    const active = new Map(); // id -> alarm

    // the list holds the coming alarms and the alarms that went but still wait
    // for acknowledgement (the PLC keeps them until they are acknowledged)
    const showCount = () => {
      const all = [...active.values()];
      const n = all.filter((a) => a.state === 'coming').length;
      const unack = all.filter((a) => a.acknowledged === false).length;
      const t = unack ? text(RED, 's7rs-alarms.status.unack', { n, u: unack }) : text(RED, 's7rs-alarms.status.active', { n });
      node.status({ fill: unack ? 'red' : n ? 'yellow' : 'green', shape: 'dot', text: t });
    };

    const onAlarms = (ev) => {
      if (ev.sub !== node.id) return;
      // a new subscription (start or reconnect): the alarms that went meanwhile are gone
      if (ev.snapshot) active.clear();
      for (const a of ev.alarms) {
        if (a.state === 'coming' || (a.state === 'going' && a.acknowledged === false)) active.set(a.id, a);
        else if (a.state === 'going') active.delete(a.id);
        else if (a.state === 'acknowledged' && active.has(a.id)) {
          const x = active.get(a.id);
          if (x.state === 'going') active.delete(a.id);
          else x.acknowledged = true;
        }
        const msg = { topic: a.id, payload: a, state: a.state, ts: ev.ts };
        if (a.initial) msg.initial = true;
        node.send(msg);
      }
      showCount();
    };
    ep.bridge.on('alarms', onAlarms);

    // msg.ack: an alarm id ("0x…", the msg.topic of an alarm message), a list of
    // ids, or true / "all" (every active alarm that waits for acknowledgement)
    node.on('input', async (msg, send, done) => {
      if (msg.ack === undefined || msg.ack === false || msg.ack === null) {
        done(new Error(text(RED, 's7rs-alarms.errors.noAck')));
        return;
      }
      const all = msg.ack === true || msg.ack === 'all';
      try {
        await ep.ready;
        await ep.bridge.call(ep.id, 'acknowledge', all ? { all: true } : { ids: [].concat(msg.ack) });
        done();
      } catch (err) {
        done(new Error(text(RED, 's7rs-alarms.errors.ack', { error: err.message })));
      }
    });

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
