'use strict';
// s7rs-bsend: sends msg.payload as a block to a PLC BRCV over an S7 partner
// endpoint (S7 connection, BSEND/BRCV). A write to the PLC.

const { text, elapsedMs, getEndpoint } = require('../lib/common');

// Buffer as is; a byte array; a string as UTF-8.
function toBlock(p) {
  if (Buffer.isBuffer(p)) return p;
  if (Array.isArray(p) && p.every((x) => Number.isInteger(x) && x >= 0 && x <= 255)) return Buffer.from(p);
  if (typeof p === 'string') return Buffer.from(p, 'utf8');
  return null;
}

module.exports = function (RED) {
  function S7rsBsend(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    const cfgRid = String(config.rid ?? '').trim();

    node.on('input', async (msg, send, done) => {
      if (!ep) return done(new Error(text(RED, 's7rs-bsend.errors.noEndpoint')));
      const ridIn = msg.rid !== undefined ? msg.rid : cfgRid;
      const rid = Number(ridIn);
      if (ridIn === '' || !Number.isInteger(rid) || rid < 0 || rid > 0xffffffff) return done(new Error(text(RED, 's7rs-bsend.errors.rid')));
      const data = toBlock(msg.payload);
      if (!data || data.length === 0 || data.length > 65534) return done(new Error(text(RED, 's7rs-bsend.errors.payload')));
      try {
        await ep.ready;
        const t0 = process.hrtime.bigint();
        await ep.bridge.call(ep.id, 'bsend', { rid, data: data.toString('hex') });
        msg.s7 = { rid, bytes: data.length, ms: elapsedMs(t0) };
        send(msg);
        done();
      } catch (err) {
        done(err);
      }
    });
  }
  RED.nodes.registerType('s7rs-bsend', S7rsBsend);
};
