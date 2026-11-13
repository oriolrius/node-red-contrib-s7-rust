'use strict';
// s7rs-usend: sends msg.payload (one to four areas) as a USEND to a PLC URCV over an
// S7 partner endpoint (S7 connection, USEND/URCV). A write to the PLC, not acknowledged.

const { text, elapsedMs, getEndpoint } = require('../lib/common');

// Buffer as is; a byte array; a string as UTF-8.
function toBytes(p) {
  if (Buffer.isBuffer(p)) return p;
  if (Array.isArray(p) && p.length > 0 && p.every((x) => Number.isInteger(x) && x >= 0 && x <= 255)) return Buffer.from(p);
  if (typeof p === 'string') return Buffer.from(p, 'utf8');
  return null;
}

// One area (Buffer, byte array, string) or an array of 1..4 areas (Buffers or strings).
function toAreas(p) {
  const one = toBytes(p);
  if (one) return [one];
  if (!Array.isArray(p) || p.length < 1 || p.length > 4) return null;
  const list = p.map((x) => (Array.isArray(x) ? toBytes(x) : Buffer.isBuffer(x) || typeof x === 'string' ? toBytes(x) : null));
  return list.every((b) => b && b.length > 0) ? list : null;
}

module.exports = function (RED) {
  function S7rsUsend(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    const ep = getEndpoint(RED, node, config);
    const cfgRid = String(config.rid ?? '').trim();

    node.on('input', async (msg, send, done) => {
      if (!ep) return done(new Error(text(RED, 's7rs-usend.errors.noEndpoint')));
      const ridIn = msg.rid !== undefined ? msg.rid : cfgRid;
      const rid = Number(ridIn);
      if (ridIn === '' || !Number.isInteger(rid) || rid < 0 || rid > 0xffffffff) return done(new Error(text(RED, 's7rs-usend.errors.rid')));
      const areas = toAreas(msg.payload);
      if (!areas || areas.some((a) => a.length === 0)) return done(new Error(text(RED, 's7rs-usend.errors.payload')));
      try {
        await ep.ready;
        const t0 = process.hrtime.bigint();
        await ep.bridge.call(ep.id, 'usend', { rid, areas: areas.map((a) => ({ data: a.toString('hex') })) });
        msg.s7 = { rid, bytes: areas.map((a) => a.length), ms: elapsedMs(t0) };
        send(msg);
        done();
      } catch (err) {
        done(err);
      }
    });
  }
  RED.nodes.registerType('s7rs-usend', S7rsUsend);
};
