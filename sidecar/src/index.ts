#!/usr/bin/env node
/**
 * Pullcept room sidecar
 *
 * A stdio MCP server that puts one CLI session into the room.
 *
 * Two protocol faces, mirroring the shape verified in
 * Liplus-Project/github-webhook-mcp (local-mcp/src/index.ts):
 *
 *   CLI  -> sidecar : stdio MCP server. Declares the `claude/channel`
 *                     experimental capability, so the host accepts
 *                     `notifications/claude/channel` pushes from this side.
 *   sidecar -> room : WebSocket client. The app hosts the room socket; the
 *                     CLI spawns this process, so the app cannot know the
 *                     port or the launch moment from its own side.
 *
 * Direction of travel:
 *   someone posts -> WebSocket frame -> channel notification -> agent reacts
 *   this agent posts -> `say_to_room` tool -> WebSocket frame -> the room
 *
 * Both directions carry the same frame. A person and a session are both
 * participants of the room, and what separates them is a name (#39).
 *
 * The CLI terminal output is never read as a message source. stdout belongs to
 * the MCP transport; every log line goes to stderr.
 */
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  ListToolsRequestSchema,
  CallToolRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import WebSocket from "ws";
import { randomUUID } from "node:crypto";

const ROOM_URL = process.env.PULLCEPT_ROOM_URL ?? "";
const AGENT_NAME = process.env.PULLCEPT_AGENT_NAME ?? "session";
const ROOM_TOKEN = process.env.PULLCEPT_ROOM_TOKEN ?? "";
const CHAT_ID = process.env.PULLCEPT_ROOM_ID ?? "pullcept";

/**
 * The hue this session was launched under, in oklch degrees, or null when it
 * was launched without one.
 *
 * Null rather than a default. An undeclared participant is one the room derives
 * a colour for from their name, and a number invented here would be indelible:
 * the room cannot tell a declaration from a fallback once it is on the wire.
 */
const AGENT_HUE = readHue(process.env.PULLCEPT_AGENT_HUE);

function readHue(raw: string | undefined): number | null {
  if (raw === undefined || raw.trim() === "") return null;
  const hue = Number(raw);
  return Number.isFinite(hue) ? hue : null;
}

/**
 * The account this session was launched as, or null when it was launched
 * without one.
 *
 * Carried into `hello` and nothing else. It is not this session's identity in
 * the room — the room mints that from the connection and keeps it there (#39 /
 * #40 / #47) — and nothing in this file reads it to decide anything. It exists
 * so the screen can say which of its accounts a participant is without
 * matching on a name, which is the match #40 and #53 ruled out (#59).
 *
 * Null is a real state, not a launch that went wrong: a room does not presume
 * an account exists behind a connection, and something joining from outside
 * this app has none to declare.
 */
const ACCOUNT_ID = process.env.PULLCEPT_ACCOUNT_ID?.trim() || null;

const PROTOCOL_VERSION = 5;

/**
 * How long a post waits for the room to answer it.
 *
 * The room answers every post, so silence past this is the room having gone
 * away mid-post rather than a slow decision. Reported as unconfirmed, never as
 * delivered: the frame may well have landed, and claiming either way would be
 * a guess the agent then acts on.
 */
const POST_RESULT_TIMEOUT = 15_000;

function log(line: string): void {
  process.stderr.write(`[pullcept sidecar] ${line}\n`);
}

// ── Room frames ──────────────────────────────────────────────────────────────
//
// Sidecar -> room:
//   { type: "hello", protocol, name, hue?, account_id? }
//   { type: "post",  message_id, content, to?, ts, last_seen? }
// Room -> sidecar:
//   { type: "post",  message_id, speaker, content, to?, ts }
//   { type: "post_result", message_id, delivered, missed }
//
// One frame kind carries speech, whoever produced it. The room stamps
// `speaker` from the connection the frame arrived on, so this side does not
// send it: a participant names an addressee, never itself.
//
// `to` is optional in both directions and means the same thing on each: the
// display name of the participant addressed. The room fans every post out to
// everyone regardless — whether an utterance is yours to answer is decided
// here, by the agent, not by the room narrowing its delivery.
//
// `hello` is where this session says who it is: the name it answers to and,
// when it was launched with one, the hue it is drawn in. Both arrive from the
// launch (`PULLCEPT_AGENT_NAME` / `PULLCEPT_AGENT_HUE`) rather than from anything
// this file decides, because both are the person's declaration, made at the
// moment of joining.
//
// `account_id` rides along on the same frame, and is the one field on it that
// is not a declaration about how to be shown. It says which account of the app
// launched this session, so the screen can join its own list against the room's
// roster by id instead of by name (#59). It is optional in both directions: a
// connection with no account behind it is a participant like any other, and the
// room presumes nothing about one. Nothing here or in the room reads it to
// decide identity, self-suppression or attribution — those stay on the
// connection (#39 / #40 / #47).
//
// A participant never receives its own post. The room drops it on the way out,
// judged on the connection it arrived on, so nothing here has to recognise
// itself — and a name collision cannot make this side swallow someone else's
// post (#40).
//
// `last_seen` is the agent's own account of the newest post it had actually
// seen. It rides on the post because the room refuses one whose speaker was
// behind the floor, and only the speaker can supply it: this process receives
// every post, but whether one reached the agent's context is decided by where
// the agent's next tool-result boundary fell, which nothing here can observe
// (#47).
//
// `post_result` is the room's answer to a post, correlated by the
// `message_id` the post was sent under. It arrives on this connection only.
// Waiting for it is what makes the tool call a boundary: a reply that needs no
// other tool used to have none before its own send, so anything arriving while
// it was composed was unreadable until too late. Now the send itself is where
// the room hands that back.
//
// Frames whose `type` is unknown are ignored rather than rejected, so the room
// can add frame kinds without breaking a sidecar built against this revision.

interface PostFrame {
  type: "post";
  message_id?: string;
  speaker?: string;
  content?: string;
  to?: string;
  ts?: string;
}

/** One post the room says this agent had not seen when it tried to speak. */
interface MissedPost {
  message_id?: string;
  speaker?: string;
  content?: string;
  to?: string;
  ts?: string;
}

interface PostResultFrame {
  type: "post_result";
  message_id?: string;
  delivered?: boolean;
  missed?: MissedPost[];
}

// ── MCP server ───────────────────────────────────────────────────────────────

const INSTRUCTIONS = [
  "あなたは Pullcept の部屋に参加しています。",
  `この部屋でのあなたの名前は「${AGENT_NAME}」です。`,
  "",
  "この部屋は、人間と AI を区別しません。参加者は全員が同じ参加者であり、",
  "違いは名前だけです。発言もひとつの行為で、誰が出しても同じ形で届きます。",
  "相手が人間か別のセッションかを気にする必要はありません。",
  "",
  '部屋の発言は <channel source="pullcept" ...> として届きます。',
  "発言するときは say_to_room ツールを呼んでください。ターミナルへの出力は",
  "部屋には届きません。",
  "",
  "宛先:",
  "- 発言には宛先が付くことがあります。宛先は meta.to に入っています。",
  `- meta.to が「${AGENT_NAME}」なら、あなた宛です。答えてください。`,
  "- meta.to が他の参加者の名前なら、あなた宛ではありません。黙ってください。",
  "  補足したくなっても割り込まないでください。",
  "- meta.to が無い発言は部屋全体宛です。自分が答えるべきときだけ答えてください。",
  "- say_to_room の to 引数で、こちらからも宛先を指定できます。宛先には",
  "  人間の参加者も指定できます。指定の仕方は相手によって変わりません。",
  "",
  "部屋の作法:",
  "- 自分の発言は返ってきません。届いた発言はすべて他の参加者のものです。",
  "- 返信しない判断は正当です。全員が答えると部屋は読めなくなります。",
  "- 一度の発言は簡潔に。長い説明が必要なときは、まず要点だけ返してください。",
  "- 他の参加者の発言を、自分の文脈として取り込まないでください。それぞれが",
  "  自分の文脈から同じ会話に参加しています。",
  "- 先に誰かが答えていたら、その発言を読んでから自分の発言を決めてください。",
  "  全体宛の問いに、全員が答える必要はありません。",
  "- 送る直前に、届いている発言をもう一度見てください。組み立てている間にも",
  "  発言は届きます。言おうとしていたことが既に言われていたら送らず、",
  "  足りないことがあるときだけ足してください。",
  "",
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

const mcp = new Server(
  { name: "pullcept-room", version: "0.1.0" },
  {
    capabilities: {
      tools: {},
      experimental: { "claude/channel": {} },
    },
    instructions: INSTRUCTIONS,
  },
);

const TOOLS = [
  {
    name: "say_to_room",
    description:
      "Post a message to the Pullcept room. This is the only way to be heard " +
      "by the room; terminal output is not read by anyone.",
    inputSchema: {
      type: "object" as const,
      properties: {
        content: {
          type: "string",
          description: "The message body to post.",
        },
        to: {
          type: "string",
          description:
            "Optional. The participant this message is addressed to. Omit to " +
            "address the room.",
        },
        last_seen: {
          type: "string",
          description:
            "The meta.message_id of the newest room post you have actually " +
            "seen. Omit only when you have seen none. If anything reached " +
            "the room after it, this post is refused and those posts are " +
            "returned to you instead of being delivered — read them, decide " +
            "again, and call again with the newest message_id if you still " +
            "have something to add.",
        },
      },
      required: ["content"],
    },
  },
];

mcp.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: TOOLS }));

mcp.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  if (name !== "say_to_room") {
    return {
      content: [{ type: "text", text: `Unknown tool: ${name}` }],
      isError: true,
    };
  }

  const content = typeof args?.content === "string" ? args.content : "";
  if (!content.trim()) {
    return {
      content: [{ type: "text", text: "content is required and must be non-empty." }],
      isError: true,
    };
  }

  const to = typeof args?.to === "string" ? args.to : undefined;
  // Passed through as given. This process cannot check it and does not try:
  // the watermark is a statement about the agent's own context, not a claim
  // about who the agent is, and a false one costs only its author a round trip.
  const declared = typeof args?.last_seen === "string" ? args.last_seen.trim() : "";
  const lastSeen = declared || undefined;

  // No speaker field: the room stamps that from this connection. Sending one
  // would be a claim about who is speaking, and the room would overwrite it.
  const messageId = randomUUID();
  // Registered before the frame goes out, so an answer that comes back inside
  // the same tick has somewhere to land.
  const answered = awaitPostResult(messageId);
  const sent = sendToRoom({
    type: "post",
    message_id: messageId,
    content,
    ...(to ? { to } : {}),
    ...(lastSeen ? { last_seen: lastSeen } : {}),
    ts: new Date().toISOString(),
  });

  if (!sent) {
    // The room is the only audience. Reporting success on a dropped frame
    // would let the agent believe it had spoken.
    abandonPost(messageId);
    await answered;
    return {
      content: [
        {
          type: "text",
          text: `Not delivered: the room socket is not connected (${roomStatus()}).`,
        },
      ],
      isError: true,
    };
  }

  const result = await answered;

  if (result === null) {
    // Unconfirmed, and said as such. "Delivered" here would be a guess the
    // agent goes on to act on, and so would "not delivered".
    return {
      content: [
        {
          type: "text",
          text:
            `Not confirmed: the room did not answer this post (${roomStatus()}). ` +
            "It may or may not have been delivered. Do not repeat it blind.",
        },
      ],
      isError: true,
    };
  }

  if (result.delivered !== true) {
    // Refused. An error rather than a quiet note, because the agent's next
    // move depends on it: nothing was posted, and this is the one moment the
    // missed posts are in front of it.
    return {
      content: [{ type: "text", text: describeRefusal(result.missed ?? []) }],
      isError: true,
    };
  }

  return { content: [{ type: "text", text: "Delivered to the room." }] };
});

/** The room's refusal, written so the next move is unambiguous. */
function describeRefusal(missed: MissedPost[]): string {
  const lines = missed.map((one) => {
    const addressee = one.to ? ` -> ${one.to}` : "";
    const id = one.message_id ?? "?";
    return `- [${id}] ${one.speaker ?? "someone"}${addressee}: ${one.content ?? ""}`;
  });
  const newest = missed[missed.length - 1]?.message_id;
  const again = newest
    ? `call say_to_room again with last_seen: "${newest}"`
    : "call say_to_room again with last_seen set to the newest message_id above";
  return [
    "Not delivered. These posts reached the room while you were composing, " +
      "and you had not seen them:",
    ...lines,
    "",
    "Your message was not posted. Read the above and decide again. Saying " +
      "nothing is a valid outcome: if what you were going to say is already " +
      `there, do not send it. If you still have something to add, ${again}.`,
  ].join("\n");
}

// ── Room socket ──────────────────────────────────────────────────────────────

let ws: WebSocket | null = null;
let pingTimer: ReturnType<typeof setInterval> | null = null;
let retryCount = 0;
let lastError = "";

const BASE_RETRY_DELAY = 1_000;
const MAX_RETRY_DELAY = 30_000;
const PING_INTERVAL = 25_000;

function roomStatus(): string {
  if (!ROOM_URL) return "PULLCEPT_ROOM_URL is not set";
  if (ws && ws.readyState === WebSocket.OPEN) return "connected";
  return lastError ? `disconnected: ${lastError}` : "disconnected";
}

/**
 * Posts waiting for the room's answer, keyed by the id they were sent under.
 *
 * Keyed rather than a single slot: a host may have more than one tool call in
 * flight, and settling the wrong one would report another post's verdict.
 */
const awaitingResult = new Map<string, (result: PostResultFrame | null) => void>();

function awaitPostResult(messageId: string): Promise<PostResultFrame | null> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      awaitingResult.delete(messageId);
      resolve(null);
    }, POST_RESULT_TIMEOUT);
    // A pending answer must not be the reason this process stays alive.
    timer.unref?.();
    awaitingResult.set(messageId, (result) => {
      clearTimeout(timer);
      awaitingResult.delete(messageId);
      resolve(result);
    });
  });
}

/** Settle the post this answer belongs to, and only that one. */
function settlePostResult(frame: PostResultFrame): void {
  const id = frame.message_id;
  if (typeof id !== "string") return;
  awaitingResult.get(id)?.(frame);
}

/** Give up on one post's answer: nothing will come for it. */
function abandonPost(messageId: string): void {
  awaitingResult.get(messageId)?.(null);
}

/** The room went away. Everything in flight is unanswerable now, and waiting
 *  out the timeout would leave the agent blocked for no new information. */
function abandonPendingPosts(): void {
  for (const settle of [...awaitingResult.values()]) settle(null);
}

function sendToRoom(frame: Record<string, unknown>): boolean {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  try {
    ws.send(JSON.stringify(frame));
    return true;
  } catch (err) {
    lastError = String(err);
    return false;
  }
}

function pushToChannel(frame: PostFrame): void {
  const content = frame.content ?? "";
  if (!content) return;

  // `speaker` on the room's wire, `user` in the channel meta: the latter is
  // the host's key and the host renders it, so the name is translated at this
  // boundary rather than the room's frame being bent to the host's vocabulary.
  const speaker = frame.speaker ?? "someone";
  // The addressee rides in meta for the same reason the speaker does: the body
  // must stay equal to what was said. It is judgment material, not text — the
  // instructions tell the agent to read it and decide whether to answer.
  const to = typeof frame.to === "string" && frame.to ? frame.to : undefined;
  void mcp.notification({
    method: "notifications/claude/channel",
    params: {
      // Body only. The speaker rides in meta, which the host renders itself —
      // putting the name here too produced "マスター: マスター: ハロ～" (#28),
      // and it leaves the body no longer equal to what was said.
      content,
      meta: {
        chat_id: CHAT_ID,
        message_id: frame.message_id ?? randomUUID(),
        user: speaker,
        ...(to ? { to } : {}),
        ts: frame.ts ?? new Date().toISOString(),
      },
    },
  });
}

function scheduleRetry(): void {
  const delay = Math.min(BASE_RETRY_DELAY * 2 ** retryCount, MAX_RETRY_DELAY);
  retryCount++;
  log(`room socket: retrying in ${Math.round(delay / 1000)}s (attempt ${retryCount})`);
  setTimeout(connectRoom, delay);
}

function connectRoom(): void {
  const headers: Record<string, string> = {};
  if (ROOM_TOKEN) headers["Authorization"] = `Bearer ${ROOM_TOKEN}`;

  const socket = new WebSocket(ROOM_URL, { headers });
  ws = socket;

  socket.on("open", () => {
    retryCount = 0;
    lastError = "";
    log(`room socket: connected as "${AGENT_NAME}"`);
    // The hue and account keys are omitted when there is none, for the same
    // reason `to` is: the room must be able to tell "declared nothing" from a
    // value. For the account that distinction is the whole of its optionality
    // — a connection with no account is still a participant (#59).
    sendToRoom({
      type: "hello",
      protocol: PROTOCOL_VERSION,
      name: AGENT_NAME,
      ...(AGENT_HUE === null ? {} : { hue: AGENT_HUE }),
      ...(ACCOUNT_ID === null ? {} : { account_id: ACCOUNT_ID }),
    });
    pingTimer = setInterval(() => {
      if (socket.readyState === WebSocket.OPEN) socket.ping();
    }, PING_INTERVAL);
  });

  socket.on("message", (raw: Buffer | string) => {
    let data: unknown;
    try {
      data = JSON.parse(raw.toString());
    } catch {
      return;
    }
    if (typeof data !== "object" || data === null) return;
    const frame = data as { type?: string };
    if (frame.type === "post") pushToChannel(frame as PostFrame);
    // The answer to a post this agent made. Not pushed to the channel: it is
    // the tool call's own result, and putting it in the conversation would
    // read as somebody having said it.
    else if (frame.type === "post_result") settlePostResult(frame as PostResultFrame);
    // Unknown frame kinds are ignored on purpose; see the frame comment above.
  });

  socket.on("close", (code: number) => {
    if (pingTimer) clearInterval(pingTimer);
    pingTimer = null;
    ws = null;
    lastError = `closed with code ${code}`;
    log(`room socket: ${lastError}`);
    abandonPendingPosts();
    scheduleRetry();
  });

  socket.on("error", (err: Error) => {
    // A failed connect emits error then close; the close handler reconnects.
    lastError = err.message;
    log(`room socket: error: ${err.message}`);
  });
}

// ── Main ─────────────────────────────────────────────────────────────────────

await mcp.connect(new StdioServerTransport());
log("mcp: stdio transport connected");

if (ROOM_URL) {
  connectRoom();
} else {
  // Serving MCP without a room is a degraded but legible state: the agent can
  // still call the tool and gets told why nothing was delivered. Exiting here
  // would surface to the user as a bare MCP connection failure instead.
  log("room socket: PULLCEPT_ROOM_URL is not set, staying offline");
}
