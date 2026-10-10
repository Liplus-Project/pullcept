import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { WebSocketServer } from 'ws';
import { admitSidecar, ADMISSION_WAIT_MS } from '../src/admission.mjs';

const claim = { launch_id: randomUUID(), account_id: 'synthetic-account', room_id: 'synthetic-topic', instance_id: randomUUID() };
const options = { roomUrl: 'ws://127.0.0.1:1', token: 'synthetic-only', claim };

test('pre-initialize admission waits only for registration and stops at its deadline', async () => {
  let now = 0, calls = 0;
  const dependencies = {
    now: () => now, sleep: async (ms) => { now += ms; },
    request: async (_url, request) => {
      assert.deepEqual(JSON.parse(request.body), claim);
      calls++; return { status: calls < 4 ? 425 : 200, text: async () => '{}' };
    },
  };
  assert.equal(await admitSidecar(options, dependencies), true);
  assert.equal(now, 150);
  now = 0; calls = 0;
  dependencies.request = async () => { calls++; return { status: 425, text: async () => '{}' }; };
  assert.equal(await admitSidecar(options, dependencies), false);
  assert.equal(now, ADMISSION_WAIT_MS);
  assert.equal(calls, 60);
});

test('unknown, stale, unauthorized and failed admissions do not wait or expose errors', async () => {
  for (const status of [401, 404, 409, 500]) {
    let calls = 0;
    assert.equal(await admitSidecar(options, {
      request: async () => { calls++; return { status, text: async () => '{}' }; },
      sleep: async () => assert.fail('unexpected wait'),
    }), false);
    assert.equal(calls, 1);
  }
  assert.equal(await admitSidecar(options, { request: async () => { throw new Error('synthetic secret error'); } }), false);
  assert.equal(await admitSidecar({ ...options, claim: { ...claim, launch_id: null } }), false);
});

test('a hung pre-initialize HTTP request is bounded by the abort deadline', async () => {
  const started = Date.now();
  assert.equal(await admitSidecar(options, {
    request: async (_url, { signal }) => new Promise((_resolve, reject) => {
      const keepAlive = setInterval(() => {}, 100);
      signal.addEventListener('abort', () => { clearInterval(keepAlive); reject(new Error('synthetic abort')); }, { once: true });
    }),
  }), false);
  assert.ok(Date.now() - started >= ADMISSION_WAIT_MS - 50);
  assert.ok(Date.now() - started < ADMISSION_WAIT_MS + 2_000);
});

async function sidecar(t, port, launch, account = 'synthetic-account', topic = 'synthetic-topic', admission = true) {
  // Whitelist process essentials; do not inherit real room/admission credentials.
  const env = Object.fromEntries(['PATH', 'SystemRoot', 'TEMP', 'TMP'].flatMap((key) => process.env[key] ? [[key, process.env[key]]] : []));
  const child = spawn(process.execPath, ['node_modules/tsx/dist/cli.mjs', 'sidecar/src/index.ts'], {
    cwd: new URL('../../', import.meta.url), windowsHide: true,
    env: { ...env, PULLCEPT_ROOM_URL: `ws://127.0.0.1:${port}`, PULLCEPT_ROOM_TOKEN: 'synthetic-only',
      PULLCEPT_LAUNCHED_AS: account, PULLCEPT_ACCOUNT_ID: account,
      PULLCEPT_LAUNCHED_ROOM: topic, PULLCEPT_ROOM_ID: topic,
      PULLCEPT_LAUNCH_ID: admission ? launch : '', PULLCEPT_ROOM_ADMISSION: admission ? '1' : '0', PULLCEPT_AGENT_NAME: 'synthetic-parent' },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  t.after(() => { child.stdin.end(); child.kill(); });
  let buffer = '', stderr = '';
  const pending = new Map();
  child.stderr.on('data', (data) => { stderr += data; });
  child.stdout.on('data', (data) => {
    buffer += data;
    while (buffer.includes('\n')) {
      const end = buffer.indexOf('\n'), line = buffer.slice(0, end); buffer = buffer.slice(end + 1);
      if (!line.trim()) continue;
      const response = JSON.parse(line);
      if (pending.has(response.id)) { pending.get(response.id)(response); pending.delete(response.id); }
    }
  });
  let id = 0;
  const rpc = (method, params = {}) => new Promise((resolve, reject) => {
    const next = ++id;
    const timeout = setTimeout(() => reject(new Error(`synthetic RPC timeout: ${method}`)), 10_000);
    pending.set(next, (response) => { clearTimeout(timeout); resolve(response); });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: next, method, params }) + '\n');
  });
  const initialized = await rpc('initialize', { protocolVersion: '2024-11-05', clientInfo: { name: 'synthetic-client', version: '1' }, capabilities: {} });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  return { initialized, rpc, stderr: () => stderr };
}

async function fakeLauncher(t) {
  const launch = randomUUID();
  const ledger = new Map([[`synthetic-topic/synthetic-account`, { launch, live: true, owner: null }]]);
  const requests = [], connections = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const data of req) raw += data;
    const value = JSON.parse(raw); requests.push(value);
    const seat = ledger.get(`${value.room_id}/${value.account_id}`);
    let status = 409;
    if (req.url === '/room/sidecar-admission' && req.headers.authorization === 'Bearer synthetic-only' &&
        seat?.live && seat.launch === value.launch_id) {
      if (seat.pending) { seat.pending--; status = 425; }
      else if (!seat.owner || seat.owner === value.instance_id) { seat.owner = value.instance_id; status = 200; }
    }
    res.writeHead(status, { 'Content-Type': 'application/json' }); res.end('{}');
  });
  const wss = new WebSocketServer({ server });
  wss.on('connection', (socket, req) => {
    connections.push({ socket, headers: req.headers, hello: null });
    socket.on('message', (raw) => { const value = JSON.parse(raw); if (value.type === 'hello') connections.at(-1).hello = value; });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { for (const client of wss.clients) client.terminate(); wss.close(); server.close(); });
  return { launch, ledger, requests, connections, port: server.address().port };
}

async function until(predicate) {
  for (let i = 0; i < 100; i++) { if (predicate()) return; await new Promise((resolve) => setTimeout(resolve, 20)); }
  assert.fail('synthetic socket condition did not settle');
}

test('real MCP parent admitted before initialize, child empty, same sidecar reconnects', async (t) => {
  const launcher = await fakeLauncher(t);
  launcher.ledger.values().next().value.pending = 2;
  const parent = await sidecar(t, launcher.port, launcher.launch);
  assert.match(parent.initialized.result.instructions, /Pullcept room/);
  assert.equal((await parent.rpc('tools/list')).result.tools.length, 3);
  await until(() => launcher.connections.length === 1);
  const instance = launcher.connections[0].headers['x-pullcept-instance'];
  assert.equal(instance, launcher.requests[0].instance_id);
  assert.equal(launcher.requests.length, 3);
  const child = await sidecar(t, launcher.port, launcher.launch);
  assert.equal(child.initialized.result.instructions, undefined);
  assert.deepEqual((await child.rpc('tools/list')).result.tools, []);
  const denied = await child.rpc('tools/call', { name: 'read_room_history', arguments: {} });
  assert.equal(denied.result.isError, true);
  assert.equal(launcher.connections.length, 1);
  launcher.connections[0].socket.close();
  await until(() => launcher.connections.length === 2);
  assert.equal(launcher.connections[1].headers['x-pullcept-instance'], instance);
  assert.equal((await parent.rpc('tools/list')).result.tools.length, 3);
  assert.ok(!child.stderr().includes('synthetic-only'));
});

test('stale/dead launches rejected and distinct topics/accounts admitted', async (t) => {
  const launcher = await fakeLauncher(t);
  const stale = await sidecar(t, launcher.port, randomUUID());
  assert.equal(stale.initialized.result.instructions, undefined);
  assert.deepEqual((await stale.rpc('tools/list')).result.tools, []);
  launcher.ledger.values().next().value.live = false;
  const dead = await sidecar(t, launcher.port, launcher.launch);
  assert.deepEqual((await dead.rpc('tools/list')).result.tools, []);
  for (const [account, topic] of [['synthetic-account', 'topic-2'], ['account-2', 'synthetic-topic']]) {
    const launch = randomUUID(); launcher.ledger.set(`${topic}/${account}`, { launch, live: true, owner: null });
    const parent = await sidecar(t, launcher.port, launch, account, topic);
    assert.equal((await parent.rpc('tools/list')).result.tools.length, 3);
  }
  await until(() => launcher.connections.length === 2);
  assert.equal(launcher.connections.length, 2);
});


test('legacy Claude and no-account sidecars keep instructions/tools without admission', async (t) => {
  const launcher = await fakeLauncher(t);
  for (const account of ['synthetic-account', '']) {
    const legacy = await sidecar(t, launcher.port, '', account, 'synthetic-topic', false);
    assert.match(legacy.initialized.result.instructions, /Pullcept room/);
    assert.equal((await legacy.rpc('tools/list')).result.tools.length, 3);
  }
  await until(() => launcher.connections.length === 2);
  assert.equal(launcher.requests.length, 0);
  for (const connection of launcher.connections) assert.equal(connection.headers['x-pullcept-launch'], undefined);
});
