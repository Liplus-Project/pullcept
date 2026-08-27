// Frame-level round trip for the room sidecar.
//
// Stands a fake room socket up, spawns the sidecar the way a CLI would, and
// drives both faces: MCP over stdio, room frames over WebSocket. This is the
// isolation harness for the round trip — when the real app stops delivering,
// running this says whether the sidecar or the app side moved.
//
// The fake room answers posts, because the real one does: since #47 a post is
// a request the room replies to with `post_result`, and a room that never
// answers is a room the sidecar reports as unconfirmed.
//
// Run: npm run sidecar:test
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { WebSocketServer } from "ws";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const ENTRY = join(HERE, "..", "src", "index.ts");
const REPO = join(HERE, "..", "..");

const TIMEOUT = 20_000;

/**
 * The account this session is launched as. Opaque, as it is in the app: an id
 * a name could be read out of would be the wrong thing to carry (#53).
 */
const TEST_ACCOUNT = "8f14e45f-ceea-467a-b160-6f14e45fceea";

// The manners, whole.
//
// Asserted as complete literals rather than by a regex on the opening clause.
// The head-only form was checked here before and did not hold: both turn-taking
// assertions matched only up to the first comma, so every sentence after it —
// including the one being rewritten — could be deleted with CI still green
// (#47). A test that stops at the first clause is testing that a heading
// exists. What these paragraphs claim is in the tail.
const TURN_TAKING = [
  "- 先に誰かが答えていたら、その発言を読んでから自分の発言を決めてください。",
  "  全体宛の問いに、全員が答える必要はありません。",
  "- 送る直前に、届いている発言をもう一度見てください。組み立てている間にも",
  "  発言は届きます。言おうとしていたことが既に言われていたら送らず、",
  "  足りないことがあるときだけ足してください。",
].join("\n");

// Looking back. The room hands a late joiner nothing, by design, so the whole
// of what makes the read reachable is that the manners name it and say when it
// is worth calling (#115, decision 4C). Asserted in full for the reason the two
// above are: a head-only check passes on a paragraph whose tail was deleted.
const LOOKING_BACK = [
  "前を見る:",
  "- あなたが来る前の発言は届きません。部屋は過去を配らないからです。",
  "- 必要になったら read_room_history を呼んでください。今のトピックで",
  "  それまでに言われたことが、古い順で返ります。",
  "- 押し付けられないので、要らないときは呼ばないでください。話の流れが",
  "  分からないまま答えそうなときにだけ引けば足ります。",
  "- 返り切らなかったときは、いちばん古い発言の message_id を before に",
  "  入れてもう一度呼ぶと、その手前が返ります。",
].join("\n");

const SEE_THE_FLOOR = [
  "床を見てから送る:",
  "- say_to_room には last_seen を付けてください。値は、あなたが実際に見た",
  "  いちばん新しい発言の meta.message_id です。まだ何も見ていないときだけ",
  "  省いてください。",
  "- 組み立てている間に届いた発言があると、部屋はあなたの発言を配りません。",
  "  代わりに、あなたが見ていなかった発言を返します。あなたの発言は部屋に",
  "  載っていません。",
  "- 返ってきた発言を読んでから、もう一度決めてください。言おうとしていた",
  "  ことが既に言われていたら送らないでください。送らない判断は正当です。",
  "- それでも足すことがあるときは、返ってきたうちいちばん新しい message_id を",
  "  last_seen に入れて、もう一度 say_to_room を呼んでください。",
  "- 弾かれるのは、あなたの注意が足りなかったからではありません。二人が同時に",
  "  書き始めたとき、順序を付けられるのは部屋だけです。これはその順序です。",
].join("\n");

/** Whole-literal containment, with both sides shown when it fails. */
function assertContains(haystack, needle, message) {
  assert.ok(
    haystack.includes(needle),
    `${message}\n--- expected to contain ---\n${needle}\n--- actual ---\n${haystack}`,
  );
}

function deferred() {
  let resolve;
  const promise = new Promise((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** Wait for a value, or fail the test with `label` instead of hanging. */
function withTimeout(promise, label) {
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      // unref: a losing race must not hold the event loop open to its deadline.
      setTimeout(() => reject(new Error(`timed out waiting for ${label}`)), TIMEOUT).unref();
    }),
  ]);
}

test("a room post reaches the channel, and say_to_room reaches the room", async (t) => {
  // ── fake room ──────────────────────────────────────────────────────────────
  const http = createServer();
  const wss = new WebSocketServer({ server: http });
  await new Promise((r) => http.listen(0, "127.0.0.1", r));
  const port = http.address().port;

  const connected = deferred();
  const helloSeen = deferred();
  const postFrames = [];
  const postWaiters = [];
  let roomSocket = null;

  // How the fake room answers the next post. "deliver" takes it, "refuse"
  // hands back what the speaker had not seen, "silent" answers nothing.
  let answer = "deliver";

  // What a refusal carries. Two posts, one of them addressed elsewhere: the
  // room does not narrow the refusal by addressee, so both come back.
  const MISSED = [
    {
      message_id: "m-9",
      speaker: "Claude Lay",
      content: "先に答えました",
      ts: "2026-08-21T00:00:04.000Z",
    },
    {
      message_id: "m-10",
      speaker: "Master",
      content: "レイに任せる",
      to: "Claude Lay",
      ts: "2026-08-21T00:00:05.000Z",
    },
  ];

  // What the topic held before this session joined. The room delivers none of
  // it live — a later joiner missed it — so the only way it reaches the agent
  // is the pull (#115, decision 4C).
  const PAST = [
    {
      message_id: "h-1",
      speaker: "Master",
      content: "この件は昨日決めた",
      ts: "2026-08-26T00:00:00.000Z",
    },
    {
      message_id: "h-2",
      speaker: "Claude Lay",
      content: "了解しました",
      to: "Master",
      ts: "2026-08-26T00:00:01.000Z",
    },
  ];
  const historyFrames = [];

  wss.on("connection", (socket) => {
    roomSocket = socket;
    connected.resolve(socket);
    socket.on("message", (raw) => {
      const frame = JSON.parse(raw.toString());
      if (frame.type === "hello") helloSeen.resolve(frame);
      if (frame.type === "history") {
        historyFrames.push(frame);
        // An answer for a pull nobody made, sent first. The call must not
        // settle on it: pulls are correlated by request_id, the way posts are
        // by message_id, and arrival order says nothing.
        socket.send(
          JSON.stringify({
            type: "history_result",
            request_id: "not-this-pull",
            posts: [],
            has_more: false,
          }),
        );
        socket.send(
          JSON.stringify({
            type: "history_result",
            request_id: frame.request_id,
            posts: PAST,
            has_more: true,
          }),
        );
        return;
      }
      if (frame.type !== "post") return;

      postFrames.push(frame);
      for (const waiter of postWaiters.splice(0)) waiter();

      if (answer === "silent") return;
      if (answer === "refuse") {
        // A verdict for a post nobody made, sent first and saying delivered.
        // The call must not settle on it: answers are correlated by
        // message_id, not by arrival order.
        socket.send(
          JSON.stringify({
            type: "post_result",
            message_id: "not-this-post",
            delivered: true,
            missed: [],
          }),
        );
        socket.send(
          JSON.stringify({
            type: "post_result",
            message_id: frame.message_id,
            delivered: false,
            missed: MISSED,
          }),
        );
        return;
      }
      socket.send(
        JSON.stringify({
          type: "post_result",
          message_id: frame.message_id,
          delivered: true,
          missed: [],
        }),
      );
    });
  });

  /** The `index`-th post frame the room received, awaited if not yet there. */
  function nextPost(index = 0) {
    if (postFrames.length > index) return Promise.resolve(postFrames[index]);
    return withTimeout(
      new Promise((resolve) => {
        const waiter = () => {
          if (postFrames.length > index) resolve(postFrames[index]);
          else postWaiters.push(waiter);
        };
        postWaiters.push(waiter);
      }),
      `post frame #${index}`,
    );
  }

  // ── sidecar, spawned the way the CLI would ─────────────────────────────────
  const child = spawn(
    process.execPath,
    [join(REPO, "node_modules", "tsx", "dist", "cli.mjs"), ENTRY],
    {
      cwd: REPO,
      env: {
        ...process.env,
        PULLCEPT_ROOM_URL: `ws://127.0.0.1:${port}`,
        PULLCEPT_AGENT_NAME: "test-agent",
        PULLCEPT_AGENT_HUE: "145",
        PULLCEPT_ACCOUNT_ID: TEST_ACCOUNT,
        PULLCEPT_ROOM_ID: "test-room",
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );

  const stderr = [];
  child.stderr.on("data", (b) => stderr.push(b.toString()));

  t.after(() => {
    child.kill();
    // Callbacks, because the body closes these too: a bare close on a server
    // already shut down emits an unhandled error event.
    wss.close(() => {});
    http.close(() => {});
  });

  // ── MCP stdio plumbing: one JSON-RPC message per line ──────────────────────
  const pending = new Map();
  const notifications = [];
  const notificationWaiters = [];
  let buffer = "";

  child.stdout.on("data", (chunk) => {
    buffer += chunk.toString();
    let nl;
    while ((nl = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, nl).trim();
      buffer = buffer.slice(nl + 1);
      if (!line) continue;
      const msg = JSON.parse(line);
      if (msg.id !== undefined && pending.has(msg.id)) {
        pending.get(msg.id).resolve(msg);
        pending.delete(msg.id);
      } else if (msg.method) {
        notifications.push(msg);
        for (const w of notificationWaiters.splice(0)) w(msg);
      }
    }
  });

  let nextId = 1;
  function request(method, params) {
    const id = nextId++;
    const d = deferred();
    pending.set(id, d);
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    return withTimeout(d.promise, `response to ${method}`);
  }
  function notify(method, params) {
    child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
  }
  /** The `index`-th notification of `method`, awaited if it has not arrived. */
  function nextNotification(method, index = 0) {
    const matching = () => notifications.filter((n) => n.method === method);
    if (matching().length > index) return Promise.resolve(matching()[index]);
    return withTimeout(
      new Promise((resolve) => {
        const waiter = () => {
          const seen = matching();
          if (seen.length > index) resolve(seen[index]);
          else notificationWaiters.push(waiter);
        };
        notificationWaiters.push(waiter);
      }),
      `${method} #${index}`,
    );
  }

  // ── initialize: the capability and the manners both ride on this ───────────
  const init = await request("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "round-trip-test", version: "0" },
  });

  assert.ok(
    init.result.capabilities.experimental?.["claude/channel"],
    "server must declare the claude/channel experimental capability",
  );
  const instructions = init.result.instructions ?? "";
  assert.match(instructions, /say_to_room/, "instructions must name the posting tool");
  // The manners and the material they are judged on ship together. Manners
  // that say "answer what is addressed to you" without naming where the
  // addressee is, or without naming what this agent is called, ask for a
  // judgment the agent has nothing to make.
  assert.match(
    instructions,
    /meta\.to/,
    "instructions must name the addressee as judgment material",
  );
  assert.match(
    instructions,
    /test-agent/,
    "instructions must tell the agent the name it answers to",
  );
  // The model lives in the manners as much as in the frames. An agent told to
  // answer "the human" would be reading a distinction the protocol no longer
  // carries (#39).
  assert.match(
    instructions,
    /人間と AI を区別しません/,
    "instructions must state that participants are not split into human and AI",
  );
  // Turn-taking. The addressee clauses filter who a message is for; these say
  // what to do when someone already answered. Both halves are required: read
  // the earlier answer before deciding, and look again at what arrived while
  // the message was being composed, since the composing agent cannot see the
  // floor and the arrivals are all it has to look at (#49).
  assertContains(
    instructions,
    TURN_TAKING,
    "instructions must carry the turn-taking manners in full, tail included",
  );
  // Seeing the floor. These are not advice: `last_seen` is what the room
  // judges the post on, and a refusal is a state the agent has to know how to
  // leave. An agent that does not know to send the watermark is refused on
  // every post after its first; one that does not know a refusal means "not
  // posted" repeats itself blind (#47).
  assertContains(
    instructions,
    SEE_THE_FLOOR,
    "instructions must carry the floor manners in full, tail included",
  );
  // The pull. A tool nobody is told about is a tool nobody calls: the room
  // still delivers nothing that predates a seat, so a session that joined a
  // topic late learns what it missed only by knowing to go and ask (#115,
  // decision 4C).
  assertContains(
    instructions,
    LOOKING_BACK,
    "instructions must carry the looking-back manners in full, tail included",
  );

  notify("notifications/initialized", {});

  const tools = await request("tools/list", {});
  const toolNames = tools.result.tools.map((tool) => tool.name);
  // One way to speak, one way to look back. The constraint that held the count
  // at one is about *posting*: a second way to be heard would put "which one do
  // I answer through" back on the agent. `read_room_history` cannot post, so it
  // does not sit on that axis (#115, decision 4C).
  assert.deepEqual(
    toolNames,
    ["say_to_room", "read_room_history"],
    "one posting tool and one reading tool, and nothing else",
  );
  assert.equal(
    toolNames.filter((name) => name === "say_to_room").length,
    1,
    "exactly one posting tool is exposed",
  );
  // Seeing the floor is an argument of the posting tool, not a tool of its own.
  // The watermark is a claim about what the speaker saw, made at the moment of
  // speaking; split into its own call it would be a claim about a moment that
  // has already passed by the time the post goes out (#47).
  const schema = tools.result.tools[0].inputSchema;
  assert.deepEqual(
    Object.keys(schema.properties).sort(),
    ["content", "last_seen", "to"],
    "the watermark rides on say_to_room rather than adding a tool",
  );
  assert.deepEqual(
    schema.required,
    ["content"],
    "the watermark is optional: a participant that has seen nothing must still be able to speak",
  );

  // ── room -> agent ──────────────────────────────────────────────────────────
  await withTimeout(connected.promise, "sidecar to connect to the room");
  const hello = await withTimeout(helloSeen.promise, "hello frame");
  // Who this session is in the room, declared at the moment of joining: the
  // name it answers to, and the hue it is drawn in. Both come from the launch,
  // not from a stored tab attribute — a stored one made every session answer to
  // the same name (#40).
  assert.equal(hello.name, "test-agent");
  assert.equal(hello.hue, 145);
  // The account this session was launched as, carried so the screen can join
  // its own account list against the room's roster by id rather than by name
  // (#59). It rides on `hello` and decides nothing: identity in the room is the
  // connection, and this frame cannot set that (#39 / #40).
  assert.equal(hello.account_id, TEST_ACCOUNT);
  assert.equal(hello.protocol, 6);

  roomSocket.send(
    JSON.stringify({
      type: "post",
      message_id: "m-1",
      speaker: "Master",
      content: "聞こえる？",
      ts: "2026-08-21T00:00:00.000Z",
    }),
  );

  const pushed = await nextNotification("notifications/claude/channel");
  // Body only: the speaker belongs in meta, and the host renders it. Mixing it
  // into the body showed the name twice on screen (#28).
  assert.equal(pushed.params.content, "聞こえる？");
  assert.equal(pushed.params.meta.chat_id, "test-room");
  assert.equal(pushed.params.meta.message_id, "m-1");
  assert.equal(pushed.params.meta.user, "Master");
  // An unaddressed utterance is the room as a whole. No key, rather than an
  // empty one: an agent testing `meta.to` must not read "" as a name.
  assert.equal(
    "to" in pushed.params.meta,
    false,
    "an utterance with no addressee must carry no `to`",
  );

  // ── the addressee rides through to the agent ───────────────────────────────
  roomSocket.send(
    JSON.stringify({
      type: "post",
      message_id: "m-2",
      speaker: "Master",
      content: "リンだけ答えて",
      to: "test-agent",
      ts: "2026-08-21T00:00:01.000Z",
    }),
  );

  const addressed = await nextNotification("notifications/claude/channel", 1);
  // In meta, next to the speaker, for the same reason: the body stays equal to
  // what was said.
  assert.equal(addressed.params.content, "リンだけ答えて");
  assert.equal(addressed.params.meta.to, "test-agent");

  // Addressed elsewhere still arrives — the room delivers to everyone and the
  // agent decides. Filtering here would put "who heard it" in the room.
  roomSocket.send(
    JSON.stringify({
      type: "post",
      message_id: "m-3",
      speaker: "Master",
      content: "レイはどう思う",
      to: "other-agent",
      ts: "2026-08-21T00:00:02.000Z",
    }),
  );

  const elsewhere = await nextNotification("notifications/claude/channel", 2);
  assert.equal(elsewhere.params.meta.to, "other-agent");

  // ── this participant -> room ───────────────────────────────────────────────
  const call = await request("tools/call", {
    name: "say_to_room",
    arguments: { content: "聞こえてるわ", to: "Master", last_seen: "m-3" },
  });
  assert.ok(!call.result.isError, `tool call failed: ${JSON.stringify(call.result)}`);
  assert.equal(
    call.result.content[0].text,
    "Delivered to the room.",
    "a post the room admits reads as delivered and says nothing else",
  );

  const post = await nextPost(0);
  assert.equal(post.type, "post");
  assert.equal(post.content, "聞こえてるわ");
  // A person is addressed exactly like a session. One vocabulary, one frame.
  assert.equal(post.to, "Master");
  // The watermark the agent declared, carried through unchanged. This side
  // cannot check it and must not invent it: what the agent saw is the one
  // thing only the agent knows (#47).
  assert.equal(post.last_seen, "m-3");
  // Attribution belongs to the room, stamped from the connection. A sender
  // that could name itself could name somebody else.
  assert.equal(
    "speaker" in post,
    false,
    "a posting participant must not name itself; the room stamps the speaker",
  );

  // ── a participant that has seen nothing omits the key ──────────────────────
  // No key rather than an empty one, for the same reason `to` and `hue` omit:
  // the room reads a value it cannot resolve as having seen nothing, and ""
  // is such a value. Sending it would refuse a first post that should pass.
  const first = await request("tools/call", {
    name: "say_to_room",
    arguments: { content: "はじめまして" },
  });
  assert.ok(!first.result.isError, `tool call failed: ${JSON.stringify(first.result)}`);
  const firstPost = await nextPost(1);
  assert.equal(
    "last_seen" in firstPost,
    false,
    "an undeclared watermark must carry no key",
  );

  // ── the room refuses, and the refusal carries what was missed ──────────────
  answer = "refuse";
  const refused = await request("tools/call", {
    name: "say_to_room",
    arguments: { content: "私も答えます", last_seen: "m-1" },
  });
  assert.ok(
    refused.result.isError,
    `a refused post must not read as delivered: ${JSON.stringify(refused.result)}`,
  );
  const refusal = refused.result.content[0].text;
  // Correlation held: the bogus `delivered: true` for another post arrived
  // first and did not settle this call.
  assertContains(
    refusal,
    "Not delivered.",
    "a refusal must say the post did not go into the room",
  );
  assertContains(
    refusal,
    "Your message was not posted.",
    "the refusal must be unambiguous that nothing was said, not a note attached to a delivery",
  );
  // Every field of every missed post, not just the first line. The condition
  // is that the return value carries the posts the speaker had not seen — a
  // check on the opening sentence passes on a report that dropped all of them.
  for (const missed of MISSED) {
    assertContains(refusal, missed.message_id, "each missed post must carry its id");
    assertContains(refusal, missed.speaker, "each missed post must name its speaker");
    assertContains(refusal, missed.content, "each missed post must carry what was said");
  }
  // The addressee, as an addressee. Checking for the bare name would pass on
  // this post's speaker alone, which is a different field.
  assertContains(
    refusal,
    "Master -> Claude Lay:",
    "a missed post addressed elsewhere comes back carrying who it was for; the room does not narrow by addressee",
  );
  // The way out of the refusal, named concretely. Being told to try again with
  // "the newest id" and left to work out which is which is the shape that goes
  // unread.
  assertContains(
    refusal,
    'last_seen: "m-10"',
    "the refusal must name the watermark to declare on the next attempt",
  );

  // ── the pull: what the topic held before this session joined ──────────────
  // The room hands a late joiner nothing, and this is the whole of what a
  // participant can do about that. It is a read: nothing is posted, and the
  // frame that goes out is not a post (#115, decision 4C).
  const pulled = await request("tools/call", {
    name: "read_room_history",
    arguments: { limit: 2 },
  });
  assert.ok(!pulled.result.isError, `pull failed: ${JSON.stringify(pulled.result)}`);
  assert.equal(historyFrames.length, 1, "one pull produces exactly one frame");
  assert.equal(historyFrames[0].type, "history");
  assert.equal(historyFrames[0].limit, 2);
  assert.equal(
    "before" in historyFrames[0],
    false,
    "a first page names no cursor; an empty one would be a value the room has to rule out",
  );
  // A pull is not a post. Reaching the room as one would put words in the room
  // that nobody said.
  assert.equal(
    postFrames.length,
    3,
    "reading the topic must not put anything on the floor",
  );
  const past = pulled.result.content[0].text;
  // Correlation held: the answer for another pull arrived first and did not
  // settle this call.
  for (const one of PAST) {
    assertContains(past, one.message_id, "each past post must carry its id");
    assertContains(past, one.speaker, "each past post must name its speaker");
    assertContains(past, one.content, "each past post must carry what was said");
  }
  assertContains(
    past,
    "Claude Lay -> Master:",
    "a past post addressed to someone comes back carrying who it was for",
  );
  // The way to keep reading backwards, named concretely. "There is more" with
  // no cursor is a dead end the agent cannot act on.
  assertContains(
    past,
    'before: "h-1"',
    "a page with more behind it must name the cursor for the next one",
  );
  // What this is and is not. These posts were never addressed to this session
  // and were never delivered to it; read as arrivals they would be answered.
  assertContains(
    past,
    "Read it as context, not as something to answer.",
    "the pull must say that what it returns is not addressed to the reader",
  );

  // ── an unanswered post is unconfirmed, not delivered and not refused ───────
  // The frame may well have landed. Reporting either verdict would be a guess
  // the agent then acts on: "delivered" lets it believe it spoke, "refused"
  // invites it to say the same thing twice.
  answer = "silent";
  const unanswered = request("tools/call", {
    name: "say_to_room",
    arguments: { content: "届いてる？", last_seen: "m-1" },
  });
  await nextPost(3);
  // The call is itself the boundary, which is what removes the reply that has
  // none. The frame is on the wire and the call has still not resolved: what
  // the room hands back arrives inside this call, not after the turn is over.
  // Resolving on send instead is the zero-tool shape — the send is the first
  // boundary, and anything that arrived while composing is unreadable until
  // too late (#47 type 2).
  const settled = await Promise.race([
    unanswered.then(() => "resolved"),
    new Promise((r) => setTimeout(() => r("still waiting"), 200)),
  ]);
  assert.equal(
    settled,
    "still waiting",
    "say_to_room must not resolve before the room answers; a call that returns on send is not a boundary",
  );
  roomSocket.close();
  const unconfirmed = await unanswered;
  assert.ok(
    unconfirmed.result.isError,
    `an unanswered post must not read as delivered: ${JSON.stringify(unconfirmed.result)}`,
  );
  assertContains(
    unconfirmed.result.content[0].text,
    "Not confirmed",
    "a post the room never answered must read as unconfirmed, not as either verdict",
  );

  // ── a dropped frame must not read as delivered ─────────────────────────────
  // The room is taken down so the sidecar's retry cannot reconnect underneath
  // this assertion.
  wss.close(() => {});
  http.close(() => {});
  await new Promise((r) => setTimeout(r, 500));

  const offline = await request("tools/call", {
    name: "say_to_room",
    arguments: { content: "誰か聞いてる？" },
  });
  assert.ok(
    offline.result.isError,
    "a send with no room attached must report failure, not silence",
  );
  assertContains(
    offline.result.content[0].text,
    "Not delivered: the room socket is not connected",
    "a send with no socket must say so rather than wait out the answer it will never get",
  );
});

test("a session launched without a hue or an account says so by omission", async (t) => {
  // The undeclared state has to survive the wire. The room derives a colour
  // from the name for a participant who chose none, and it can only do that
  // while "chose none" is still distinguishable from a number a default put
  // there (#40).
  //
  // The account id is the same shape and a stronger case: a connection with no
  // account behind it is a participant like any other, and the room must not
  // presume one exists (#59). An empty string here would be an id no account
  // has, offered to the screen as though someone had declared it.
  const http = createServer();
  const wss = new WebSocketServer({ server: http });
  await new Promise((r) => http.listen(0, "127.0.0.1", r));
  const port = http.address().port;

  const helloSeen = deferred();
  wss.on("connection", (socket) => {
    socket.on("message", (raw) => {
      const frame = JSON.parse(raw.toString());
      if (frame.type === "hello") helloSeen.resolve(frame);
    });
  });

  const child = spawn(
    process.execPath,
    [join(REPO, "node_modules", "tsx", "dist", "cli.mjs"), ENTRY],
    {
      cwd: REPO,
      env: {
        ...process.env,
        PULLCEPT_ROOM_URL: `ws://127.0.0.1:${port}`,
        PULLCEPT_AGENT_NAME: "no-colour",
        PULLCEPT_AGENT_HUE: "",
        PULLCEPT_ROOM_ID: "test-room",
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  // Nothing reads either pipe in this test, and a full one would block the
  // sidecar before it ever reaches the socket.
  child.stdout.resume();
  child.stderr.resume();

  t.after(() => {
    child.kill();
    wss.close(() => {});
    http.close(() => {});
  });

  const hello = await withTimeout(helloSeen.promise, "hello frame");
  assert.equal(hello.name, "no-colour");
  assert.equal("hue" in hello, false, "an undeclared hue must carry no key");
  assert.equal(
    "account_id" in hello,
    false,
    "a connection with no account behind it must carry no account key",
  );
});
