// What the status-line reporter sends, and what it draws (#155).
//
// Stands a listener up where the room's would be, runs the script the way the
// CLI does — JSON on stdin, one address on the line — and reads both faces:
// the body that reaches the app, and the line that reaches the terminal.
//
// The two failure shapes are the point of the last two cases. The bar must be
// drawn whether or not the app is there to hear, because the person looking at
// it is looking at a terminal; and the body must go over as it arrived, because
// the app is what reads the fields out of it (#155, decision 3).
//
// Run: npm run sidecar:test
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, "..", "src", "status.mjs");

const TIMEOUT = 20_000;

/** The seat this launch was made for. Opaque, as it is in the app. */
const ROOM = "5d41402a-bc4b-4a76-b971-9d911017c592";
const ACCOUNT = "8f14e45f-ceea-467a-b160-6f14e45fceea";
const TOKEN = "test-room-token";

/** One session's JSON, in the shape the CLI hands a status line. */
const SESSION = {
  session_id: "0f5a0000-0000-4000-8000-000000000000",
  model: { id: "claude-opus-5", display_name: "Opus" },
  context_window: { context_window_size: 200000, used_percentage: 8.4 },
  effort: { level: "high" },
  rate_limits: {
    five_hour: { used_percentage: 23.5, resets_at: 1738425600 },
    seven_day: { used_percentage: 41.2, resets_at: 1738857600 },
  },
};

/** A listener that answers the way the room's own does, and records one POST. */
async function fakeRoom() {
  const received = [];
  const server = createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      received.push({
        target: request.url,
        authorization: request.headers.authorization,
        contentType: request.headers["content-type"],
        body: Buffer.concat(chunks).toString("utf8"),
      });
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end("{}");
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  return { received, port, close: () => new Promise((done) => server.close(done)) };
}

/** Run the script the way the CLI does, and answer with what it printed. */
function run(url, input) {
  const child = spawn(process.execPath, url ? [SCRIPT, url] : [SCRIPT], {
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, PULLCEPT_ROOM_TOKEN: TOKEN },
  });
  let out = "";
  child.stdout.on("data", (chunk) => (out += chunk));
  child.stdin.end(input);
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, out }));
  });
}

test("the session's own JSON reaches the seat's address, and one line reaches the bar", { timeout: TIMEOUT }, async () => {
  const room = await fakeRoom();
  try {
    const url = `http://127.0.0.1:${room.port}/hooks/status/${ROOM}/${ACCOUNT}`;
    const { code, out } = await run(url, JSON.stringify(SESSION));

    assert.equal(code, 0);
    assert.equal(room.received.length, 1);
    const post = room.received[0];
    // The seat is in the address. The JSON names the CLI's session id, which is
    // not what a seat is keyed on.
    assert.equal(post.target, `/hooks/status/${ROOM}/${ACCOUNT}`);
    // The token is presented, and it came out of the environment the launch
    // set — it is not on the line the person can see.
    assert.equal(post.authorization, `Bearer ${TOKEN}`);
    assert.equal(post.contentType, "application/json");
    // As it arrived. Nothing is picked out on this side: the app reads the
    // fields, so a second reader here would be a second thing to keep in step
    // with the CLI.
    assert.deepEqual(JSON.parse(post.body), SESSION);

    // The five values the panel shows, in the panel's order, as whole percents.
    assert.equal(out, "Opus · high · 5h 24% · 7d 41% · ctx 8%\n");
  } finally {
    await room.close();
  }
});

test("a value the CLI did not send is left out rather than drawn blank", { timeout: TIMEOUT }, async () => {
  const room = await fakeRoom();
  try {
    const url = `http://127.0.0.1:${room.port}/hooks/status/${ROOM}/${ACCOUNT}`;
    // No `rate_limits` and no `effort`: an account off a claude.ai plan, on a
    // model with no effort parameter. `5h —%` would be a claim about a limit
    // that does not apply to this session.
    const { out } = await run(
      url,
      JSON.stringify({ model: { display_name: "Opus" }, context_window: { used_percentage: 8 } }),
    );
    assert.equal(out, "Opus · ctx 8%\n");
  } finally {
    await room.close();
  }
});

test("the bar is drawn even when nothing is listening", { timeout: TIMEOUT }, async () => {
  // The app closed while the session kept running. The person is looking at a
  // terminal, and an empty bar reads as a status line that is broken.
  const room = await fakeRoom();
  const port = room.port;
  await room.close();
  const { code, out } = await run(
    `http://127.0.0.1:${port}/hooks/status/${ROOM}/${ACCOUNT}`,
    JSON.stringify(SESSION),
  );
  assert.equal(code, 0);
  assert.equal(out, "Opus · high · 5h 24% · 7d 41% · ctx 8%\n");
});

test("input that is not JSON still goes over, and still draws a line", { timeout: TIMEOUT }, async () => {
  const room = await fakeRoom();
  try {
    const url = `http://127.0.0.1:${room.port}/hooks/status/${ROOM}/${ACCOUNT}`;
    const { code, out } = await run(url, "not json at all");
    assert.equal(code, 0);
    assert.equal(room.received[0].body, "not json at all");
    // Named, so the bar says which thing put it there.
    assert.equal(out, "pullcept\n");
  } finally {
    await room.close();
  }
});
