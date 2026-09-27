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

/** A worker's `/events` socket on loopback, which hands over whoever connects. */
async function fakeWorker() {
  const server = new WebSocketServer({ host: "127.0.0.1", port: 0, path: "/events" });
  await new Promise((resolve) => server.on("listening", resolve));
  const connected = new Promise((resolve) =>
    server.on("connection", (socket, request) => {
      socket.send(JSON.stringify({ status: "connected" }));
      resolve({ socket, authorization: request.headers.authorization });
    }),
  );
  return {
    url: `http://127.0.0.1:${server.address().port}`,
    connected,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

/** Start the receiver against `worker`, and read its stdout one message per line. */
function startBridge(worker) {
  const home = mkdtempSync(join(tmpdir(), "pullcept-webhook-"));
  const tokens = join(home, ".github-webhook-mcp");
  mkdirSync(tokens);
  writeFileSync(
    join(tokens, "oauth-tokens.json"),
    JSON.stringify({ flow: "web", access_token: "test-access-token" }),
  );

  const child = spawn(process.execPath, [SCRIPT], {
    env: { ...process.env, HOME: home, USERPROFILE: home, WEBHOOK_WORKER_URL: worker.url },
    stdio: ["pipe", "pipe", "pipe"],
  });
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
  const exited = new Promise((resolve) => child.on("exit", (code) => resolve(code)));
  return {
    child,
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
