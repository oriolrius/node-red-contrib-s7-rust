'use strict';
// s7rs-endpoint: a connection to one Siemens S7 CPU (config node). The protocol
// runs in Rust inside this process (lib/napi-bridge.js); the flow nodes only
// route messages.

const { acquire, release } = require('../lib/napi-bridge');
const { text, intOrUndef } = require('../lib/common');

// Diagnostics the editor may call through the admin API (read-only actions).
const ADMIN_ACTIONS = new Set([
  'identify', 'cpuState', 'szl', 'szlList', 'clock', 'blocks', 'listBlocks', 'blockInfo',
  'info', 'certificate', 'limits', 'dbs', 'browse', 'alarms',
]);

module.exports = function (RED) {
  const hooked = new WeakSet();

  function hookLogs(bridge) {
    if (hooked.has(bridge)) return;
    hooked.add(bridge);
    bridge.on('log', (level, text) => {
      const f = { error: 'error', warn: 'warn', debug: 'debug', trace: 'trace' }[level] || 'info';
      RED.log[f](`[s7rs] ${text}`);
    });
  }

  function S7rsEndpoint(config) {
    RED.nodes.createNode(this, config);
    const node = this;
    node.bridge = acquire();
    hookLogs(node.bridge);
    node.state = { state: 'connecting', detail: '' };
    node.setMaxListeners(0);

    const onStatus = (ev) => {
      if (ev.conn !== node.id) return;
      node.state = ev;
      node.emit('s7status', ev);
    };
    node.bridge.on('status', onStatus);

    const creds = node.credentials || {};
    const params = {
      conn: node.id,
      protocol: config.protocol || 's7',
      logLevel: config.logLevel || 'warn',
      tsap: config.tsap || '',
      tls: config.tls === true || config.tls === 'true',
      certVerify: config.certVerify || 'none',
      certFingerprint: config.certFingerprint || '',
      certTrusted: config.certTrusted || '',
      certName: String(config.certName || '').trim(),
      username: config.username || '',
      password: creds.password || '',
      host: String(config.host || '').trim(),
      port: intOrUndef(config.port) || 102,
      cpu: config.cpu || '',
      rack: intOrUndef(config.rack),
      slot: intOrUndef(config.slot),
      link: intOrUndef(config.link),
      localTsap: config.localTsap || '',
      remoteTsap: config.remoteTsap || '',
      pdu: intOrUndef(config.pdu) || 0,
      timeoutMs: intOrUndef(config.timeout) || 5000,
      attempts: intOrUndef(config.attempts) || 0,
      reconnectMs: intOrUndef(config.reconnect) || 5000,
      gap: intOrUndef(config.gap),
      littleEndian: config.byteOrder === 'little',
    };
    node.ready = node.bridge.open(params).catch((err) => {
      node.error(text(RED, 's7rs-endpoint.errors.open', { host: params.host, error: err.message }));
      node.state = { state: 'faulted', detail: err.message };
      node.emit('s7status', node.state);
      throw err;
    });
    node.ready.catch(() => {}); // flow nodes await it and report the error themselves

    node.on('close', (removed, done) => {
      node.bridge.off('status', onStatus);
      node.bridge
        .close(node.id)
        .catch(() => {})
        .finally(() => {
          release();
          done();
        });
    });
  }
  RED.nodes.registerType('s7rs-endpoint', S7rsEndpoint, { credentials: { password: { type: 'password' } } });

  // Admin API for the editor:
  //   GET /s7rs/status/:id         connection diagnostics
  //   GET /s7rs/call/:id/:action   a read-only info action (e.g. browse = S7CommPlus symbols)
  const deployed = (req, res) => {
    const ep = RED.nodes.getNode(req.params.id);
    if (!ep || !ep.bridge) {
      res.status(404).json({ error: 'endpoint not deployed' });
      return null;
    }
    return ep;
  };

  RED.httpAdmin.get('/s7rs/call/:id/:action', RED.auth.needsPermission('s7rs-endpoint.read'), async (req, res) => {
    const ep = deployed(req, res);
    if (!ep) return;
    const { action } = req.params;
    if (!ADMIN_ACTIONS.has(action)) return res.status(400).json({ error: `action ${action} not allowed here` });
    try {
      await ep.ready;
      const args = req.query && Object.keys(req.query).length ? req.query : undefined;
      res.json(await ep.bridge.call(ep.id, action, args));
    } catch (err) {
      res.status(500).json({ error: err.message });
    }
  });

  RED.httpAdmin.get('/s7rs/status/:id', RED.auth.needsPermission('s7rs-endpoint.read'), async (req, res) => {
    const ep = deployed(req, res);
    if (!ep) return;
    try {
      res.json(await ep.bridge.status(ep.id));
    } catch (err) {
      res.status(500).json({ error: err.message });
    }
  });
};
