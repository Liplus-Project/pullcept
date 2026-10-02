// The room surface.
//
// Everyone in the room is a participant, and a post is one act whoever made it
// (#39). Messages arrive on one event (`room-message`) either way, so the room
// has a single ordering authority; what is typed here is not appended locally
// on send but comes back through that same event. See src-tauri/src/room.rs.
//
// The diagnostics pane carries a real terminal for the launched CLI. That is a
// display, not a message source: the room's lines come from the room's own
// `room-message` event, and nothing in this file reads terminal output as
// speech. The app types every post into the sessions' terminals (#195), and
// the keys this pane sends are read for one thing only — whether the person
// has left something unsent there, which is when those posts wait. The rejected design is the one where the app parses CLI output to
// find messages (docs/1-room.md); showing the CLI is not that.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, type CloseRequestedEvent } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

interface RoomMessage {
  /** The room it was said in, which is its topic. Every topic open in the app
   *  keeps talking, so a post arrives whether or not its topic is the one on the
   *  glass, and this is what keeps it out of the one that is not (#141). */
  topic_id: string;
  message_id: string;
  speaker: string;
  /** The hue the speaker declared, or null when they declared none. Stamped by
   *  the room from the connection the post arrived on, so it is that speaker's
   *  and not whoever else currently answers to the same name. */
  hue: number | null;
  /** The account the speaker declared, or null when they declared none. For a
   *  notice, the account of the local MCP server that pushed it (#193): what
   *  the fold is decided on (`foldOf`). */
  account: string | null;
  content: string;
  /** The names it was addressed to, or empty when it was said to the room
   *  (#204). */
  to: string[];
  ts: string;
  /** True when this screen's own participant posted it. Self/other, not
   *  human/AI: the room no longer carries that axis. */
  own: boolean;
}

/**
 * What the room did with a post from this screen.
 *
 * The room refuses a post whose speaker had not seen everything on the floor,
 * and hands back what they missed instead of delivering (#47). Not an error:
 * being told what arrived while the message was being typed, and deciding
 * again, is the point.
 */
interface PostOutcome {
  delivered: boolean;
  /** The id the post is filed under, or null when it was refused. */
  message_id: string | null;
  /** What this screen had not seen, oldest first. Empty when delivered. */
  missed: MissedPost[];
}

/**
 * One post the room handed back in place of delivering.
 *
 * The post itself, not a summary of it: everything a line needs is here, which
 * is what lets the refusal put it on the glass (#108).
 */
interface MissedPost {
  message_id: string;
  speaker: string;
  /** The hue it was said in, or null when the speaker declared none. Carried
   *  so the drawn line is the line it would have been. */
  hue: number | null;
  /** The account it was said as, for the same reason (#193). */
  account: string | null;
  content: string;
  /** Empty when it was said to the room (#204). */
  to: string[];
  ts: string;
}

/**
 * One post as the room's log kept it (src-tauri/src/room_log.rs).
 *
 * Six fields, and the two a live post also carries are absent by decision
 * rather than by loss. `own` is a property of whoever is looking, so a file
 * could only have recorded one viewer's position as if it were part of the
 * utterance. `hue` was a declaration made at a seat that no longer exists by
 * the time this is read, so the history derives a colour from the name instead
 * — which means two participants who answered to one name are one colour here.
 * That is the known cost of not storing a declaration nobody is making any
 * more, and it is a panel of the past rather than the room's own attribution
 * surface (#48).
 */
interface LoggedPost {
  message_id: string;
  speaker: string;
  /** The account it was said as. Absent when the speaker declared none, and on
   *  every line written before the log carried it (#193). What lets a line read
   *  back fold the way it did live (`foldOf`). */
  account?: string;
  content: string;
  /** The names it was addressed to. Absent, not null or empty, when it was
   *  said to the room: the field's presence is what carries the two states, in
   *  the file and on the way here alike. A line written while a post had one
   *  addressee is read back by the app as a list of that one (#204). */
  to?: string[];
  ts: string;
}

/**
 * One topic, as the index holds it (src-tauri/src/room_log.rs).
 *
 * The vessel a conversation happens in, not a section of a transcript. Picking
 * one puts its posts back in the room and hands its session ids to the next
 * launch, so what comes back is the participants' own context rather than a
 * reading of the log to them (#115).
 */
interface Topic {
  topic_id: string;
  /** Generated from the first post's opening, editable after. Empty until that
   *  post lands, which the list draws as its own state rather than as a blank. */
  title: string;
  created_at: string;
  /** Which session each account was in while this was open, by account id.
   *  The launch decides on it. The screen only shows it — the セッション ID
   *  row reads the shown pane's account here, so the value on screen is the
   *  record's and nothing on this side decides on it (#139). */
  sessions: Record<string, string>;
}

/**
 * The topic on the glass (#141: one of the rooms, the one the screen shows).
 *
 * Held apart from the list because it may not be in the list: a launch opens a
 * new topic and nothing is written down until something is said in it, so the
 * index has no entry for it yet (#115). While it is not in the list, the panel
 * says where the room is by drawing 新規 as picked rather than by adding a row
 * for it (#125, 決定1 / AI 判断4).
 */
interface TopicRef {
  topic_id: string;
  created_at: string;
}

/**
 * One participant of the room, as the roster lists them.
 *
 * `id` is the connection they are in the room on, and it is the identity. The
 * name is what they are called and what a post is addressed to; two
 * participants may answer to one name and are still two.
 */
interface Participant {
  id: string;
  name: string;
  hue: number | null;
  /**
   * The account this participant joined as, or null when they joined with
   * none. What the panel joins its own account list against (#59).
   *
   * Not the identity, and not what anything here decides on. `id` above is the
   * identity, `own` below is the room's answer to self/other, and both are the
   * connection. An account id looks like the more stable of the two and is not
   * the one that was chosen: two connections could carry one account id — from
   * somewhere this app did not launch — and they would still be two
   * participants (#39 / #40 / #47).
   */
  account: string | null;
  own: boolean;
}

/**
 * One room's roster, as `room-participants` carries it.
 *
 * Named by its topic: a roster changes in a topic that is not on the glass — a
 * session joining the topic it was started in — and it is kept for when that
 * topic is opened (#141).
 */
interface Roster {
  topic_id: string;
  participants: Participant[];
}

/**
 * What one session says about itself, through its own status line (#155).
 *
 * Keyed on the seat: the JSON the CLI hands its status line names that CLI's
 * session id, which is not what a terminal is keyed on here, so the launch
 * wrote the topic and the account into the address it posts to and the app
 * reads them back off it (`mcp-config`, `status_hook_url`).
 *
 * Every value is nullable because every one of them is a field the CLI may not
 * send — the rate limits are absent off a claude.ai plan and before the first
 * API answer, the effort is absent on a model with no such parameter, and the
 * context percentage is null early in a session. Null reaches the row as `—`,
 * which is a different thing from `0%`.
 *
 * `five_hour` and `seven_day` are read twice over: as two rows of the panel,
 * and as the row's own 制限中 (`limitedByUsage`). That second reading is what
 * replaced the `StopFailure` hook #149 had put on the launch line (#161).
 */
interface SessionStats {
  topic_id: string;
  account_id: string;
  model: string | null;
  effort: string | null;
  five_hour: number | null;
  seven_day: number | null;
  context: number | null;
}

/**
 * What kind of participant an account is, declared when it is made.
 *
 * Never inferred from the connection: the room sees only what kind of
 * connection someone arrived on, and a person joining from another client
 * arrives the same way a session does. The account form is the one moment
 * anyone can say which this is (#59).
 *
 * Two of the three launch, and what separates them is what the app knows about
 * the command under them (#156). `claude_code` names a CLI whose conventions
 * the app holds — how a session id is handed over, how one is resumed, how the
 * session reports itself — so none of that is written by hand. `cli` is an
 * account the app knows nothing of the sort about: it launches, and the line is
 * the person's own. A second CLI is a third value here, not a second reading of
 * somebody's launch options.
 *
 * `mcp` is a local MCP server the app runs itself (#193). It speaks — what the
 * server pushes is posted as its account — and nothing is launched under it.
 * Declared on the form when an account is made, which writes its entry in
 * `mcp-servers.json` (#200), or given by the app to an entry the file holds
 * with no account; either way no other kind turns into it or out of it.
 */
type AccountKind = "admin" | "claude_code" | "cli" | "mcp";

/**
 * Whether this account launches a session.
 *
 * The two CLI kinds: what they differ in is what the app puts on their line,
 * which is not this question. A person has no command under them to spawn at
 * all (#59), and a local MCP server is started with the app from its entry in
 * the file, not from a row (#193).
 */
function launches(account: Account): boolean {
  return account.kind === "claude_code" || account.kind === "cli";
}

/**
 * One account: someone who exists whether or not they are running.
 *
 * `id` is the identity and never changes. Everything else is an attribute the
 * person edits — the name included, which is why an account can be renamed
 * without anything losing track of it. The launch recipe this replaced had no
 * identity of its own, so the name a session took was the only handle on it,
 * and two launches off one recipe took the same name (#40).
 */
interface Account {
  id: string;
  /** What the room lists this account under, and what a post is addressed to. */
  name: string;
  command: string;
  args: string[];
  cwd: string | null;
  /** The hue chosen for this account, or null when none was — the derived one
   *  from the name is used then. */
  hue: number | null;
  /** Declared when the account was made. What the participant list groups on,
   *  and nothing else — the room still has one kind of participant. */
  kind: AccountKind;
  /** Which character this account speaks as: the `name:` of an output style in
   *  its working directory's `.claude/output-styles/`, or null when it declares
   *  none and that directory's own default stands. An attribute of the account
   *  rather than a string inside `args`, for the reason the name and the hue
   *  are attributes (#40) — it is who this account is when it runs (#99). */
  character: string | null;
  /** The whole command line that puts this account back into a session it was
   *  already in, with `{session_id}` where the id goes — or null when it
   *  declares none. A topic holds which session each account was in while it
   *  was open, and reopening one hands that id to this line: what comes back is
   *  the participant's own context, carried by the CLI rather than read out to
   *  it (#115, decision 4B).
   *
   *  Null is the common state. An account with no resume line joins a reopened
   *  topic as a new session and pulls what it needs out of the room instead
   *  (#115, decision 4C). */
  resume_command: string | null;
  /** Variables set on the environment of the CLI this account launches (#163).
   *  The name in the clear and the value sealed with DPAPI: this screen never
   *  holds a stored value, only its mask (`account_env_text`), and hands what
   *  was typed back to be sealed (`seal_account_env`). Opened only at launch,
   *  and never onto the launch line. */
  env: EnvVar[];
  /** The entry of `mcp-servers.json` an `mcp` account answers to, by its name;
   *  null for every other kind (#193). The account holds who speaks, the entry
   *  what is run. */
  server: string | null;
}

/** One stored environment variable. `sealed` is opaque here — ciphertext this
 *  screen neither reads nor makes. */
interface EnvVar {
  name: string;
  sealed: string;
}

/**
 * Which of the two panels flanking the room are open.
 *
 * In the config rather than in this screen's own storage, where the two text
 * sizes are kept (#60 / #68). Those answer "is what I am reading a readable
 * size", which is the reader's own question; this is the window's layout, and
 * a panel folded away is expected to still be folded the next time the app
 * opens (#118, decision 1).
 */
interface PanelState {
  history: boolean;
  participants: boolean;
}

interface AppConfig {
  accounts: Account[];
  panels: PanelState;
}

interface StartedSession {
  pty_id: string;
  mcp_config: string;
  /** When the session was launched, stamped by the room's own clock. */
  started_at: string;
  /** The topic this launch went into — the one the screen named (#141). The
   *  topic a failed resume has to be undone on (#127). */
  topic_id: string;
  /** The session id this went back into, or null when it started fresh — the
   *  topic held a session for it, and the CLI came back carrying its own
   *  context (#115, decision 4B). Null is not a failure: it is a seat starting
   *  fresh in a reopened topic, which is what a topic does for every seat it
   *  cannot resume (decision 6). The id itself rather than a flag, because a
   *  resume that ends without the room ever seeing it has a record to drop, and
   *  dropping it names it (#127). */
  resumed_from: string | null;
  /** The session id this launch took off the topic before starting, because
   *  the conversation it named is not on disk — or null when it took none.
   *  Never set together with `resumed_from`: the drop is what made this the
   *  fresh line. Said on the status line for the reason the one #127 drops is
   *  said there — the way back into a conversation went, and nobody asked for
   *  that (#131, decision 2). */
  dropped_resume: string | null;
}

/**
 * What a launch would do with each value the account form holds (#154, 決定4).
 *
 * Four answers because what a value costs differs by which one it is: a
 * character and launch options the Windows launch line cannot carry are left
 * off it and the session still starts, while a command it cannot carry is a
 * session that does not start at all. The judgment is the app's — the same
 * functions the launch itself goes through — and the sentences are here,
 * because they are read here.
 */
interface LaunchFieldReport {
  character: boolean;
  options: boolean;
  command: boolean;
  resume: boolean;
}

/**
 * What is running under one held seat, as the app reports it.
 *
 * The same facts a `SessionView` holds, from the side that survives a reload of
 * this screen. The pty id is why it is sent at all: it is made at spawn and
 * handed over once, so a screen that has forgotten it cannot reach the session
 * again — and the account cannot be started either, because the seat refuses it
 * (#84).
 *
 * The command and the directory are the launch's own, not the account's as it
 * reads now. An account is editable while its session runs.
 */
interface RunningSession {
  pty_id: string;
  started_at: string;
  command: string;
  cwd: string;
  /** The topic this session was started into, and the room it is in. A
   *  launch's own fact: the screen moves between topics while a session keeps
   *  running in its own (#141, decision 2). What its terminal is filed under,
   *  and what a topic delete ends it by (#119, decision 4). */
  topic_id: string;
  /** The session id this launch went back into, or null when it started fresh.
   *  A launch's own fact like the three above, and kept on the seat for the
   *  reason the pty id is: this screen loses it on a reload and the seat does
   *  not (#127). */
  resumed_from: string | null;
}

/** One account holding a seat in one topic, and what it is running. */
interface SeatedAccount {
  account_id: string;
  /** The topic the seat is in. On the seat as well as on the session, because a
   *  seat whose launch is in flight has no session yet and is still in one topic
   *  and not another (#141). */
  topic_id: string;
  /** Null while its launch is in flight: claimed seat, nothing spawned yet. */
  session: RunningSession | null;
}

/**
 * Where the name and hue of this screen's person used to live, and the only
 * thing still read out of them: the values to make their account from, once.
 *
 * They were the person's whole identity here while a person was not an account
 * (#53 left that open). They are an account now (#59), so these two keys are a
 * migration source and are not written to again.
 */
const NAME_KEY = "pullcept.display-name";
const HUE_KEY = "pullcept.display-hue";
/**
 * Which account is the person at this screen.
 *
 * Held here rather than in the config, because it is a property of this screen
 * rather than of the account list: the same config opened elsewhere would have
 * a different person at it. The account itself is in the config like every
 * other.
 */
const LOCAL_KEY = "pullcept.local-account";

/**
 * How large the conversation is drawn, in `rem`.
 *
 * In `localStorage` beside the key above, and for the same reason: this is a
 * property of the screen being read from, not of anybody in the room. Two
 * people reading one conversation do not have to want the same size, and a
 * size carried on a participant would make the answer travel with whoever
 * declared it. It is the shape #40 settled for a screen's own settings.
 *
 * Not a participant attribute in the other sense either: nothing here is
 * written per speaker. Every line in the room is drawn at one size, whoever
 * said it (#39).
 */
const ROOM_FONT_SIZE_KEY = "pullcept.room-font-size";

/**
 * The sizes the conversation can be set to, in `rem`.
 *
 * A list rather than a continuous range, like the hues below: what this has to
 * buy is a readable size that fits, and a ladder buys it without asking anyone
 * to judge fractions of a millimetre. The ends of the list are the bounds —
 * there is no size off the ladder to clamp, so nothing separate enforces them.
 *
 * The rungs are dense below the default and sparse above it. The observation
 * this comes from is that the room reads large (#60), so the direction that
 * gets used is downward and the steps there are the ones worth being fine.
 */
const ROOM_FONT_SIZES = [0.7, 0.75, 0.8, 0.85, 0.9, 1, 1.1, 1.25, 1.4, 1.6];

/**
 * Where a screen that has never chosen sits.
 *
 * `1rem`, which is what the room already rendered at: `.message .body` is
 * given no size and inherits none, so the surface has been showing the user
 * agent's default. Keeping it is a completion condition of #60 — this change
 * adds the means to move, and moves nobody.
 */
const DEFAULT_ROOM_FONT_SIZE = 1;

/**
 * The keys that move along the ladder, and by how far.
 *
 * `Ctrl` with `=` / `-` / `0`, the combination browsers and editors have
 * trained. Both faces of the shifted keys are listed because a keyboard that
 * needs `Shift` for `+` reports `+`, and one that does not reports `=`; the
 * person pressing them is doing the same thing either way. `0` is the reset
 * and carries a step of zero, so the lookup below tests for `undefined` rather
 * than for falsity.
 */
const ROOM_FONT_SIZE_KEYS: Record<string, number> = {
  "=": 1,
  "+": 1,
  "-": -1,
  "_": -1,
  "0": 0,
};

/**
 * The hues a participant can declare.
 *
 * Hue only. Lightness and chroma stay the accent's in whichever theme is
 * showing, so a declared colour sits at the same weight on the page as every
 * other participant's and stays readable in both themes (#43). Offering a full
 * colour picker would ask for two values and silently discard them.
 *
 * A short list rather than a continuous dial: what this has to buy is that any
 * two participants can be told apart, and eight positions spread round the
 * wheel buy it without asking anyone to judge degrees.
 */
const HUES: { label: string; hue: number }[] = [
  { label: "赤", hue: 25 },
  { label: "橙", hue: 55 },
  { label: "黄", hue: 95 },
  { label: "緑", hue: 145 },
  { label: "青緑", hue: 195 },
  { label: "青", hue: 250 },
  { label: "紫", hue: 300 },
  { label: "桃", hue: 350 },
];

/**
 * Hue of `--accent`, and the arc the other participants are drawn from.
 *
 * `#3a6ea5` measured in oklch. The accent is this screen's own colour, so the
 * derived hues start a gap past it and stop a gap short of it: a participant
 * whose name happened to land on the accent would look like oneself.
 */
const ACCENT_HUE = 251.5;
const RESERVED_ARC = 25;
const DERIVED_ARC = 360 - RESERVED_ARC * 2;

/**
 * The drawings the screen's buttons carry (#221), one place for all of them.
 *
 * A button on the screen is its drawing, with no frame and no fill until the
 * pointer is on it; its name is on `aria-label` and on `title`, which is the
 * tooltip. Drawn rather than typed: an emoji is painted by a colour font that
 * answers to no `color`, so it could not take the muted, the danger or the
 * hover colour the rest of the button does, and its width is whichever font the
 * host resolved it in (#71). A line drawing in `currentColor` takes all of them
 * and is one width everywhere.
 *
 * Every drawing is on the 16-unit grid the panel toggles were drawn on (#118),
 * stroked and unfilled, so the set reads as one hand. The buttons in
 * index.html name theirs with `data-icon` and are filled from here at start
 * (`fillIcons`); the buttons this file builds call `icon` directly. One copy of
 * each drawing, so the ✕ that folds the pane and the ✕ on an ended tab cannot
 * drift into two shapes.
 */
type IconName =
  | "panel"
  | "terminal"
  | "settings"
  | "plus"
  | "copy"
  | "close"
  | "send"
  | "at"
  | "attach"
  | "start"
  | "edit"
  | "more";

type IconShape = [tag: string, attrs: Record<string, string>];

const ICONS: Record<IconName, IconShape[]> = {
  // A panel seen edge-on, the divider on the side it folds (#118). The right
  // one is this drawing turned over in src/styles.css.
  panel: [
    ["rect", { x: "1.7", y: "2.7", width: "12.6", height: "10.6", rx: "2.4" }],
    ["line", { x1: "6.2", y1: "2.7", x2: "6.2", y2: "13.3" }],
  ],
  // A window with a prompt in it: the pane 端末 opens.
  terminal: [
    ["rect", { x: "1.7", y: "2.7", width: "12.6", height: "10.6", rx: "2.4" }],
    ["polyline", { points: "4.6 6.3 6.6 8 4.6 9.7" }],
    ["line", { x1: "8.2", y1: "10.2", x2: "11.4", y2: "10.2" }],
  ],
  // Three sliders. Not the gear: that drawing is 編集 in every account's menu,
  // and the app's own settings are a different window (#172).
  settings: [
    ["line", { x1: "2", y1: "4", x2: "8.4", y2: "4" }],
    ["line", { x1: "11.6", y1: "4", x2: "14", y2: "4" }],
    ["circle", { cx: "10", cy: "4", r: "1.6" }],
    ["line", { x1: "2", y1: "8", x2: "3.4", y2: "8" }],
    ["line", { x1: "6.6", y1: "8", x2: "14", y2: "8" }],
    ["circle", { cx: "5", cy: "8", r: "1.6" }],
    ["line", { x1: "2", y1: "12", x2: "9.4", y2: "12" }],
    ["line", { x1: "12.6", y1: "12", x2: "14", y2: "12" }],
    ["circle", { cx: "11", cy: "12", r: "1.6" }],
  ],
  plus: [
    ["line", { x1: "8", y1: "3", x2: "8", y2: "13" }],
    ["line", { x1: "3", y1: "8", x2: "13", y2: "8" }],
  ],
  copy: [
    ["rect", { x: "5.5", y: "5.5", width: "8", height: "8", rx: "1.6" }],
    ["path", { d: "M10.5 5.5V4.1A1.6 1.6 0 0 0 8.9 2.5H4.1A1.6 1.6 0 0 0 2.5 4.1v4.8a1.6 1.6 0 0 0 1.6 1.6h1.4" }],
  ],
  // Folding the pane, closing an ended tab, ending a session and deleting a
  // topic. Which of those it is, is the colour and the label: the two that
  // cannot be taken back carry `--danger`.
  close: [
    ["line", { x1: "4", y1: "4", x2: "12", y2: "12" }],
    ["line", { x1: "12", y1: "4", x2: "4", y2: "12" }],
  ],
  send: [
    ["line", { x1: "8", y1: "13", x2: "8", y2: "3.2" }],
    ["polyline", { points: "3.8 7.4 8 3.2 12.2 7.4" }],
  ],
  // 宛先: the `@` the button types (#222).
  at: [
    ["circle", { cx: "8", cy: "8", r: "2.6" }],
    ["path", { d: "M10.6 5.4v3.3a2 2 0 0 0 4 0V8a6.6 6.6 0 1 0-2.6 5.25" }],
  ],
  // 添付 (#223): a paper clip, standing.
  attach: [["path", { d: "M10.8 5.2v5.6a2.8 2.8 0 0 1-5.6 0V4.1a1.9 1.9 0 0 1 3.8 0v6.5a1 1 0 0 1-2 0V5.4" }]],
  start: [["path", { d: "M5 3.2v9.6L12.6 8Z" }]],
  // A gear: 編集, the window an account's settings are made in.
  edit: [
    [
      "path",
      {
        d:
          "M7.08 1.46L8.92 1.46L9.17 3.14L10.61 3.74L11.97 2.73L13.27 4.03L12.26 5.39" +
          "L12.86 6.83L14.54 7.08L14.54 8.92L12.86 9.17L12.26 10.61L13.27 11.97" +
          "L11.97 13.27L10.61 12.26L9.17 12.86L8.92 14.54L7.08 14.54L6.83 12.86" +
          "L5.39 12.26L4.03 13.27L2.73 11.97L3.74 10.61L3.14 9.17L1.46 8.92" +
          "L1.46 7.08L3.14 6.83L3.74 5.39L2.73 4.03L4.03 2.73L5.39 3.74L6.83 3.14Z",
      },
    ],
    ["circle", { cx: "8", cy: "8", r: "2.1" }],
  ],
  // Three dots: the row's menu, the way into what right-click opens (#224).
  // Drawn as stroked rings small enough to read as dots, so it is stroked like
  // every other drawing here and takes the button's colour the same way.
  more: [
    ["circle", { cx: "3.5", cy: "8", r: "0.55" }],
    ["circle", { cx: "8", cy: "8", r: "0.55" }],
    ["circle", { cx: "12.5", cy: "8", r: "0.55" }],
  ],
};

const SVG_NS = "http://www.w3.org/2000/svg";

/** One drawing, ready to go into a button. Hidden from the reader: the button's label names it. */
function icon(name: IconName): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  for (const [tag, attrs] of ICONS[name]) {
    const shape = document.createElementNS(SVG_NS, tag);
    for (const [key, value] of Object.entries(attrs)) shape.setAttribute(key, value);
    svg.appendChild(shape);
  }
  return svg;
}

/** Put its drawing into every button index.html names one for. */
function fillIcons(): void {
  for (const button of document.querySelectorAll<HTMLElement>("[data-icon]")) {
    const name = button.dataset.icon as IconName;
    if (name in ICONS) button.replaceChildren(icon(name));
  }
}

const roomEl = document.getElementById("room") as HTMLElement;
const historyEl = document.getElementById("history") as HTMLElement;
const participantsEl = document.getElementById("participants") as HTMLElement;
const toggleHistoryEl = document.getElementById("toggle-history") as HTMLButtonElement;
const toggleParticipantsEl = document.getElementById(
  "toggle-participants",
) as HTMLButtonElement;
const rosterEl = document.getElementById("roster") as HTMLElement;
const topicListEl = document.getElementById("topic-list") as HTMLElement;
const topicNewEl = document.getElementById("topic-new") as HTMLButtonElement;
const roomTitleNameEl = document.getElementById("room-title-name") as HTMLElement;
const roomTitleCountEl = document.getElementById("room-title-count") as HTMLElement;
const accountNewEl = document.getElementById("account-new") as HTMLButtonElement;
const accountMenuEl = document.getElementById("account-menu") as HTMLElement;
const inputEl = document.getElementById("input") as HTMLTextAreaElement;
const sendEl = document.getElementById("send") as HTMLButtonElement;
const mentionEl = document.getElementById("mention") as HTMLButtonElement;
const composerEl = document.getElementById("composer") as HTMLElement;
const composerBoxEl = document.getElementById("composer-box") as HTMLElement;
const attachEl = document.getElementById("attach") as HTMLButtonElement;
const attachInputEl = document.getElementById("attach-input") as HTMLInputElement;
const attachmentsEl = document.getElementById("attachments") as HTMLUListElement;
const mentionListEl = document.getElementById("mention-list") as HTMLUListElement;
const statusEl = document.getElementById("status") as HTMLElement;
const diagnosticsEl = document.getElementById("diagnostics") as HTMLElement;
const toggleEl = document.getElementById("toggle-diagnostics") as HTMLButtonElement;
const socketStateEl = document.getElementById("socket-state") as HTMLElement;
const factsMoreEl = document.querySelector("#participants .facts-more") as HTMLDetailsElement;
const sessionStateEl = document.getElementById("session-state") as HTMLElement;
const transportEl = document.getElementById("session-transport") as HTMLElement;
const commandEl = document.getElementById("session-command") as HTMLElement;
const dirEl = document.getElementById("session-dir") as HTMLElement;
const startedEl = document.getElementById("session-started") as HTMLElement;
const windowEl = document.getElementById("session-window") as HTMLElement;
// The five the session reports about itself (#155), in the order they are read.
const statsEls = {
  model: document.getElementById("session-model") as HTMLElement,
  effort: document.getElementById("session-effort") as HTMLElement,
  five_hour: document.getElementById("session-five-hour") as HTMLElement,
  seven_day: document.getElementById("session-seven-day") as HTMLElement,
  context: document.getElementById("session-context") as HTMLElement,
};
const sessionIdEl = document.getElementById("session-id") as HTMLElement;
const sessionIdCopyEl = document.getElementById("session-id-copy") as HTMLButtonElement;
const terminalEl = document.getElementById("terminal") as HTMLElement;
const tabsEl = document.getElementById("terminal-tabs") as HTMLElement;
const diagnosticsCloseEl = document.getElementById("diagnostics-close") as HTMLButtonElement;
const dialogEl = document.getElementById("account-dialog") as HTMLDialogElement;
const dialogFormEl = document.getElementById("account-form") as HTMLFormElement;
const dialogTitleEl = document.getElementById("account-dialog-title") as HTMLElement;
const dialogNameEl = document.getElementById("dialog-name") as HTMLInputElement;
const dialogKindEl = document.getElementById("dialog-kind") as HTMLSelectElement;
const dialogHueEl = document.getElementById("dialog-hue") as HTMLSelectElement;
const dialogLaunchEl = document.getElementById("dialog-launch") as HTMLElement;
const dialogCwdEl = document.getElementById("dialog-cwd") as HTMLInputElement;
const dialogCharacterEl = document.getElementById("dialog-character") as HTMLInputElement;
const dialogOptionsEl = document.getElementById("dialog-options") as HTMLInputElement;
const dialogResumeEl = document.getElementById("dialog-resume") as HTMLInputElement;
const dialogResumeFieldEl = document.getElementById("dialog-resume-field") as HTMLElement;
const dialogEnvEl = document.getElementById("dialog-env") as HTMLTextAreaElement;
const dialogPreviewEl = document.getElementById("dialog-preview") as HTMLElement;
const dialogNoticeEl = document.getElementById("dialog-notice") as HTMLElement;
const dialogErrorEl = document.getElementById("dialog-error") as HTMLElement;
const dialogDeleteEl = document.getElementById("dialog-delete") as HTMLButtonElement;
const dialogCancelEl = document.getElementById("dialog-cancel") as HTMLButtonElement;
const endDialogEl = document.getElementById("end-dialog") as HTMLDialogElement;
const endMessageEl = document.getElementById("end-dialog-message") as HTMLElement;
const endCancelEl = document.getElementById("end-cancel") as HTMLButtonElement;
const endCommitEl = document.getElementById("end-commit") as HTMLButtonElement;
const quitDialogEl = document.getElementById("quit-dialog") as HTMLDialogElement;
const quitMessageEl = document.getElementById("quit-dialog-message") as HTMLElement;
const quitCancelEl = document.getElementById("quit-cancel") as HTMLButtonElement;
const quitCommitEl = document.getElementById("quit-commit") as HTMLButtonElement;
const topicDeleteDialogEl = document.getElementById(
  "topic-delete-dialog",
) as HTMLDialogElement;
const topicDeleteMessageEl = document.getElementById("topic-delete-message") as HTMLElement;
const topicDeleteSessionsEl = document.getElementById("topic-delete-sessions") as HTMLElement;
const topicDeleteCancelEl = document.getElementById("topic-delete-cancel") as HTMLButtonElement;
const topicDeleteCommitEl = document.getElementById("topic-delete-commit") as HTMLButtonElement;
const openSettingsEl = document.getElementById("open-settings") as HTMLButtonElement;
const settingsDialogEl = document.getElementById("settings-dialog") as HTMLDialogElement;
const settingsCloseEl = document.getElementById("settings-close") as HTMLButtonElement;
// The display section's three pickers (#194), the only on-screen controls for
// the three sizes since the title bar's and the terminal header's were taken
// out (#209).
const settingsRoomFontSizeEl = document.getElementById(
  "settings-room-font-size",
) as HTMLSelectElement;
const settingsTerminalFontSizeEl = document.getElementById(
  "settings-terminal-font-size",
) as HTMLSelectElement;
const settingsUiScaleEl = document.getElementById("settings-ui-scale") as HTMLSelectElement;
const mcpOpenFileEl = document.getElementById("mcp-open-file") as HTMLButtonElement;
const mcpFileEl = document.getElementById("mcp-file") as HTMLElement;
const mcpFileErrorEl = document.getElementById("mcp-file-error") as HTMLElement;
const dialogMcpEl = document.getElementById("dialog-mcp") as HTMLElement;
const dialogKindMcpEl = dialogKindEl.querySelector('option[value="mcp"]') as HTMLOptionElement;
const mcpStateEl = document.getElementById("mcp-state") as HTMLElement;
const mcpRestartEl = document.getElementById("mcp-restart") as HTMLButtonElement;
const mcpStaleEl = document.getElementById("mcp-stale") as HTMLElement;
const mcpFieldsEl = document.getElementById("mcp-fields") as HTMLElement;
const mcpCommandEl = document.getElementById("mcp-command") as HTMLInputElement;
const mcpArgsEl = document.getElementById("mcp-args") as HTMLTextAreaElement;
const mcpEnvEl = document.getElementById("mcp-env") as HTMLTextAreaElement;
const mcpErrorEl = document.getElementById("mcp-error") as HTMLElement;
const mcpSaveEl = document.getElementById("mcp-save") as HTMLButtonElement;
const mcpLogEl = document.getElementById("mcp-log") as HTMLElement;
const mcpLogHeadEl = document.getElementById("mcp-log-head") as HTMLElement;

let accounts: Account[] = [];
/**
 * Which panels are open, until the config says otherwise.
 *
 * Both, which is the layout every screen showed before either could be folded.
 * A config that has never recorded a fold reads back the same pair, so the
 * value here and the value on disk agree without either being the fallback for
 * the other.
 */
let panels: PanelState = { history: true, participants: true };
/**
 * The seats held in every topic, by `seatKey`.
 *
 * Ids, never names: this is matched against the account list to decide who is
 * offline, and a name match would tie the wrong account as soon as two share a
 * name — which they may, now that a name is an editable attribute (#53). The
 * app is the authority (`seated_accounts`); the screen re-reads it rather than
 * keeping a count of its own launches.
 *
 * A map rather than a set of ids, because the answer carries what is running
 * under each seat as well. That is what a terminal is rebuilt from when this
 * screen has been reloaded out from under a running session (#84).
 *
 * Keyed on the topic and the account together (#141). One account may hold a
 * seat in each of two topics — a session left running where the screen was, and
 * the same account started where the screen is now — and what is refused is the
 * same account twice in one topic.
 */
let seated = new Map<string, SeatedAccount>();
/**
 * Every room's roster the screen has heard, by topic id.
 *
 * Kept for every topic rather than for the one on the glass: rosters move in
 * topics nobody is looking at, and opening one of them should find who is there
 * rather than an empty list waiting for the next change (#141).
 */
const rosters = new Map<string, Participant[]>();
/**
 * The account the person at this screen is, or "" before it is resolved.
 *
 * They are an account like every other one — that is what #53 left open and
 * this settles (#59). What is theirs alone is being the one at the keyboard,
 * which is why the id is here and in `localStorage`, not a flag on the account.
 */
let localAccountId = "";
/** Prefill for an account that has never been given a working directory. */
let homeDir = "";
/**
 * The newest post this screen has drawn, declared as `last_seen` when posting.
 *
 * Drawn, not read — the screen can only speak for what it put on the glass. It
 * is an honest watermark all the same: a line is drawn only after the room has
 * admitted it, so a post arriving in the same instant as a send is either
 * already on screen or genuinely behind this value, and the room orders the
 * two. A person who leaves a message unread on screen is a gap the room cannot
 * see and does not pretend to (#47).
 */
/**
 * The topics as the index holds them, oldest first, and the one on the glass.
 *
 * Two values because the current one may not be in the list: a launch opens a
 * new topic and nothing is written down until something is said in it (#115).
 */
let topics: Topic[] = [];
let currentTopic: TopicRef | null = null;
/**
 * The topic `#topic-delete-dialog` is standing open on, or null when it is not.
 *
 * The topic rather than its id, because the answer is reported by name and the
 * row it was opened from is not under the pointer any more. Cleared on every
 * path out of the dialog, so a close by Escape leaves nothing a later click
 * could fire — the same discipline `endingAccount` keeps (#71).
 */
let deletingTopic: Topic | null = null;
let lastSeenId: string | null = null;
/**
 * Every `message_id` this screen has put on the glass.
 *
 * The floor hands a refusal its whole retained window when it cannot resolve
 * the declared watermark — an id that has aged out of the 512, or one the room
 * never issued, is read as having seen nothing — so what comes back then
 * includes lines this screen already drew. This is what tells the two apart
 * (#108).
 *
 * Unbounded, like the log it mirrors: nothing here prunes drawn lines, so a
 * set of their ids grows no faster than the DOM already does.
 */
const drawnIds = new Set<string>();
/** The size the conversation is currently drawn at, in `rem`. */
let roomFontSize = DEFAULT_ROOM_FONT_SIZE;

/**
 * How large a terminal is drawn, in `px`.
 *
 * The third size axis and an independent one: the conversation (#60), this, and
 * the whole UI (#66) are three separate answers, and none of them is expressed
 * relative to another. What makes this one different in kind from #60 is that it
 * is not only a display size — xterm.js computes the session's columns and rows
 * from it, so moving it changes the window the CLI is drawing for.
 *
 * `localStorage` and not the config, for #60's reason: it is a property of the
 * screen being read from rather than of anybody in the room.
 */
const TERMINAL_FONT_SIZE_KEY = "pullcept.terminal-font-size";

/**
 * The sizes a terminal can be set to, in `px`.
 *
 * A ladder with its ends as the bounds, the shape #60 settled for the
 * conversation: there is no size off the ladder to clamp, so nothing separate
 * enforces the limits.
 *
 * In `px` and labelled in `px`, where #60 labels a proportion. The two are
 * asked different questions. A conversation is read against nothing in
 * particular, so "larger or smaller than what I have" is the whole of it; a
 * terminal is read against the CLI's own layout, and the number that decides how
 * many columns fit is this one. It is also the unit xterm takes.
 *
 * Spread evenly rather than dense at one end. #60's rungs lean downward because
 * the observation behind it was that the room reads large; nothing says which
 * direction this one gets used in, and inventing a lean would be answering a
 * question nobody has asked yet.
 */
const TERMINAL_FONT_SIZES = [9, 10, 11, 12, 13, 14, 16, 18, 20, 24];

/**
 * Where a screen that has never chosen sits.
 *
 * `13px`, which is what every terminal has been opened at. Keeping it is the
 * same completion condition #60 had: this adds the means to move, and moves
 * nobody.
 */
const DEFAULT_TERMINAL_FONT_SIZE = 13;

/**
 * How large everything but the conversation and the terminal is drawn, as a
 * multiple of what it has always been (#66, #194).
 *
 * The third of the three size axes. It is carried as the root's `font-size`,
 * because the UI around the two other surfaces is written in `rem` — the title
 * bar, the panels, the windows — so one value on `:root` moves all of it without
 * naming any of it. The two other axes are kept out by construction rather than
 * by exception: the conversation's size divides this one back out
 * (`--room-font-size` in src/styles.css), and the terminal is sized in `px`,
 * which a root size does not reach. Not the webview's own zoom: that takes the
 * whole screen, the two excluded surfaces with it, and those are what this may
 * not move.
 *
 * `localStorage`, for #60's reason: a property of the screen being read from.
 */
const UI_SCALE_KEY = "pullcept.ui-scale";

/**
 * The multiples the UI can be set to.
 *
 * A ladder with its ends as the bounds, the shape #60 settled. More rungs above
 * the default than below: what #66 asks for is a screen that reads from further
 * away, so the direction that gets used is upward.
 *
 * The top is `1.5` because the panels grow with it. They are written in `rem`,
 * and the conversation between them takes what is left: at `1.5` two open panels
 * already leave almost nothing of a 900px window. Past that the ladder would
 * mostly offer ways to lose the room.
 */
const UI_SCALES = [0.8, 0.9, 1, 1.1, 1.2, 1.3, 1.4, 1.5];

/** `1`, which is what every screen has been drawn at. It moves nobody. */
const DEFAULT_UI_SCALE = 1;

/**
 * The emulator options every session's terminal is opened with.
 *
 * One set for all of them, so that two sessions on this screen are two of the
 * same kind of thing and a difference between their panes says something about
 * the sessions rather than about the panes. The size is one of them: it is the
 * screen's, not a session's, so opening a second terminal does not open it at
 * some other size than the first (`openView` passes the current one).
 */
const TERMINAL_OPTIONS = {
  cursorBlink: true,
  fontSize: DEFAULT_TERMINAL_FONT_SIZE,
  fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, monospace',
  // The CLI is a full-screen TUI: it moves the cursor, clears regions and
  // repaints. Anything less than an emulator turns that into debris, which is
  // what the previous line-appending pane did (#24).
  convertEol: false,
  scrollback: 5000,
};

/**
 * The line a terminal opens with when it is picked up rather than launched.
 *
 * It stands where the missing output would have been, which is the only place
 * it answers the question it exists for: this pane is not empty because the
 * session has said nothing. Dim, because it is the app speaking inside a pane
 * that otherwise belongs entirely to the session (#84).
 */
const RESUMED_NOTICE =
  "\x1b[2m[pullcept] 画面が再読み込みされました。セッションは走ったままで、この端末はそこへ繋ぎ直したものです。これより前の出力は残っていません。\x1b[0m";

/**
 * One account's terminal: the session's output, its scrollback, and the way in.
 *
 * One per account, never one shared. A shared emulator was handed the output of
 * every running session at once, and a TUI's repaint cannot be told from
 * another's after the two have been written into one screen — the panes were
 * not taking turns, they were overlapping (#57). Separate emulators also decide
 * where input goes: this view writes to `ptyId` and to nothing else, so what is
 * typed reaches the session that is being looked at.
 */
interface SessionView {
  /** The account this terminal belongs to. The identity, so a rename is free. */
  accountId: string;
  /** Empty until the launch returns; nothing may be written before then. */
  ptyId: string;
  /** The name the account had at launch, for a view whose account is gone. */
  name: string;
  command: string;
  cwd: string | null;
  startedAt: string;
  term: Terminal;
  fit: FitAddon;
  host: HTMLElement;
  unlisten: UnlistenFn[];
  /** The topic this terminal belongs to: the one ▶ was pressed in, which is the
   *  one the launch goes into (#141). Set when the terminal is made, since it
   *  decides where the terminal is shown before any launch has answered, and
   *  what reaches this session's record after it ends. */
  topicId: string;
  /**
   * The session id this launch went back into, or null when it started fresh.
   *
   * Half of what says a resume failed. The other half is `seenInRoom` below,
   * and the exit is where the two are read together (#127).
   */
  resumedFrom: string | null;
  /**
   * True once the room's roster has carried this account.
   *
   * The screen's own definition of a session having arrived, and the one the
   * rows already draw 起動中 from: a process is up and the room has not seen it
   * yet (`memberRow`). Raised and never lowered — a session that was in the
   * room and then dropped its connection did arrive, and what this answers is
   * whether it ever did.
   *
   * Read at the exit, because a resume that ends without this having been
   * raised is a resume that went back into nothing: the CLI it was handed to
   * stopped before it started the servers that join the room. That is
   * observable without reading a word the CLI printed, which is what the id
   * being dropped on an error message would have cost (#127).
   */
  seenInRoom: boolean;
  /** How the session ended, or null while it is still running. */
  ended: string | null;
  /**
   * True when the app ended this session itself: the row's ✕, a topic being
   * deleted, the app closing (#121).
   *
   * Carried by the exit event (`PtyExit` in `pty.rs`), not worked out here. An
   * end asked for is not reported as a failure and does not open the pane; an
   * end nobody asked for still does both, because that one is worth checking.
   */
  endRequested: boolean;
  /**
   * True while output is still arriving from this session.
   *
   * Raised by the first byte and lowered by `OUTPUT_QUIET_MS` of silence, so it
   * says "this terminal is printing right now" and not "this terminal has
   * printed at some point". It is the whole of what this screen can observe
   * about a running CLI: the bytes are not read, only counted as having
   * arrived (#82).
   */
  outputting: boolean;
  /**
   * True once this terminal has been observed silent for `OUTPUT_QUIET_MS`, and
   * false again from the next byte.
   *
   * Not the negation of `outputting`. Both are false in the window before a
   * silence has been timed — right after the session is attached, and after a
   * reload picks a running session up again — and that window says nothing: 待機
   * claims a silence this screen measured, never one it assumed (#148).
   */
  silent: boolean;
  /**
   * The last thing this session said about itself, or null while it has said
   * nothing (#155).
   *
   * Held rather than recomputed, and never cleared on its own: the status line
   * runs when the session runs, so a session sitting quiet keeps the values it
   * last reported (decision 4). They survive its exit for the reason the rest
   * of the facts do — the question that column answers is what ran.
   *
   * Null after a reload of this screen, until the next report arrives. The
   * report is not replayed, the same as the address record (#84 / #86): what
   * this screen did not see, it does not say. 制限中 goes with it, since the
   * word is read off two of these values (#161).
   */
  stats: SessionStats | null;
  /** The pending fall back to silence, or undefined when none is armed. */
  quiet: number | undefined;
}

/**
 * How long a terminal must stay silent before its row stops saying that it is
 * printing, and says 待機 instead (#148), in milliseconds.
 *
 * Both halves of this number are load-bearing. Long enough that the gaps inside
 * one burst of output — a TUI's spinner frame, a pause between two paragraphs of
 * a streamed answer — do not read as the session having stopped, which is what
 * makes the word hold still instead of flickering once per repaint. Short enough
 * that a word describing something that has stopped is gone about as fast as a
 * person can look up from the terminal, because a word left standing over a
 * session that has fallen quiet is the failure this feature is most able to
 * cause: 考え中 over a CLI that is in fact sitting at a prompt waiting to be
 * answered (#82).
 *
 * It is also the whole of the redraw budget. The row is redrawn when the word
 * changes and at no other time, so a session printing without pause costs two
 * draws — one when it starts, one when it stops — however many bytes it sends.
 */
const OUTPUT_QUIET_MS = 1000;

/**
 * The terminals this screen holds, by `seatKey`, in launch order.
 *
 * One per account per topic (#141, decision 1). A topic is where a session was
 * started and where it keeps running, so its terminal belongs to the topic:
 * opening another topic puts that topic's terminals on the glass, and the ones
 * left behind keep filling their own buffers (`showTopicTerminals`).
 */
const views = new Map<string, SessionView>();
/**
 * The names the room is waiting on: addressed in a post, and not heard from
 * since.
 *
 * Names rather than account ids, because that is what the room addresses. `to`
 * carries a display name and the room fans every post out regardless, so being
 * addressed is a thing that happens to whoever answers to that name — two
 * participants sharing one are both being asked. The panel's own rows resolve
 * back to it through the roster, which is the same name they are drawn under.
 *
 * Only ever filled by a post arriving while this screen is open. A screen that
 * reloaded into a session already mid-answer has an empty set and says nothing
 * about it, which is the honest answer: the room's history is not replayed here,
 * so nothing on this side knows that account was asked (#84 / #86). Silence is
 * the failure this is allowed to have; a word for a state nobody observed is not
 * (#82).
 */
const awaiting = new Map<string, Set<string>>();
/**
 * The account whose terminal is on the glass, in the topic on the glass, or
 * null when none is.
 */
let shownAccount: string | null = null;
/**
 * Which account's terminal each topic was showing when it was left, by topic id.
 *
 * So coming back to a topic finds the pane that was being watched in it, and
 * not whichever pane the topic just left had on the glass (#141).
 */
const shownByTopic = new Map<string, string | null>();
/** The size every terminal on this screen is currently drawn at, in `px`. */
let terminalFontSize = DEFAULT_TERMINAL_FONT_SIZE;
/**
 * Why an account's last launch failed, by `seatKey`, until it is tried again.
 *
 * The status line carries the app's own reason and is the full account of it,
 * but it is one line for the whole screen and the next thing written takes it.
 * A launch that failed leaves nothing else behind — its terminal is discarded,
 * there being no session under it — so without this the row that was pressed
 * goes back to reading 未起動, as though it never had been.
 */
const launchFailures = new Map<string, string>();
/**
 * The terminal the open 終了 dialog is asking about, by `seatKey`, or null while
 * it is closed.
 *
 * The key rather than the view: the dialog stays open across whatever else the
 * screen does, and a view can be discarded while it is (`showView`). Resolving
 * the key when the answer comes back finds a session that is still there, or
 * finds nothing and ends nothing. The topic is in the key because the same
 * account may be running in two topics, and the one asked about is the one whose
 * ✕ was pressed (#141).
 */
let endingAccount: string | null = null;
/**
 * What is waiting on the open アプリの終了 dialog, or null while it is closed.
 *
 * A resolver rather than an id, because the caller is not a click that can be
 * left to finish on its own: the window's close is held open across this
 * question, and the answer is what releases it (`onQuitRequested`). Holding the
 * resolver is what makes every way out of the dialog — either button, Escape —
 * an answer rather than a question nobody is left to answer.
 */
let quitAnswer: ((confirmed: boolean) => void) | null = null;
/**
 * Whether a close is already being decided.
 *
 * Separate from `quitAnswer`, which is only set once the app has answered who
 * is running — the window can be closed again inside that gap, and a flag that
 * is not up yet would let a second decision start. The title bar is the native
 * one (`tauri.conf.json` leaves `decorations` at its default), so its `✕` sits
 * outside the webview and stays clickable while the modal is up: a second close
 * while the question stands is a normal thing to do, not a corner.
 */
let quitPending = false;

/**
 * One seat, or one terminal: an account in a topic.
 *
 * The pair is the key because it is the unit (#141). A newline cannot occur in
 * either id, so no two pairs join to the same string.
 */
function seatKey(topicId: string, accountId: string): string {
  return `${topicId}\n${accountId}`;
}

/** The topic on the glass, or "" before the app has answered which it is. */
function shownTopicId(): string {
  return currentTopic?.topic_id ?? "";
}

/** The roster of the topic on the glass. */
function shownRoster(): Participant[] {
  return rosters.get(shownTopicId()) ?? [];
}

/** The terminals of the topic on the glass, in launch order. */
function topicViews(): SessionView[] {
  return [...views.values()].filter((view) => view.topicId === shownTopicId());
}

/** Whether an account holds a seat in any topic. */
function seatedAnywhere(accountId: string): boolean {
  return [...seated.values()].some((seat) => seat.account_id === accountId);
}

function status(text: string, kind: "info" | "error" = "info"): void {
  statusEl.textContent = text;
  statusEl.dataset.kind = kind;
}

function revealDiagnostics(): void {
  diagnosticsEl.hidden = false;
  toggleEl.setAttribute("aria-expanded", "true");
  // The container has no size while hidden, so the fit has to wait for layout.
  requestAnimationFrame(() => fitShown());
}

/**
 * Fold the pane away. Nothing under it stops.
 *
 * Every terminal keeps its session, its listeners and its scrollback, so the
 * pane comes back with the same tabs on it. The two ways in and out are the
 * 端末 button in the title bar and ✕ in the pane's own header: one is reachable
 * while the pane is folded and the other while it is open, which is why both
 * exist for one act (#68).
 */
function hideDiagnostics(): void {
  diagnosticsEl.hidden = true;
  toggleEl.setAttribute("aria-expanded", "false");
}

/**
 * Draw both panels the way `panels` says, and say so on their buttons.
 *
 * One function for both states and both panels, called from the restore and
 * from every toggle. The button's `aria-expanded` is set from the same value
 * that folds the panel, so there is no path where the bar says one thing and
 * the window shows another.
 *
 * Folding is `hidden` and nothing else. Nothing is torn down: both panels are
 * drawn from events that keep arriving whether or not they are on screen — the
 * roster, the seats, the topic index — so a panel opened after a session ended
 * behind it opens showing that it ended (#118, 制約).
 */
function applyPanels(): void {
  historyEl.hidden = !panels.history;
  participantsEl.hidden = !panels.participants;
  toggleHistoryEl.setAttribute("aria-expanded", String(panels.history));
  toggleParticipantsEl.setAttribute("aria-expanded", String(panels.participants));
}

/**
 * Fold one panel away, or bring it back, and remember which.
 *
 * The write goes to disk on every press rather than at the end of the session:
 * the app is closed by the window's `✕` and by whatever ends it less politely,
 * and a layout only saved on the way out is one the second of those loses.
 */
function togglePanel(which: keyof PanelState): void {
  panels = { ...panels, [which]: !panels[which] };
  applyPanels();
  saveConfig();
}

/** The terminal currently on the glass, or null when none is. */
function shownView(): SessionView | null {
  return shownAccount === null
    ? null
    : (views.get(seatKey(shownTopicId(), shownAccount)) ?? null);
}

/** What a view is called now — its account's current name, renames included. */
function viewName(view: SessionView): string {
  return accounts.find((account) => account.id === view.accountId)?.name || view.name;
}

/**
 * Lay out the terminal that is showing, and tell its session the new size.
 *
 * Only that one. A hidden pane has no size to fit against, and a session told
 * it has zero columns draws for a window it does not have.
 */
function fitShown(): void {
  const view = shownView();
  if (!view || diagnosticsEl.hidden) return;
  try {
    view.fit.fit();
  } catch {
    // A fit against a zero-sized container is not worth a message.
    return;
  }
  showWindowSize();
  if (view.ptyId === "" || view.ended !== null) return;
  void invoke("resize_pty", {
    id: view.ptyId,
    cols: view.term.cols,
    rows: view.term.rows,
  }).catch(() => {
    // The session may have exited between the fit and the call.
  });
}

/**
 * Lay out one terminal against the folded pane, without opening the pane.
 *
 * A launch has to hand its PTY a size before the CLI's first paint, and a
 * folded pane has none to measure — the same limit `fitShown` has. So the pane
 * is unfolded for the length of one fit and folded again in the same task: the
 * browser paints nothing in between, and the person sees the pane stay folded
 * (#215).
 *
 * The session is not told anything here. There is none yet, and the size goes
 * out with `start_session`. When the pane is opened later `revealDiagnostics`
 * fits again, which is what catches a window resized in the meantime.
 */
function fitFolded(view: SessionView): void {
  if (!diagnosticsEl.hidden) return;
  diagnosticsEl.hidden = false;
  try {
    view.fit.fit();
  } catch {
    // Same as `fitShown`: the default size is what the PTY starts at then.
  } finally {
    diagnosticsEl.hidden = true;
  }
}

/**
 * The size the CLI is laid out for.
 *
 * A TUI that is drawing at the wrong size looks like a broken TUI, and the
 * number it was given is the one thing that says which of the two it is.
 */
function showWindowSize(): void {
  const view = shownView();
  windowEl.textContent = view ? `${view.term.cols}×${view.term.rows}` : "—";
}

/** Render saved arguments back into an editable line. */
function joinArgs(args: string[]): string {
  return args.map((arg) => (arg === "" || arg.includes(" ") ? `"${arg}"` : arg)).join(" ");
}

/** The account the person at this screen is, or null before it is resolved. */
function localAccount(): Account | null {
  return accounts.find((account) => account.id === localAccountId) ?? null;
}

/**
 * Write the config back to disk: the accounts, and which panels are open.
 *
 * One function for the whole file, because that is what a save is — the command
 * takes an `AppConfig` and writes it whole, so a second saver that knew about
 * only one of the two fields would drop the other every time it ran.
 *
 * Called when an account is decided, deleted, or created for the person at this
 * screen — never from a field losing focus. An account is a thing that exists
 * whether or not it runs (#53), and the field-by-field save made every keystroke
 * on the way to a name into a state that existed: pressing ＋ created an
 * account, and from there the only way out was to delete it (#59). A panel is
 * the other shape: folding one is the whole act, so it saves as it happens
 * (#118, decision 1).
 */
/**
 * Write the accounts and panels to `config.json`. Resolves false when that
 * failed, which is said on the status line either way; most callers do not wait
 * for it, and the one that starts a server under a new account does (#200).
 */
function saveConfig(): Promise<boolean> {
  return invoke("save_config", { config: { accounts, panels } }).then(
    () => true,
    () => {
      status("設定を保存できませんでした。", "error");
      return false;
    },
  );
}

/**
 * Fill the text size picker.
 *
 * Labelled as a proportion of the default rather than in `rem`, because the
 * choice being made is "larger or smaller than what I have", and the unit the
 * size happens to be held in answers a question nobody is asking.
 */
function fillRoomFontSizes(select: HTMLSelectElement): void {
  for (const size of ROOM_FONT_SIZES) {
    const option = document.createElement("option");
    option.value = String(size);
    option.textContent = `${Math.round((size / DEFAULT_ROOM_FONT_SIZE) * 100)}%`;
    select.appendChild(option);
  }
}

/**
 * The stored size, or the default.
 *
 * Only a size that is on the ladder is honoured. What is in `localStorage` was
 * written by some version of this app and can be anything — a rung that a
 * later version dropped, a value left by hand, or nothing at all — and the
 * failure it would cause is silent: a size off the ladder cannot be stepped
 * from, so the keys and the picker would both stop working with nothing on
 * screen saying why.
 */
function storedRoomFontSize(): number {
  const stored = Number(localStorage.getItem(ROOM_FONT_SIZE_KEY));
  return ROOM_FONT_SIZES.includes(stored) ? stored : DEFAULT_ROOM_FONT_SIZE;
}

/**
 * Draw the conversation at `size`, and remember it if it was chosen.
 *
 * The property goes on the two elements that render the conversation's words —
 * `#room` and the composer's textarea — and on nothing else. What is typed is
 * the same sentence that is then read, so the two move together (#81). Their
 * nearest shared ancestor is `#conversation`, which also holds the diagnostics
 * pane and the status line; setting it there, or on the root, would reach
 * surfaces that are not on this axis, and the terminal computes its columns and
 * rows from its own size. Two `setProperty` calls make the scope the placement
 * itself, so nothing has to be cancelled anywhere.
 *
 * The row under the text — 宛先, the keys, 送信 (#222) — and the list `@` opens
 * sit in the composer but do not follow. They are controls, not the sentence,
 * and they stay on the whole-UI axis (#66, #204).
 *
 * `save` is false for the restore at startup. Writing the value back there
 * would put a size in storage for a screen that never chose one, which is the
 * one state this is supposed to leave alone.
 */
function applyRoomFontSize(size: number, save: boolean): void {
  roomFontSize = size;
  // Divided by the UI's multiple, so the size is this axis's alone: the root a
  // `rem` is counted from is what the UI scale moves (#194), and dividing it back
  // out is what keeps the two axes from riding on each other.
  const value = `calc(${size}rem / var(--ui-scale))`;
  roomEl.style.setProperty("--room-font-size", value);
  inputEl.style.setProperty("--room-font-size", value);
  // Kept in step with the keys, which move the size without the picker.
  settingsRoomFontSizeEl.value = String(size);
  if (save) localStorage.setItem(ROOM_FONT_SIZE_KEY, String(size));
}

/**
 * Move one rung, or back to the default when `step` is zero.
 *
 * The ends hold: stepping past either one lands on it again, so there is no
 * size to reach that cannot be read or does not fit.
 */
function stepRoomFontSize(step: number): void {
  if (step === 0) {
    applyRoomFontSize(DEFAULT_ROOM_FONT_SIZE, true);
    return;
  }
  const at = ROOM_FONT_SIZES.indexOf(roomFontSize);
  const next = Math.min(Math.max(at + step, 0), ROOM_FONT_SIZES.length - 1);
  applyRoomFontSize(ROOM_FONT_SIZES[next], true);
}

/** Fill a terminal size picker. Labelled in `px`; see the ladder above. */
function fillTerminalFontSizes(select: HTMLSelectElement): void {
  for (const size of TERMINAL_FONT_SIZES) {
    const option = document.createElement("option");
    option.value = String(size);
    option.textContent = `${size}px`;
    select.appendChild(option);
  }
}

/**
 * The stored terminal size, or the default.
 *
 * Only a size on the ladder is honoured, for the reason `storedRoomFontSize`
 * gives: a value off it cannot be stepped from, so the picker would stop working
 * with nothing on screen saying why.
 */
function storedTerminalFontSize(): number {
  const stored = Number(localStorage.getItem(TERMINAL_FONT_SIZE_KEY));
  return TERMINAL_FONT_SIZES.includes(stored) ? stored : DEFAULT_TERMINAL_FONT_SIZE;
}

/**
 * Draw every terminal at `size`, and remember it if it was chosen.
 *
 * Every one, not only the one on the glass. The size is the screen's, so a pane
 * switched to later must not be the odd one out; and a terminal opened after
 * this reads the same value (`openView`).
 *
 * The re-fit that follows only reaches the shown pane, which is the same limit
 * `fitShown` has always had — a hidden container has no size to measure against.
 * The others are laid out when they are next shown, because `showView` fits what
 * it puts on the glass. Their sessions are told the new column count at that
 * moment rather than this one.
 *
 * `save` is false for the restore at startup, so a screen that never chose is
 * not given a stored size by being opened.
 */
function applyTerminalFontSize(size: number, save: boolean): void {
  terminalFontSize = size;
  for (const view of views.values()) view.term.options.fontSize = size;
  settingsTerminalFontSizeEl.value = String(size);
  if (save) localStorage.setItem(TERMINAL_FONT_SIZE_KEY, String(size));
  fitShown();
}

/** Fill the UI scale picker. Labelled as a percentage, as #60's is. */
function fillUiScales(): void {
  for (const scale of UI_SCALES) {
    const option = document.createElement("option");
    option.value = String(scale);
    option.textContent = `${Math.round((scale / DEFAULT_UI_SCALE) * 100)}%`;
    settingsUiScaleEl.appendChild(option);
  }
}

/**
 * The stored UI scale, or the default. Only a rung on the ladder is honoured,
 * for the reason `storedRoomFontSize` gives.
 */
function storedUiScale(): number {
  const stored = Number(localStorage.getItem(UI_SCALE_KEY));
  return UI_SCALES.includes(stored) ? stored : DEFAULT_UI_SCALE;
}

/**
 * Draw the UI at `scale`, and remember it if it was chosen.
 *
 * One property on `:root`, which src/styles.css turns into the root's
 * `font-size`. The conversation divides it back out and the terminal is in `px`,
 * so neither moves (see `UI_SCALE_KEY`). The terminal's pane does change size
 * when the chrome around it grows, and the `ResizeObserver` on it re-fits and
 * tells the session, as it does for any other change to the pane's size.
 *
 * `save` is false for the restore at startup, as for the two other axes.
 */
function applyUiScale(scale: number, save: boolean): void {
  document.documentElement.style.setProperty("--ui-scale", String(scale));
  settingsUiScaleEl.value = String(scale);
  if (save) localStorage.setItem(UI_SCALE_KEY, String(scale));
}

/**
 * Fill a hue picker, with "not declared" first.
 *
 * Both pickers are filled from the one list, because a person and a session
 * declare a colour from the same set. Not declaring is an option rather than an
 * omission: a participant who chose no colour still joins, and the room derives
 * one for them from their name.
 */
function fillHues(select: HTMLSelectElement, saved: string | null): void {
  const none = document.createElement("option");
  none.value = "";
  none.textContent = "既定（名前から）";
  select.appendChild(none);

  for (const { label, hue } of HUES) {
    const option = document.createElement("option");
    option.value = String(hue);
    option.textContent = label;
    select.appendChild(option);
  }
  select.value = saved !== null && HUES.some(({ hue }) => String(hue) === saved) ? saved : "";
}

/** The hue a picker currently declares, or null for "not declared". */
function declaredHue(select: HTMLSelectElement): number | null {
  return select.value === "" ? null : Number(select.value);
}

/**
 * The hue a participant's name lands on.
 *
 * From the name, so the same participant is the same colour every time they
 * speak and in the roster beside their name. Not from arrival order: a
 * participant who reconnects would come back a different colour, and the
 * colour would then say when they joined rather than who they are.
 */
function hueFor(name: string): number {
  // FNV-1a. Any stable spread would do; this one is four lines.
  let hash = 2166136261;
  for (let i = 0; i < name.length; i += 1) {
    hash ^= name.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return (ACCENT_HUE + RESERVED_ARC + (Math.abs(hash) % DERIVED_ARC)) % 360;
}

/**
 * The colour a participant is drawn in.
 *
 * Lightness and chroma are the accent's, in whichever theme is showing; only
 * the hue turns. One ladder, in this order:
 *
 *   declared > oneself's accent > derived from the name
 *
 * A declaration outranks both. Derivation is stable but not choosable, and the
 * three names actually in use landed inside a 75° band — at the accent's low
 * chroma that is not a difference anyone can read, so colour stopped doing the
 * one job it was added for (#40).
 *
 * That the accent is below the declaration is the deliberate half. Oneself
 * keeps it while nothing is declared, so nothing changes for a participant who
 * declares nothing; declaring a colour takes it, because a declared colour that
 * showed to everyone except the person who declared it is not the colour they
 * declared. What then says which participant is oneself is the roster's
 * 「（あなた）」 and the name on the line — colour was the faster of the three
 * carriers, never the only one, and it is the one that is now chosen rather
 * than assigned.
 */
function speakerColor(name: string, hue: number | null, own: boolean): string {
  if (hue !== null) return `oklch(var(--speaker-l) var(--speaker-c) ${hue.toFixed(1)})`;
  if (own) return "var(--accent)";
  return `oklch(var(--speaker-l) var(--speaker-c) ${hueFor(name).toFixed(1)})`;
}

function shortTime(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** The local calendar day a stamp falls on, as a key two stamps compare by. */
function dayKey(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return `${at.getFullYear()}-${at.getMonth() + 1}-${at.getDate()}`;
}

/**
 * A day, as short as it can be said and still place it (#225): 今日, else the
 * month and the day, and the year only when it is not this one.
 *
 * One wording for the two places a day is shown — the room's day dividers and
 * the topic list — so a topic and the lines in it name their day alike. The
 * clock is not part of it: a line carries its own, and a topic's is on its
 * tooltip.
 */
function dayLabel(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const now = new Date();
  if (dayKey(iso) === dayKey(now.toISOString())) return "今日";
  const md = `${at.getMonth() + 1}月${at.getDate()}日`;
  return at.getFullYear() === now.getFullYear() ? md : `${at.getFullYear()}年${md}`;
}

/** The day and the clock in full, for a tooltip under a short label. */
function fullDateTime(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const day = at.toLocaleDateString([], { year: "numeric", month: "2-digit", day: "2-digit" });
  return `${day} ${at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

/**
 * One line of the room, whenever it was said.
 *
 * The same element for a post arriving now and a post read back out of the
 * topic's log, because they are the same conversation: picking a topic puts its
 * posts in the room rather than beside it (#115, decision 2). The stamp is the
 * clock alone for both: the day is the divider above the line (`placeDay`,
 * #225).
 *
 * Who said it is a circle carrying the speaker's initial, on a tint of their
 * own colour (#225). It replaced the colour bar down the line's left edge; the
 * colour is the same one, so the circle and the dot beside the name in the
 * panel still match.
 *
 * The screen person's own line is a bubble at the right edge (#185) and carries
 * neither the circle nor the name: where it stands and its tint already say
 * whose it is. Its clock is at its foot, and a run of them shows only the last
 * one's (src/styles.css, `.message.mine`).
 */
function roomLine(line: {
  speaker: string;
  colour: string;
  /** Empty for a line said to the room. */
  to: string[];
  ts: string;
  stamp: string;
  content: string;
  past: boolean;
  /** Drawn at the right edge, as the screen person's own words (#185). */
  mine: boolean;
}): HTMLElement {
  const article = document.createElement("article");
  article.className = "message";
  if (line.past) article.classList.add("past");
  if (line.mine) article.classList.add("mine");
  // The screen person's lines take their colour from the room, not from the
  // moment they were drawn, so a colour chosen in the account reaches every one
  // of them at once — the live and the read-back alike (#189, `paintMine`).
  article.style.setProperty("--speaker", line.mine ? MINE_SPEAKER : line.colour);

  const head = document.createElement("div");
  head.className = "meta";

  if (!line.mine) {
    article.appendChild(avatar(line.speaker));
    const speaker = document.createElement("span");
    speaker.className = "speaker";
    speaker.textContent = line.speaker;
    head.appendChild(speaker);
  }

  if (line.to.length) {
    const to = document.createElement("span");
    to.className = "to";
    to.textContent = `→ ${line.to.join("、")}`;
    head.appendChild(to);
  }

  const time = document.createElement("time");
  time.className = "ts";
  time.dateTime = line.ts;
  time.textContent = line.stamp;
  time.title = fullDateTime(line.ts);

  const body = document.createElement("div");
  body.className = "body";
  body.textContent = line.content;

  if (line.mine) {
    // The clock goes to the bubble's foot, so a run of them can keep only the
    // last one's. A head is drawn only when it has an addressee to carry.
    if (head.childElementCount) article.appendChild(head);
    article.append(body, time);
  } else {
    head.appendChild(time);
    article.append(head, body);
  }
  return article;
}

/**
 * The circle that says who a line is from (#225): the name's first character on
 * a tint of the speaker's colour, taken from `--speaker` on the element it
 * stands in. Hidden from a screen reader, which reads the name beside it.
 */
function avatar(name: string): HTMLElement {
  const mark = document.createElement("span");
  mark.className = "avatar";
  mark.setAttribute("aria-hidden", "true");
  mark.textContent = (Array.from(name.trim())[0] ?? "?").toUpperCase();
  return mark;
}

/**
 * Put a day divider in the room ahead of a line, when the line is the first of
 * its day on the glass (#225). The day was on every read-back line's stamp
 * until then; it is said once, between the days, and the lines keep the clock.
 *
 * The day the room last drew is `roomDay`, cleared with the room in
 * `drawTopic`. A divider says 今日 by the clock it was drawn at, so when a new
 * one is drawn every divider on the glass is relabelled — a room left open
 * past midnight does not end up with two of them saying 今日.
 */
let roomDay = "";

function placeDay(ts: string): void {
  const key = dayKey(ts);
  if (key === "" || key === roomDay) return;
  roomDay = key;
  const divider = document.createElement("div");
  divider.className = "day";
  divider.setAttribute("role", "separator");
  divider.dataset.ts = ts;
  divider.appendChild(document.createElement("span"));
  roomEl.appendChild(divider);
  for (const one of roomEl.querySelectorAll<HTMLElement>(":scope > .day")) {
    one.firstElementChild!.textContent = dayLabel(one.dataset.ts ?? "");
  }
}

/**
 * The name notices were posted under before they were said as an account
 * (#169 → #193). Read, never written: a line in an older topic's log carries this
 * name and no account, and it is folded as it was when it was said.
 */
const LEGACY_NOTICE_SPEAKER = "webhook";

/**
 * The fold a line goes into, or null for a line drawn as it stands (#169 / #193).
 *
 * A line said as an `mcp` account is folded: what a local MCP server says is a
 * notice, and a run of notices is not read line by line. Decided on the account
 * the line carries, which a live line and one read back from the log both have
 * (`LoggedPost`) — so the two fold alike, and a rename or a second account on the
 * same name moves nothing. The kind is this screen's list's: an account deleted
 * since is no longer one, and its lines are drawn as they stand.
 *
 * Folds are one account's: a run under one server is one fold, and another
 * server speaking starts a fold of its own. The fold is drawn in the account's
 * colour and headed with its name as it is now, the live and the read-back alike
 * — the account is this screen's to read, as the screen person's own is (#189).
 */
interface Fold {
  key: string;
  label: string;
  colour: string;
}

function foldOf(speaker: string, account: string | null | undefined): Fold | null {
  if (account) {
    const owner = accounts.find((one) => one.id === account);
    if (owner?.kind !== "mcp") return null;
    return {
      key: owner.id,
      label: owner.name,
      colour: speakerColor(owner.name, owner.hue, false),
    };
  }
  if (speaker === LEGACY_NOTICE_SPEAKER) {
    return {
      key: `legacy:${speaker}`,
      label: speaker,
      colour: speakerColor(speaker, null, false),
    };
  }
  return null;
}

/**
 * Whether a line is drawn at the right edge, as the screen person's (#185).
 *
 * Decided by the name: a line read back from the log carries no `own`, and a
 * live line and a read-back one must land on the same side. `own` would answer
 * the live half better — the room decides it on the connection — but it has no
 * read-back half, and a rule that switched between the two would move a line
 * from one side to the other when its topic is picked again. So this is where a
 * line sits, not who said it: someone joining under this screen's name is drawn
 * on this side too, and after a rename the lines said under the old name return
 * to the left once redrawn. Colour keeps its own ladder (`speakerColor`); only
 * the side is decided here. A folded line is never on this side (#185).
 */
function isMine(speaker: string, fold: Fold | null): boolean {
  return fold === null && speaker === localName();
}

/**
 * The colour of the screen person's lines, as one value on the room (#189).
 *
 * A line's colour was fixed when it was drawn, so a colour chosen in the
 * account reached only what was said after it, and a read-back line never
 * reached it at all: the log carries no hue, and the name-derived one is not
 * the one chosen. The screen person's lines are the one set whose colour this
 * screen knows without the room telling it — it is the account's — so they
 * point at this value instead, and setting it repaints all of them.
 *
 * The same ladder as every other line (`speakerColor`), read as oneself: the
 * account's hue if one is chosen, else the accent. Which lines are the screen
 * person's is `isMine`'s answer, by name, so this colour goes where the right
 * edge does — a line said under this screen's name by someone else included,
 * the same cost the side already carries.
 */
const MINE_SPEAKER = "var(--mine-speaker, var(--accent))";

function paintMine(): void {
  roomEl.style.setProperty(
    "--mine-speaker",
    speakerColor(localName(), localAccount()?.hue ?? null, true),
  );
}

/**
 * Put one line in the room, folding notices away (#169 / #193).
 *
 * A notice is drawn folded, and a run of them — everything one server says until
 * someone else speaks — is one fold, headed with how many it holds and opened by
 * a click. The fold is the screen's alone: each notice is still its own post in
 * the room, in the log and typed into every session's terminal, and nothing but
 * this drawing groups them. Which lines fold is `foldOf`'s answer.
 */
function placeLine(line: HTMLElement, fold: Fold | null): void {
  if (!fold) {
    roomEl.appendChild(line);
    return;
  }
  const last = roomEl.lastElementChild;
  let box =
    last instanceof HTMLDetailsElement &&
    last.classList.contains("notice-fold") &&
    last.dataset.fold === fold.key
      ? last
      : null;
  if (!box) {
    box = document.createElement("details");
    box.className = "notice-fold";
    box.dataset.fold = fold.key;
    box.style.setProperty("--speaker", fold.colour);
    // Headed by the server's circle, as its lines are (#225).
    const summary = document.createElement("summary");
    const label = document.createElement("span");
    label.className = "label";
    summary.append(avatar(fold.label), label);
    box.appendChild(summary);
    roomEl.appendChild(box);
  }
  box.appendChild(line);
  const count = box.querySelectorAll(":scope > .message").length;
  box.querySelector("summary > .label")!.textContent = `${fold.label} ${count} 件`;
}

function appendMessage(message: RoomMessage): void {
  // The room is scrolled to the bottom only when it already was, so reading
  // back through the log is not yanked away by an arriving message.
  const atBottom = roomEl.scrollHeight - roomEl.scrollTop - roomEl.clientHeight < 40;

  const fold = foldOf(message.speaker, message.account);
  placeDay(message.ts);
  placeLine(
    roomLine({
      speaker: message.speaker,
      // `own` rather than a name test: the room decides self on the connection
      // a post arrived on, which a rename cannot blur (#40). A folded line takes
      // its account's colour, so it is the colour it has when read back (#193).
      colour: fold?.colour ?? speakerColor(message.speaker, message.hue, message.own),
      to: message.to,
      ts: message.ts,
      stamp: shortTime(message.ts),
      content: message.content,
      past: false,
      mine: isMine(message.speaker, fold),
    }),
    fold,
  );
  // On the glass, so it is what this screen can declare having seen. Own posts
  // included: the room does not hold a speaker's own posts against them, and
  // carrying the newest id either way keeps this one value rather than two.
  lastSeenId = message.message_id;
  drawnIds.add(message.message_id);

  if (atBottom) roomEl.scrollTop = roomEl.scrollHeight;
}

/**
 * Put a topic's posts in the room, in place of whatever was there.
 *
 * Picking a topic is picking where the conversation is, so what is drawn is the
 * topic itself and not a strip beside it. #48 held the opposite — the room
 * started empty and the past was read in a column of its own — and #115
 * overwrites that line: the column is a list of topics now, and the room is
 * where a topic is read.
 *
 * The watermark is the last line drawn. A topic's room keeps its floor while the
 * screen is elsewhere (#141) — nothing empties it on the way in any more — and
 * every post that floor holds was written to the log inside the acquisition
 * that admitted it, so the newest line read back is the newest post on the
 * floor whenever the floor holds any. When it holds none, or the line is from
 * an earlier run, the room reads the watermark as naming nothing, which costs
 * one refusal and draws nothing twice (`drawMissed`).
 *
 * The ids are kept, because they are on the glass: a refusal that hands back
 * the whole retained floor must not draw a line twice (#108).
 *
 * The colour is derived from the name and `own` is false for every line. The
 * log carries neither declaration (see `LoggedPost`). The screen person's own
 * lines are the exception: they take this screen's colour from `paintMine`,
 * which is the account's and needs nothing from the log (#189).
 */
function drawTopic(posts: LoggedPost[]): void {
  roomEl.replaceChildren();
  roomDay = "";
  lastSeenId = posts[posts.length - 1]?.message_id ?? null;
  drawnIds.clear();

  for (const post of posts) {
    const fold = foldOf(post.speaker, post.account);
    placeDay(post.ts);
    placeLine(
      roomLine({
        speaker: post.speaker,
        colour: fold?.colour ?? speakerColor(post.speaker, null, false),
        to: post.to ?? [],
        ts: post.ts,
        // The clock alone: a topic spans days, and the day is the divider
        // above (`placeDay`, #225).
        stamp: shortTime(post.ts),
        content: post.content,
        past: true,
        mine: isMine(post.speaker, fold),
      }),
      fold,
    );
    drawnIds.add(post.message_id);
  }

  // Opened at the end, which is where the conversation continues.
  roomEl.scrollTop = roomEl.scrollHeight;
}

/**
 * The room's heading on the title bar (#225): the topic on the glass, and how
 * many are in its room.
 *
 * The name is the one its row carries (`topicName`), so the two never say it
 * differently. A topic the index does not carry yet has no row and no title —
 * the moment between a launch or 新規 and the first post (#115) — and is called
 * what it is, 新しいトピック.
 *
 * The count is the room's roster for that topic: who is connected to it now.
 * An account that is not running is not in it, and neither is a local MCP
 * server, which takes no seat (#193) — the panel lists both, and this counts
 * only who is here.
 */
function renderRoomTitle(): void {
  const current = currentTopic;
  const topic = current ? topics.find((one) => one.topic_id === current.topic_id) : undefined;
  const name = current === null ? "" : topic ? topicName(topic) : "新しいトピック";
  roomTitleNameEl.textContent = name;
  roomTitleNameEl.title = name;
  roomTitleNameEl.classList.toggle("unnamed", topic !== undefined && !topic.title);
  roomTitleCountEl.textContent = current === null ? "" : `${shownRoster().length} 人`;
}

/**
 * The topics, newest first, with the current one marked.
 *
 * Newest first because that is the end a list of conversations is read from.
 * The file keeps them in the order they happened, which is the order an append
 * produces; the reversal is a reading decision rather than a storage one.
 *
 * A topic the index does not carry is not drawn as a row (#125, 決定1). A launch
 * opens a new topic and writes nothing until something is said in it, so at the
 * top of every run there is exactly one topic in the room and not in the list
 * (#115) — and this list used to put a row there for it. The row named a
 * conversation nobody had started; 新規 above the list names the place one
 * starts, which is the true thing about that state, and it is drawn as picked
 * while the room is in it (#125, AI 判断4). Without that the room would be
 * somewhere no part of the list showed, which is the fault the row was avoiding.
 *
 * Every row here is therefore a row of the index, which is what lets ✕ reach
 * the ones with no title (#125, 決定2 — see `topicRow`).
 */
function renderTopics(): void {
  topicListEl.replaceChildren();

  const listed = [...topics].reverse();
  const current = currentTopic;
  const unlisted = current !== null && !listed.some((one) => one.topic_id === current.topic_id);
  topicNewEl.classList.toggle("current", unlisted);
  // A launch realises its topic, so a session running in a topic the list does
  // not carry is the moment between ▶ and the launch answering. 新規 is where
  // that topic is drawn, so the mark goes there for that moment (#141).
  topicNewEl.classList.toggle("running", unlisted && current !== null && runsIn(current.topic_id));
  renderRoomTitle();

  if (listed.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = "トピックなし";
    topicListEl.appendChild(empty);
    return;
  }

  for (const topic of listed) topicListEl.appendChild(topicRow(topic));
}

/**
 * One row of the list: the row opens the topic, and ✕ deletes it.
 *
 * The mark is the one the participant panel's 終了 uses, for the same reason it
 * is used there — this is the control on the row that cannot be taken back, and
 * it sits beside one that merely changes what is showing.
 *
 * **Every row carries one, a row with no title included (#125, 決定2).** #119
 * decision 5 withheld it from the untitled ones because "neither is a topic
 * anyone has a reason to delete from here", and one of that pair was the current
 * topic before anything had been said in it — which is no longer a row at all
 * (#125, 決定1). What is left reading as untitled is a row the index carries:
 * the orphan whose file is gone (#117), and the entry a launch wrote by
 * recording a session it never spoke in (`record_session`). Both are rows
 * someone wants gone, and until now the list held them with no way to say so.
 * Decision 5 is not overturned; its premise left with the row it was about.
 */
function topicRow(topic: Topic): HTMLLIElement {
  const row = document.createElement("li");
  row.className = "topic";
  if (topic.topic_id === currentTopic?.topic_id) row.classList.add("current");

  const pick = document.createElement("button");
  pick.type = "button";
  pick.className = "pick";

  const name = document.createElement("span");
  name.className = topic.title ? "name" : "name unnamed";
  name.textContent = topicName(topic);
  pick.appendChild(name);

  // Sessions are left running when the topic is (#141, decision 2), so a topic
  // not on the glass can be holding one — and a session nobody can see running
  // is one nobody remembers to end. The list is the one place every topic is
  // on screen at once, which is why the mark is here (AI 判断5).
  if (runsIn(topic.topic_id)) {
    row.classList.add("running");
    const mark = document.createElement("span");
    mark.className = "running-mark";
    mark.textContent = "●";
    mark.title = "セッションが走っています";
    mark.setAttribute("aria-label", "セッションが走っています");
    pick.appendChild(mark);
  }

  const when = document.createElement("span");
  when.className = "when";
  // The day only, short (#225): 今日, or 9月28日. A list of topics spans
  // months, and the day is the part that places one; the clock is on the
  // tooltip for when it is wanted.
  when.textContent = dayLabel(topic.created_at);
  when.title = fullDateTime(topic.created_at);
  pick.appendChild(when);

  pick.addEventListener("click", () => void openTopic(topic));
  // Renamed where it is read. The generated title is a starting point in an
  // editable field, the same thing an account's name is (#115, decision 9).
  pick.addEventListener("dblclick", (event) => {
    event.preventDefault();
    beginRename(row, topic);
  });

  row.appendChild(pick);
  row.appendChild(deleteButton(topic));
  return row;
}

/**
 * Whether a topic holds a seat: a session running in it, or a launch into it
 * still in flight.
 *
 * The launch counts. It is about to be a running session, and a mark that
 * arrived only once the CLI had spawned would say nothing about the one
 * pressed ▶ just now in a topic that was then left.
 */
function runsIn(topicId: string): boolean {
  return [...seated.values()].some((seat) => seat.topic_id === topicId);
}

/**
 * What a topic is called on the screen.
 *
 * A row the index carries with nothing said in it has no title, and the screen
 * calls it 未記入 (#125, AI 判断3) — everywhere it is named, so that a sentence
 * about it never comes out as 「」. Said rather than left blank: blank is also
 * what a row would look like if its title had failed to read.
 *
 * Not written into the index. The word is what the screen says about an empty
 * title, not a title, which is why renaming such a row opens on an empty field
 * rather than on this.
 */
function topicName(topic: Topic): string {
  return topic.title || "未記入";
}

/** The ✕ on one row. It asks; `#topic-delete-dialog` is where it is answered. */
function deleteButton(topic: Topic): HTMLButtonElement {
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "delete";
  remove.appendChild(icon("close"));
  remove.title = `「${topicName(topic)}」を削除する`;
  remove.setAttribute("aria-label", `トピック「${topicName(topic)}」を削除する`);
  remove.addEventListener("click", () => void openTopicDeleteDialog(topic));
  return remove;
}

/**
 * Rename one topic in place.
 *
 * Escape and losing focus both mean 取消, Enter means 決定 — the three ways out
 * the account form has. The row is redrawn from the list on every path, so none
 * of them leaves half an edit applied.
 */
function beginRename(row: HTMLLIElement, topic: Topic): void {
  const field = document.createElement("input");
  field.type = "text";
  field.className = "rename";
  field.value = topic.title;
  field.spellcheck = false;

  let settled = false;
  const cancel = (): void => {
    if (settled) return;
    settled = true;
    renderTopics();
  };
  const commit = (): void => {
    if (settled) return;
    settled = true;
    const title = field.value.trim();
    if (!title || title === topic.title) {
      renderTopics();
      return;
    }
    void invoke("room_rename_topic", { topicId: topic.topic_id, title })
      .then(() => status(`トピックを「${title}」にしました。`))
      .catch((err) => status(`トピック名を変えられませんでした: ${err}`, "error"))
      // The list arrives on `room-topics` when the rename lands. This redraw is
      // for the path where it did not: a field left standing over a failed
      // rename is an edit that looks like it took.
      .finally(() => renderTopics());
  };

  field.addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    } else if (event.key === "Escape") {
      event.preventDefault();
      cancel();
    }
  });
  field.addEventListener("blur", () => cancel());

  row.replaceChildren(field);
  field.focus();
  field.select();
}

/**
 * Put a topic back in the room.
 *
 * The room is told first and the posts are read second: the room decides where
 * the next post is written down, and drawing a topic the room is not in would
 * show one conversation while another was being recorded.
 *
 * Nothing is launched. A topic opens whether or not the sessions it held can be
 * resumed, and resuming one is a press of ▶ on its row afterwards (#115,
 * decision 6).
 *
 * Nothing is stopped either (#141, decision 2). The topic being left keeps its
 * sessions running and its terminals filling; what changes is which topic's
 * conversation, roster and terminals are on the glass (`enterTopic`).
 */
async function openTopic(topic: Topic): Promise<void> {
  if (topic.topic_id === currentTopic?.topic_id) return;
  try {
    await invoke("room_select_topic", {
      topicId: topic.topic_id,
      createdAt: topic.created_at,
    });
    await enterTopic({ topic_id: topic.topic_id, created_at: topic.created_at });
    drawTopic(await invoke<LoggedPost[]>("room_topic_log", { topicId: topic.topic_id }));
    status(`トピック「${topicName(topic)}」を開きました。`);
  } catch (err) {
    status(`トピックを開けませんでした: ${err}`, "error");
  }
}

/**
 * Put one topic on the glass: its roster, its terminals, and its place in the
 * list. The conversation is the caller's to draw, since where it comes from
 * differs — a topic's log, or nothing for a topic just made.
 *
 * The roster is asked for rather than taken from what this screen last heard,
 * because a room made by opening its topic has had no event yet, and one heard
 * long ago may have moved while the event was missed.
 */
async function enterTopic(topic: TopicRef): Promise<void> {
  // The pane being watched in the topic being left, kept for coming back.
  const leaving = shownTopicId();
  if (leaving !== "") shownByTopic.set(leaving, shownAccount);
  currentTopic = topic;
  try {
    renderRoster(
      topic.topic_id,
      await invoke<Participant[]>("room_participants", { topicId: topic.topic_id }),
    );
  } catch {
    // What this screen last heard of that room stands, or nothing: a roster
    // that failed to read is not a reason to leave the topic half opened.
  }
  showTopicTerminals();
  renderTopics();
}

/**
 * Show the terminals of the topic on the glass, and the pane that topic was
 * last watching.
 *
 * The other topics' terminals are hidden, never discarded. Their sessions are
 * running (#141, decision 2), their output keeps arriving into their own
 * emulators, and coming back finds each scrollback where it was left.
 */
function showTopicTerminals(): void {
  const remembered = shownByTopic.get(shownTopicId()) ?? null;
  const here = topicViews();
  const pick =
    remembered !== null && here.some((view) => view.accountId === remembered)
      ? remembered
      : (here.pop()?.accountId ?? null);
  showView(pick);
}

/**
 * Ask whether one topic is to be deleted. Nothing is deleted until answered.
 *
 * The question names what goes: the posts, and the way back into the sessions
 * that were in it. Both are gone for good — the log is not in git and nothing
 * copies it — which is why this is a dialog and not a second click on the
 * button (#119, 決定1から導かれること).
 *
 * The sessions running in the topic are named on their own line, and only when
 * there are any: deleting the topic ends them (#119, decision 4), and a
 * question that spoke only about the log would make a stopped session an
 * ambush. Asked of the app rather than read off `seated`, for the reason
 * `runningSeatNames` asks: the app is what would end them, and this screen's
 * copy is only as fresh as whatever last refreshed it.
 */
async function openTopicDeleteDialog(topic: Topic): Promise<void> {
  // A question is already standing, so this one is not opened. Without it the
  // two interleave across the read below: the second click overwrites the first
  // one's topic and its sentence, then the first read returns and opens the
  // dialog carrying one topic's name over the other's sessions. `deletingTopic`
  // is cleared on every path out of the dialog, so nothing is wedged by it.
  if (deletingTopic) return;
  deletingTopic = topic;
  topicDeleteMessageEl.textContent =
    `トピック「${topicName(topic)}」を削除します。` +
    `発言の記録と、セッションへの戻り道が消えます。取り消せません。`;

  const running = await runningNamesInTopic(topic.topic_id);
  topicDeleteSessionsEl.hidden = running.length === 0;
  topicDeleteSessionsEl.textContent =
    running.length === 0
      ? ""
      : `${running.join("、")} のセッションも終了します。`;

  // Opened after the read, so the question is whole the first time it is on the
  // glass. A dialog that grew a line while it was open would be one the person
  // may have already answered without it.
  topicDeleteDialogEl.showModal();
}

/**
 * The names of the accounts whose sessions are running in one topic.
 *
 * A seat carries the topic it was started into, so this is the topic's own
 * sessions and not every seat there is: an account that ran here once may be
 * running in another topic now (#119, decision 4; #141).
 *
 * On a failed read this screen's own copy stands, for the reason
 * `runningSeatNames` keeps it: a read that failed says nothing about who is
 * running, and answering "nobody" would put the person in front of a question
 * that does not mention the sessions it is about to end.
 */
async function runningNamesInTopic(topicId: string): Promise<string[]> {
  let held: SeatedAccount[];
  try {
    held = await invoke<SeatedAccount[]>("seated_accounts");
  } catch {
    held = [...seated.values()];
  }
  return held
    .filter((seat) => seat.session !== null && seat.topic_id === topicId)
    .map(
      (seat) =>
        accounts.find((one) => one.id === seat.account_id)?.name.trim() || seat.account_id,
    );
}

/** Leave the question unanswered. Escape lands here too, by the close handler. */
function closeTopicDeleteDialog(): void {
  deletingTopic = null;
  if (topicDeleteDialogEl.open) topicDeleteDialogEl.close();
}

/** The answer that acts. */
function confirmTopicDeleteDialog(): void {
  const topic = deletingTopic;
  closeTopicDeleteDialog();
  if (topic) void deleteTopic(topic);
}

/**
 * Delete one topic.
 *
 * The app answers with the topic the room moved to, or null when the room was
 * somewhere else and did not move. Deleting the open topic leaves the room in a
 * new one, which is the state a launch already produces — an empty room, in a
 * topic not yet in the index (#119, decision 6) — so what is drawn for it is
 * what `startNewTopic` draws: no row, and 新規 picked (#125, 決定1).
 *
 * The list itself arrives on `room-topics`; this redraw is for the current
 * topic, which is this screen's own value and is not in that payload.
 *
 * The seats are re-read either way. Sessions running in the topic were ended by
 * the delete, and the panel is drawn from a copy that does not know it yet.
 *
 * The topic's terminals go with it. Their sessions were ended by the delete, and
 * a terminal filed under a topic that no longer exists is one no list row and
 * no tab could ever lead back to (#141).
 */
async function deleteTopic(topic: Topic): Promise<void> {
  try {
    const moved = await invoke<TopicRef | null>("room_delete_topic", {
      topicId: topic.topic_id,
    });
    for (const view of [...views.values()]) {
      if (view.topicId === topic.topic_id) discardView(view);
    }
    if (moved) {
      await enterTopic(moved);
      drawTopic([]);
    }
    // After the move, which records what the topic being left was showing.
    rosters.delete(topic.topic_id);
    awaiting.delete(topic.topic_id);
    shownByTopic.delete(topic.topic_id);
    renderTopics();
    status(`トピック「${topicName(topic)}」を削除しました。`);
  } catch (err) {
    status(`トピックを削除できませんでした: ${err}`, "error");
  }
  await refreshSeats();
}

/**
 * Cut here: a new topic, and an empty room.
 *
 * The boundary is drawn by hand and by nothing else. Starting the app opens one
 * too, but the two are independent — one run may hold several topics, and one
 * topic may span several runs (#115, decision 1).
 *
 * The topic being left keeps running (#141, decision 2): its sessions, its
 * terminals and its room all stay, and it is reached again from the list.
 */
async function startNewTopic(): Promise<void> {
  try {
    await enterTopic(await invoke<TopicRef>("room_new_topic"));
    drawTopic([]);
    status("新しいトピックを始めました。");
  } catch (err) {
    status(`新しいトピックを始められませんでした: ${err}`, "error");
  }
}

/** Say on the topic list why it has nothing to show. */
function topicsFailed(reason: string): void {
  topicListEl.replaceChildren();
  const line = document.createElement("li");
  line.className = "empty";
  line.textContent = `トピックを読めませんでした: ${reason}`;
  topicListEl.appendChild(line);
}

/**
 * Put the posts a refusal handed back on the glass, and answer how many were
 * new. Oldest first, in the order the room put them in.
 *
 * The refusal is the only place these posts are reachable from. The room has no
 * read-out for the floor — `room_participants` / `room_port` / `room_join` /
 * `room_post` is the whole surface — and the watermark advances inside
 * `appendMessage`, that is, only for a line actually drawn. So a screen that
 * missed the delivering event had no second path to those posts: the watermark
 * stayed behind, the next post was refused for the same reason, and nothing the
 * person could do moved it. Only another participant speaking broke it, which
 * is not something the person could cause while refused (#108).
 *
 * Drawn as ordinary lines, with no mark saying they were caught up on. They are
 * posts of the room like any other, and "arrived while you were typing" is not
 * an attribute of a post.
 *
 * The watermark is then the newest of them, drawn here or drawn earlier: the
 * screen has seen every one either way. Setting it is what the already-drawn
 * case needs — nothing is appended there, so nothing else would move it, and
 * leaving it standing is the refusal loop this exists to break. Against the
 * watermark that was declared it always advances, since every missed post is by
 * definition ahead of that one. A post admitted after the refusal was computed
 * and drawn before this runs is the one thing it can fall behind, and that
 * costs the round trip the floor already prices (#47).
 */
function drawMissed(missed: MissedPost[]): number {
  let drew = 0;
  for (const one of missed) {
    if (drawnIds.has(one.message_id)) continue;
    appendMessage({
      // Drawn into the topic on the glass, which is the one the refused post was
      // written in (`send` checks that before calling this).
      topic_id: shownTopicId(),
      message_id: one.message_id,
      speaker: one.speaker,
      hue: one.hue,
      account: one.account,
      content: one.content,
      to: one.to,
      ts: one.ts,
      // Never this screen's own. The floor holds a speaker's own posts back
      // from their own refusal: they were not delivered to their author, so
      // there was nothing there to have missed.
      own: false,
    });
    drew += 1;
  }

  const newest = missed[missed.length - 1];
  if (newest) lastSeenId = newest.message_id;
  return drew;
}

/**
 * One line of the participant list: an account, whoever is in the room as it,
 * or both.
 *
 * Both halves are optional, and each absence is a real state rather than a
 * defect. An account with no participant is someone who exists and is not
 * running (#53). A participant with no account is a connection that declared
 * none — the room does not presume one exists, and something joining from
 * outside this app has none to declare (#59).
 */
interface Member {
  account: Account | null;
  participant: Participant | null;
}

/** Which group a row falls in, and the heading it is drawn under. */
const GROUPS: { kind: AccountKind | "guest"; label: string }[] = [
  { kind: "admin", label: "admin" },
  // One heading per launched kind (#156). A group with nobody in it is not
  // drawn, so a screen whose accounts are all one kind reads as it did before
  // the split — the second heading appears when a second kind does.
  { kind: "claude_code", label: "Claude Code" },
  { kind: "cli", label: "CLI" },
  // Under the AI accounts, as a heading of its own (#193, Master 判断
  // 2026-09-28). A server speaks and launches nothing, so it is not one of them.
  { kind: "mcp", label: "MCP" },
  // Not a kind: the absence of one. A connection carrying no account has
  // declared nothing, and inferring a kind from how it arrived is the mistake
  // the declaration exists to avoid (#59).
  { kind: "guest", label: "ゲスト" },
];

/**
 * Join the room's roster against this app's accounts, into one list.
 *
 * **On the account id**, which the room now carries on every seat (#59). Never
 * on the name: a name is an editable attribute, two accounts may answer to one,
 * and a running account may have been renamed since it joined — matching by
 * name ties the wrong pair in all three cases, which is the failure #40 was
 * about in another shape and the reason #53 refused it.
 *
 * The room's roster is the whole of the live half. The screen keeps no second
 * list of who is present, so a name on a live row is a name a post can be
 * addressed to.
 */
function members(): Member[] {
  const rows: Member[] = [];
  const placed = new Set<string>();

  for (const account of accounts) {
    // Plural on purpose. One account holding two seats is refused by the app
    // that launches it (`RoomSeats`), and this list is not the place to enforce
    // that: something joining from elsewhere could carry the same id, and
    // dropping the second one would hide a participant who is genuinely there.
    const matches = shownRoster().filter((one) => one.account === account.id);
    for (const participant of matches) {
      placed.add(participant.id);
      rows.push({ account, participant });
    }
    if (!matches.length) rows.push({ account, participant: null });
  }

  for (const participant of shownRoster()) {
    if (!placed.has(participant.id)) rows.push({ account: null, participant });
  }
  return rows;
}

/** What a row is called: the room's name while it is in the room. */
function memberName(row: Member): string {
  return row.participant?.name ?? row.account?.name ?? "";
}

// ── what a running account is doing ──────────────────────────────────────────
//
// The row could say whether an account was running and nothing more: the four
// words it had — 未起動 / 起動中 / 終了 / 起動失敗 — all come from whether a
// process exists. What follows adds the two things this screen can observe about
// one that does, and stops there (#82):
//
//   the room's round trip — addressed in a post, not heard from since
//   the byte stream     — output arriving at this account's terminal
//
// Neither reads what the CLI printed. Reading it is the only way to tell 考え中
// from ツール使用中, and it is a separate implementation per CLI that breaks
// whenever the other side changes its display, so it is refused here and judged
// on its own (#82 決まったこと).
//
// Both words are gated on output still arriving, and that gate is the design
// rather than an optimisation. A word that outlives the thing it describes is
// worse than no word at all, and being addressed has no end of its own: a
// session that is asked something and then sits at a confirmation prompt never
// answers, so 考え中 on the address alone would stand there for as long as the
// app is open — which is exactly the shape the issue named as the worst one.
// What stands there instead is 待機, and it claims only the silence itself —
// a quiet window this screen timed — not what the CLI is silent about (#148).

/**
 * Note that this session is printing, and arm its fall back to silence.
 *
 * Called once per chunk, and cheap on purpose: the timer is pushed forward every
 * time, and the panel is redrawn only on the edge where the word appears.
 */
function markOutput(view: SessionView): void {
  armQuiet(view);
  view.silent = false;
  if (view.outputting) return;
  view.outputting = true;
  renderPanel();
}

/**
 * Start (or restart) the clock that turns this terminal's silence into 待機.
 *
 * Armed by every byte, and once when the session is attached, so a session that
 * prints nothing at all after starting still reaches 待機 after one quiet window
 * rather than saying nothing forever (#148).
 */
function armQuiet(view: SessionView): void {
  if (view.quiet !== undefined) clearTimeout(view.quiet);
  view.quiet = window.setTimeout(() => {
    view.quiet = undefined;
    view.outputting = false;
    view.silent = true;
    renderPanel();
  }, OUTPUT_QUIET_MS);
}

/**
 * Take this session's word down at once, without waiting out the quiet window.
 *
 * For the two ends that are not silence: the session exited, or its terminal was
 * discarded. The timer goes with it — one left armed on a discarded view would
 * redraw the panel from a session nothing else can reach.
 */
function stopOutput(view: SessionView): void {
  if (view.quiet !== undefined) clearTimeout(view.quiet);
  view.quiet = undefined;
  view.outputting = false;
  view.silent = false;
}

/**
 * Read one post for who the room is now waiting on.
 *
 * Speaking clears first, then being addressed marks — in that order, so a
 * participant answering one question and being asked another in the same instant
 * ends up marked. Whoever spoke is no longer owed an answer whether or not the
 * post was the one they were asked for: they are audibly not stuck.
 *
 * A name nobody currently answers to is not marked. The room delivers a post
 * addressed to no one present just the same, and marking it would leave the word
 * armed for whoever takes that name next — a row saying 考え中 about a question
 * asked before it was even running.
 */
function trackAddress(message: RoomMessage): void {
  // In the topic it was said in. A name asked in one topic is not being waited
  // on in another, where someone else — or the same account's other session —
  // answers to it (#141).
  let waiting = awaiting.get(message.topic_id);
  if (!waiting) {
    waiting = new Set<string>();
    awaiting.set(message.topic_id, waiting);
  }
  let moved = waiting.delete(message.speaker);
  const present = rosters.get(message.topic_id) ?? [];
  // Each name on it, the same way: a post to several is a question to each of
  // them, and every one that has not answered is being waited on (#204).
  for (const to of message.to) {
    if (!waiting.has(to) && present.some((one) => one.name === to && !one.own)) {
      waiting.add(to);
      moved = true;
    }
  }
  if (moved && message.topic_id === shownTopicId()) renderPanel();
}

/**
 * Drop everyone the room is waiting on who is no longer in it.
 *
 * The roster is the authority on who is present, so this runs when it arrives. A
 * session that exits with a question outstanding leaves the room, and this is
 * what takes its mark with it.
 */
function pruneAwaiting(topicId: string): void {
  const waiting = awaiting.get(topicId);
  if (!waiting) return;
  const present = new Set(
    (rosters.get(topicId) ?? []).filter((one) => !one.own).map((one) => one.name),
  );
  for (const name of waiting) {
    if (!present.has(name)) waiting.delete(name);
  }
}

/**
 * Whether the account behind this report has a rate-limit window that is full
 * (#161).
 *
 * Either window is enough, and neither is weighted against the other: a session
 * that cannot spend against its five-hour window is stopped whether or not its
 * week has room, and the other way round (決定1).
 *
 * `>=` rather than `===`, because a spend limit may report past 100% (決定7;
 * the docs line is the issue's citation, not one read here). A window the CLI
 * did not report is not a window at 0: null is absent, and an absent window
 * says nothing either way — the same line the panel's `—` stands on.
 *
 * Read off the last report and nothing else, which is what makes the word clear
 * itself: the next report carrying a lower percentage is the word going away,
 * with no edge to catch and no burst to wait for (決定2). What it costs is what
 * 決定4 accepted — a report arrives only while the session is moving, so an
 * account that stopped and then hit its limit says nothing until it moves
 * again, and one whose limit has lifted keeps the word until then.
 */
function limitedByUsage(stats: SessionStats | null): boolean {
  if (!stats) return false;
  return (stats.five_hour ?? 0) >= 100 || (stats.seven_day ?? 0) >= 100;
}

/**
 * What a running account is doing, in the one word the row has room for.
 *
 * 考え中… when the room is waiting on this name, 出力中 otherwise, and 待機 once
 * the terminal has been silent for a whole quiet window (#148).
 *
 * 待機 says that the terminal is silent and nothing more. A CLI waiting for input
 * and one stopped at a confirmation prompt both read as 待機 — telling them apart
 * means reading what the CLI printed, which #82 refused and #148 keeps refused.
 * It is left uncoloured: the coloured words are the ones that say an utterance is
 * still under way, and 待機 is where that ends.
 *
 * 制限中 is the one silence that is told apart, and it is told apart without
 * reading anything: the CLI's own status line reports its rate-limit
 * percentages, and 100% of either window is the limit (`limitedByUsage`, #161).
 * The word says that the account's window is full, which is narrower than
 * "cannot run" and wider than the turn-level signal it replaced. It stays
 * uncoloured beside 待機 for the same reason.
 *
 * The order is not a preference between two equal signals. Both words stand on
 * the same observation — this terminal is printing — and the address is what says
 * why: the account owes the room an answer and has not given it. That is strictly
 * more than the other word says, so a row that could say both says that one.
 *
 * 出力中 rather than 動作中 for what is left. What was observed is that bytes
 * arrived, and a CLI repainting the prompt it is waiting at is producing output
 * without doing any work — 動作中 would be a claim about the CLI that this screen
 * has no way to check, and it would be wrong in exactly the case the issue
 * measured on the device (an `Enter to confirm` prompt). Both words are three
 * characters or so, inside the width 起動失敗 already costs the name beside it,
 * so neither buys anything back at the panel's 16.5rem (#71).
 */
function activityNote(name: string, view: SessionView | undefined): string {
  if (!view || view.ended !== null) return "";
  if (view.outputting) return awaiting.get(view.topicId)?.has(name) ? "考え中…" : "出力中";
  // 制限中 over 待機, because it says what 待機 cannot: which of the silences
  // this is (#161, 決定6, carried over from #149). It loses to the two words
  // above for the same reason 待機 does — output arriving is this screen's own
  // observation of a session that is going again.
  if (limitedByUsage(view.stats)) return "制限中";
  return view.silent ? "待機" : "";
}

/**
 * Draw one line of the participant list.
 *
 * The colour is the one that participant's lines carry in the room, which is
 * what makes the panel a legend for the conversation rather than a second copy
 * of the same names. It is on the name itself here, not only on the dot.
 *
 * An offline row keeps its colour and is dimmed; it does not fall to grey. The
 * colour carries the account's identity, and someone merely absent must not
 * read as someone else (#59).
 */
function memberRow(row: Member): HTMLLIElement {
  const name = memberName(row);
  const hue = row.participant ? row.participant.hue : (row.account?.hue ?? null);
  // From the room, decided on the connection. A name test here would mark every
  // participant answering to this screen's name as oneself (#40).
  const own = row.participant?.own ?? false;
  // This topic's terminal for the account. The same account may be running in
  // another topic too, and that session is that topic's row (#141).
  const view = row.account ? views.get(seatKey(shownTopicId(), row.account.id)) : undefined;
  // The server an `mcp` account is. It takes no seat, so what says it is here
  // is the server running, not the roster (#193).
  const server = row.account?.kind === "mcp" ? mcpServerOf(row.account) : null;

  const entry = document.createElement("li");
  entry.className = "member";
  entry.style.setProperty("--speaker", speakerColor(name, hue, own));
  if (!row.participant && server?.view?.state?.state !== "running") {
    entry.classList.add("offline");
  }

  const dot = document.createElement("span");
  dot.className = "dot";

  const who = document.createElement("span");
  who.className = "who";
  who.textContent = name;
  who.title = name;

  // Asked for and not back yet. The view exists from the moment 開始 is pressed
  // and its pty id arrives with the session, so this pair is exactly that
  // window (`startSession`).
  const launching = view != null && view.ended === null && view.ptyId === "";
  const failure = row.account
    ? launchFailures.get(seatKey(shownTopicId(), row.account.id))
    : undefined;

  // What this line says about itself beyond the name. Someone present and not
  // oneself says what they are doing, when this screen can observe it, and
  // otherwise says nothing — being in the list is what it would have said (#82).
  // A terminal timed silent is something this screen observes, so 待機 is said
  // here too (#148); the window before that silence is timed still says nothing.
  //
  // One note, in one place. 未起動 and 考え中… are mutually exclusive states of
  // the same account, so they need no second slot, and the two fixed tracks #71
  // measured are untouched.
  let noteText = "";
  let noteKind = "";
  let noteTitle = failure ?? "";
  if (own) noteText = "（あなた）";
  else if (server) {
    // A server says where its run is, on the note the sessions' words are on.
    // Running says nothing, the way a session in the room says nothing (#82).
    ({ text: noteText, kind: noteKind, title: noteTitle } = mcpNote(server.view));
  } else if (launching) noteText = "起動中";
  else if (row.participant) {
    noteText = activityNote(name, view);
    // 待機 and 制限中 both stand where an utterance has ended, so both stay the
    // ground colour; the coloured words are the ones saying one is still under
    // way (#148 / #149).
    if (noteText && noteText !== "待機" && noteText !== "制限中") noteKind = "active";
  } else if (failure) {
    noteText = "起動失敗";
    noteKind = "error";
  } else if (view && view.ended === null) {
    // The process is alive and the room has not seen it yet. 起動中 is the word
    // the status panel already puts on exactly this state — `renderSessionFacts`
    // reads `view.ended === null` and writes `${name} 起動中` — and one screen
    // must not carry two definitions of running. The panel was reading the
    // process while this row read the roster, and the two disagree for as long
    // as a CLI takes to spawn and join the room's websocket: the row fell
    // through to 未起動, the word for an account that was never started, while
    // the same row offered ✕. On the device that window lasted minutes,
    // because the development-channels flag the launch carried then stopped
    // the CLI at a confirm prompt, and a running session was indistinguishable
    // by word from an idle account — only the button said which was which
    // (#89). The flag is off the line since #195; a CLI can still stop at a
    // prompt of its own before it joins.
    //
    // Below `row.participant` on purpose. A row that is in the room says
    // nothing unless it has something to report, and being in the list is what
    // that silence says (#82); a branch above would take that back. Only a row
    // outside the roster reaches here, which reads both windows correctly: the
    // one right after 開始, and a CLI that has lost a connection it once had.
    //
    // No new word, and no new kind. This is what is so about the session, the
    // same as 終了 and 未起動 beside it, so it stays uncoloured — 起動失敗 above
    // is the row's only error.
    noteText = "起動中";
  } else if (view?.ended != null) noteText = "終了";
  else if (row.account) noteText = "未起動";

  // A row whose account has a terminal here picks that terminal; the whole line
  // is the control, so choosing which session to watch is one click on the
  // session rather than on something beside it. A row with no terminal is not a
  // button at all — a disabled one would grey out a name whose colour is
  // load-bearing.
  let pick: HTMLElement;
  if (view) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "pick";
    button.setAttribute("aria-pressed", String(view.accountId === shownAccount));
    button.title = `${name} の端末を見る`;
    // Picking is selection, and the pane's fold is not part of it. A folded
    // pane stays folded: the pick moves which terminal is chosen, the row's
    // highlight and the session facts below the panel, and the terminal takes
    // no focus, since nothing it would type into is on screen. 端末 later opens
    // the pane on the terminal that was picked. An open pane switches the
    // terminal it shows, as it always did (#175).
    button.addEventListener("click", () => {
      const folded = diagnosticsEl.hidden;
      showView(view.accountId);
      if (!folded) view.term.focus();
    });
    if (view.accountId === shownAccount) entry.classList.add("shown");
    pick = button;
  } else {
    pick = document.createElement("div");
    pick.className = "pick static";
  }
  pick.append(dot, who);
  if (noteText) {
    const note = document.createElement("span");
    note.className = "note";
    note.textContent = noteText;
    // A state is a badge, tinted by its kind (#225). 「（あなた）」 is not a state
    // but who the row is, and stays plain beside the name.
    if (!own) note.classList.add("badge");
    if (noteKind) note.dataset.kind = noteKind;
    // The app's own reason, on the row carrying the word. The status line has
    // it in full; this is so a row saying 起動失敗 is not a dead end. A server's
    // is the detail its run ended on; the whole log is in its window.
    if (noteTitle) note.title = noteTitle;
    pick.appendChild(note);
  }
  entry.appendChild(pick);

  // The account's own operations ride on its row, which is what one list buys:
  // the row is keyed on the account id, so these act on one account and cannot
  // be tied to the wrong one by a shared name (#53, #59).
  //
  // Two fixed columns: 開始, then the menu. 終了 and 編集 sat in those two
  // columns until #224 and are in the menu now, with 端末を開く and 色を変える
  // beside them (`openAccountMenu`). 開始 stays out on the row: it is the one
  // operation a row that is not running has, and it acts on the click. The
  // slot is emitted whether or not it holds a button, so a row with no 開始
  // keeps the column open rather than sliding its menu left of every other
  // row's.
  const lifecycle = document.createElement("span");
  lifecycle.className = "lifecycle";
  // A running session has nothing here: its 終了 is in the menu. The one
  // running state that keeps 開始 is the launch that has not come back yet,
  // which leaves the pressed button dead on the row that was pressed (#62).
  const running = view != null && view.ended === null;
  if (row.account && launches(row.account) && (!running || launching)) {
    // A launched kind only. An `admin` account is a person and there is no CLI
    // under a person to spawn; `start_session` refuses one and that refusal is
    // the authority, but a refusal is the wrong way for the person to find out
    // (#59). Their row keeps the empty column, and their menu with it.
    lifecycle.appendChild(startButton(row.account, launching));
  }
  entry.appendChild(lifecycle);
  if (row.account) {
    const account = row.account;
    entry.appendChild(moreButton(account));
    // Right-click anywhere on the row, the name and the note included. The
    // context-menu key and Shift+F10 arrive here too, as the same event, on the
    // row's focused button.
    entry.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      const anchor = event.target instanceof Element ? event.target : entry;
      // A pointer gives a point; a key gives none (both coordinates zero), and
      // the menu then opens under the control that has the focus.
      const at =
        event.clientX === 0 && event.clientY === 0
          ? cornerOf(anchor.closest("button") ?? entry)
          : { x: event.clientX, y: event.clientY };
      openAccountMenu(account, at);
    });
  }

  return entry;
}

/**
 * The control that starts one account's session.
 *
 * It acts on the click, where 終了 beside it asks first. The asymmetry is the
 * difference between the two acts: ending a session cannot be taken back, and
 * starting one is undone by the button that replaces this one. A question here
 * would charge every deliberate start an answer, to guard a mistake that undoes
 * itself.
 *
 * It carries its own launch. Pressed, it goes dead until the launch comes back,
 * on the row that was pressed — the launcher's button held that state for
 * whichever account its picker was on, and could not say which one (#62). What
 * the state *is* stays in the note beside the name, where 未起動 and 終了 and
 * 起動失敗 are: the button says what can be done, the note says what is so.
 *
 * A mark rather than a word (#71). The word it was is on `aria-label` and on
 * `title`, because a mark is not a name: the label is what a screen reader
 * says and what the pointer resting here reads.
 */
function startButton(account: Account, launching: boolean): HTMLButtonElement {
  const start = document.createElement("button");
  start.type = "button";
  start.className = "start";
  start.appendChild(icon("start"));
  start.disabled = launching;
  const label = launching
    ? `${account.name} を起動しています`
    : `${account.name} のセッションを開始する`;
  start.title = label;
  start.setAttribute("aria-label", label);
  start.addEventListener("click", () => void startSession(account));
  return start;
}

/**
 * The control that opens one account's menu: the same menu right-click opens.
 *
 * Kept on the row so the menu does not depend on right-click (#224). A pointer
 * with no second button, and a keyboard on a row whose name is not a button —
 * an account with no terminal here — would otherwise have no way to 編集 at
 * all. One mark where 終了 and 編集 were two.
 */
function moreButton(account: Account): HTMLButtonElement {
  const more = document.createElement("button");
  more.type = "button";
  more.className = "more";
  more.dataset.account = account.id;
  more.appendChild(icon("more"));
  more.title = `${account.name} の操作`;
  more.setAttribute("aria-label", `${account.name} の操作`);
  more.setAttribute("aria-haspopup", "menu");
  more.setAttribute("aria-expanded", String(menuAccountId === account.id));
  more.addEventListener("click", () => {
    // A second press on the button whose menu is open closes it, the way a
    // menu button does.
    if (menuAccountId === account.id) {
      closeAccountMenu(false);
      return;
    }
    openAccountMenu(account, cornerOf(more));
  });
  return more;
}

/** Where a menu opened from an element, not from a pointer, stands: its lower left. */
function cornerOf(element: Element): { x: number; y: number } {
  const rect = element.getBoundingClientRect();
  return { x: rect.left, y: rect.bottom };
}

/**
 * The account whose menu is open, or null when none is.
 *
 * The id rather than the account or the row: the panel is redrawn under an open
 * menu every time the roster or a terminal moves, and both the row and the
 * account object it was drawn from are replaced. Each item reads the account
 * and its terminal again when it is chosen, for the same reason
 * `confirmEndDialog` reads the view again rather than keeping the one it was
 * opened on.
 */
let menuAccountId: string | null = null;

/**
 * Open one account's menu at a point: 端末を開く, 編集, 色を変える, and 終了.
 *
 * An item that has nothing to act on is left out, not greyed: 端末を開く on an
 * account with no terminal in this topic (the row has no terminal operation
 * then either, #57), and 終了 on an account with no session to end. 終了 is
 * also left out before the launch has returned an id — a kill aimed at an
 * empty id reports success having done nothing (#57).
 *
 * 終了 is the one item that cannot be taken back, so it stands apart below a
 * divider in the danger colour, last, where a slip down the list does not land
 * on it. It opens the same question it always did (`#end-dialog`); the menu
 * moves where it is asked from, not whether it is asked.
 *
 * 色を変える is not a colour picker of its own. The colour is a field of the
 * account's form and nothing else writes it, so the item opens that form on
 * that field (`openAccountDialog`).
 */
function openAccountMenu(account: Account, at: { x: number; y: number }): void {
  const view = views.get(seatKey(shownTopicId(), account.id));
  accountMenuEl.replaceChildren();

  const item = (label: string, drawing: Element, act: () => void): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.tabIndex = -1;
    button.append(drawing, label);
    button.addEventListener("click", () => {
      closeAccountMenu(false);
      act();
    });
    accountMenuEl.appendChild(button);
    return button;
  };

  if (view) {
    item("端末を開く", icon("terminal"), () => openTerminalOf(account.id));
  }
  item("編集", icon("edit"), () => openAccountFrom(account.id, "name"));
  // A swatch in the account's own colour where the others have a drawing: it
  // says what the item changes by showing it.
  const swatch = document.createElement("span");
  swatch.className = "swatch";
  swatch.setAttribute("aria-hidden", "true");
  item("色を変える", swatch, () => openAccountFrom(account.id, "hue"));
  if (view && view.ended === null && view.ptyId !== "") {
    const divider = document.createElement("div");
    divider.setAttribute("role", "separator");
    accountMenuEl.appendChild(divider);
    item("終了", icon("close"), () => endFromMenu(account.id)).classList.add("danger");
  }

  // The colour the row draws the account in, so the swatch is the row's own.
  const row = document.querySelector<HTMLElement>(
    `#roster button.more[data-account="${CSS.escape(account.id)}"]`,
  )?.closest<HTMLElement>(".member");
  accountMenuEl.style.setProperty(
    "--speaker",
    row?.style.getPropertyValue("--speaker") || "var(--accent)",
  );
  accountMenuEl.setAttribute("aria-label", `${account.name} の操作`);

  menuAccountId = account.id;
  accountMenuEl.hidden = false;
  // Placed after it is shown, because a hidden menu has no size to keep inside
  // the window. Pulled back from the right and bottom edges rather than
  // flipped, which keeps the menu's corner at the point wherever there is room.
  const width = accountMenuEl.offsetWidth;
  const height = accountMenuEl.offsetHeight;
  const x = Math.max(0, Math.min(at.x, window.innerWidth - width - 4));
  const y = Math.max(0, Math.min(at.y, window.innerHeight - height - 4));
  accountMenuEl.style.left = `${x}px`;
  accountMenuEl.style.top = `${y}px`;
  markMenuButtons();
  menuItems()[0]?.focus();
}

/**
 * Mark which row's `⋯` has its menu open, on the buttons already drawn.
 *
 * In place rather than by redrawing the panel (#229). Opening and closing the
 * menu changes nothing on the row but this one attribute, and a redraw swaps
 * the `⋯` for a new element under a pointer that has not moved. Escape is the
 * close that leaves the pointer resting there, and on the device the press
 * after it was lost: the menu opened only on the second. Kept the same element,
 * the button under the pointer is the one that was there when it came to rest.
 * A redraw for any other reason while the menu stands draws the mark from
 * `menuAccountId` itself (`moreButton`).
 */
function markMenuButtons(): void {
  for (const more of rosterEl.querySelectorAll<HTMLButtonElement>("button.more")) {
    more.setAttribute("aria-expanded", String(more.dataset.account === menuAccountId));
  }
}

/** The menu's items, in the order they are drawn. */
function menuItems(): HTMLButtonElement[] {
  return [...accountMenuEl.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
}

/**
 * Close the menu. With `restore`, the focus goes back to the row's menu button,
 * which is what Escape does; a click elsewhere or a chosen item leaves the focus
 * where that put it.
 */
function closeAccountMenu(restore: boolean): void {
  if (menuAccountId === null) return;
  const id = menuAccountId;
  menuAccountId = null;
  accountMenuEl.hidden = true;
  accountMenuEl.replaceChildren();
  markMenuButtons();
  if (restore) {
    // Found rather than held: the panel may have been redrawn while the menu
    // stood, which replaces the button the menu was opened from.
    document
      .querySelector<HTMLButtonElement>(`#roster button.more[data-account="${CSS.escape(id)}"]`)
      ?.focus();
  }
}

/**
 * 端末を開く: pick the terminal and open the pane on it.
 *
 * Picking from the row leaves a folded pane folded (#175); this item is the
 * one that says "open", so it opens the pane, and the terminal takes the focus
 * as a terminal picked from its tab does.
 */
function openTerminalOf(accountId: string): void {
  const view = views.get(seatKey(shownTopicId(), accountId));
  if (!view) return;
  showView(accountId);
  revealDiagnostics();
  view.term.focus();
}

/** 編集 and 色を変える: the account's form, on the field the item names. */
function openAccountFrom(accountId: string, field: "name" | "hue"): void {
  const account = accounts.find((one) => one.id === accountId);
  if (account) openAccountDialog(account, field);
}

/** 終了: the question `#end-dialog` asks, about the session as it is now. */
function endFromMenu(accountId: string): void {
  const key = seatKey(shownTopicId(), accountId);
  const view = views.get(key);
  // The session may have ended, or the topic changed, while the menu stood.
  if (!view || view.ended !== null || view.ptyId === "") return;
  openEndDialog(key, viewName(view));
}

/** Arrow keys walk the items, Home and End jump, Escape and Tab leave. */
function onAccountMenuKey(event: KeyboardEvent): void {
  const items = menuItems();
  const at = items.indexOf(document.activeElement as HTMLButtonElement);
  const go = (index: number) => items[(index + items.length) % items.length]?.focus();
  switch (event.key) {
    case "ArrowDown":
      go(at + 1);
      break;
    case "ArrowUp":
      go(at < 0 ? -1 : at - 1);
      break;
    case "Home":
      go(0);
      break;
    case "End":
      go(-1);
      break;
    case "Escape":
    case "Tab":
      closeAccountMenu(true);
      break;
    default:
      return;
  }
  event.preventDefault();
}

/**
 * One tab: an open terminal, named by the account it belongs to.
 *
 * The name rather than the command it was launched from. The command is on the
 * row's `title` and in the account's own form, and a strip of `claude` repeated
 * once per session tells two sessions apart by nothing at all.
 *
 * The colour is the account's, the same one its lines carry in the room and its
 * dot carries in the panel — which is what lets a tab and a row be read as one
 * participant rather than as two names that happen to match.
 *
 * ✕ appears on an ended tab and on no other. On a running one it would be read
 * as "end this session", and ending a session is 終了 on the row, asked in a
 * dialog and answered there (#57 / #71); a second, plainer way to do it beside a
 * control that merely changes what is showing is the slip those two were built
 * against (#68).
 */
function terminalTab(view: SessionView): HTMLElement {
  const name = viewName(view);
  const account = accounts.find((one) => one.id === view.accountId) ?? null;
  const shown = view.accountId === shownAccount;

  const tab = document.createElement("div");
  tab.className = "tab";
  // Never oneself: a terminal belongs to a session, and the person at this
  // screen is not launched (`start_session` refuses an `admin` account).
  tab.style.setProperty("--speaker", speakerColor(name, account?.hue ?? null, false));
  if (shown) tab.classList.add("shown");
  if (view.ended !== null) tab.classList.add("ended");

  const pick = document.createElement("button");
  pick.type = "button";
  pick.className = "name";
  pick.textContent = name;
  pick.title = view.ended === null ? `${name} の端末` : `${name} の端末（${endedNote(view)}）`;
  pick.setAttribute("aria-pressed", String(shown));
  pick.addEventListener("click", () => {
    showView(view.accountId);
    view.term.focus();
  });
  tab.appendChild(pick);

  if (view.ended !== null) {
    const close = document.createElement("button");
    close.type = "button";
    close.className = "close";
    close.appendChild(icon("close"));
    close.title = `${name} の端末を閉じる`;
    close.setAttribute("aria-label", `${name} の端末を閉じる`);
    close.addEventListener("click", () => closeView(view));
    tab.appendChild(close);
  }

  return tab;
}

/**
 * Draw the tab strip: every terminal this screen holds, in launch order.
 *
 * Drawn from `views`, which is the same source the rows read to decide whether
 * they are a picker — so the tabs and the rows cannot disagree about what is
 * open. That they are two renderings of one selection is the duplication #59
 * removed from the roster and #68 chose here deliberately: the strip answers
 * "which terminals are open" at the pane being looked at, and the rows answer it
 * only by being read alongside it. Being drawn together is what keeps the
 * accepted duplication from becoming a divergence.
 */
function renderTerminalTabs(): void {
  tabsEl.replaceChildren();
  // The topic on the glass only. The others' terminals are open and hidden, and
  // a tab for one would switch the pane to a session of a conversation that is
  // not the one being read (#141).
  for (const view of topicViews()) tabsEl.appendChild(terminalTab(view));
}

/**
 * Close one ended terminal for good, from its tab.
 *
 * This is where "the person has read it" is said now. It used to be said by
 * choosing another account — `showView` discarded an ended terminal the moment
 * one was — and a tab that stays put until it is closed makes that an explicit
 * act instead of a side effect of looking elsewhere (#68). The signal is not
 * lost; it moved.
 *
 * What is on the glass afterwards is the most recently opened of what is left,
 * which is the nearest neighbour in launch order. Nothing left is an honest
 * answer too, and `showView(null)` is it.
 */
function closeView(view: SessionView): void {
  const wasShown = view === shownView();
  discardView(view);
  if (wasShown) {
    showView(topicViews().pop()?.accountId ?? null);
    return;
  }
  renderPanel();
  renderSessionFacts();
}

/**
 * Draw the participant list: everyone who is here, and everyone who exists.
 *
 * One list. It was two — a roster keyed on the connection and a terminal list
 * keyed on the account id — because a `Participant` carried no account id and
 * the only join available was on the name, which #40 and #53 had ruled out
 * (#57). The room carries the id now, so the two questions ("who is here" and
 * "what can I do with them") are answered on one row.
 *
 * Grouped by the kind declared at creation, with the count in the heading. A
 * participant with no account has declared no kind and is grouped as such;
 * guessing one from the connection would mistake a person who joined from
 * another client for a session (#59).
 *
 * An account that is not running is still someone, so it is listed rather than
 * left out — that is the whole point of an account existing while it is off
 * (#53). It does not become an addressee: `addressable` reads the live roster
 * only, because a name that cannot be reached is not worth naming.
 */
function renderPanel(): void {
  // The title bar's count is this roster's length (#225).
  renderRoomTitle();
  // The tabs are redrawn here rather than on their own schedule. Both surfaces
  // read `views` and both mark the same selection, so drawing them from one call
  // is what makes "they move together" true by construction (#68).
  renderTerminalTabs();
  rosterEl.replaceChildren();
  const rows = members();

  if (!rows.length) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = "参加者なし";
    rosterEl.appendChild(empty);
    if (!mentionListEl.hidden) refreshMentions();
    return;
  }

  for (const group of GROUPS) {
    const inGroup = rows
      .filter((row) => (row.account?.kind ?? "guest") === group.kind)
      .sort((a, b) => memberName(a).localeCompare(memberName(b)));
    if (!inGroup.length) continue;

    const heading = document.createElement("li");
    heading.className = "group";
    // Name and count, joined by an em dash. The count is what a heading buys
    // over a divider: how many of this kind are here is read without counting.
    heading.textContent = `${group.label} — ${inGroup.length}`;
    rosterEl.appendChild(heading);

    for (const row of inGroup) rosterEl.appendChild(memberRow(row));
  }

  // An open list follows the room: someone who left is not offered, someone who
  // came is. A shut one stays shut — it opens on `@`, not on the roster.
  if (!mentionListEl.hidden) refreshMentions();
}

/**
 * Take the room's roster and redraw the panel around it.
 *
 * The one door the roster comes in by, the event and the first read alike. A
 * second assignment to `participants` elsewhere would be a roster that arrived
 * without the two readings below happening to it.
 */
function renderRoster(topicId: string, joined: Participant[]): void {
  rosters.set(topicId, joined);
  // Against the roster that just arrived, before it is drawn: who the room is
  // waiting on is only meaningful about someone who is in it (#82).
  pruneAwaiting(topicId);
  // The other reading of the same roster: which sessions have arrived at all.
  // Kept on the view rather than asked at the exit, because by then the
  // connection is gone and the roster no longer remembers it was there (#127).
  // This room's terminals only: the same account arriving in another topic is
  // another session arriving (#141).
  for (const view of views.values()) {
    if (view.seenInRoom || view.topicId !== topicId) continue;
    if (joined.some((one) => one.account === view.accountId)) view.seenInRoom = true;
  }
  if (topicId === shownTopicId()) renderPanel();
}

/**
 * Re-read which accounts hold a seat, from the app.
 *
 * The app is the authority because it is what refuses a second launch; a count
 * kept on this side would be a second opinion about the same fact. It is read
 * after a launch and after a session exits, which are the two moments the
 * answer changes.
 */
async function refreshSeats(): Promise<void> {
  try {
    const held = await invoke<SeatedAccount[]>("seated_accounts");
    seated = new Map(held.map((seat) => [seatKey(seat.topic_id, seat.account_id), seat]));
  } catch {
    // The panel keeps the last answer rather than declaring everyone offline
    // on a failed read. It still redraws: what failed is this one value, and
    // whatever else moved since the last draw is not held back by it.
  }
  await adoptSeats();
  renderPanel();
  // The list marks the topics holding a seat, and this is the answer that
  // changed (#141, AI 判断5).
  renderTopics();
}

/**
 * Give a running session its terminal back, wherever this screen has none.
 *
 * The terminals live in the webview and the sessions do not. A reload takes
 * every `SessionView` and leaves every PTY running, so the account is left held
 * by a session this screen has no id for: the row draws 開始 because it finds no
 * view, and 開始 is refused because the seat is taken. No way in and no way out
 * (#84). The app's answer carries the pty id, and subscribing to it again is
 * the whole of the way back.
 *
 * What does not come back is the scrollback — it was in the emulator that went
 * with the old screen, and nothing else ever held it — nor whatever the session
 * printed between the reload and this call. The terminal says so on its first
 * line instead of opening blank, because a blank terminal under 起動中 reads as
 * a session that has printed nothing, and reading what a session last printed
 * is how the person decides whether to end it (#57).
 *
 * Only ever after a reload, never after a restart: the seats are the app's own
 * memory and go with it, so an app that has just started holds none.
 */
async function adoptSeats(): Promise<void> {
  const adopted: string[] = [];
  for (const seat of seated.values()) {
    // No session yet: the seat is claimed and the launch is still in flight.
    // Nothing to subscribe to, and it is this screen's own launch in every case
    // but a reload landing inside that window.
    if (!seat.session || views.has(seatKey(seat.topic_id, seat.account_id))) continue;
    const account = accounts.find((one) => one.id === seat.account_id);
    // An account this screen does not have is one it cannot draw a row for, and
    // the row is the only way that terminal could be reached. The app refuses
    // to delete a seated account, so this is a config edited from outside.
    if (!account) continue;
    const view = openView(account, seat.topic_id, seat.session);
    view.term.writeln(RESUMED_NOTICE);
    await attachSession(view, seat.session.pty_id);
    adopted.push(viewName(view));
  }
  if (!adopted.length) return;
  // Open, because open is where it was: a session is reloaded out from under
  // while it is being watched, which is to say while this pane is showing it.
  revealDiagnostics();
  status(`${adopted.join("、")} の端末に繋ぎ直しました。再読み込みより前の出力は残っていません。`);
}

/** The name this screen posts under, and is listed in the roster under. */
function localName(): string {
  return localAccount()?.name.trim() || "human";
}

/**
 * Take a seat in the room as the account this screen's person is.
 *
 * Being in the room is not the same as having spoken in it: without this the
 * roster would list only the sessions, and nobody could address someone who
 * had not spoken yet.
 *
 * All three go together because a seat carries all three, and sending fewer
 * would withdraw a declaration nobody withdrew. The account id rides for the
 * same reason a session's does — so this screen's own row in the list is joined
 * by id like every other, rather than by the name that #40 ruled out. It is not
 * what makes this participant oneself: the room decides that on the connection,
 * and `Participant.own` is its answer (#59).
 */
async function join(): Promise<void> {
  const account = localAccount();
  // On the glass first: the lines already drawn are this screen's to repaint,
  // and they do not wait on the room answering (#189).
  paintMine();
  try {
    await invoke("room_join", {
      name: localName(),
      hue: account?.hue ?? null,
      accountId: account?.id ?? null,
    });
  } catch {
    // Failing to seat is not worth interrupting anything: the first post
    // seats the name anyway.
  }
}

/**
 * Find or make the account the person at this screen is.
 *
 * #53 left "is a human an account" open, and the two lists in the panel were
 * one consequence: the person was a name and a colour in `localStorage`, so
 * they had no row of the kind everyone else had. They are an account now, of
 * kind `admin`, and the migration is the obvious one — the name and colour they
 * had been joining under become that account's (#59).
 *
 * Resolved before the room is joined, because the id goes into the join.
 */
function resolveLocalAccount(): void {
  const stored = localStorage.getItem(LOCAL_KEY);
  let account =
    accounts.find((one) => one.id === stored && one.kind === "admin") ??
    accounts.find((one) => one.kind === "admin") ??
    null;

  if (!account) {
    const savedHue = localStorage.getItem(HUE_KEY);
    account = {
      id: crypto.randomUUID(),
      name: (localStorage.getItem(NAME_KEY) ?? "").trim() || "human",
      // Carried so the shape of an account is one shape. Nothing launches a
      // person, and `start_session` refuses an `admin` account outright.
      command: "claude",
      args: [],
      cwd: null,
      hue: savedHue !== null && HUES.some(({ hue }) => String(hue) === savedHue)
        ? Number(savedHue)
        : null,
      kind: "admin",
      // Carried for the same reason `command` is: one shape of account. A
      // person speaks as themselves, and there is no launch to select a style
      // on.
      character: null,
      // And for the same reason again: a person is not resumed into a topic,
      // they are at the screen when it is opened.
      resume_command: null,
      // Nothing is launched under a person, so nothing has an environment.
      env: [],
      server: null,
    };
    accounts.push(account);
    saveConfig();
  }

  localAccountId = account.id;
  localStorage.setItem(LOCAL_KEY, account.id);
  // Before a topic is read back, which at startup is ahead of `join` (#189).
  paintMine();
}

/*
 * The list `@` opens in the composer (#204) is an aid to typing and nothing
 * more. What addresses a post is the `@名前` in its text: the room reads those
 * against the names it holds, moves each one that names someone into `to`, and
 * takes it out of the text (#206). A name typed out by hand addresses as a
 * picked one does.
 */

/** Who the open list offers, and which of them the keys are on. */
let mentionCandidates: Participant[] = [];
let mentionActive = 0;
/** Where the `@` the list is completing sits in the text; -1 when it is shut. */
let mentionAt = -1;
/** An `@` whose list was shut with Esc. Typing on after it does not open the
 *  list again; a different `@` does. */
let mentionDismissedAt = -1;

/**
 * Who can be addressed: everyone in the room on the glass but this screen's own
 * person, one entry per name.
 *
 * Sessions and people alike — the roster does not separate them. The room's
 * roster is the whole source, so an account with no session in it is absent by
 * construction: naming a participant who cannot be reached is a post addressed
 * to nobody, which is the reason addressees are picked from a roster at all
 * (#43). Names, deduplicated: `to` carries a display name, so two participants
 * answering to one name are one addressee, and offering both would be a choice
 * between two things that address the same pair.
 */
function addressable(): Participant[] {
  const seen = new Set<string>();
  return shownRoster().filter((one) => {
    if (one.own || seen.has(one.name)) return false;
    seen.add(one.name);
    return true;
  });
}

/**
 * The `@` the caret is completing, and what has been typed after it — or null
 * when the caret is not completing one.
 *
 * An `@` counts at the start of the text or after whitespace, so one in the
 * middle of a word (`a@b`) does not open the list. What follows may hold spaces,
 * because names do, but not a line break. The full-width `＠` counts too: it is
 * what an IME in kana mode types for the same key.
 */
function mentionQuery(): { at: number; query: string } | null {
  const caret = inputEl.selectionStart;
  if (caret !== inputEl.selectionEnd) return null;
  const before = inputEl.value.slice(0, caret);
  const at = Math.max(before.lastIndexOf("@"), before.lastIndexOf("＠"));
  if (at < 0) return null;
  if (at > 0 && !/\s/.test(before[at - 1])) return null;
  const query = before.slice(at + 1);
  if (query.includes("\n")) return null;
  return { at, query };
}

/**
 * Open, narrow or shut the list for where the caret is now.
 *
 * Narrowed to the names that start with what was typed after the `@`, case
 * aside. Nothing left is the list shut: after a pick the name and its space are
 * past the `@`, and no name starts with those, so typing on past a pick shuts it
 * without anything having to say so.
 */
function refreshMentions(): void {
  const found = mentionQuery();
  if (!found || found.at !== mentionDismissedAt) mentionDismissedAt = -1;
  const query = found?.query.toLowerCase() ?? "";
  const candidates =
    found && found.at !== mentionDismissedAt
      ? addressable().filter((one) => one.name.toLowerCase().startsWith(query))
      : [];
  if (!found || !candidates.length) {
    closeMentions();
    return;
  }
  // The keys stay on the same participant while the list narrows around them.
  const current = mentionCandidates[mentionActive]?.name;
  const kept = candidates.findIndex((one) => one.name === current);
  mentionCandidates = candidates;
  mentionActive = kept >= 0 ? kept : 0;
  mentionAt = found.at;
  renderMentions();
}

/**
 * 宛先 on the composer's row (#222): type an `@` at the caret, as the key would.
 *
 * Only the typing. The list it opens, and what a name picked from it does, are
 * the `@` key's (#204, #206), so the button cannot address a post any way the
 * key does not. A space goes in first when the caret is against a word, because
 * an `@` in the middle of one does not open the list.
 */
function typeMention(): void {
  inputEl.focus();
  const start = inputEl.selectionStart;
  const before = inputEl.value.slice(0, start);
  const at = before === "" || /\s$/.test(before) ? "@" : " @";
  inputEl.setRangeText(at, start, inputEl.selectionEnd, "end");
  refreshMentions();
}

function closeMentions(): void {
  mentionCandidates = [];
  mentionActive = 0;
  mentionAt = -1;
  mentionListEl.hidden = true;
  mentionListEl.replaceChildren();
}

function renderMentions(): void {
  mentionListEl.replaceChildren();
  mentionCandidates.forEach((one, at) => {
    const item = document.createElement("li");
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", String(at === mentionActive));
    // In the colour the room draws them in, so the name picked is recognisably
    // the participant on the roster.
    item.style.setProperty("--speaker", speakerColor(one.name, one.hue, false));
    item.textContent = one.name;
    // mousedown rather than click: by the time a click lands the textarea has
    // lost focus, and the caret the pick goes in at with it.
    item.addEventListener("mousedown", (event) => {
      event.preventDefault();
      pickMention(at);
    });
    mentionListEl.appendChild(item);
  });
  mentionListEl.hidden = false;
  mentionListEl.children[mentionActive]?.scrollIntoView({ block: "nearest" });
}

/**
 * Put a candidate into the text as `@名前`.
 *
 * What was typed after the `@` is replaced by the whole name and a space, so the
 * next word does not run into it.
 */
function pickMention(at: number): void {
  const one = mentionCandidates[at];
  if (!one || mentionAt < 0) return;
  const caret = inputEl.selectionStart;
  inputEl.setRangeText(`@${one.name} `, mentionAt, caret, "end");
  closeMentions();
}

/**
 * The keys of an open list. Answers whether the key was the list's, in which
 * case it must not also send, or move the caret.
 *
 * Enter and Tab pick, the arrows move, Esc shuts. Shift+Enter is still a line
 * break. Nothing while an IME is composing: those keys belong to the conversion.
 */
function mentionKey(event: KeyboardEvent): boolean {
  if (mentionListEl.hidden || event.isComposing) return false;
  const count = mentionCandidates.length;
  switch (event.key) {
    case "ArrowDown":
      mentionActive = (mentionActive + 1) % count;
      renderMentions();
      break;
    case "ArrowUp":
      mentionActive = (mentionActive - 1 + count) % count;
      renderMentions();
      break;
    case "Enter":
    case "Tab":
      if (event.shiftKey) return false;
      pickMention(mentionActive);
      break;
    case "Escape":
      mentionDismissedAt = mentionAt;
      closeMentions();
      break;
    default:
      return false;
  }
  event.preventDefault();
  return true;
}

/*
 * Attachments (#223). A post reaches a session as text typed into its
 * terminal (docs/1-room.md 発言は端末へ入る), and a file's contents cannot be
 * typed. So an attached file is saved under the topic and the post carries its
 * path in the text, where a CLI can open it. The tag line is untouched: the
 * paths are text, and the text is what was said.
 *
 * Three ways in — the 添付 button, a drop onto the composer, Ctrl+V of an
 * image — and one chip each above the text, by name, with a ✕ to take it off.
 * Nothing is saved until the post is sent, so a chip taken off leaves nothing
 * behind.
 */

/** One chip. What it was attached from, and where it was saved once it was. */
type Attachment = {
  name: string;
  /** Bytes the webview holds (the button, a paste), or a path a drop named. */
  source: { file: File } | { path: string };
  /** Where it was saved, and for which topic. Kept so a post the floor
   *  refused, sent again, does not save the same file a second time. */
  saved: { topicId: string; path: string } | null;
};

let attachments: Attachment[] = [];

/** The last part of a path, either separator: what a dropped file is called. */
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function attach(found: Attachment[]): void {
  if (!found.length) return;
  attachments = [...attachments, ...found];
  renderAttachments();
}

function attachFiles(files: Iterable<File>): void {
  attach(
    [...files].map((file) => ({
      // A pasted image arrives as `image.png`; a nameless one is still a file.
      name: file.name || "image.png",
      source: { file },
      saved: null,
    })),
  );
}

function attachPaths(paths: string[]): void {
  attach(paths.map((path) => ({ name: baseName(path), source: { path }, saved: null })));
}

function renderAttachments(): void {
  attachmentsEl.replaceChildren();
  attachments.forEach((one, at) => {
    const chip = document.createElement("li");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = one.name;
    name.title = one.name;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `${one.name} を外す`);
    remove.title = "外す";
    remove.appendChild(icon("close"));
    // The textarea keeps its focus and caret through the press, as it does
    // for 宛先.
    remove.addEventListener("mousedown", (event) => event.preventDefault());
    remove.addEventListener("click", () => {
      attachments = attachments.filter((_, other) => other !== at);
      renderAttachments();
    });
    chip.append(name, remove);
    attachmentsEl.appendChild(chip);
  });
  attachmentsEl.hidden = attachments.length === 0;
}

/**
 * A name as a header can carry it: a JSON string with everything outside
 * printable ASCII escaped. The app reads it back with a JSON parse
 * (`room_attach_bytes`).
 */
function asciiJson(text: string): string {
  return JSON.stringify(text).replace(
    /[\u007f-\uffff]/g,
    (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
}

/** Save one attachment under `topicId`, or answer where it already was. */
async function saveAttachment(one: Attachment, topicId: string): Promise<string> {
  if (one.saved?.topicId === topicId) return one.saved.path;
  const path =
    "path" in one.source
      ? await invoke<string>("room_attach_path", { topicId, path: one.source.path })
      : await invoke<string>(
          "room_attach_bytes",
          new Uint8Array(await one.source.file.arrayBuffer()),
          { headers: { "Pullcept-Topic": topicId, "Pullcept-Name": asciiJson(one.name) } },
        );
  one.saved = { topicId, path };
  return path;
}

/**
 * The text a post with attachments carries: what was written, a blank line,
 * and the paths under `添付:`, one to a line. A post of nothing but files is
 * the block alone.
 */
function withAttachments(text: string, paths: string[]): string {
  if (!paths.length) return text;
  const block = ["添付:", ...paths].join("\n");
  return text ? `${text}\n\n${block}` : block;
}

/**
 * Whether a point the drop reported is over the composer.
 *
 * The drop reports physical pixels from the webview's corner; the page lays
 * out in CSS pixels, and the two differ by the device pixel ratio.
 */
function overComposer(position: { x: number; y: number }): boolean {
  const scale = window.devicePixelRatio || 1;
  const x = position.x / scale;
  const y = position.y / scale;
  const box = composerEl.getBoundingClientRect();
  return x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
}

/** Show that letting go here attaches. */
function markDrop(over: boolean): void {
  if (over) composerBoxEl.dataset.drop = "over";
  else delete composerBoxEl.dataset.drop;
}

async function send(): Promise<void> {
  const text = inputEl.value.trim();
  const pending = attachments;
  if (!text && !pending.length) return;

  const speaker = localName();
  // The topic on the glass as this was typed, which is the conversation the
  // watermark below belongs to. Read now, not after the round trip: the post is
  // said where it was written (#141).
  const topicId = shownTopicId();
  // No addressee is sent beside the text: the `@名前` in it are the addressees,
  // and the room reads them out (#206). The app still delivers to everyone;
  // addressees are judgment material for the participants, not a delivery
  // filter.
  // Read before the await: what the screen had drawn when this was sent is the
  // watermark, and an arrival during the round trip must not be folded into it.
  const lastSeen = lastSeenId;
  inputEl.value = "";
  closeMentions();
  // Off the composer at once with the text, so a second Enter while the files
  // are being saved does not send them twice. Put back with it below on
  // anything but a delivery.
  attachments = [];
  renderAttachments();
  const putBack = (): void => {
    inputEl.value = text;
    attachments = [...pending, ...attachments];
    renderAttachments();
  };
  // Saved before the post, so every path in it names a file that is there by
  // the time a session reads it. One at a time, in the order the chips stand,
  // which is the order the paths are written in.
  const paths: string[] = [];
  try {
    for (const one of pending) paths.push(await saveAttachment(one, topicId));
  } catch (err) {
    putBack();
    status(`添付を保存できませんでした: ${err}`, "error");
    return;
  }
  const content = withAttachments(text, paths);
  try {
    const outcome = await invoke<PostOutcome>("room_post", {
      topicId,
      speaker,
      content,
      lastSeen,
    });
    if (!outcome.delivered) {
      // Refused, not failed. The refusal carries the posts themselves, so they
      // go on the glass here and the person reads them and decides again — #47
      // unchanged, except that what it says to read is now there to read. The
      // reading is left where it belongs; only the means of doing it is added.
      // The text as written and the chips as they stood: the paths are the
      // chips, already saved, and go into the text again when it is sent.
      putBack();
      // Drawn only into the conversation they belong to. A topic opened during
      // the round trip is another conversation, and it has drawn its own log.
      const drew = topicId === shownTopicId() ? drawMissed(outcome.missed) : 0;
      const speakers = [...new Set(outcome.missed.map((one) => one.speaker))];
      // What was drawn, and nothing about how much arrived. Beyond the floor's
      // 512 the room cannot enumerate what it dropped and this screen cannot
      // count it, so a total here would be a number that looks complete and is
      // not.
      status(
        drew
          ? `送っていません。書いている間に届いた発言 ${drew} 件を下へ描きました（${speakers.join("、")}）。読んでから送るか決めてください。`
          : `送っていません。書いている間に届いた発言（${speakers.join("、")}）は画面に出ています。読んでから送るか決めてください。`,
        "error",
      );
      return;
    }
    status("");
  } catch (err) {
    // Put the text back rather than losing what was typed.
    putBack();
    status(`発言を送れませんでした: ${err}`, "error");
  }
}

/**
 * Show what the terminal on the glass was launched from.
 *
 * Read off the shown view rather than written once at launch: with a terminal
 * per account these values answer "what is this pane", and a pane switched away
 * from that left its command on screen would be answering for the wrong one.
 * They survive the session's exit — the question is what ran.
 */
function renderSessionFacts(): void {
  const view = shownView();
  if (!view) {
    sessionStateEl.textContent = "未起動";
    sessionStateEl.dataset.kind = "info";
    transportEl.textContent = "—";
    commandEl.textContent = "—";
    dirEl.textContent = "—";
    dirEl.title = "";
    startedEl.textContent = "—";
    windowEl.textContent = "—";
    renderSessionStats();
    renderSessionId();
    return;
  }
  const name = viewName(view);
  sessionStateEl.textContent =
    view.ended === null
      ? `${name} 起動中`
      : view.endRequested
        ? `${name} 終了`
        : `${name} 終了（${view.ended}）`;
  sessionStateEl.dataset.kind = view.ended === null ? "ok" : view.endRequested ? "info" : "error";
  transportEl.textContent = "PTY";
  commandEl.textContent = view.command;
  dirEl.textContent = view.cwd ?? "—";
  dirEl.title = view.cwd ?? "";
  startedEl.textContent = view.startedAt === "" ? "—" : shortTime(view.startedAt);
  showWindowSize();
  renderSessionStats();
  renderSessionId();
}

/**
 * A percentage as this column shows one: a whole number.
 *
 * The CLI sends a fraction (`23.5`), and the column is the narrow half of a
 * 16.5rem panel. The tenth would cost a character in every one of three rows to
 * say something nobody reads a usage bar that closely for.
 */
function usedPercent(value: number | null): string {
  return value === null ? "—" : `${Math.round(value)}%`;
}

/**
 * Show what the terminal on the glass last said about itself (#155).
 *
 * `—` for a pane with no terminal, and for one whose session has not reported
 * yet. The two are the same answer here on purpose: what a row would otherwise
 * show is a value this screen does not have, and #82's line — no word for a
 * state nobody observed — is the same line one column over.
 *
 * Not cleared when the session ends or falls quiet. The status line runs when
 * the session runs, so these rows stand at the last thing that was reported
 * (decision 4); a row blanked on silence would say the session stopped using a
 * context window it is still holding.
 */
function renderSessionStats(): void {
  const stats = shownView()?.stats ?? null;
  statsEls.model.textContent = stats?.model ?? "—";
  statsEls.effort.textContent = stats?.effort ?? "—";
  renderUsage(statsEls.five_hour, stats?.five_hour ?? null);
  renderUsage(statsEls.seven_day, stats?.seven_day ?? null);
  renderUsage(statsEls.context, stats?.context ?? null);
}

/**
 * One usage row: the number, and a bar under the same value (#225).
 *
 * `—` with no bar while nothing has been reported — an empty bar would say 0%,
 * and not yet knowing is not that (#155). The bar is capped at full; the number
 * is not, because a spend limit can go past 100% and the number is what says by
 * how much. At 100% or over the bar takes the danger colour, the line 制限中
 * stands on (#161).
 */
function renderUsage(cell: HTMLElement, value: number | null): void {
  const text = document.createElement("span");
  text.className = "value";
  text.textContent = usedPercent(value);
  if (value === null) {
    cell.replaceChildren(text);
    return;
  }
  const meter = document.createElement("span");
  meter.className = "meter";
  const fill = document.createElement("span");
  fill.className = "fill";
  fill.style.width = `${Math.min(100, Math.max(0, value))}%`;
  if (value >= 100) fill.dataset.kind = "error";
  meter.appendChild(fill);
  cell.replaceChildren(meter, text);
}

/**
 * The session id the topic index holds for the shown pane's account.
 *
 * Read from the index and not from the launch's answer (#139, decision 1). The
 * record is what outlives the app, so reopening it and opening the topic shows
 * the same value; and it is what `forget_session` takes away, so a record that
 * went takes the value off the screen with it rather than leaving an id nothing
 * can resume into. Redrawn whenever the index is, since a launch records its id
 * after the pane already exists.
 *
 * The topic is the pane's own — the one its launch went into. Every pane on the
 * glass belongs to the topic on the glass (#141), and reading the pane's own is
 * what keeps that true by construction rather than by agreement. Before the
 * launch answers there is no record to read yet, and the row reads — like the
 * other values that wait on the launch.
 *
 * なし is a value, not a blank: an account whose launch line declares no id, or
 * a record that is gone, leaves nothing to resume by hand, and saying so is
 * different from not yet knowing (#139, decision 2).
 */
function renderSessionId(): void {
  const view = shownView();
  let id: string | null = null;
  let known = false;
  if (view && view.ptyId !== "") {
    known = true;
    id = topics.find((topic) => topic.topic_id === view.topicId)?.sessions[view.accountId] ?? null;
  } else if (!view) {
    // No pane on the glass: after a restart there is none, and the record is
    // still there to read (#144, decision 1). Whose record is `idleAccount`'s
    // answer; the topic is the one open, because no launch has named another.
    const accountId = idleAccount();
    const topic = topics.find((one) => one.topic_id === shownTopicId());
    if (accountId !== null && topic) {
      known = true;
      id = topic.sessions[accountId] ?? null;
    }
  }
  sessionIdEl.textContent = id ?? (known ? "なし" : "—");
  sessionIdEl.title = id ?? "";
  sessionIdCopyEl.hidden = id === null;
  sessionIdCopyEl.dataset.id = id ?? "";
}

/**
 * The account the panel's values speak for while no terminal is on the glass.
 *
 * The row chosen in the list when there is one, and otherwise the first account
 * that launches, in the order the list draws them (#144, decision 1). An `admin`
 * account has no session to have recorded, so it is never the fallback; which
 * CLI the others launch does not enter into it. Once a terminal is on the glass
 * this is not asked: the values follow the pane again (decision 2).
 */
function idleAccount(): string | null {
  const launched = members()
    .filter((row) => row.account !== null && launches(row.account))
    .sort((a, b) => memberName(a).localeCompare(memberName(b)))
    .map((row) => row.account!.id);
  if (shownAccount !== null && launched.includes(shownAccount)) return shownAccount;
  return launched[0] ?? null;
}

/**
 * Copy the shown session id, for a `--resume` typed by hand (#139, decision 3).
 *
 * The value copied is the one on screen, not a re-read: what the person saw is
 * what they get.
 */
async function copySessionId(): Promise<void> {
  const id = sessionIdCopyEl.dataset.id ?? "";
  if (id === "") return;
  try {
    await writeText(id);
    status(`セッション ID をコピーしました: ${id}`);
  } catch (err) {
    status(`セッション ID をコピーできませんでした: ${err}`, "error");
  }
}

/**
 * Ask whether one account's session is to end. Nothing ends until answered.
 *
 * Ending a session cannot be undone, and the menu it is chosen from sits on a
 * row whose plain click merely changes which pane is showing. One plain click
 * away from a harmless neighbour is how a slip ends a session that was
 * mid-answer, so choosing 終了 opens the question and the dialog is where it
 * is answered (#71, #224).
 *
 * A dialog of this app's own, never `window.confirm`. That one answers on the
 * host's terms and the two ways it can fail here are both wrong: a host that
 * answers nothing either makes the item silently dead or — reading its own
 * default as yes — ends the session on the single click. `#end-dialog` is in
 * the webview and has neither failure (#57).
 */
function openEndDialog(key: string, name: string): void {
  endingAccount = key;
  endMessageEl.textContent = `${name} のセッションを終了します。よろしいですか？`;
  endDialogEl.showModal();
}

/** Leave the question unanswered. Escape lands here too, by the close handler. */
function closeEndDialog(): void {
  endingAccount = null;
  if (endDialogEl.open) endDialogEl.close();
}

/** The answer that acts. */
function confirmEndDialog(): void {
  const key = endingAccount;
  closeEndDialog();
  if (key === null) return;
  // Resolved now, not when the dialog opened: the session may have ended on its
  // own while the question stood, and there is then nothing left to end.
  const view = views.get(key);
  if (view) void endSession(view);
}

/**
 * End one account's session.
 *
 * The seat is released by the session ending, not by this call: `RoomSeats`
 * reads liveness off the PTY, so the account is offline again and startable the
 * moment the process is gone (#53). The view stays until another account is
 * chosen, because what it last printed is the only account of how it ended.
 */
async function endSession(view: SessionView): Promise<void> {
  const name = viewName(view);
  try {
    await invoke("kill_pty", { id: view.ptyId });
  } catch (err) {
    renderPanel();
    status(`${name} を終了できませんでした: ${err}`, "error");
    return;
  }
  status(`${name} を終了しました。`);
  // The exit event marks the view ended and redraws the row; this call only
  // says the kill was delivered.
  await refreshSeats();
  renderPanel();
}

/**
 * The names of whatever is running under a seat right now.
 *
 * Asked of the app rather than read off `seated`, which is this screen's copy
 * and is only as fresh as whatever last refreshed it. The question about to be
 * put is whether closing would end anything, and the app is what would end it
 * (`seated_accounts` is the authority the panel already defers to).
 *
 * A seat with no session counts. It is a launch still in flight, and what it is
 * about to leave behind is a process — asking about it costs one dialog, while
 * reading it as nothing to end is how the one case that is not visible yet
 * becomes the orphan (#85).
 *
 * On a failed read the last answer stands, for the reason `refreshSeats` keeps
 * it: a read that failed says nothing about who is running, and answering
 * "nobody" would close the app over a running session without ever asking.
 */
async function runningSeatNames(): Promise<string[]> {
  let held: SeatedAccount[];
  try {
    held = await invoke<SeatedAccount[]>("seated_accounts");
  } catch {
    held = [...seated.values()];
  }
  // Named from the account list, and by id where the account is not in it. The
  // app refuses to delete a seated account, so a seat with no account is a
  // config edited from outside — it is still running, so it is still counted,
  // and the id is what the app can be asked about it under.
  //
  // Once per account, however many topics it is running in (#141): the question
  // is who is running, and a name said twice reads as two people.
  return [
    ...new Set(
      held.map(
        (seat) =>
          accounts.find((one) => one.id === seat.account_id)?.name.trim() || seat.account_id,
      ),
    ),
  ];
}

/**
 * Ask whether the app is to close with sessions still running.
 *
 * A promise rather than a callback, because the window's close is held open
 * across the answer and the caller is the one holding it.
 */
function askQuit(names: string[]): Promise<boolean> {
  quitMessageEl.textContent =
    `${names.join("、")} のセッションが走っています。` +
    `アプリを終了すると、これらのセッションも終了します。よろしいですか？`;
  return new Promise((resolve) => {
    quitAnswer = resolve;
    quitDialogEl.showModal();
  });
}

/** Answer the standing question, once. Escape lands here too, as 取消. */
function answerQuit(confirmed: boolean): void {
  const answer = quitAnswer;
  // Cleared before the close, because closing fires the handler that calls this
  // again: that second call finds nothing standing and does nothing. Without it
  // the answer would be delivered twice, and the second one is always 取消.
  quitAnswer = null;
  if (quitDialogEl.open) quitDialogEl.close();
  answer?.(confirmed);
}

/**
 * Decide what closing the window does, with the close held open until it is.
 *
 * There is no round trip to arrange for this. Tauri prevents the close itself
 * the moment this screen is listening for it — `WindowEvent::CloseRequested`
 * calls `api.prevent_close()` when `window.has_js_listener` finds a webview
 * listener (tauri 2.11.5, `src/manager/window.rs`) — and `onCloseRequested`
 * destroys the window after this returns without `preventDefault`
 * (`@tauri-apps/api/window`). So preventing is what says "stay open", and
 * returning is what closes; no `on_window_event` handler on the Rust side is
 * in the path.
 *
 * Nothing running means nothing to ask about, and the app closes on the click
 * that asked for it. A confirmation over an empty list is one more step on a
 * close that would end nothing (#85).
 *
 * 取消 takes the close back and nothing else: the window stays and every
 * session keeps running. There is deliberately no answer here that ends the
 * sessions and leaves the app standing — ending one session is what the row's
 * 終了 is for, and this dialog is the app going away.
 */
async function onQuitRequested(event: CloseRequestedEvent): Promise<void> {
  // A second close while the first is still being decided is the same question,
  // and it is already on the screen. Taken back rather than asked again:
  // `showModal` on an open dialog throws, and a throw here leaves the window's
  // close unresolved with no dialog able to release it (`onCloseRequested`
  // awaits this before it destroys anything).
  if (quitPending) {
    event.preventDefault();
    return;
  }
  quitPending = true;
  try {
    const running = await runningSeatNames();
    if (!running.length) return;
    if (!(await askQuit(running))) {
      event.preventDefault();
      return;
    }
    // Nothing is caught around this, and nothing here takes the close back on a
    // failed sweep. `kill_all_ptys` has no failure to return: the kill it calls
    // cannot report one on Windows (`PtyState::kill_all`, and #96 for the root
    // of it). A catch here would be a branch that never runs, next to a comment
    // promising the app stays open when the sessions survive — a protection
    // that reads as present and is not. If #96 lands, this is where it goes
    // back.
    await invoke("kill_all_ptys");
  } finally {
    // Cleared on every path, the closing one included: the window is destroyed
    // after this returns, and a flag left standing would refuse the next close
    // if the destroy never came.
    quitPending = false;
  }
}

/**
 * Give an account its own terminal and put it on the glass.
 *
 * Made before the launch, because the CLI's first paint is laid out for the
 * size this pane reports and there is nothing else to measure.
 *
 * `running` is passed when the session is already up and this terminal is
 * being made for it rather than ahead of it — a session picked up again after
 * this screen was reloaded (#84). Its facts then come from the app's record of
 * the launch instead of from the account, which may have been edited since.
 *
 * `topicId` is the topic the terminal is filed under (#141). It is put on the
 * glass only when that topic is; a session picked up in another topic after a
 * reload is opened hidden, where coming back to its topic will find it.
 */
function openView(account: Account, topicId: string, running?: RunningSession): SessionView {
  const key = seatKey(topicId, account.id);
  // A relaunch replaces the previous run's pane. Two panes for one account in
  // one topic would be two rows under one name, and the row is what the
  // operations hang on; the scrollback that goes with it is the one the person
  // just decided to start over from.
  discardView(views.get(key));

  const host = document.createElement("div");
  host.className = "term";
  terminalEl.appendChild(host);

  // At the size this screen is set to, not at the default in the options: a
  // terminal opened after the size was changed would otherwise be the one pane
  // that is a different size from the rest.
  const term = new Terminal({ ...TERMINAL_OPTIONS, fontSize: terminalFontSize });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);
  term.options.theme = terminalTheme();

  const view: SessionView = {
    accountId: account.id,
    ptyId: running?.pty_id ?? "",
    name: account.name,
    command: running?.command ?? account.command,
    cwd: running?.cwd ?? account.cwd,
    startedAt: running?.started_at ?? "",
    topicId,
    resumedFrom: running?.resumed_from ?? null,
    term,
    fit,
    host,
    unlisten: [],
    // False even for a session picked up again after a reload. The roster is
    // read on its own event and this account is on it if the session is in the
    // room, so the answer arrives rather than being assumed here — and assuming
    // it from the roster as it stands would read a connection the previous run
    // of this account has not finished dropping as this one having arrived.
    seenInRoom: false,
    ended: null,
    endRequested: false,
    // Quiet until something arrives. A session picked up again after a reload
    // starts here too: its terminal is new even though its process is not, so
    // what this screen can say about it begins at the next byte (#86).
    outputting: false,
    silent: false,
    // Nothing reported yet, on a fresh terminal and on one picking a running
    // session up again alike. The status line runs on the next assistant
    // message, so the values arrive on their own (#155) — and 制限中 arrives
    // with them, since the word is read off two of these values (#161).
    stats: null,
    quiet: undefined,
  };

  term.onData((data) => {
    // This view's own session, never "the session that started last". The
    // terminal being typed into is the one on the glass, and the two were not
    // the same thing while one `activePtyId` stood for both (#57).
    if (view.ptyId === "" || view.ended !== null) return;
    void invoke("write_pty", { id: view.ptyId, data }).catch((err) => {
      status(`セッションへ送れませんでした: ${err}`, "error");
    });
  });

  // The webview does not deliver a native paste to xterm, so Ctrl+V is bridged
  // explicitly. preventDefault stops the input arriving twice.
  term.attachCustomKeyEventHandler((event) => {
    const isPaste =
      event.type === "keydown" &&
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      (event.key === "v" || event.key === "V");
    if (!isPaste) return true;

    event.preventDefault();
    void readText().then((text) => {
      // Same destination as a keystroke: the pasted text goes to this pane's
      // session, so a paste cannot land in a session that is not on screen.
      if (text && view.ptyId !== "" && view.ended === null) {
        void invoke("write_pty", { id: view.ptyId, data: text });
      }
    });
    return false;
  });

  views.set(key, view);
  if (topicId === shownTopicId()) showView(account.id);
  else host.hidden = true;
  return view;
}

/** Close one terminal for good: its listeners, its emulator, its scrollback. */
function discardView(view: SessionView | undefined): void {
  if (!view) return;
  for (const off of view.unlisten) off();
  view.unlisten = [];
  stopOutput(view);
  view.term.dispose();
  view.host.remove();
  views.delete(seatKey(view.topicId, view.accountId));
  if (view.topicId === shownTopicId() && shownAccount === view.accountId) shownAccount = null;
  if (shownByTopic.get(view.topicId) === view.accountId) shownByTopic.delete(view.topicId);
}

/**
 * Put one account's terminal on the glass, and take the others off it.
 *
 * Hidden, not discarded: a session keeps running while another is being
 * watched, and its output keeps arriving into its own emulator, so switching
 * back finds the scrollback where it was left.
 *
 * A terminal whose session has ended is no longer the exception. It was
 * discarded here, on the reasoning that choosing another account is the person
 * saying they have read what it printed on its way out (#57). Its tab carries a
 * ✕ now, so that saying is an act rather than a by-product of looking elsewhere,
 * and `closeView` is where it lands (#68). The cost is that an ended terminal
 * holds its scrollback until someone closes it — a tab left alone is memory held
 * — and that is the accepted half of the trade.
 */
function showView(accountId: string | null): void {
  shownAccount = accountId;
  const topicId = shownTopicId();
  for (const view of views.values()) {
    view.host.hidden = view.topicId !== topicId || view.accountId !== accountId;
  }
  renderPanel();
  renderSessionFacts();
  fitShown();
  // A pane that was `display: none` kept filling its buffer and painted
  // nothing, so coming back to it has to repaint from the buffer. The fit above
  // does that only when the measured size changed, and returning to a pane the
  // same size as the one just left is exactly when it did not.
  const shown = shownView();
  if (shown) shown.term.refresh(0, shown.term.rows - 1);
}

/**
 * Subscribe one view to its session: everything it prints, and its exit.
 *
 * Both listeners are this view's, and are dropped with it. The shared terminal
 * subscribed once per launch and unsubscribed never, which is how every running
 * session ended up writing into one pane (#57).
 *
 * Reached from two directions: a launch this screen just made, and a session it
 * is picking up again after having been reloaded out from under it (#84). The
 * pty id is the whole of what either one needs — nothing else about a session
 * is remembered on this side, which is why one can be followed again from the
 * id alone, and why losing the id is what made a running session unreachable.
 */
/**
 * What `pty-exit-{id}` carries (`PtyExit` in `pty.rs`).
 *
 * `requested` is the app's own account of whether it ended the session, marked
 * where the kill happened. Nothing on this side infers it — a missing code is
 * not the same fact, since a code can go missing on an end nobody asked for.
 */
interface PtyExit {
  code: number | null;
  requested: boolean;
}

/** How an ended session's end reads in a parenthesis: the code, or 終了 alone. */
function endedNote(view: SessionView): string {
  return view.endRequested ? "終了" : view.ended ?? "";
}

async function attachSession(view: SessionView, ptyId: string): Promise<void> {
  view.unlisten.push(
    await listen<string>(`pty-data-${ptyId}`, (event) => {
      view.term.write(event.payload);
      // The bytes go to the emulator and are not looked at here. That this
      // chunk arrived is the whole of the signal (#82).
      markOutput(view);
    }),
  );
  // From here the silence is being timed. A session that has printed nothing
  // yet reaches 待機 after one quiet window, the same as one that stopped (#148).
  if (view.ended === null && view.quiet === undefined) armQuiet(view);
  view.unlisten.push(
    await listen<PtyExit>(`pty-exit-${ptyId}`, (event) => {
      const { code, requested } = event.payload;
      const detail = code === null ? "終了コード不明" : `終了コード ${code}`;
      view.ended = detail;
      view.endRequested = requested;
      // Nothing more will print, so the row must not spend the quiet window
      // still saying that something is (#82).
      stopOutput(view);
      // Nothing more will arrive on this pty. The view lives on for what it
      // has already printed, not for anything it is still waiting to hear.
      for (const off of view.unlisten) off();
      view.unlisten = [];
      const name = viewName(view);
      // An end the app asked for is one the person has just agreed to in a
      // dialog. Calling it an unexplained exit and sending them to the terminal
      // has them look for a problem that is not there, and a warning that is
      // always safe to ignore is ignored on the day it is not (#121).
      if (requested) status(`${name} を終了しました。`);
      else status(`${name} が終了しました（${detail}）。端末を確認してください。`, "error");
      // A resume that ended on its own without the room ever having seen it
      // went back into a session that is not there. The record it went in on is
      // what every later launch into this topic will fail on the same way, so
      // it goes (#127).
      void dropDeadResume(view, event.payload, detail);
      // The seat this account held is free the moment its session ends, so the
      // panel says 未起動 again and the account can be started once more.
      void refreshSeats();
      renderPanel();
      if (view === shownView()) {
        renderSessionFacts();
        // Only for an end nobody asked for: what it printed on the way out is
        // the account of why. An end asked for leaves the pane as it was — not
        // opened, and not closed either if it was already open (#121).
        if (!requested) revealDiagnostics();
      }
    }),
  );
}

/**
 * Take this topic's record of a session a resume could not go back into.
 *
 * The failure it answers has no other way out. A session id is recorded when the
 * spawn returns, which is a process having started and not a conversation having
 * been made — a CLI stopped at a confirm prompt and closed there leaves an id
 * behind it. Every launch of that account into that topic afterwards takes the
 * resume line, fails on the id, and ends; the pair is broken until the record
 * goes, and nothing was taking it (#127).
 *
 * The condition is three things this app already observes: the launch went in on
 * the resume line, the room never carried the account, and the session ended on
 * its own. The CLI's own words are not read for any of them. Matching the
 * message it prints would be a check that stops working the day the wording
 * changes, and stops silently (#127, 制約).
 *
 * The third is what the exit event says about who ended it (`PtyExit.requested`,
 * #121), and not any value of the code: the row's ✕, the topic delete, and the
 * app closing arrive marked as ends the app asked for, and none of them says the
 * resume failed. The window it covers is real: the confirm prompt holds a
 * launched CLI outside the room for minutes (#89), and ending the wrong account
 * during it is an ordinary act that must not cost a resume that would have
 * worked. An end nobody asked for that still arrives with no code falls the same
 * way, which is the safe side — the poisoned launch ends by itself and is caught
 * on the next press.
 *
 * It does not start anything again. The record is off, so the next press is a
 * normal launch — and whether to press is the person's. Retrying here would be
 * this screen running a failure round and round, which is the shape
 * `model-loop-safety` is about (#127, AI 判断2).
 *
 * Said in the status line, because a repair nobody is told about is a history
 * that quietly stopped being continuous. Only after the app answers that a
 * record was actually dropped: the account is seatless the moment it exits, so
 * a launch made in between owns the record now, and this says what happened
 * rather than what it asked for.
 */
async function dropDeadResume(
  view: SessionView,
  exit: PtyExit,
  detail: string,
): Promise<void> {
  const dead = view.resumedFrom;
  if (dead === null || view.seenInRoom || exit.requested || exit.code === null) return;
  const name = viewName(view);
  try {
    const dropped = await invoke<boolean>("room_forget_session", {
      topicId: view.topicId,
      accountId: view.accountId,
      sessionId: dead,
    });
    if (!dropped) return;
    status(
      `${name} は会話へ戻れないまま終了しました（${detail}）。このトピックの再開先を外したので、次は通常の起動になります。`,
      "error",
    );
  } catch (err) {
    // The record is still there, which means the next launch fails the same
    // way. Saying so is the whole of what is left to do here — a repair that
    // failed quietly reads as a repair that happened.
    status(`${name} の再開先を外せませんでした: ${err}`, "error");
  }
}

/**
 * Follow a launched session until it dies.
 *
 * A session that exits on startup is the failure mode with no other witness:
 * the room simply stays empty. Without this the screen is identical whether
 * the CLI is running or was never there.
 */
async function followSession(view: SessionView, started: StartedSession): Promise<void> {
  view.ptyId = started.pty_id;
  view.startedAt = started.started_at;
  // Which line ran, from the launch's own answer. It is what the exit reads,
  // and the exit can arrive as soon as the listener below is attached, so it is
  // set before it (#127). Where it ran is not set here: the terminal was filed
  // under its topic when ▶ was pressed, and the launch went into the topic it
  // was told (#141).
  view.resumedFrom = started.resumed_from;
  renderPanel();
  renderSessionFacts();

  await attachSession(view, started.pty_id);

  // The pane is left as it was. It used to open here because the first thing a
  // session showed was a question — the development-channels confirm — and the
  // answer went in through this terminal. That flag left the launch line in
  // #195, so nothing waits on the person at startup, and a pane that opens on
  // every ▶ is one the person keeps folding back (#215). What still opens it is
  // an end nobody asked for (`attachSession`, #121).
  //
  // Focus only into a terminal that is on screen. A folded pane has nothing to
  // type into, the same as a row picked while it is folded (#175).
  if (view === shownView() && !diagnosticsEl.hidden) view.term.focus();
}

/**
 * Start one account's session: the row's half of the lifecycle 終了 closes.
 *
 * Takes the account rather than reading a picker. There is no picker — which
 * account this is, is which row was pressed (#62).
 */
async function startSession(account: Account): Promise<void> {
  const name = account.name.trim();
  if (!name) {
    status(
      "アカウントの名前を入力してください。部屋での名乗りになります。",
      "error",
    );
    openAccountDialog(account);
    return;
  }

  // One account, one seat per room. Said here so the reason is on screen in the
  // language it is read in; the app refuses it as well, and that refusal is the
  // authority — this check only gets there first (#53).
  //
  // Re-read before refusing, never after. The copy this screen holds is only as
  // new as the last thing that moved, and a launch that was in flight when the
  // screen reloaded is a seat with no session under it yet — a row that draws
  // 開始 with nothing to end beside it. Asking again resolves that seat, and
  // resolving it is what puts 終了 on the row (`adoptSeats`), so the refusal
  // below now names something the person can act on (#84).
  //
  // In this topic. The same account running in another topic is the shape
  // #141 asks for, and is not refused.
  const topicId = shownTopicId();
  const key = seatKey(topicId, account.id);
  if (seated.has(key)) await refreshSeats();
  if (seated.has(key)) {
    status(
      `「${name}」は既にこのトピックに居ます。一つのアカウントが持てる席は一つのトピックにつき一つです。このトピックで起動中のセッションを終了してから、もう一度起動してください。`,
      "error",
    );
    return;
  }

  // Read off the account, which is where the person set it — this row no longer
  // carries a field of its own to read it out of (#59). Not defaulted to
  // whatever directory the app process happens to sit in: that is what put a
  // session in src-tauri (#20).
  if (!(account.cwd ?? "").trim()) {
    status(
      `「${name}」に作業ディレクトリがありません。編集から設定してください。`,
      "error",
    );
    openAccountDialog(account);
    return;
  }

  // What was on the glass is kept, so a launch that fails can put it back
  // rather than leaving a blank pane where a running session had been.
  const previous = shownAccount;
  // Cleared as the attempt starts rather than as it fails: 起動失敗 stands on
  // the row until this account is asked again, and this is that moment.
  launchFailures.delete(key);
  // From here the row carries the launch. The view exists and has no pty id
  // yet, which is what puts its 開始 into 起動中; `openView` redraws through
  // `showView`.
  const view = openView(account, topicId);
  // Size the PTY to the terminal that will display it, so the CLI's first
  // paint is not laid out for a window it does not have. An open pane was
  // fitted by `showView` just now; a folded one is measured without being
  // opened, because ▶ no longer opens it (#215).
  fitFolded(view);

  status(`${name} を起動しています…`);
  try {
    const started = await invoke<StartedSession>("start_session", {
      account,
      topicId,
      cols: view.term.cols,
      rows: view.term.rows,
    });
    // Which of the two lines ran is said, because the person is the one who
    // can tell whether it mattered. A seat that came back fresh in a reopened
    // topic is a legitimate outcome and not a silent one (#115, decision 6).
    //
    // A fresh line the topic had a record for is a third thing to say. The
    // record went before this launch was made, and a launch that reads as an
    // ordinary one leaves the person to find out from the next 起動しました
    // that the way back is gone (#131, decision 2).
    status(
      started.resumed_from !== null
        ? `${name} を再開しました。${started.mcp_config} に登録済み。`
        : started.dropped_resume !== null
          ? `${name} を起動しました。戻る先の会話が見つからなかったため、このトピックの再開先は外しました。${started.mcp_config} に登録済み。`
          : `${name} を起動しました。${started.mcp_config} に登録済み。`,
    );
    await refreshSeats();
    await followSession(view, started);
  } catch (err) {
    // Nothing was spawned, so this terminal has nothing to show and no session
    // to end. It goes, and the app's own reason stands in the status line —
    // recorded against the account first, because the pane going is what would
    // otherwise leave the row reading 未起動 as though it had never been
    // pressed. The row says 起動失敗 and holds the reason; the status line has
    // it in full.
    launchFailures.set(key, String(err));
    discardView(view);
    // Only when the topic it was pressed in is still on the glass. Otherwise
    // the pane was hidden with its topic, and the topic on the glass has its
    // own pane showing.
    if (topicId === shownTopicId()) {
      showView(
        previous !== null && views.has(seatKey(topicId, previous))
          ? previous
          : (topicViews().pop()?.accountId ?? null),
      );
    }
    status(`${name} を起動できませんでした: ${err}`, "error");
    // The pane is left as it was here too. Nothing was spawned, so there is no
    // output in it to read about why; the reason is the status line above and
    // the row's 起動失敗. What opens the pane is a CLI that did start and then
    // ended on its own, since what it printed is the account (#121, #215).
    // A launch that failed after the app claimed the seat releases it there;
    // this keeps the panel in step with that.
    await refreshSeats();
  } finally {
    // Both paths above redraw already. This is so that no path can leave a row
    // saying 起動中 for a launch that is over.
    renderPanel();
  }
}

/**
 * A default name for a new account that no existing account already answers to.
 *
 * A constant default would put every new account on one name, which is the
 * defect #40 removed — two participants answering alike, neither addressable.
 * The identity is the id and would survive that, but being able to name one of
 * them is the point of a name, so the default counts up past whatever is taken.
 * It is a starting point in an editable field, not a value anyone is stuck with.
 */
function unusedAccountName(): string {
  const taken = new Set(accounts.map((account) => account.name.trim()));
  for (let n = accounts.length + 1; ; n += 1) {
    const candidate = `アカウント ${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

// ── the account dialog ───────────────────────────────────────────────────────
//
// One form for making, editing and deleting an account. It holds a draft and
// writes nothing until 決定; 取消 leaves nothing behind, for a new account as
// much as for an edit. The fields used to save on `change`, which meant ＋
// created an account the instant it was pressed and every keystroke on the way
// to a name was a state that had existed — there was no deciding and no undoing
// (#59).

/** The account being edited, or null while the form is making a new one. */
let editing: Account | null = null;
/** The draft the form is filling in. Never the account itself. */
let draft: Account | null = null;
/** True once 削除 has been armed. The shape 終了 held until #71; see #72. */
let deleteArmed = false;
/**
 * The environment field exactly as it was drawn for the draft (masks, one line
 * per variable), or null while it has not been drawn — still loading, or the
 * drawing failed (#163).
 *
 * What 決定 compares the field against. Unchanged, the draft's sealed values
 * stand as they are and nothing is sent to be sealed. Null, the field is not
 * read at all: an empty box that never received the stored lines is not the
 * person clearing them, and reading it as that would delete every variable.
 */
let dialogEnvDrawn: string | null = null;

/** Say why the form cannot be decided yet, or clear that. */
function dialogError(text: string): void {
  dialogErrorEl.textContent = text;
}

/** Put 削除 back to resting. */
function disarmDelete(): void {
  deleteArmed = false;
  dialogDeleteEl.textContent = "削除";
  dialogDeleteEl.classList.remove("armed");
}

/**
 * Show only the fields that mean something for the kind being declared.
 *
 * A person has no command under them, so a working directory and launch options
 * would be two fields that never do anything.
 *
 * The resume line is the same judgment one level in (#156, 決定6). A kind that
 * names a CLI holds the way back into one of its sessions, so the field would
 * be a second answer to a question already answered — and a second answer is
 * one that can disagree. The kind that names none has only the field.
 */
function showDialogKind(): void {
  const kind = dialogKindEl.value as AccountKind;
  // A server launches nothing either; what it is started with is its entry in
  // the file, and that is the section below rather than these fields (#193).
  dialogLaunchEl.hidden = !launchesKind(kind);
  dialogMcpEl.hidden = kind !== "mcp";
  dialogResumeFieldEl.hidden = kind !== "cli";
  if (launchesKind(kind)) refreshDialogLine();
  // Chosen on a form making an account: the server is written and started at
  // 決定, so what there is to fill in now is what it is started with (#200).
  if (kind === "mcp" && editing === null) drawNewMcp();
}

/** `launches`, for a kind the form holds rather than an account. */
function launchesKind(kind: AccountKind): boolean {
  return kind === "claude_code" || kind === "cli";
}

/**
 * Show the command this account's launch would actually run.
 *
 * The app adds the room's own settings to whatever is typed and selects the
 * character named above, so the line written here is not the line that
 * launches; showing the result is cheaper than explaining either. The character
 * is why this reads the fields rather than the draft: it is the one place the
 * `--settings` it becomes can be seen before 決定, and a preview built from the
 * draft would only show it on the next open. The entry names this account's own
 * server, which follows the account id — so the preview holds still while the
 * name in the field above it is edited. Holding still is the point: the
 * identity being launched is the account, and renaming it does not make it
 * something else (#53).
 *
 * The working directory goes with them: another account's room registration
 * sitting in it is named on the line as one this session does not start (#103).
 * That is the case worth seeing before 決定 — pointing an account at a shared
 * directory is what puts `--settings` on a line that had none.
 */
async function refreshDialogPreview(): Promise<void> {
  if (!draft) return;
  const id = draft.id;
  try {
    const parsed = await invoke<string[]>("parse_launch_options", {
      text: dialogOptionsEl.value,
    });
    const merged = await invoke<string[]>("preview_launch_args", {
      args: parsed,
      accountId: id,
      // The field rather than the draft, for the reason the character is read
      // that way: what the kind's conventions put on the line is on the line
      // shown, and the kind is being edited right there (#156).
      kind: dialogKindEl.value as AccountKind,
      // The topic on the glass, which is where ▶ would launch it: the entry is
      // this account's in this topic (#141, decision 4).
      topicId: shownTopicId() || null,
      // The field rather than the draft: the preview answers for what the form
      // holds now, and the draft is only written at 決定.
      character: dialogCharacterEl.value.trim() || null,
      cwd: dialogCwdEl.value.trim() || null,
    });
    // The form may have been closed or reopened during the round trip.
    if (draft?.id !== id) return;
    dialogPreviewEl.textContent = `${draft.command} ${joinArgs(merged)}`;
  } catch {
    dialogPreviewEl.textContent = "";
  }
}

/** The characters `cmd.exe` acts on, as the sentences below name them. */
const CONSOLE_HAZARDS = '& | < > ^ ( ) "';

/**
 * Say what a launch would do with what the form holds (#154, 決定4).
 *
 * The line that runs is drawn above this, and it is where the result is
 * visible — an account whose character was left off it shows a line with no
 * `outputStyle` in the JSON, and one whose options were left off shows a line
 * without them. That is legible once the person already knows what to look
 * for. This says it: what is missing from that line, and what happens at 起動.
 *
 * It does not refuse anything. The value is one the person wrote and the
 * account saves as written — what they can act on is knowing, before the
 * launch, which of these four things it will do. The launch itself refuses the
 * two it has to (`session::start_session`), and that refusal is the authority;
 * this only gets there first, at the moment it can be fixed rather than at the
 * moment it fails — the shape the two-`--settings` check here already has
 * (#99).
 */
async function refreshDialogNotice(): Promise<void> {
  const kind = dialogKindEl.value as AccountKind;
  if (!draft || !launchesKind(kind)) {
    dialogNoticeEl.textContent = "";
    return;
  }
  const id = draft.id;
  try {
    const report = await invoke<LaunchFieldReport>("launch_field_report", {
      character: dialogCharacterEl.value.trim() || null,
      options: dialogOptionsEl.value,
      // The draft's, because no field writes it: it is the command the kind
      // names (`config.rs`), and an account carrying one from an older file is
      // the way an unlaunchable one is reached.
      command: draft.command,
      // Only where the field is the answer. On a kind that holds its own way
      // back, a line stored here is not the one that runs (#156, 決定6).
      resume: kind === "cli" ? dialogResumeEl.value.trim() || null : null,
    });
    if (draft?.id !== id) return;
    const said: string[] = [];
    if (!report.character) {
      said.push(
        "キャラクター名に、Windows の起動の行へ載せられない文字があります。" +
          "保存はできますが、起動時はキャラクターを指定せず、作業ディレクトリの既定で立ちます" +
          "（載せられるのは ASCII の英数字と空白と / : . _ - です）。",
      );
    }
    if (!report.options) {
      said.push(
        `起動オプションに、Windows の起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
          "保存はできますが、起動時はこの欄を丸ごと載せずに起動します。",
      );
    }
    if (!report.command) {
      said.push(
        `このアカウントの起動コマンドに、起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
          "このままでは起動できません。",
      );
    }
    if (!report.resume) {
      said.push(
        `再開コマンドに、起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
          "載せずに起動すれば戻る先へ戻らないため、このトピックが持つセッションへは戻れません。",
      );
    }
    dialogNoticeEl.textContent = said.join("\n");
  } catch {
    dialogNoticeEl.textContent = "";
  }
}

/**
 * Redraw both halves of what the form says about the line that would run.
 *
 * One call rather than two at each field, so a field wired to one of them
 * cannot be missing the other — which is the same reason the line itself is
 * composed in one place (`mcp_config::launch_args`).
 */
function refreshDialogLine(): void {
  void refreshDialogPreview();
  void refreshDialogNotice();
}

/**
 * Open the form on one account, or on a new one when given none.
 *
 * A new account's id is minted here so the launch preview has something to name
 * a server after. That is all it is until 決定 — nothing is pushed into the
 * account list, so 取消 leaves no account behind and no id in use.
 */
function openAccountDialog(account: Account | null, field: "name" | "hue" = "name"): void {
  editing = account;
  draft = account
    ? { ...account, args: [...account.args] }
    : {
        // Opaque and minted once. Nothing reads a name out of it — the key in
        // `.mcp.json` derives from it precisely so renaming is free (#53).
        id: crypto.randomUUID(),
        name: unusedAccountName(),
        // The command the kind below names. See src-tauri/src/config.rs.
        command: "claude",
        args: [],
        // A prefill, not a default: the app launches nothing in a directory the
        // person has not seen on screen (#20).
        cwd: homeDir || null,
        hue: null,
        // The one vendor the room is built on, and the kind that knows how to
        // drive it (#156). See src-tauri/src/config.rs.
        kind: "claude_code",
        // Nothing, rather than a guess at a style name: an unnamed character
        // launches on whatever the working directory's own settings say, which
        // is an answer. A guessed name that resolves to no style is not.
        character: null,
        // Nothing, because the kind above holds the way back. This field is
        // the generic kind's, and it is blank there too until someone writes
        // the line the app has none of (#156, 決定6).
        resume_command: null,
        // Nothing added to the environment until someone writes a line (#163).
        env: [],
        // No server until one is written: choosing `mcp` below makes one at
        // 決定, and the entry's name comes back from the app then (#200).
        server: null,
      };

  dialogTitleEl.textContent = account ? "アカウントの編集" : "アカウントの追加";
  dialogNameEl.value = draft.name;
  // `mcp` is offered where an account is being made (#200) and on an `mcp`
  // account's own form, and on no other: an account that exists as another kind
  // does not become a server, and the form of one that is a server cannot
  // change it (#193) — the account answers to an entry in the file, and the
  // kinds either side of it launch.
  const server = draft.kind === "mcp";
  const offered = account === null || server;
  dialogKindMcpEl.hidden = !offered;
  dialogKindMcpEl.disabled = !offered;
  dialogKindEl.disabled = server;
  dialogKindEl.value = draft.kind;
  mcpDrawn = null;
  mcpErrorEl.textContent = "";
  // Emptied for every form, so a new server starts from nothing rather than
  // from the fields of the last account the form was open on.
  mcpCommandEl.value = "";
  mcpArgsEl.value = "";
  mcpEnvEl.value = "";
  dialogHueEl.value = draft.hue === null ? "" : String(draft.hue);
  dialogCwdEl.value = draft.cwd ?? "";
  dialogCharacterEl.value = draft.character ?? "";
  dialogOptionsEl.value = joinArgs(draft.args);
  dialogResumeEl.value = draft.resume_command ?? "";
  void drawDialogEnv(draft);
  dialogDeleteEl.hidden = account === null;
  disarmDelete();
  dialogError("");
  // Cleared before the round trip that refills it, so the account being opened
  // is never read against the last one's notice.
  dialogNoticeEl.textContent = "";
  showDialogKind();
  dialogEl.showModal();
  // On the colour when the form was opened to change it (色を変える, #224);
  // on the name otherwise, selected so typing replaces it.
  if (field === "hue") dialogHueEl.focus();
  else {
    dialogNameEl.focus();
    dialogNameEl.select();
  }
  if (server) {
    // What was last read, at once, and then read again: the log may have moved
    // while the form was closed. The tail is where a log is read from.
    drawDialogMcp();
    mcpLogEl.scrollTop = mcpLogEl.scrollHeight;
    void refreshMcpServers();
  }
}

/**
 * Draw the draft's environment into the form: `NAME=<mask>` per line (#163).
 *
 * The masks are made on the app's side, which is the only side that can open a
 * value; this screen is handed the drawing and nothing else. The field is
 * read-only until it arrives, so nothing typed into it is overwritten by a
 * drawing that lands late, and a drawing for a form that has since been opened
 * on another account is dropped.
 */
async function drawDialogEnv(forDraft: Account): Promise<void> {
  dialogEnvDrawn = null;
  dialogEnvEl.value = "";
  dialogEnvEl.readOnly = true;
  let text: string;
  try {
    text = await invoke<string>("account_env_text", { env: forDraft.env });
  } catch (err) {
    if (draft !== forDraft) return;
    // Left read-only and undrawn: 決定 then keeps the stored variables as they
    // are rather than reading an empty box as their removal.
    dialogError(`環境変数を表示できませんでした: ${err}`);
    return;
  }
  if (draft !== forDraft) return;
  dialogEnvEl.value = text;
  dialogEnvDrawn = text;
  dialogEnvEl.readOnly = false;
}

/**
 * Take what the form holds and put it into the account list.
 *
 * The one moment anything here reaches the list. Returns false when the form
 * cannot be decided yet, so the dialog stays open on its own reason.
 */
async function commitAccountDialog(): Promise<boolean> {
  if (!draft) return false;
  // Read before the await below. Escape closes the dialog on its own, and the
  // close handler clears both — reading them afterwards would push a second
  // copy of an account that was being edited.
  const target = editing;
  const settling = draft;

  const name = dialogNameEl.value.trim();
  if (!name) {
    dialogError("名前を入力してください。部屋での名乗りになります。");
    dialogNameEl.focus();
    return false;
  }

  const kind = dialogKindEl.value as AccountKind;
  // Made here, from nothing: the entry is written and the account with it
  // (#200). Only a form making an account reaches this — an existing account
  // is not offered the kind.
  if (kind === "mcp" && !target) return await createMcpAccount(settling, name);
  // A server's account is a name and a colour over an entry in the file, and the
  // entry is the section below the form's fields (#193). Nothing else here
  // applies to it, and no other kind becomes it or stops being it.
  if (kind === "mcp" || target?.kind === "mcp") {
    if (!target || target.kind !== "mcp" || kind !== "mcp") {
      dialogError("MCP サーバのアカウントの種別は変えられません。");
      return false;
    }
    // An edit to the server not yet saved is saved with the rest, rather than
    // lost to the form closing over it. A field that will not save keeps the
    // form open on its reason.
    if (mcpFieldsEdited() && !(await saveMcpServer())) {
      dialogError("サーバの設定を保存できませんでした。");
      return false;
    }
    const settled: Account = { ...settling, name, hue: declaredHue(dialogHueEl) };
    const at = accounts.findIndex((one) => one.id === target.id);
    if (at >= 0) accounts[at] = settled;
    saveConfig();
    renderPanel();
    status(`アカウント「${settled.name}」を保存しました。`);
    return true;
  }
  // A running account cannot change kind. Its session is in the room under this
  // account, and turning it into a person would drop the working directory and
  // options that session was launched from while it is still running.
  if (target && kind !== target.kind && seatedAnywhere(target.id)) {
    dialogError(`「${target.name}」は起動中です。種別を変えるには先に終了してください。`);
    return false;
  }
  // The person at this screen is a person. Turning their account into one that
  // launches would list them under the wrong heading and offer to start a CLI
  // under their name, which is not a thing there is one of.
  if (target && target.id === localAccountId && kind !== "admin") {
    dialogError("この画面の本人のアカウントは種別 admin のままです。");
    return false;
  }
  const cwd = dialogCwdEl.value.trim();
  const character = dialogCharacterEl.value.trim();
  // Only for a session. A person's working directory, character and options
  // would be values nothing ever reads, kept alive by an edit that once set
  // them. A person is not launched, so nothing selects a style for them.
  const args =
    kind === "admin"
      ? []
      : await invoke<string[]>("parse_launch_options", { text: dialogOptionsEl.value });

  // The character rides in `--settings`, so one written by hand up in the
  // options is the same setting declared twice. Said here because this is the
  // one place both fields are on screen together, and in the language they are
  // read in; the app refuses the launch as well, and that refusal is the
  // authority — this check only gets there first, at the moment it can be
  // fixed rather than at the moment it fails (#99).
  if (character && args.some((arg) => arg.split("=")[0] === "--settings")) {
    dialogError(
      "起動オプションの --settings とキャラクターは同じ設定を指します。どちらか一方にしてください。",
    );
    return false;
  }

  // Sealed on the app's side before anything is stored (#163). Only when the
  // field was drawn and then changed: a field left as drawn is the stored
  // values, and one that was never drawn says nothing about them.
  let env = settling.env;
  if (kind === "admin") {
    env = [];
  } else if (dialogEnvDrawn !== null && dialogEnvEl.value !== dialogEnvDrawn) {
    try {
      env = await invoke<EnvVar[]>("seal_account_env", {
        text: dialogEnvEl.value,
        previous: settling.env,
      });
    } catch (err) {
      dialogError(String(err));
      dialogEnvEl.focus();
      return false;
    }
  }

  const settled: Account = {
    ...settling,
    name,
    kind,
    hue: declaredHue(dialogHueEl),
    cwd: kind === "admin" ? null : cwd || null,
    // Blank clears it, and clearing it is a state: the account goes back to
    // launching on whatever its working directory's own settings name.
    character: kind === "admin" ? null : character || null,
    // Only the kind whose form shows this field keeps it (#156, 決定6). On a
    // kind that holds its own way back, a line stored here would be one nothing
    // reads and nobody can see to correct. Blank is a state on the kind that
    // does keep it, and the common one: an account with no resume line joins a
    // reopened topic as a new session and reads back what it needs through the
    // room's own pull instead (#115, decision 4C).
    resume_command: kind === "cli" ? dialogResumeEl.value.trim() || null : null,
    args,
    env,
  };

  if (target) {
    const at = accounts.findIndex((one) => one.id === target.id);
    if (at >= 0) accounts[at] = settled;
  } else {
    accounts.push(settled);
  }
  saveConfig();

  renderPanel();
  renderSessionFacts();
  // The room holds this screen's person's name and colour on its seat, so a
  // rename here has to be re-declared or the roster keeps the old pair.
  if (settled.id === localAccountId) await join();
  status(
    target
      ? `アカウント「${settled.name}」を保存しました。`
      : `アカウント「${settled.name}」を追加しました。`,
  );
  return true;
}

/**
 * Make an account of kind `mcp` from the form, and the server it is (#200).
 *
 * The entry first, because it is what the account answers to: the app writes it
 * under a name taken from the account's and hands back that name and the id the
 * account is given. Then the account, saved before the server is started, so
 * the server's first post is said under the name and colour chosen here rather
 * than the entry's name the app would fall back to. Then the start, from the
 * file, the way 再起動 starts one.
 *
 * A field the app refuses keeps the form open on its reason, with nothing
 * written. A server that will not start is still made: its account is there,
 * and its window says what happened and holds 起動.
 */
async function createMcpAccount(settling: Account, name: string): Promise<boolean> {
  mcpErrorEl.textContent = "";
  let created: { server: string; account_id: string };
  try {
    created = await invoke<{ server: string; account_id: string }>("create_mcp_server", {
      name,
      command: mcpCommandEl.value,
      args: mcpArgsEl.value,
      env: mcpEnvEl.value,
    });
  } catch (err) {
    mcpErrorEl.textContent = String(err);
    dialogError("サーバを設定ファイルに書けませんでした。");
    return false;
  }
  const settled: Account = {
    ...settling,
    id: created.account_id,
    name,
    kind: "mcp",
    hue: declaredHue(dialogHueEl),
    // An account has one shape, and nothing is launched from these: the server
    // is started from its entry in the file (`mcp_servers::migrate_accounts`).
    command: "",
    args: [],
    cwd: null,
    character: null,
    resume_command: null,
    env: [],
    server: created.server,
  };
  accounts.push(settled);
  await saveConfig();
  let started = true;
  try {
    await invoke("restart_mcp_server", { name: created.server });
  } catch {
    started = false;
  }
  renderPanel();
  await refreshMcpServers();
  if (started) {
    status(`アカウント「${settled.name}」を追加し、サーバ「${created.server}」を起動しました。`);
  } else {
    status(
      `アカウント「${settled.name}」を追加しましたが、サーバ「${created.server}」を起動できませんでした。`,
      "error",
    );
  }
  return true;
}

/**
 * Delete the account the form is open on, on the second click.
 *
 * Two clicks rather than `window.confirm`, for the reason 終了 does not use one
 * either: a host that answers nothing makes the button either silently dead or
 * — the bias `confirm` defaults to — destructive on one click (#57). 終了 asks
 * in a `<dialog>` of the app's own since #71; whether this follows is #72.
 *
 * Refused while it is running: the session in the room belongs to this account,
 * and deleting the account under it would leave a participant on the roster
 * that nothing on this screen can name or account for. Refused for the person
 * at this screen too — they are in the room by being here, and there would be
 * nothing left to be here as.
 *
 * A server's account takes its server with it (#200): the entry comes out of
 * the file and the run is ended, before the account goes. In that order,
 * because an entry still listed would be given an account again on the next
 * read of the config — so a file that cannot be written keeps the account.
 */
async function deleteFromDialog(): Promise<void> {
  const account = editing;
  if (!account) return;

  if (seatedAnywhere(account.id)) {
    dialogError(`「${account.name}」は起動中です。セッションを終了してから削除してください。`);
    disarmDelete();
    return;
  }
  if (account.id === localAccountId) {
    dialogError("この画面の本人のアカウントは削除できません。");
    disarmDelete();
    return;
  }
  const server = account.kind === "mcp" ? account.server : null;
  if (!deleteArmed) {
    deleteArmed = true;
    dialogDeleteEl.textContent = "本当に削除";
    dialogDeleteEl.classList.add("armed");
    dialogError(
      server
        ? `もう一度押すと削除します。設定ファイルからサーバ「${server}」を外し、動いていれば止めます。`
        : "もう一度押すと削除します。",
    );
    return;
  }

  if (server) {
    try {
      await invoke("delete_mcp_server", { name: server });
    } catch (err) {
      dialogError(`サーバを設定ファイルから外せませんでした: ${err}`);
      disarmDelete();
      return;
    }
  }

  accounts = accounts.filter((candidate) => candidate.id !== account.id);
  // Its terminal goes with it. An account that no longer exists cannot be named
  // in the panel, and the row is the only way that pane could be reached.
  for (const view of [...views.values()]) {
    if (view.accountId === account.id) discardView(view);
  }
  saveConfig();
  closeAccountDialog();
  renderPanel();
  renderSessionFacts();
  status(`アカウント「${account.name}」を削除しました。`);
  if (server) void refreshMcpServers();
}

/** Drop the draft and close. Nothing it held reached the account list. */
function closeAccountDialog(): void {
  editing = null;
  draft = null;
  disarmDelete();
  if (dialogEl.open) dialogEl.close();
}

function renderSocket(port: number | null, error?: string): void {
  // The socket's row is folded under 詳細 (#225), and a fold must not hide that
  // the room is not listening: the summary takes the error colour with it.
  factsMoreEl.dataset.kind = error || port === null ? "error" : "";
  if (error) {
    socketStateEl.textContent = error;
    socketStateEl.dataset.kind = "error";
    return;
  }
  if (port === null) {
    socketStateEl.textContent = "未待受";
    socketStateEl.dataset.kind = "error";
    return;
  }
  // The address alone. It is an address, and the panel column is one line
  // wide; that it is being listened on is what the accent colour says.
  socketStateEl.textContent = `127.0.0.1:${port}`;
  socketStateEl.dataset.kind = "ok";
}

/** The page's own colours, so a terminal is not a light rectangle in the dark. */
function terminalTheme(): { background: string; foreground: string } {
  const style = getComputedStyle(document.documentElement);
  return {
    background: style.getPropertyValue("--bg").trim() || "#17171a",
    foreground: style.getPropertyValue("--fg").trim() || "#e8e8ea",
  };
}

// ── the local MCP servers, as accounts (#172 / #193) ───────────────────────
//
// What src-tauri/src/app_mcp.rs hands the screen. The file is read on that side
// and so is the text of the three fields: this screen draws what it is given
// and hands back what was typed, so the reading of `NAME=value` has one
// implementation, and it is the tested one (`crates/mcp-servers`).
//
// Each server is an account of kind `mcp` (#193). Its row in the participant
// list says where its run is, and its window — the account form — holds what
// the settings menu held until then: the server's state, what it is started
// with, its log and the file. The view below is read for both, and read again
// whenever a server moves.

/** Where one run of a server is. `null` when this app run has not started it. */
type McpRunState =
  | { state: "starting" }
  | { state: "running" }
  | { state: "ended"; detail: string }
  | { state: "failed"; detail: string }
  | { state: "stopped" };

interface McpLogLine {
  /** Milliseconds since the epoch, drawn in local time. */
  at_ms: number;
  text: string;
}

interface McpServerView {
  name: string;
  /** False for a server still running under a name the file no longer lists. */
  listed: boolean;
  command: string;
  /** One argument per line. */
  args: string;
  /** One `NAME=value` per line. */
  env: string;
  state: McpRunState | null;
  /** The file holds something other than what the running server was started
   *  from. */
  stale: boolean;
  log: McpLogLine[];
}

interface McpPanelView {
  file: string;
  error: string | null;
  servers: McpServerView[];
}

/** The servers as last read, or null before the first read answered. */
let mcpPanel: McpPanelView | null = null;
/** A read is on its way, and another was asked for while it was. */
let mcpReading = false;
let mcpReadAgain = false;
/** The fields as last drawn from the file, to tell an edit from what is saved.
 *  A refresh redraws state and log under an edit, never the edit itself. */
let mcpDrawn: { name: string; command: string; args: string; env: string } | null = null;

/**
 * The server an `mcp` account answers to, as last read.
 *
 * `view` is undefined when the file lists no entry of that name and this run
 * has not run one under it: the account outlived its entry, and says so.
 */
function mcpServerOf(account: Account): { name: string; view: McpServerView | undefined } {
  const name = account.server ?? "";
  return { name, view: mcpPanel?.servers.find((server) => server.name === name) };
}

/**
 * What an `mcp` account's row says about its server (#193).
 *
 * The words the session rows use where they mean the same thing — 起動中,
 * 終了, 起動失敗, 未起動 — so one list does not say one state two ways. Running
 * says nothing, as a session in the room says nothing (#82). 停止 is a run
 * ended by hand, and 未登録 an account whose entry is gone from the file. Each
 * fits the width 起動失敗 already takes (#71).
 */
function mcpNote(view: McpServerView | undefined): { text: string; kind: string; title: string } {
  if (!view) return { text: "未登録", kind: "", title: "設定ファイルにこのサーバはありません。" };
  const state = view.state;
  if (!state) return { text: view.listed ? "未起動" : "未登録", kind: "", title: "" };
  switch (state.state) {
    case "starting":
      return { text: "起動中", kind: "", title: "" };
    case "running":
      return { text: "", kind: "", title: "" };
    case "ended":
      return { text: "終了", kind: "", title: state.detail };
    case "failed":
      return { text: "起動失敗", kind: "error", title: state.detail };
    case "stopped":
      return { text: "停止", kind: "", title: "" };
  }
}

function mcpStateText(state: McpRunState | null): string {
  if (!state) return "未起動";
  switch (state.state) {
    case "starting":
      return "起動中";
    case "running":
      return "実行中";
    case "ended":
      return `終了（${state.detail}）`;
    case "failed":
      return `失敗（${state.detail}）`;
    case "stopped":
      return "停止";
  }
}

/** `ok` for running, `error` for a run that ended or never started, and nothing
 *  for the states on the way — the same two colours the socket row uses. */
function mcpStateKind(state: McpRunState | null): string {
  if (state?.state === "running") return "ok";
  if (state?.state === "ended" || state?.state === "failed") return "error";
  return "";
}

/** The server the account form is open on, or null when it is on no `mcp`
 *  account. */
function dialogServer(): string | null {
  return draft?.kind === "mcp" ? draft.server : null;
}

function mcpFieldsEdited(): boolean {
  const name = dialogServer();
  if (!mcpDrawn || name === null || mcpDrawn.name !== name) return false;
  return (
    mcpCommandEl.value !== mcpDrawn.command ||
    mcpArgsEl.value !== mcpDrawn.args ||
    mcpEnvEl.value !== mcpDrawn.env
  );
}

function mcpLogText(lines: McpLogLine[]): string {
  return lines
    .map((line) => {
      const time = new Date(line.at_ms).toLocaleTimeString("ja-JP", { hour12: false });
      return `${time}  ${line.text}`;
    })
    .join("\n");
}

/**
 * What the rows read off the servers: the note each would draw. Compared before
 * and after a read, so a log line — which moves no row — does not redraw the
 * participant list under the person using it.
 */
function mcpRowSignature(): string {
  return accounts
    .filter((account) => account.kind === "mcp")
    .map((account) => `${account.id}:${mcpNote(mcpServerOf(account).view).text}`)
    .join("\n");
}

/**
 * Read the servers again: the rows, and the account form when it is open on
 * one of them.
 *
 * Called once at startup, after each act, and on every `mcp-servers-changed`.
 * A read asked for while one is on its way is folded into one more after it, so
 * a burst of log lines is not a burst of reads.
 */
async function refreshMcpServers(): Promise<void> {
  if (mcpReading) {
    mcpReadAgain = true;
    return;
  }
  mcpReading = true;
  try {
    do {
      mcpReadAgain = false;
      const before = mcpRowSignature();
      try {
        mcpPanel = await invoke<McpPanelView>("mcp_servers");
      } catch (err) {
        mcpFileErrorEl.textContent = String(err);
        continue;
      }
      if (mcpRowSignature() !== before) renderPanel();
      drawDialogMcp();
    } while (mcpReadAgain);
  } finally {
    mcpReading = false;
  }
}

/**
 * Draw the account form's server section from the last read (#193).
 *
 * The fields are redrawn only when they hold what was last drawn into them: a
 * log line arriving while someone types is not a reason to take what they typed
 * away.
 */
function drawDialogMcp(): void {
  const name = dialogServer();
  if (name === null || !dialogEl.open) return;
  // Put back what a form making a server took away (`drawNewMcp`).
  mcpSaveEl.hidden = false;
  mcpLogHeadEl.hidden = false;
  mcpLogEl.hidden = false;
  const panel = mcpPanel;
  mcpFileEl.textContent = panel?.file ?? "";
  mcpFileErrorEl.textContent = panel?.error ?? "";
  const server = panel?.servers.find((each) => each.name === name);

  mcpStateEl.textContent = server ? mcpStateText(server.state) : "未登録";
  mcpStateEl.dataset.kind = server ? mcpStateKind(server.state) : "";
  // One button, named for what it will do: start what has not run, start again
  // what has, and stop what the file no longer lists. Nothing to do for an entry
  // that is neither listed nor running.
  mcpRestartEl.hidden = !server;
  if (server) {
    mcpRestartEl.textContent = !server.listed ? "停止" : server.state ? "再起動" : "起動";
  }
  mcpStaleEl.textContent = !server
    ? `設定ファイルにサーバ「${name}」はありません。このアカウントは、そのサーバが部屋で言ったことの話し手として残っています。`
    : !server.listed
      ? "設定ファイルにこのサーバはありません。停止しても、一覧から消えるのはアプリを起動し直したときです。"
      : server.stale
        ? "保存した設定は、再起動するまで反映されません。"
        : "";

  mcpFieldsEl.hidden = !server?.listed;
  if (server?.listed && !mcpFieldsEdited()) {
    mcpCommandEl.value = server.command;
    mcpArgsEl.value = server.args;
    mcpEnvEl.value = server.env;
    mcpDrawn = { name: server.name, command: server.command, args: server.args, env: server.env };
  }

  // Follow the tail while it is being followed: a log scrolled back up to read
  // is left where it was put.
  const following = mcpLogEl.scrollTop + mcpLogEl.clientHeight >= mcpLogEl.scrollHeight - 4;
  mcpLogEl.textContent = server?.log.length ? mcpLogText(server.log) : "（まだ何も出ていません）";
  if (following) mcpLogEl.scrollTop = mcpLogEl.scrollHeight;
}

/**
 * Draw the server section for a form making an account of kind `mcp` (#200).
 *
 * The three fields, empty, and nothing that answers for a server: there is none
 * yet to have a state, a log, a 保存 or a 再起動 of its own. 決定 writes it and
 * starts it (`createMcpAccount`). The file it will be written to is named, as
 * the section of an existing server names it.
 */
function drawNewMcp(): void {
  mcpStateEl.textContent = "決定で設定ファイルに書き、起動します";
  mcpStateEl.dataset.kind = "";
  mcpRestartEl.hidden = true;
  mcpStaleEl.textContent = "";
  mcpFieldsEl.hidden = false;
  mcpSaveEl.hidden = true;
  mcpLogHeadEl.hidden = true;
  mcpLogEl.hidden = true;
  mcpFileEl.textContent = mcpPanel?.file ?? "";
  mcpFileErrorEl.textContent = mcpPanel?.error ?? "";
}

/** Write the open server's fields into the file. The running server is left as
 *  it is; the section then says the two differ until 再起動. */
async function saveMcpServer(): Promise<boolean> {
  const name = dialogServer();
  if (name === null) return false;
  mcpErrorEl.textContent = "";
  try {
    await invoke("save_mcp_server", {
      name,
      command: mcpCommandEl.value,
      args: mcpArgsEl.value,
      env: mcpEnvEl.value,
    });
  } catch (err) {
    mcpErrorEl.textContent = String(err);
    return false;
  }
  // Drawn again from the file, so what the fields hold is what was stored —
  // blank lines dropped, quotes taken off — rather than what was typed.
  mcpDrawn = null;
  await refreshMcpServers();
  return true;
}

/** Start the open server again from what the file holds. A field typed into
 *  and not saved is not what starts, so an unsaved edit is said instead. */
async function restartMcpServer(): Promise<void> {
  const name = dialogServer();
  if (name === null) return;
  if (mcpFieldsEdited()) {
    mcpErrorEl.textContent = "保存していない変更があります。先に保存してください。";
    return;
  }
  mcpErrorEl.textContent = "";
  try {
    await invoke("restart_mcp_server", { name });
  } catch (err) {
    mcpErrorEl.textContent = String(err);
    return;
  }
  await refreshMcpServers();
}

async function openMcpServersFile(): Promise<void> {
  try {
    await invoke("open_mcp_servers_file");
  } catch (err) {
    mcpFileErrorEl.textContent = String(err);
  }
}

async function main(): Promise<void> {
  // Before anything else is drawn, so no button is ever on screen empty.
  fillIcons();

  // The pane is one container holding every session's terminal, so the observer
  // is on the container and the fit lands on whichever one is showing.
  new ResizeObserver(() => fitShown()).observe(terminalEl);
  renderSessionFacts();

  // ── closing the app ────────────────────────────────────────────────────────
  //
  // First, and the dialog's own wiring with it. The window can be closed at any
  // moment this screen is up, and two things depend on being ready before that:
  // a close arriving before the listener is registered is one Tauri does not
  // hold open — it takes the window and leaves every session under it running —
  // and a dialog opened before its buttons are wired is a question that cannot
  // be answered while the close waits on it. A reload lands here with sessions
  // already running (`adoptSeats`), so this is not only a first-launch window.
  quitCancelEl.addEventListener("click", () => answerQuit(false));
  quitCommitEl.addEventListener("click", () => answerQuit(true));
  // Escape closes the dialog itself, and it means 取消. Answering from the close
  // is what makes that true on every path out, this time load-bearing rather
  // than tidy: a question left standing would hold the window's close open with
  // nothing left able to answer it.
  quitDialogEl.addEventListener("close", () => answerQuit(false));
  await getCurrentWindow().onCloseRequested(onQuitRequested);

  // The account's colour is the account's, so nothing is restored into this
  // picker — the form fills it from whichever account it was opened on.
  fillHues(dialogHueEl, null);

  // Restored before anything is drawn, so the first line to arrive is already
  // at the size this screen reads at rather than jumping once it lands.
  //
  // The UI's multiple goes first: the conversation's size is written against it.
  fillUiScales();
  applyUiScale(storedUiScale(), false);
  settingsUiScaleEl.addEventListener("change", () => {
    applyUiScale(Number(settingsUiScaleEl.value), true);
  });
  fillRoomFontSizes(settingsRoomFontSizeEl);
  applyRoomFontSize(storedRoomFontSize(), false);
  settingsRoomFontSizeEl.addEventListener("change", () => {
    applyRoomFontSize(Number(settingsRoomFontSizeEl.value), true);
  });
  // On the window rather than on the room: the keys are meant to work while
  // something is being typed, and the room is not what holds focus then.
  window.addEventListener("keydown", (event) => {
    if (!event.ctrlKey || event.altKey || event.isComposing) return;
    const step = ROOM_FONT_SIZE_KEYS[event.key];
    if (step === undefined) return;
    // Load-bearing, not tidiness: the webview answers these same keys with its
    // own zoom, which takes the whole screen — the terminal, the panel, and the
    // composer's own controls along with its textarea. Scaling those is the one
    // thing this control may not do, so the default has to be stopped for the
    // scoped version to be what happens.
    event.preventDefault();
    stepRoomFontSize(step);
  });

  // Restored before any terminal is opened, so the first session is laid out at
  // the size this screen reads at rather than being re-fitted once it lands.
  fillTerminalFontSizes(settingsTerminalFontSizeEl);
  applyTerminalFontSize(storedTerminalFontSize(), false);
  settingsTerminalFontSizeEl.addEventListener("change", () => {
    applyTerminalFontSize(Number(settingsTerminalFontSizeEl.value), true);
  });

  // The two ends of one act. 端末 is reachable while the pane is folded and ✕
  // while it is open, which is the whole of why both exist (#68).
  toggleEl.addEventListener("click", () => {
    if (diagnosticsEl.hidden) revealDiagnostics();
    else hideDiagnostics();
  });
  diagnosticsCloseEl.addEventListener("click", () => hideDiagnostics());

  // ── the two panels ─────────────────────────────────────────────────────────
  //
  // Each button folds the panel it stands over. Both stay on the bar in either
  // state: a folded panel leaves nothing behind to press (see `#history[hidden]`
  // in src/styles.css), so the way back has to be somewhere that is always
  // there.
  toggleHistoryEl.addEventListener("click", () => togglePanel("history"));
  toggleParticipantsEl.addEventListener("click", () => togglePanel("participants"));
  // `Ctrl+B`, the left panel only. Claude Desktop — which is where Master took
  // the shape of these buttons from — puts the key on that side, and inventing
  // a second combination for the panel opposite would be adding a key before
  // anyone has reached for one (#118, AI 判断6).
  //
  // On the window, like the text size keys above, and stopped before the
  // window's own handling for the same reason those are. What is different here
  // is the terminal. `attachCustomKeyEventHandler` returns true for everything
  // but `Ctrl+V`, so a session receives every other key it is sent — and this
  // event still reaches the window afterwards, which would fold the panel and
  // send `Ctrl+B` to the shell in one press. `Ctrl+B` means something there
  // (tmux's prefix, readline's backward-char), and a session's pane is the
  // session's (#84), so while the terminal holds focus this key is not the
  // app's and the app does not take it (#118, AI 判断5). The panels are still
  // reachable from the two buttons, which is why giving the key up costs
  // nothing.
  window.addEventListener("keydown", (event) => {
    if (!event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
    if (event.isComposing) return;
    if (event.key !== "b" && event.key !== "B") return;
    if (terminalEl.contains(document.activeElement)) return;
    event.preventDefault();
    togglePanel("history");
  });

  await listen<RoomMessage>("room-message", (event) => {
    // Drawn only into the topic it was said in. The others' posts are in their
    // own logs, and opening one of them reads them from there (#141).
    if (event.payload.topic_id === shownTopicId()) appendMessage(event.payload);
    // The same post read twice: once as a line in the conversation, once for
    // who the room is now waiting on. The second reading is what puts 考え中…
    // on a row without anything having to read the CLI's output (#82).
    trackAddress(event.payload);
  });
  await listen<Roster>("room-participants", (event) =>
    renderRoster(event.payload.topic_id, event.payload.participants),
  );
  // A session reported what it is running on, through its own status line
  // (#155). Keyed on the seat, so it reaches the terminal of the topic that
  // session is in and not whichever topic is on the glass — the same way a post
  // does (#141). A seat this screen has no terminal for is dropped: nothing is
  // drawn from it, and nothing is kept for a terminal that may be made later.
  //
  // The roster is redrawn only when 制限中 turns over, which is the same rule
  // the output words are drawn under: the word changes, or nothing is drawn
  // (#82). A report arrives on every assistant message, and the two that cross
  // the threshold are the only ones the row has anything to say about — the
  // rest move the facts column alone.
  await listen<SessionStats>("session-stats", (event) => {
    const view = views.get(seatKey(event.payload.topic_id, event.payload.account_id));
    if (!view) return;
    const was = limitedByUsage(view.stats);
    view.stats = event.payload;
    if (view === shownView()) renderSessionStats();
    if (limitedByUsage(view.stats) !== was) renderPanel();
  });
  // The index changed underneath: a topic realised by its own first post, or a
  // session id recorded by a launch. Both happen without the screen asking, and
  // the first is how a topic gets the name the list shows it under (#115).
  await listen<Topic[]>("room-topics", (event) => {
    topics = event.payload;
    renderTopics();
    renderSessionId();
  });
  // The room went on without the log. Saying so is the whole of what this does
  // — a log that had quietly stopped recording would still look like a log, and
  // the next person to go looking would read the gap as nothing having been
  // said (#48).
  await listen<string>("room-log-error", (event) => {
    status(`記録に失敗しました: ${event.payload}`, "error");
  });
  // The socket binds after the frontend loads, so the event is the authority
  // and the poll below is only for a listener that attached too late.
  await listen<number>("room-ready", (event) => renderSocket(event.payload));

  // ── settings (#172) ─────────────────────────────────────────────────────────
  //
  // The display settings (#194). Its pickers are wired with the two size axes
  // and the UI scale above; each takes effect as it is changed, so the dialog
  // has nothing to commit and only closes.
  openSettingsEl.addEventListener("click", () => {
    if (!settingsDialogEl.open) settingsDialogEl.showModal();
  });
  settingsCloseEl.addEventListener("click", () => settingsDialogEl.close());

  // ── the local MCP servers (#172 / #193) ─────────────────────────────────────
  //
  // Read again whenever a server moves — a state, a log line — whether or not a
  // window is open: each server's row says where its run is.
  mcpOpenFileEl.addEventListener("click", () => void openMcpServersFile());
  mcpSaveEl.addEventListener("click", () => void saveMcpServer());
  mcpRestartEl.addEventListener("click", () => void restartMcpServer());
  await listen<string>("mcp-servers-changed", () => void refreshMcpServers());

  // The one thing that draws a topic boundary, and the head of the list it
  // appears in (#125, 決定1). `renderTopics` is what draws it as picked; this is
  // only the press.
  topicNewEl.addEventListener("click", () => void startNewTopic());

  sendEl.addEventListener("click", () => void send());
  // mousedown is stopped so the textarea keeps its focus and its caret through
  // the press; the click is what types.
  mentionEl.addEventListener("mousedown", (event) => event.preventDefault());
  mentionEl.addEventListener("click", () => typeMention());
  inputEl.addEventListener("keydown", (event) => {
    // The list's keys first: Enter on an open list picks, it does not send.
    if (mentionKey(event)) return;
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void send();
    }
  });
  // `@` opens the list of who can be addressed (#204). It follows the caret,
  // so a click or an arrow key that moves it off the `@` shuts it.
  inputEl.addEventListener("input", () => refreshMentions());
  inputEl.addEventListener("click", () => refreshMentions());
  inputEl.addEventListener("keyup", (event) => {
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
      refreshMentions();
    }
  });
  inputEl.addEventListener("blur", () => closeMentions());

  // ── attachments (#223) ──────────────────────────────────────────────────────
  //
  // 添付 opens the system's file dialog through the hidden picker. The picker
  // is emptied after each choice, so picking the same file again is a change.
  attachEl.addEventListener("mousedown", (event) => event.preventDefault());
  attachEl.addEventListener("click", () => attachInputEl.click());
  attachInputEl.addEventListener("change", () => {
    if (attachInputEl.files) attachFiles(attachInputEl.files);
    attachInputEl.value = "";
    inputEl.focus();
  });
  // Ctrl+V of an image. Only when the clipboard holds no plain text: what
  // Excel or a browser copies carries a picture of the selection beside its
  // text, and a paste of those is a paste of the text.
  inputEl.addEventListener("paste", (event) => {
    const data = event.clipboardData;
    if (!data || data.files.length === 0 || data.types.includes("text/plain")) return;
    event.preventDefault();
    attachFiles(data.files);
  });
  // A drop onto the composer. The webview does not hand a dropped file to the
  // page — Tauri takes the drop and reports the paths and where it landed
  // (`dragDropEnabled`, on by default) — so the composer is found by the
  // point, not by an element's drop event. A drop anywhere else attaches
  // nothing.
  await getCurrentWebview().onDragDropEvent((event) => {
    const drag = event.payload;
    if (drag.type === "leave") {
      markDrop(false);
      return;
    }
    const over = overComposer(drag.position);
    if (drag.type === "drop") {
      markDrop(false);
      if (over) {
        attachPaths(drag.paths);
        inputEl.focus();
      }
      return;
    }
    markDrop(over);
  });

  try {
    homeDir = await invoke<string>("home_dir");
  } catch {
    // Only the prefill is lost; the field is still typed into by hand.
  }

  // ── the account form ───────────────────────────────────────────────────────
  //
  // Nothing here writes to an account. Every field edits the form's own draft,
  // and only 決定 puts that draft into the list.
  accountNewEl.addEventListener("click", () => openAccountDialog(null));
  // An account row's menu (#224). It closes the way a menu does: on a press
  // anywhere outside it, and when what is under it moves — a menu left
  // standing at a point the row has scrolled away from reads as belonging to
  // whichever row is beside it now. The press on the row's own menu button is left to
  // that button, which closes what it opened.
  accountMenuEl.addEventListener("keydown", onAccountMenuKey);
  document.addEventListener(
    "pointerdown",
    (event) => {
      if (menuAccountId === null || !(event.target instanceof Node)) return;
      if (accountMenuEl.contains(event.target)) return;
      const opener = `button.more[data-account="${CSS.escape(menuAccountId)}"]`;
      if (event.target instanceof Element && event.target.closest(opener)) return;
      closeAccountMenu(false);
    },
    true,
  );
  // The panel's own scroll only. The room and the terminals scroll as text
  // arrives, and a menu that closed on every line a session printed could not
  // be used while one was printing.
  participantsEl.addEventListener("scroll", () => closeAccountMenu(false), true);
  window.addEventListener("resize", () => closeAccountMenu(false));
  window.addEventListener("blur", () => closeAccountMenu(false));
  sessionIdCopyEl.addEventListener("click", () => void copySessionId());
  dialogKindEl.addEventListener("change", () => showDialogKind());
  dialogOptionsEl.addEventListener("input", () => refreshDialogLine());
  // The character ends up in the line that runs, so it redraws the preview for
  // the same reason the options do: the line shown has to be the line spawned.
  dialogCharacterEl.addEventListener("input", () => refreshDialogLine());
  // So does the working directory: which registrations the line stops is read
  // out of the directory it is pointed at (#103).
  dialogCwdEl.addEventListener("input", () => refreshDialogLine());
  // The resume line is not in the preview — the preview answers for a fresh
  // launch — but it is a line that runs, and what it cannot carry is a topic
  // this account cannot go back into (#154, 決定4).
  dialogResumeEl.addEventListener("input", () => refreshDialogLine());
  // Anything but the second click of 削除 disarms it: an arm left standing is
  // one that an unrelated click fires later.
  for (const field of [
    dialogNameEl,
    dialogKindEl,
    dialogHueEl,
    dialogCwdEl,
    dialogCharacterEl,
    dialogOptionsEl,
    dialogResumeEl,
    dialogEnvEl,
  ]) {
    field.addEventListener("input", () => disarmDelete());
  }
  dialogDeleteEl.addEventListener("click", () => void deleteFromDialog());
  dialogCancelEl.addEventListener("click", () => closeAccountDialog());
  // Escape closes the dialog itself, and it means 取消: the draft is dropped by
  // the close handler below, so there is no path out of this form that leaves
  // half of it applied.
  dialogEl.addEventListener("close", () => {
    editing = null;
    draft = null;
    mcpDrawn = null;
    disarmDelete();
  });
  dialogFormEl.addEventListener("submit", (event) => {
    // Always prevented: `method="dialog"` would close on submit, and the form
    // may not be decidable yet. The commit closes it once it has succeeded.
    event.preventDefault();
    void commitAccountDialog().then((done) => {
      if (done) closeAccountDialog();
    });
  });

  endCancelEl.addEventListener("click", () => closeEndDialog());
  endCommitEl.addEventListener("click", () => confirmEndDialog());
  // Escape closes the dialog itself, and it means 取消. Clearing the account
  // here as well is what makes that true on every path out: a dialog closed by
  // anything but 終了 leaves nothing standing that a later click could fire.
  endDialogEl.addEventListener("close", () => {
    endingAccount = null;
  });

  topicDeleteCancelEl.addEventListener("click", () => closeTopicDeleteDialog());
  topicDeleteCommitEl.addEventListener("click", () => confirmTopicDeleteDialog());
  // Escape closes the dialog itself, and it means 取消 — the same discipline the
  // dialog above keeps, and for the same reason: no path out of it leaves a
  // topic standing that a later click could delete.
  topicDeleteDialogEl.addEventListener("close", () => {
    deletingTopic = null;
  });

  try {
    const config = await invoke<AppConfig>("load_config");
    accounts = config.accounts;
    // Before the panel is drawn below. A panel folded when the app was last
    // closed is folded again here, and the config is the only place that
    // knows it (#118, decision 1).
    panels = config.panels;
    applyPanels();
    // Before the join below: the id this resolves goes into it, and an account
    // may have to be made here for it (#59).
    resolveLocalAccount();
    // Drawn here, off the config alone. An account that is not running is
    // listed from the moment the app opens rather than once the room has
    // answered — being listed is not conditional on ever having been started
    // (#59).
    renderPanel();
  } catch (err) {
    status(`設定を読み込めませんでした: ${err}`, "error");
  }
  // Where each server's run is, for its row (#193). After the accounts, which
  // are what the rows are; the rows are drawn before this answers and say 未起動
  // until it does.
  void refreshMcpServers();

  // The topics, read once. Outside the room's try below and independent of it:
  // the index is a file this app wrote, so a room that never answers is no
  // reason for the list to stay blank — what was said last week is readable
  // whether or not anything is listening now (#48).
  //
  // The room itself is left empty. Starting the app opens a new topic and does
  // not continue the last one (#115, Master 判断5), so there is nothing to draw
  // into it — which is the behaviour #48 had for a different reason and this
  // keeps for its own: the past is opened by being picked.
  try {
    topics = await invoke<Topic[]>("room_topics");
    currentTopic = await invoke<TopicRef>("room_current_topic");
    renderTopics();
    renderSessionId();
  } catch (err) {
    topicsFailed(String(err));
  }
  // The open topic's conversation, which after a reload is one in progress: the
  // sessions in it are picked up again below, and a room drawn empty over them
  // would be a conversation the screen had forgotten while its speakers had
  // not (#141). A topic nothing was said in reads back as nothing. Apart from
  // the list's try, because a log that fails to read is not a list that did.
  if (currentTopic) {
    try {
      drawTopic(await invoke<LoggedPost[]>("room_topic_log", { topicId: currentTopic.topic_id }));
    } catch (err) {
      status(`トピックの発言を読めませんでした: ${err}`, "error");
    }
  }

  // Before the room, and outside its try. A session running under a seat this
  // screen has forgotten is reachable again from the seats alone (`adoptSeats`),
  // and a room that fails to answer is no reason to leave it unreachable — the
  // failure this is here for is the screen having been reloaded, which the room
  // knows nothing about (#84). It swallows its own errors, so it needs none.
  await refreshSeats();

  try {
    await join();
    // Through the same door the event uses. Read directly into `participants`,
    // this first roster would be the one arrival the readings in `renderRoster`
    // never see — and a session adopted just above (`refreshSeats`) is exactly
    // what is on it (#127).
    renderRoster(
      shownTopicId(),
      await invoke<Participant[]>("room_participants", { topicId: shownTopicId() }),
    );
    const port = await invoke<number | null>("room_port");
    if (port !== null) renderSocket(port);
    renderPanel();
  } catch (err) {
    renderPanel();
    renderSocket(null, `取得できませんでした: ${err}`);
  }
}

void main();
