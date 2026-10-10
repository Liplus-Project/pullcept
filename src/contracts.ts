import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import type { UnlistenFn } from "@tauri-apps/api/event";

export interface RoomMessage {
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
  /** True when the app itself said it, one of its own notices (#294). Stamped
   *  by the room from where the post came in, not read off the name: what the
   *  circle draws the app's icon by (#340). */
  from_app: boolean;
  content: string;
  /** The names it was addressed to, or empty when it was said to the room
   *  (#204). */
  to: string[];
  ts: string;
  /** True when this screen's own participant posted it. Self/other, not
   *  human/AI: the room no longer carries that axis. */
  own: boolean;
}

export interface PostOutcome {
  delivered: boolean;
  /** The id the post is filed under, or null when it was refused. */
  message_id: string | null;
  /** What this screen had not seen, oldest first. Empty when delivered. */
  missed: MissedPost[];
}

export interface MissedPost {
  message_id: string;
  speaker: string;
  /** The hue it was said in, or null when the speaker declared none. Carried
   *  so the drawn line is the line it would have been. */
  hue: number | null;
  /** The account it was said as, for the same reason (#193). */
  account: string | null;
  /** True when the app itself said it, for the same reason again (#340).
   *  Absent on every other post. */
  from_app?: boolean;
  content: string;
  /** Empty when it was said to the room (#204). */
  to: string[];
  ts: string;
}

export interface LoggedPost {
  message_id: string;
  speaker: string;
  /** The account it was said as. Absent when the speaker declared none, and on
   *  every line written before the log carried it (#193). What lets a line read
   *  back fold the way it did live (`foldOf`). */
  account?: string;
  /** True when the app itself said it (#340). Absent on every other line and
   *  on every line written before the log carried it. What lets a line read
   *  back draw the app's icon the way it did live. */
  from_app?: boolean;
  content: string;
  /** The names it was addressed to. Absent, not null or empty, when it was
   *  said to the room: the field's presence is what carries the two states, in
   *  the file and on the way here alike. A line written while a post had one
   *  addressee is read back by the app as a list of that one (#204). */
  to?: string[];
  ts: string;
}

export interface Topic {
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

export interface TopicRef {
  topic_id: string;
  created_at: string;
}

export interface Participant {
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

export interface Roster {
  topic_id: string;
  participants: Participant[];
}

export interface SessionStats {
  topic_id: string;
  account_id: string;
  model: string | null;
  effort: string | null;
  five_hour: number | null;
  seven_day: number | null;
  context: number | null;
  /**
   * Backend stop/recovery: Codex uses its app-server (#294); Claude uses
   * current parent rejection and normal response (#342). Null is unknown.
   */
  limited: boolean | null;
  limited_source?: "claude-parent" | "codex" | null;
  pty_id?: string | null;
  /**
   * When the 5-hour and weekly windows reset, as Unix seconds (#306), read off
   * the same window as the percentage. Shown under those two rows as the time
   * left (`resetIn`); null when the CLI did not say.
   */
  five_hour_resets_at: number | null;
  seven_day_resets_at: number | null;
}

export interface SeatActivity {
  topic_id: string;
  account_id: string;
  /** The launch's terminal, so a run that ended cannot speak for the next. */
  pty_id: string;
  /**
   * False once the app can no longer hear the seat's server. Not the same as
   * no word: an idle seat falls back to the screen's own words, 待機 among
   * them, and a seat that cannot be heard says 様子不明 instead.
   */
  connected: boolean;
  /**
   * What the seat's thread is doing, as far as its server has said (#329):
   * "idle" or "active", null while nothing has said either and after the
   * connection ends. The start or resume answer's status is told as it is,
   * before any turn (#368). Not the same as no word: a turn reasoning or
   * writing its answer is active with no word. Only "idle" on a connected seat says 待機
   * over a terminal that keeps repainting.
   */
  thread_status: "idle" | "active" | null;
  /** The row's badge: 許可待ち, 答え待ち, 実行中, 編集中, ツール, 委任中… */
  word: string | null;
  /** The longer form, for the line under the room and the badge's title. */
  line: string | null;
  /** A wait on the person (許可待ち / 答え待ち) rather than work under way. */
  waiting: boolean;
}

export type AccountKind = "admin" | "claude_code" | "codex_cli" | "cli" | "mcp";

export interface Account {
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
  /** Claude's output style name or Codex's effective developer-instruction
   *  character heading name (#276). Null delegates to the CLI's defaults. An attribute of the account
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
  /** Whether this account carries an image, drawn in its circle in place of
   *  the initial (#236). The flag only: the image is a file the app keeps
   *  (`account_avatar`), read once into `avatarImages`. */
  avatar: boolean;
  /** A Codex CLI account's seat runs through its own app-server and gets its
   *  character as `developerInstructions` (#299). Absent or false: the Li+
   *  output style hook, which stays the default. */
  codex_app_server?: boolean;
}

export interface EnvVar {
  name: string;
  sealed: string;
}

export interface PanelState {
  history: boolean;
  participants: boolean;
}

export interface AppConfig {
  accounts: Account[];
  panels: PanelState;
}

export interface StartedSession {
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

export interface LaunchFieldReport {
  character: boolean;
  options: boolean;
  command: boolean;
  resume: boolean;
}

export interface RunningSession {
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

export interface SeatedAccount {
  account_id: string;
  /** The topic the seat is in. On the seat as well as on the session, because a
   *  seat whose launch is in flight has no session yet and is still in one topic
   *  and not another (#141). */
  topic_id: string;
  /** Null while its launch is in flight: claimed seat, nothing spawned yet. */
  session: RunningSession | null;
}

export type IconName =
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
  | "stop"
  | "edit"
  | "more"
  | "chevron-down"
  | "minimize"
  | "maximize"
  | "restore";

export type IconShape = [tag: string, attrs: Record<string, string>];

export interface SessionView {
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
  /**
   * What the seat's app-server last said — what it is doing and whether it
   * can still be heard — or null when this seat has no app-server or has not
   * reported yet (#326). Like `stats`, not replayed
   * after a reload: what this screen did not see, it does not say.
   */
  activity: SeatActivity | null;
  /** The pending fall back to silence, or undefined when none is armed. */
  quiet: number | undefined;
}

export interface Fold {
  key: string;
  /** The account the fold is, or null for the legacy name — whose image heads
   *  the fold when it carries one (#236). */
  account: string | null;
  label: string;
  colour: string;
}

export interface PermissionCard {
  id: string;
  topic_id: string;
  account_id: string;
  pty_id: string;
  at: string;
  tool_name: string;
  /** The MCP server, for a tool of one. */
  server: string | null;
  tool: string;
  /** The subagent's type, when a subagent asked. */
  agent_type: string | null;
  fields: { name: string; value: string; cut: boolean; }[];
  more_fields: number;
  /** The rules 常に許可 would add, one line each. */
  always_rules: string[];
  can_always: boolean;
  hold_secs: number;
}

export interface PermissionResolved {
  id: string;
  topic_id: string;
  account_id: string;
  outcome: "deny" | "allow" | "always" | "elsewhere" | "closed" | "timeout";
}

export interface Member {
  account: Account | null;
  participant: Participant | null;
}

export interface RowWord {
  word: string;
  line: string;
  kind: "" | "active" | "waiting";
}

export type Attachment = {
  name: string;
  /** Bytes the webview holds (the button, a paste), or a path a drop named. */
  source: { file: File; } | { path: string; };
  /** Where it was saved, and for which topic. Kept so a post the floor
   *  refused, sent again, does not save the same file a second time. */
  saved: { topicId: string; path: string; } | null;
  /** The picture its chip shows, as an object URL over the bytes the webview
   *  already holds (#318). Only for an image picked or pasted: a dropped file
   *  is a path outside the attachments folder, and the screen does not read
   *  one of those. Revoked when the chip goes. */
  preview: string | null;
};

export type TextPiece = { kind: "text" | "url" | "path"; text: string; };

export interface PtyExit {
  code: number | null;
  requested: boolean;
}

export type AvatarEdit = { kind: "keep"; } | { kind: "set"; png: Blob; url: string; } | { kind: "clear"; };

export type DialogSection = "basic" | "character" | "launch" | "env" | "server";

export interface CharacterOpened {
  path: string;
  exists: boolean;
  folder_exists: boolean;
  body: string;
  crlf: boolean;
  bom: boolean;
  name_in_file: string | null;
  stamp: string | null;
}

export interface CharacterPlace {
  kind: "claude_code" | "codex_cli";
  cwd: string;
  name: string;
}

export type McpRunState =
  | { state: "starting"; }
  | { state: "running"; }
  | { state: "ended"; detail: string; }
  | { state: "failed"; detail: string; }
  | { state: "stopped"; };

export interface McpLogLine {
  /** Milliseconds since the epoch, drawn in local time. */
  at_ms: number;
  text: string;
}

export interface McpServerView {
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

export interface McpPanelView {
  file: string;
  error: string | null;
  servers: McpServerView[];
}
