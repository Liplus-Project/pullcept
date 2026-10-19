// The app's webhook receiver, run the way the app runs it (#169).
//
// Three things the app rests on, all about the bridge as it is installed rather
// than as its source reads:
//
//   - it declares `claude/channel` to a client that is not a CLI. The app's
//     whole reception is that declaration being made to whoever connects.
//   - an event reaching its WebSocket comes out on stdout as
//     `notifications/claude/channel`, with the event id in `meta.message_id`.
//     The app reads that id to name the event; it calls no tool with it (#180).
//   - it ends when its stdin does, with its WebSocket open. The bridge alone
//     does not; the wrapper is what adds it, and a receiver that outlived the
//     app would keep taking events nobody shows.
//
// A worker stands in on loopback (`WEBHOOK_WORKER_URL`), and the home directory
// is a fresh one holding a token file of the shape the bridge writes, so the
// bridge opens its WebSocket at startup the way it does on an authorised
// machine. Nothing here reaches the network, and no real token is read.
//
// Run: npm run sidecar:test
import { test } from "node:test";
import { createServer } from "node:http";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { WebSocketServer } from "ws";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, "..", "src", "webhook-bridge.mjs");

const TIMEOUT = 20_000;

/** A loopback worker with raw status RPC and an `/events` socket. */
async function fakeWorker({ holdStatus = false } = {}) {
  const calls = [];
  let statusStarted;
  const statusRequest = new Promise(resolve => { statusStarted = resolve; });
  const snapshot = { pending_count: 3, types: { issues: 3 }, latest_received_at: "2026-10-19T00:00:00Z" };
  const http = createServer(async (req, res) => {
    let body = "";
    for await (const chunk of req) body += chunk;
    if (!body) { res.writeHead(200).end(); return; }
    const rpc = JSON.parse(body);
    calls.push({ path: req.url, method: rpc.method, tool: rpc.params?.name });
    if (holdStatus && rpc.method === "tools/call") {
      statusStarted({ closed: new Promise(resolve => req.socket.once("close", resolve)) });
      return;
    }
    const result = rpc.method === "server/discover"
      ? { supportedVersions: ["2026-07-28"], capabilities: { tools: {} } }
      : { resultType: "complete", content: [{ type: "text", text: JSON.stringify(snapshot) }], structuredContent: snapshot };
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ jsonrpc: "2.0", id: rpc.id, result }));
  });
  const server = new WebSocketServer({ server: http, path: "/events" });
  await new Promise((resolve) => http.listen(0, "127.0.0.1", resolve));
  const connected = new Promise((resolve) =>
    server.on("connection", (socket, request) => {
      socket.send(JSON.stringify({ status: "connected" }));
      resolve({ socket, authorization: request.headers.authorization });
    }),
  );
  return {
    calls, snapshot, statusRequest,
    url: `http://127.0.0.1:${http.address().port}`,
    connected,
    close: () => new Promise((resolve) => server.close(() => http.close(resolve))),
  };
}

/** Start the receiver against `worker`, and read its stdout one message per line. */
function startBridge(worker, extraEnv = {}, nodeArgs = []) {
  const home = mkdtempSync(join(tmpdir(), "pullcept-webhook-"));
  const tokens = join(home, ".github-webhook-mcp");
  mkdirSync(tokens);
  writeFileSync(
    join(tokens, "oauth-tokens.json"),
    JSON.stringify({ flow: "web", access_token: "test-access-token" }),
  );

  const child = spawn(process.execPath, [...nodeArgs, SCRIPT], {
    cwd: tmpdir(),
    env: { ...process.env, HOME: home, USERPROFILE: home, WEBHOOK_WORKER_URL: worker.url, ...extraEnv },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", chunk => { stderr += chunk; });
  const waiting = [];
  const unmatched = [];
  let buffer = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let at;
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at).trim();
      buffer = buffer.slice(at + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      const index = waiting.findIndex((w) => w.match(message));
      if (index >= 0) waiting.splice(index, 1)[0].resolve(message);
      else unmatched.push(message);
    }
  });
  const next = (match) => {
    const found = unmatched.findIndex(match);
    if (found >= 0) return Promise.resolve(unmatched.splice(found, 1)[0]);
    return new Promise((resolve) => waiting.push({ match, resolve }));
  };
  const exited = new Promise((resolve) => child.on("close", (code) => resolve(code)));
  return {
    child,
    stderr: () => stderr,
    exited,
    next,
    request(id, method, params) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      return next((message) => message.id === id);
    },
    cleanup() {
      child.kill();
      rmSync(home, { recursive: true, force: true });
    },
  };
}

const INITIALIZE = {
  protocolVersion: "2025-06-18",
  capabilities: {},
  clientInfo: { name: "pullcept", version: "0.1.0" },
};

test("the bridge declares claude/channel to a client that is not a CLI", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker();
  const bridge = startBridge(worker);
  try {
    const answer = await bridge.request(1, "initialize", INITIALIZE);
    assert.equal(answer.error, undefined);
    assert.equal(answer.result.serverInfo.name, "github-webhook-mcp");
    assert.deepEqual(answer.result.capabilities.experimental, { "claude/channel": {} });
    const tools = await bridge.request(2, "tools/list", {});
    assert.equal(tools.result.tools.find(tool => tool.name === "get_pending_status")._meta?.["io.github.Liplus-Project/github-webhook-mcp/pending-status-observer"], undefined);
  } finally {
    bridge.cleanup();
    await worker.close();
  }
});

test("an event on the socket comes out as a channel push carrying its id", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker();
  const bridge = startBridge(worker);
  try {
    await bridge.request(1, "initialize", INITIALIZE);
    // Opened at startup, off the token file alone: no tool call has been made.
    const { socket, authorization } = await worker.connected;
    assert.equal(authorization, "Bearer test-access-token");

    socket.send(
      JSON.stringify({
        event_id: "delivery-1",
        type: "issues",
        summary: {
          id: "delivery-1",
          type: "issues",
          received_at: "2026-09-27T00:00:00.000Z",
          action: "opened",
          repo: "Liplus-Project/pullcept",
          sender: "someone",
          number: 169,
          title: "a title",
          url: "https://github.com/Liplus-Project/pullcept/issues/169",
        },
      }),
    );

    const push = await bridge.next((message) => message.method === "notifications/claude/channel");
    assert.equal(push.params.meta.message_id, "delivery-1");
    assert.match(push.params.content, /\[issues\] Liplus-Project\/pullcept/);
    assert.match(push.params.content, /#169 a title/);
  } finally {
    bridge.cleanup();
    await worker.close();
  }
});

test("the receiver ends when its stdin does, with its socket open", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker();
  const bridge = startBridge(worker);
  try {
    await bridge.request(1, "initialize", INITIALIZE);
    await worker.connected;
    bridge.child.stdin.end();
    const code = await Promise.race([
      bridge.exited,
      new Promise((resolve) => setTimeout(() => resolve("still running"), 5_000)),
    ]);
    assert.equal(code, 0);
  } finally {
    bridge.cleanup();
    await worker.close();
  }
});


test("the published observer advertises raw snapshots without opening a channel", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker();
  let channels = 0; worker.connected.then(() => channels++);
  const bridge = startBridge(worker, {WEBHOOK_OBSERVER:"1",WEBHOOK_CHANNEL:"0"});
  try {
    await bridge.request(1,"initialize",INITIALIZE);
    const tools = await bridge.request("pullcept:observer:tools","tools/list",{});
    const pending=tools.result.tools.find(t=>t.name==="get_pending_status");
    assert.ok(pending);
    assert.equal(pending._meta?.["io.github.Liplus-Project/github-webhook-mcp/pending-status-observer"],1);
    const answer = await bridge.request("pullcept:observer:status", "tools/call", { name: "get_pending_status", arguments: {} });
    assert.equal(answer.error, undefined);
    assert.deepEqual(answer.result.structuredContent, worker.snapshot);
    assert.deepEqual(JSON.parse(answer.result.content[0].text), worker.snapshot);
    Object.assign(worker.snapshot, { pending_count: 0, types: {}, latest_received_at: null });
    const empty = await bridge.request("pullcept:observer:empty", "tools/call", { name: "get_pending_status", arguments: {} });
    assert.deepEqual(empty.result.structuredContent, worker.snapshot);
    assert.ok(worker.calls.length > 0);
    assert.ok(worker.calls.every(call => call.path === "/mcp" && (call.method === "server/discover" || (call.method === "tools/call" && call.tool === "get_pending_status"))));
    assert.equal(channels,0);
    bridge.child.stdin.end();
    assert.equal(await Promise.race([bridge.exited,new Promise(resolve=>setTimeout(()=>resolve("running"),5000))]),0, bridge.stderr());
  } finally { bridge.cleanup(); await worker.close(); }
});

test("observer EOF cancels a pending HTTP snapshot and closes its connection", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker({ holdStatus: true });
  const bridge = startBridge(worker, { WEBHOOK_OBSERVER: "1", WEBHOOK_CHANNEL: "0" });
  try {
    await bridge.request(1, "initialize", INITIALIZE);
    void bridge.request(2, "tools/call", { name: "get_pending_status", arguments: {} });
    const { closed } = await worker.statusRequest;
    bridge.child.stdin.end();
    assert.equal(await Promise.race([
      bridge.exited,
      new Promise(resolve => setTimeout(() => resolve("still running"), 5_000)),
    ]), 0, bridge.stderr());
    await closed;
  } finally {
    bridge.cleanup();
    await worker.close();
  }
});

test("unexpected bridge worker errors and nonzero exits remain failures", { timeout: TIMEOUT }, async () => {
  const worker = await fakeWorker();
  try {
    for (const [statement, expectedCode] of [["throw new Error('test worker failure')", 1], ["process.exit(7)", 7]]) {
      // Worker inherits the preload; only its copy fails. No installed package is changed.
      const preload = `data:text/javascript,${encodeURIComponent(`import { isMainThread } from 'node:worker_threads'; if (!isMainThread) { ${statement}; }`)}`;
      const bridge = startBridge(worker, {}, ["--import", preload]);
      try {
        assert.equal(await bridge.exited, expectedCode);
        if (expectedCode === 1) assert.match(bridge.stderr(), /test worker failure/);
      } finally { bridge.cleanup(); }
    }
  } finally { await worker.close(); }
});
