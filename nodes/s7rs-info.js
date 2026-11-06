'use strict';
// s7rs-info: diagnostics and browsing. msg.action (or the node's action), args in
// msg.payload when it is an object (or the node's JSON args). Result in msg.payload.

const { text, isPlainObject, elapsedMs, getEndpoint } = require('../lib/common');

module.exports = function (RED) {
  function S7rsInfo(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    let cfgArgs;
    try {
      cfgArgs = config.args ? JSON.parse(config.args) : undefined;
    } catch (err) {
      node.error(text(RED, 's7rs-info.errors.args', { error: err.message }));
    }

    node.on('input', async (msg, send, done) => {
      if (!ep) return done(new Error(text(RED, 's7rs-info.errors.noEndpoint')));
      const action = String(msg.action || config.action || '').trim();
      if (!action) return done(new Error(text(RED, 's7rs-info.errors.noAction')));
      const args = isPlainObject(msg.payload) ? msg.payload : cfgArgs;
      try {
        await ep.ready;
        const t0 = process.hrtime.bigint();
        msg.payload = action === 'status' ? await ep.bridge.status(ep.id) : await ep.bridge.call(ep.id, action, args);
        msg.topic = msg.topic || action;
        msg.s7 = { action, ms: elapsedMs(t0) };
        send(msg);
        done();
      } catch (err) {
        done(err);
      }
    });
  }
  RED.nodes.registerType('s7rs-info', S7rsInfo);
};
