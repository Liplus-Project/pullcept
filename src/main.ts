// The room surface.
//
// Everyone in the room is a participant, and a post is one act whoever made it
// (#39). Messages arrive on one event (`room-message`) either way, so the room
// has a single ordering authority; what is typed here is not appended locally
// on send but comes back through that same event. See src-tauri/src/room.rs.
//
// The diagnostics pane carries a real terminal for the launched CLI. That is a
// display, not a message source: the room's lines come from the channel and
// from `say_to_room`, and nothing in this file reads terminal output as
// speech. The rejected design is the one where the app parses CLI output to
// find messages (docs/0-requirements.md); showing the CLI is not that.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow, type CloseRequestedEvent } from "@tauri-apps/api/window";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

interface RoomMessage {
  message_id: string;
  speaker: string;
  /** The hue the speaker declared, or null when they declared none. Stamped by
   *  the room from the connection the post arrived on, so it is that speaker's
   *  and not whoever else currently answers to the same name. */
  hue: number | null;
  content: string;
  to: string | null;
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
  missed: {
    message_id: string;
    speaker: string;
    content: string;
    to: string | null;
    ts: string;
  }[];
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
 * What kind of participant an account is, declared when it is made.
 *
 * Never inferred from the connection: the room sees only what kind of
 * connection someone arrived on, and a person joining from another client
 * arrives the same way a session does. The account form is the one moment
 * anyone can say which this is (#59).
 */
type AccountKind = "user" | "ai";

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
}

interface AppConfig {
  accounts: Account[];
}

interface StartedSession {
  pty_id: string;
  mcp_config: string;
  /** When the session was launched, stamped by the room's own clock. */
  started_at: string;
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
}

/** One account holding a seat, and what it is running. */
interface SeatedAccount {
  account_id: string;
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

const roomEl = document.getElementById("room") as HTMLElement;
const rosterEl = document.getElementById("roster") as HTMLElement;
const accountNewEl = document.getElementById("account-new") as HTMLButtonElement;
const inputEl = document.getElementById("input") as HTMLTextAreaElement;
const sendEl = document.getElementById("send") as HTMLButtonElement;
const toEl = document.getElementById("to-select") as HTMLSelectElement;
const statusEl = document.getElementById("status") as HTMLElement;
const diagnosticsEl = document.getElementById("diagnostics") as HTMLElement;
const toggleEl = document.getElementById("toggle-diagnostics") as HTMLButtonElement;
const fontSizeEl = document.getElementById("room-font-size") as HTMLSelectElement;
const socketStateEl = document.getElementById("socket-state") as HTMLElement;
const sessionStateEl = document.getElementById("session-state") as HTMLElement;
const transportEl = document.getElementById("session-transport") as HTMLElement;
const commandEl = document.getElementById("session-command") as HTMLElement;
const dirEl = document.getElementById("session-dir") as HTMLElement;
const startedEl = document.getElementById("session-started") as HTMLElement;
const windowEl = document.getElementById("session-window") as HTMLElement;
const terminalEl = document.getElementById("terminal") as HTMLElement;
const tabsEl = document.getElementById("terminal-tabs") as HTMLElement;
const terminalFontSizeEl = document.getElementById("terminal-font-size") as HTMLSelectElement;
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
const dialogPreviewEl = document.getElementById("dialog-preview") as HTMLElement;
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

let accounts: Account[] = [];
/**
 * The accounts holding a seat in the room, by id.
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
 */
let seated = new Map<string, SeatedAccount>();
/** Everyone in the room, this screen's person included. */
let participants: Participant[] = [];
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
let lastSeenId: string | null = null;
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
  /** How the session ended, or null while it is still running. */
  ended: string | null;
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
  /** The pending fall back to silence, or undefined when none is armed. */
  quiet: number | undefined;
}

/**
 * How long a terminal must stay silent before its row stops saying anything
 * about it, in milliseconds.
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

/** The terminals this screen holds, by account id, in launch order. */
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
const awaiting = new Set<string>();
/** The account whose terminal is on the glass, or null when none is. */
let shownAccount: string | null = null;
/** The size every terminal on this screen is currently drawn at, in `px`. */
let terminalFontSize = DEFAULT_TERMINAL_FONT_SIZE;
/**
 * Why an account's last launch failed, by account id, until it is tried again.
 *
 * The status line carries the app's own reason and is the full account of it,
 * but it is one line for the whole screen and the next thing written takes it.
 * A launch that failed leaves nothing else behind — its terminal is discarded,
 * there being no session under it — so without this the row that was pressed
 * goes back to reading 未起動, as though it never had been.
 */
const launchFailures = new Map<string, string>();
/**
 * The account the open 終了 dialog is asking about, or null while it is closed.
 *
 * The id rather than the view: the dialog stays open across whatever else the
 * screen does, and a view can be discarded while it is (`showView`). Resolving
 * the id when the answer comes back finds a session that is still there, or
 * finds nothing and ends nothing.
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

/** The terminal currently on the glass, or null when none is. */
function shownView(): SessionView | null {
  return shownAccount === null ? null : (views.get(shownAccount) ?? null);
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
 * Write the account list back to disk.
 *
 * Called when an account is decided, deleted, or created for the person at this
 * screen — never from a field losing focus. An account is a thing that exists
 * whether or not it runs (#53), and the field-by-field save made every keystroke
 * on the way to a name into a state that existed: pressing ＋ created an
 * account, and from there the only way out was to delete it (#59).
 */
function saveAccounts(): void {
  void invoke("save_config", { config: { accounts } }).catch(() => {
    status("アカウントを保存できませんでした。", "error");
  });
}

/**
 * Fill the text size picker.
 *
 * Labelled as a proportion of the default rather than in `rem`, because the
 * choice being made is "larger or smaller than what I have", and the unit the
 * size happens to be held in answers a question nobody is asking.
 */
function fillRoomFontSizes(): void {
  for (const size of ROOM_FONT_SIZES) {
    const option = document.createElement("option");
    option.value = String(size);
    option.textContent = `${Math.round((size / DEFAULT_ROOM_FONT_SIZE) * 100)}%`;
    fontSizeEl.appendChild(option);
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
 * 宛先 and 送信 sit in the composer but do not follow. They are controls, not
 * the sentence, and they stay on the whole-UI axis (#66).
 *
 * `save` is false for the restore at startup. Writing the value back there
 * would put a size in storage for a screen that never chose one, which is the
 * one state this is supposed to leave alone.
 */
function applyRoomFontSize(size: number, save: boolean): void {
  roomFontSize = size;
  roomEl.style.setProperty("--room-font-size", `${size}rem`);
  inputEl.style.setProperty("--room-font-size", `${size}rem`);
  fontSizeEl.value = String(size);
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

/** Fill the terminal's size picker. Labelled in `px`; see the ladder above. */
function fillTerminalFontSizes(): void {
  for (const size of TERMINAL_FONT_SIZES) {
    const option = document.createElement("option");
    option.value = String(size);
    option.textContent = `${size}px`;
    terminalFontSizeEl.appendChild(option);
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
  terminalFontSizeEl.value = String(size);
  if (save) localStorage.setItem(TERMINAL_FONT_SIZE_KEY, String(size));
  fitShown();
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

function appendMessage(message: RoomMessage): void {
  // The room is scrolled to the bottom only when it already was, so reading
  // back through the log is not yanked away by an arriving message.
  const atBottom = roomEl.scrollHeight - roomEl.scrollTop - roomEl.clientHeight < 40;

  const line = document.createElement("article");
  line.className = "message";
  // `own` rather than a name test: the room decides self on the connection a
  // post arrived on, which a rename cannot blur (#40).
  line.style.setProperty(
    "--speaker",
    speakerColor(message.speaker, message.hue, message.own),
  );

  const head = document.createElement("div");
  head.className = "meta";

  const speaker = document.createElement("span");
  speaker.className = "speaker";
  speaker.textContent = message.speaker;
  head.appendChild(speaker);

  if (message.to) {
    const to = document.createElement("span");
    to.className = "to";
    to.textContent = `→ ${message.to}`;
    head.appendChild(to);
  }

  const time = document.createElement("time");
  time.className = "ts";
  time.dateTime = message.ts;
  time.textContent = shortTime(message.ts);
  head.appendChild(time);

  const body = document.createElement("div");
  body.className = "body";
  body.textContent = message.content;

  line.append(head, body);
  roomEl.appendChild(line);
  // On the glass, so it is what this screen can declare having seen. Own posts
  // included: the room does not hold a speaker's own posts against them, and
  // carrying the newest id either way keeps this one value rather than two.
  lastSeenId = message.message_id;

  if (atBottom) roomEl.scrollTop = roomEl.scrollHeight;
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
  { kind: "user", label: "user" },
  { kind: "ai", label: "AI" },
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
    const matches = participants.filter((one) => one.account === account.id);
    for (const participant of matches) {
      placed.add(participant.id);
      rows.push({ account, participant });
    }
    if (!matches.length) rows.push({ account, participant: null });
  }

  for (const participant of participants) {
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
// Silence is what this says instead, and silence is allowed to be wrong.

/**
 * Note that this session is printing, and arm its fall back to silence.
 *
 * Called once per chunk, and cheap on purpose: the timer is pushed forward every
 * time, and the panel is redrawn only on the edge where the word appears.
 */
function markOutput(view: SessionView): void {
  if (view.quiet !== undefined) clearTimeout(view.quiet);
  view.quiet = window.setTimeout(() => {
    view.quiet = undefined;
    view.outputting = false;
    renderPanel();
  }, OUTPUT_QUIET_MS);
  if (view.outputting) return;
  view.outputting = true;
  renderPanel();
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
  let moved = awaiting.delete(message.speaker);
  const to = message.to;
  if (to !== null && !awaiting.has(to) && participants.some((one) => one.name === to && !one.own)) {
    awaiting.add(to);
    moved = true;
  }
  if (moved) renderPanel();
}

/**
 * Drop everyone the room is waiting on who is no longer in it.
 *
 * The roster is the authority on who is present, so this runs when it arrives. A
 * session that exits with a question outstanding leaves the room, and this is
 * what takes its mark with it.
 */
function pruneAwaiting(): void {
  const present = new Set(participants.filter((one) => !one.own).map((one) => one.name));
  for (const name of awaiting) {
    if (!present.has(name)) awaiting.delete(name);
  }
}

/**
 * What a running account is doing, in the one word the row has room for.
 *
 * 考え中… when the room is waiting on this name, 出力中 otherwise, and nothing at
 * all while the terminal is quiet.
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
  if (!view || view.ended !== null || !view.outputting) return "";
  return awaiting.has(name) ? "考え中…" : "出力中";
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
  const view = row.account ? views.get(row.account.id) : undefined;

  const entry = document.createElement("li");
  entry.className = "member";
  entry.style.setProperty("--speaker", speakerColor(name, hue, own));
  if (!row.participant) entry.classList.add("offline");

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
  const failure = row.account ? launchFailures.get(row.account.id) : undefined;

  // What this line says about itself beyond the name. Someone present and not
  // oneself says what they are doing, when this screen can observe it, and
  // otherwise says nothing — being in the list is what it would have said (#82).
  //
  // One note, in one place. 未起動 and 考え中… are mutually exclusive states of
  // the same account, so they need no second slot, and the two fixed tracks #71
  // measured are untouched.
  let noteText = "";
  let noteKind = "";
  if (own) noteText = "（あなた）";
  else if (launching) noteText = "起動中";
  else if (row.participant) {
    noteText = activityNote(name, view);
    if (noteText) noteKind = "active";
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
    // the same row offered ❌. On the device that window lasted minutes,
    // because the development-channels flag stops the CLI at a confirm prompt,
    // and a running session was indistinguishable by word from an idle
    // account — only the button said which was which (#89).
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
    button.addEventListener("click", () => {
      revealDiagnostics();
      showView(view.accountId);
      view.term.focus();
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
    if (noteKind) note.dataset.kind = noteKind;
    // The app's own reason, on the row carrying the word. The status line has
    // it in full; this is so a row saying 起動失敗 is not a dead end.
    if (failure) note.title = failure;
    pick.appendChild(note);
  }
  entry.appendChild(pick);

  // The account's own operations ride on its row, which is what one list buys:
  // the row is keyed on the account id, so these act on one account and cannot
  // be tied to the wrong one by a shared name (#53, #59).
  //
  // Two fixed columns: the session's lifecycle, then 編集. 開始 and 終了 are the
  // two ends of one thing and belong in one place — 開始 was on a launcher row
  // above the conversation until #62, a screen away from the 終了 that #57 had
  // already put here. The slot is emitted whether or not it holds a button, so
  // a row with no lifecycle keeps the column open rather than sliding its 編集
  // left of every other row's.
  const lifecycle = document.createElement("span");
  lifecycle.className = "lifecycle";
  if (view && view.ended === null && view.ptyId !== "") {
    // No 終了 before the launch has returned an id: there is no session to end
    // yet, and a kill aimed at an empty id reports success having done nothing
    // (#57).
    lifecycle.appendChild(endButton(view, name));
  } else if (row.account?.kind === "ai") {
    // Kind `ai` only. A `user` account is a person and there is no CLI under a
    // person to spawn; `start_session` refuses one and that refusal is the
    // authority, but a refusal is the wrong way for the person to find out
    // (#59). Their row keeps the empty column, and their 編集 with it.
    lifecycle.appendChild(startButton(row.account, launching));
  }
  entry.appendChild(lifecycle);
  if (row.account) entry.appendChild(editButton(row.account));

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
  start.textContent = "▶️";
  start.disabled = launching;
  const label = launching
    ? `${account.name} を起動しています`
    : `${account.name} のセッションを開始する`;
  start.title = label;
  start.setAttribute("aria-label", label);
  start.addEventListener("click", () => void startSession(account));
  return start;
}

/** The control that opens one account's form. A mark, named by its label. */
function editButton(account: Account): HTMLButtonElement {
  const edit = document.createElement("button");
  edit.type = "button";
  edit.className = "edit";
  edit.textContent = "⚙️";
  edit.title = `${account.name} の設定`;
  edit.setAttribute("aria-label", `${account.name} の設定`);
  edit.addEventListener("click", () => openAccountDialog(account));
  return edit;
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
  // screen is not launched (`start_session` refuses a `user` account).
  tab.style.setProperty("--speaker", speakerColor(name, account?.hue ?? null, false));
  if (shown) tab.classList.add("shown");
  if (view.ended !== null) tab.classList.add("ended");

  const pick = document.createElement("button");
  pick.type = "button";
  pick.className = "name";
  pick.textContent = name;
  pick.title = view.ended === null ? `${name} の端末` : `${name} の端末（${view.ended}）`;
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
    close.textContent = "✕";
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
  for (const view of views.values()) tabsEl.appendChild(terminalTab(view));
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
  const wasShown = shownAccount === view.accountId;
  discardView(view);
  if (wasShown) {
    showView([...views.keys()].pop() ?? null);
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
 * (#53). It does not become an addressee: `renderAddressees` reads the live
 * roster only, because a name that cannot be reached is not worth naming.
 */
function renderPanel(): void {
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
    renderAddressees();
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

  renderAddressees();
}

/** Take the room's roster and redraw the panel around it. */
function renderRoster(joined: Participant[]): void {
  participants = joined;
  // Against the roster that just arrived, before it is drawn: who the room is
  // waiting on is only meaningful about someone who is in it (#82).
  pruneAwaiting();
  renderPanel();
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
    seated = new Map(held.map((seat) => [seat.account_id, seat]));
  } catch {
    // The panel keeps the last answer rather than declaring everyone offline
    // on a failed read. It still redraws: what failed is this one value, and
    // whatever else moved since the last draw is not held back by it.
  }
  await adoptSeats();
  renderPanel();
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
    if (!seat.session || views.has(seat.account_id)) continue;
    const account = accounts.find((one) => one.id === seat.account_id);
    // An account this screen does not have is one it cannot draw a row for, and
    // the row is the only way that terminal could be reached. The app refuses
    // to delete a seated account, so this is a config edited from outside.
    if (!account) continue;
    const view = openView(account, seat.session);
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
 * kind `user`, and the migration is the obvious one — the name and colour they
 * had been joining under become that account's (#59).
 *
 * Resolved before the room is joined, because the id goes into the join.
 */
function resolveLocalAccount(): void {
  const stored = localStorage.getItem(LOCAL_KEY);
  let account =
    accounts.find((one) => one.id === stored && one.kind === "user") ??
    accounts.find((one) => one.kind === "user") ??
    null;

  if (!account) {
    const savedHue = localStorage.getItem(HUE_KEY);
    account = {
      id: crypto.randomUUID(),
      name: (localStorage.getItem(NAME_KEY) ?? "").trim() || "human",
      // Carried so the shape of an account is one shape. Nothing launches a
      // person, and `start_session` refuses a `user` account outright.
      command: "claude",
      args: [],
      cwd: null,
      hue: savedHue !== null && HUES.some(({ hue }) => String(hue) === savedHue)
        ? Number(savedHue)
        : null,
      kind: "user",
      // Carried for the same reason `command` is: one shape of account. A
      // person speaks as themselves, and there is no launch to select a style
      // on.
      character: null,
    };
    accounts.push(account);
    saveAccounts();
  }

  localAccountId = account.id;
  localStorage.setItem(LOCAL_KEY, account.id);
}

/**
 * Redraw the addressee list from the roster.
 *
 * The names have to be the ones participants answer to, so they come from the
 * roster rather than being typed: a mistyped addressee is an utterance
 * addressed to nobody, and nothing on screen would say so. A chosen addressee
 * survives a roster change while that participant is still present, and falls
 * back to the whole room when they leave.
 *
 * Everyone but oneself is addressable — sessions and people alike, since the
 * roster no longer separates them. The room's roster is the whole source, so an
 * account with no session in it is absent from here by construction: naming a
 * participant who cannot be reached is a post addressed to nobody, which is the
 * reason addressees are picked from a roster at all (#43).
 */
function renderAddressees(): void {
  const chosen = toEl.value;
  // Names, deduplicated: `to` carries a display name, so two participants
  // answering to one name are one option — listing it twice would offer a
  // choice between two identical things that address the same pair anyway.
  const addressable = [
    ...new Set(participants.filter((one) => !one.own).map((one) => one.name)),
  ];

  // Left alone when the roster has not moved. The panel is now redrawn whenever
  // a row's word changes — when a session starts printing and again when it
  // stops (#82) — and this control is the one thing in the panel a person can be
  // in the middle of using: rebuilding a `<select>` closes the list that is open
  // over it. The roster is what this reads from, so the same list of names is
  // the same options, and replacing them would be work with a cost and no
  // effect.
  const wanted = ["", ...addressable];
  if (
    toEl.options.length === wanted.length &&
    wanted.every((name, at) => toEl.options[at].value === name)
  ) {
    return;
  }

  toEl.replaceChildren();

  const everyone = document.createElement("option");
  everyone.value = "";
  everyone.textContent = "全体";
  toEl.appendChild(everyone);

  for (const participant of addressable) {
    const option = document.createElement("option");
    option.value = participant;
    option.textContent = participant;
    toEl.appendChild(option);
  }

  toEl.value = addressable.includes(chosen) ? chosen : "";
}

async function send(): Promise<void> {
  const content = inputEl.value.trim();
  if (!content) return;

  const speaker = localName();
  // Empty means the room as a whole. The app still delivers to everyone; the
  // addressee is judgment material for the participants, not a delivery filter.
  const to = toEl.value || null;
  // Read before the await: what the screen had drawn when this was sent is the
  // watermark, and an arrival during the round trip must not be folded into it.
  const lastSeen = lastSeenId;
  inputEl.value = "";
  try {
    const outcome = await invoke<PostOutcome>("room_post", {
      speaker,
      content,
      to,
      lastSeen,
    });
    if (!outcome.delivered) {
      // Refused, not failed. The missed posts are already on screen — the room
      // drew them through the same event — so the report names who spoke and
      // leaves the reading where it belongs.
      inputEl.value = content;
      const speakers = [...new Set(outcome.missed.map((one) => one.speaker))];
      status(
        `送っていません。書いている間に届いた発言が ${outcome.missed.length} 件あります（${speakers.join("、")}）。読んでから送るか決めてください。`,
        "error",
      );
      return;
    }
    status("");
  } catch (err) {
    // Put the text back rather than losing what was typed.
    inputEl.value = content;
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
    return;
  }
  const name = viewName(view);
  sessionStateEl.textContent = view.ended === null ? `${name} 起動中` : `${name} 終了（${view.ended}）`;
  sessionStateEl.dataset.kind = view.ended === null ? "ok" : "error";
  transportEl.textContent = "PTY";
  commandEl.textContent = view.command;
  dirEl.textContent = view.cwd ?? "—";
  dirEl.title = view.cwd ?? "";
  startedEl.textContent = view.startedAt === "" ? "—" : shortTime(view.startedAt);
  showWindowSize();
}

/**
 * The 終了 control, which asks before it acts.
 *
 * Ending a session cannot be undone, and this control sits in a list whose
 * other click merely changes which pane is showing. One plain click away from a
 * harmless neighbour is how a slip ends a session that was mid-answer, so the
 * click opens the question and the dialog is where it is answered (#71).
 *
 * A dialog of this app's own, never `window.confirm`. That one answers on the
 * host's terms and the two ways it can fail here are both wrong: a host that
 * answers nothing either makes the button silently dead or — reading its own
 * default as yes — ends the session on the single click. `#end-dialog` is in
 * the webview and has neither failure (#57).
 *
 * The mark is fixed, so the button no longer changes width under the pointer.
 * That is what the two-click form had to hold a settle window for, and what
 * made the lifecycle column's width a thing the whole list paid for.
 */
function endButton(view: SessionView, name: string): HTMLButtonElement {
  const end = document.createElement("button");
  end.type = "button";
  end.className = "end";
  end.textContent = "❌";
  end.title = `${name} のセッションを終了する`;
  end.setAttribute("aria-label", `${name} のセッションを終了する`);
  end.addEventListener("click", () => openEndDialog(view.accountId, name));
  return end;
}

/** Ask whether one account's session is to end. Nothing ends until answered. */
function openEndDialog(accountId: string, name: string): void {
  endingAccount = accountId;
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
  const accountId = endingAccount;
  closeEndDialog();
  if (accountId === null) return;
  // Resolved now, not when the dialog opened: the session may have ended on its
  // own while the question stood, and there is then nothing left to end.
  const view = views.get(accountId);
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
  return held.map(
    (seat) =>
      accounts.find((one) => one.id === seat.account_id)?.name.trim() || seat.account_id,
  );
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
 */
function openView(account: Account, running?: RunningSession): SessionView {
  // A relaunch replaces the previous run's pane. Two panes for one account
  // would be two rows under one name, and the row is what the operations hang
  // on; the scrollback that goes with it is the one the person just decided to
  // start over from.
  discardView(views.get(account.id));

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
    term,
    fit,
    host,
    unlisten: [],
    ended: null,
    // Quiet until something arrives. A session picked up again after a reload
    // starts here too: its terminal is new even though its process is not, so
    // what this screen can say about it begins at the next byte (#86).
    outputting: false,
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

  views.set(account.id, view);
  showView(account.id);
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
  views.delete(view.accountId);
  if (shownAccount === view.accountId) shownAccount = null;
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
  for (const view of views.values()) {
    view.host.hidden = view.accountId !== accountId;
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
async function attachSession(view: SessionView, ptyId: string): Promise<void> {
  view.unlisten.push(
    await listen<string>(`pty-data-${ptyId}`, (event) => {
      view.term.write(event.payload);
      // The bytes go to the emulator and are not looked at here. That this
      // chunk arrived is the whole of the signal (#82).
      markOutput(view);
    }),
  );
  view.unlisten.push(
    await listen<number | null>(`pty-exit-${ptyId}`, (event) => {
      const code = event.payload;
      const detail = code === null ? "終了コード不明" : `終了コード ${code}`;
      view.ended = detail;
      // Nothing more will print, so the row must not spend the quiet window
      // still saying that something is (#82).
      stopOutput(view);
      // Nothing more will arrive on this pty. The view lives on for what it
      // has already printed, not for anything it is still waiting to hear.
      for (const off of view.unlisten) off();
      view.unlisten = [];
      const name = viewName(view);
      status(`${name} が終了しました（${detail}）。端末を確認してください。`, "error");
      // The seat this account held is free the moment its session ends, so the
      // panel says 未起動 again and the account can be started once more.
      void refreshSeats();
      renderPanel();
      if (shownAccount === view.accountId) {
        renderSessionFacts();
        revealDiagnostics();
      }
    }),
  );
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
  renderPanel();
  renderSessionFacts();

  await attachSession(view, started.pty_id);

  // The first thing a session shows is a question, so the pane that carries
  // the answer opens with it rather than waiting for a failure.
  revealDiagnostics();
  if (shownAccount === view.accountId) view.term.focus();
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
  if (seated.has(account.id)) await refreshSeats();
  if (seated.has(account.id)) {
    status(
      `「${name}」は既にこの部屋に居ます。一つのアカウントが持てる席は一つの部屋につき一つです。起動中のセッションを終了してから、もう一度起動してください。`,
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

  // Size the PTY to the terminal that will display it, so the CLI's first
  // paint is not laid out for a window it does not have. The pane is revealed
  // first because a hidden container has no size to measure.
  //
  // What was on the glass is kept, so a launch that fails can put it back
  // rather than leaving a blank pane where a running session had been.
  const previous = shownAccount;
  revealDiagnostics();
  // Cleared as the attempt starts rather than as it fails: 起動失敗 stands on
  // the row until this account is asked again, and this is that moment.
  launchFailures.delete(account.id);
  // From here the row carries the launch. The view exists and has no pty id
  // yet, which is what puts its 開始 into 起動中; `openView` redraws through
  // `showView`.
  const view = openView(account);

  status(`${name} を起動しています…`);
  try {
    const started = await invoke<StartedSession>("start_session", {
      account,
      cols: view.term.cols,
      rows: view.term.rows,
    });
    status(`${name} を起動しました。${started.mcp_config} に登録済み。`);
    await refreshSeats();
    await followSession(view, started);
  } catch (err) {
    // Nothing was spawned, so this terminal has nothing to show and no session
    // to end. It goes, and the app's own reason stands in the status line —
    // recorded against the account first, because the pane going is what would
    // otherwise leave the row reading 未起動 as though it had never been
    // pressed. The row says 起動失敗 and holds the reason; the status line has
    // it in full.
    launchFailures.set(account.id, String(err));
    discardView(view);
    showView(previous !== null && views.has(previous) ? previous : ([...views.keys()].pop() ?? null));
    status(`${name} を起動できませんでした: ${err}`, "error");
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
 */
function showDialogKind(): void {
  const kind = dialogKindEl.value as AccountKind;
  dialogLaunchEl.hidden = kind !== "ai";
  if (kind === "ai") void refreshDialogPreview();
}

/**
 * Show the command this account's launch would actually run.
 *
 * The app merges its own channel entry into whatever is typed and selects the
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

/**
 * Open the form on one account, or on a new one when given none.
 *
 * A new account's id is minted here so the launch preview has something to name
 * a server after. That is all it is until 決定 — nothing is pushed into the
 * account list, so 取消 leaves no account behind and no id in use.
 */
function openAccountDialog(account: Account | null): void {
  editing = account;
  draft = account
    ? { ...account, args: [...account.args] }
    : {
        // Opaque and minted once. Nothing reads a name out of it — the key in
        // `.mcp.json` derives from it precisely so renaming is free (#53).
        id: crypto.randomUUID(),
        name: unusedAccountName(),
        // The one vendor the room is built on. See src-tauri/src/config.rs.
        command: "claude",
        args: [],
        // A prefill, not a default: the app launches nothing in a directory the
        // person has not seen on screen (#20).
        cwd: homeDir || null,
        hue: null,
        kind: "ai",
        // Nothing, rather than a guess at a style name: an unnamed character
        // launches on whatever the working directory's own settings say, which
        // is an answer. A guessed name that resolves to no style is not.
        character: null,
      };

  dialogTitleEl.textContent = account ? "アカウントの編集" : "アカウントの追加";
  dialogNameEl.value = draft.name;
  dialogKindEl.value = draft.kind;
  dialogHueEl.value = draft.hue === null ? "" : String(draft.hue);
  dialogCwdEl.value = draft.cwd ?? "";
  dialogCharacterEl.value = draft.character ?? "";
  dialogOptionsEl.value = joinArgs(draft.args);
  dialogDeleteEl.hidden = account === null;
  disarmDelete();
  dialogError("");
  showDialogKind();
  dialogEl.showModal();
  dialogNameEl.focus();
  dialogNameEl.select();
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
  // A running account cannot change kind. Its session is in the room under this
  // account, and turning it into a person would drop the working directory and
  // options that session was launched from while it is still running.
  if (target && kind !== target.kind && seated.has(target.id)) {
    dialogError(`「${target.name}」は起動中です。種別を変えるには先に終了してください。`);
    return false;
  }
  // The person at this screen is a person. Turning their account into an `ai`
  // would list them under the wrong heading and offer to launch a CLI under
  // their name, which is not a thing there is one of.
  if (target && target.id === localAccountId && kind !== "user") {
    dialogError("この画面の本人のアカウントは種別 user のままです。");
    return false;
  }
  const cwd = dialogCwdEl.value.trim();
  const character = dialogCharacterEl.value.trim();
  // Only for a session. A person's working directory, character and options
  // would be values nothing ever reads, kept alive by an edit that once set
  // them. A person is not launched, so nothing selects a style for them.
  const args =
    kind === "ai"
      ? await invoke<string[]>("parse_launch_options", { text: dialogOptionsEl.value })
      : [];

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

  const settled: Account = {
    ...settling,
    name,
    kind,
    hue: declaredHue(dialogHueEl),
    cwd: kind === "ai" ? cwd || null : null,
    // Blank clears it, and clearing it is a state: the account goes back to
    // launching on whatever its working directory's own settings name.
    character: kind === "ai" ? character || null : null,
    args,
  };

  if (target) {
    const at = accounts.findIndex((one) => one.id === target.id);
    if (at >= 0) accounts[at] = settled;
  } else {
    accounts.push(settled);
  }
  saveAccounts();

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
 */
function deleteFromDialog(): void {
  const account = editing;
  if (!account) return;

  if (seated.has(account.id)) {
    dialogError(`「${account.name}」は起動中です。セッションを終了してから削除してください。`);
    disarmDelete();
    return;
  }
  if (account.id === localAccountId) {
    dialogError("この画面の本人のアカウントは削除できません。");
    disarmDelete();
    return;
  }
  if (!deleteArmed) {
    deleteArmed = true;
    dialogDeleteEl.textContent = "本当に削除";
    dialogDeleteEl.classList.add("armed");
    dialogError("もう一度押すと削除します。");
    return;
  }

  accounts = accounts.filter((candidate) => candidate.id !== account.id);
  // Its terminal goes with it. An account that no longer exists cannot be named
  // in the panel, and the row is the only way that pane could be reached.
  discardView(views.get(account.id));
  saveAccounts();
  closeAccountDialog();
  renderPanel();
  renderSessionFacts();
  status(`アカウント「${account.name}」を削除しました。`);
}

/** Drop the draft and close. Nothing it held reached the account list. */
function closeAccountDialog(): void {
  editing = null;
  draft = null;
  disarmDelete();
  if (dialogEl.open) dialogEl.close();
}

function renderSocket(port: number | null, error?: string): void {
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

async function main(): Promise<void> {
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
  fillRoomFontSizes();
  applyRoomFontSize(storedRoomFontSize(), false);
  fontSizeEl.addEventListener("change", () => {
    applyRoomFontSize(Number(fontSizeEl.value), true);
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
  fillTerminalFontSizes();
  applyTerminalFontSize(storedTerminalFontSize(), false);
  terminalFontSizeEl.addEventListener("change", () => {
    applyTerminalFontSize(Number(terminalFontSizeEl.value), true);
  });

  // The two ends of one act. 端末 is reachable while the pane is folded and ✕
  // while it is open, which is the whole of why both exist (#68).
  toggleEl.addEventListener("click", () => {
    if (diagnosticsEl.hidden) revealDiagnostics();
    else hideDiagnostics();
  });
  diagnosticsCloseEl.addEventListener("click", () => hideDiagnostics());

  await listen<RoomMessage>("room-message", (event) => {
    appendMessage(event.payload);
    // The same post read twice: once as a line in the conversation, once for
    // who the room is now waiting on. The second reading is what puts 考え中…
    // on a row without anything having to read the CLI's output (#82).
    trackAddress(event.payload);
  });
  await listen<Participant[]>("room-participants", (event) => renderRoster(event.payload));
  // The socket binds after the frontend loads, so the event is the authority
  // and the poll below is only for a listener that attached too late.
  await listen<number>("room-ready", (event) => renderSocket(event.payload));

  sendEl.addEventListener("click", () => void send());
  inputEl.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void send();
    }
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
  dialogKindEl.addEventListener("change", () => showDialogKind());
  dialogOptionsEl.addEventListener("input", () => void refreshDialogPreview());
  // The character ends up in the line that runs, so it redraws the preview for
  // the same reason the options do: the line shown has to be the line spawned.
  dialogCharacterEl.addEventListener("input", () => void refreshDialogPreview());
  // So does the working directory: which registrations the line stops is read
  // out of the directory it is pointed at (#103).
  dialogCwdEl.addEventListener("input", () => void refreshDialogPreview());
  // Anything but the second click of 削除 disarms it: an arm left standing is
  // one that an unrelated click fires later.
  for (const field of [
    dialogNameEl,
    dialogKindEl,
    dialogHueEl,
    dialogCwdEl,
    dialogCharacterEl,
    dialogOptionsEl,
  ]) {
    field.addEventListener("input", () => disarmDelete());
  }
  dialogDeleteEl.addEventListener("click", () => deleteFromDialog());
  dialogCancelEl.addEventListener("click", () => closeAccountDialog());
  // Escape closes the dialog itself, and it means 取消: the draft is dropped by
  // the close handler below, so there is no path out of this form that leaves
  // half of it applied.
  dialogEl.addEventListener("close", () => {
    editing = null;
    draft = null;
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

  try {
    const config = await invoke<AppConfig>("load_config");
    accounts = config.accounts;
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

  // Before the room, and outside its try. A session running under a seat this
  // screen has forgotten is reachable again from the seats alone (`adoptSeats`),
  // and a room that fails to answer is no reason to leave it unreachable — the
  // failure this is here for is the screen having been reloaded, which the room
  // knows nothing about (#84). It swallows its own errors, so it needs none.
  await refreshSeats();

  try {
    await join();
    participants = await invoke<Participant[]>("room_participants");
    const port = await invoke<number | null>("room_port");
    if (port !== null) renderSocket(port);
    renderPanel();
  } catch (err) {
    renderPanel();
    renderSocket(null, `取得できませんでした: ${err}`);
  }
}

void main();
