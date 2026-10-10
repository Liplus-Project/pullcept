import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { activityNote, NO_WORD } from "./seat-status";
import type { RoomMessage, PostOutcome, MissedPost, LoggedPost, Topic, TopicRef, Participant, Roster, SessionStats, SeatActivity, AccountKind, Account, PanelState, AppConfig, SeatedAccount, IconName, IconShape, Fold, PermissionCard, PermissionResolved, Member, RowWord, Attachment, TextPiece } from "./contracts";
import { createAccountDialog } from "./account-dialog";
import { createDisplaySettings } from "./display-settings";
import { createSessionController } from "./session-controller";
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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, type CloseRequestedEvent } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@xterm/xterm/css/xterm.css";
import { createComposer, findCodeFences, highlightedCode, lineInFence, splitCodeFences } from "./composer";

/**
 * Whether this account launches a session.
 *
 * The two CLI kinds: what they differ in is what the app puts on their line,
 * which is not this question. A person has no command under them to spawn at
 * all (#59), and a local MCP server is started with the app from its entry in
 * the file, not from a row (#193).
 */
function launches(account: Account): boolean {
  return account.kind === "claude_code" || account.kind === "codex_cli" || account.kind === "cli";
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
  // A gear: the app's own settings (#234, the shape of the 2026-10-02 mockup).
  // Tabler Icons `settings` (MIT, NOTICE.txt), redrawn from its 24-unit grid
  // onto this one. #221 drew sliders here to keep it apart from the row's ⚙,
  // which #224 has since moved into the row's menu.
  settings: [
    [
      "path",
      {
        d:
          "M6.883 2.878c.284 -1.171 1.949 -1.171 2.233 0a1.149 1.149 0 0 0 1.715 .711" +
          "c1.029 -.627 2.207 .551 1.58 1.58a1.149 1.149 0 0 0 .71 1.715" +
          "c1.171 .284 1.171 1.949 0 2.233a1.149 1.149 0 0 0 -.711 1.715" +
          "c.627 1.029 -.551 2.207 -1.58 1.58a1.149 1.149 0 0 0 -1.715 .71" +
          "c-.284 1.171 -1.949 1.171 -2.233 0a1.149 1.149 0 0 0 -1.715 -.711" +
          "c-1.029 .627 -2.207 -.551 -1.58 -1.58a1.149 1.149 0 0 0 -.71 -1.715" +
          "c-1.171 -.284 -1.171 -1.949 0 -2.233a1.149 1.149 0 0 0 .711 -1.715" +
          "c-.627 -1.029 .551 -2.207 1.58 -1.58c.667 .405 1.531 .047 1.715 -.71",
      },
    ],
    ["path", { d: "M6 8a2 2 0 1 0 4 0a2 2 0 0 0 -4 0" }],
  ],
  plus: [
    ["line", { x1: "8", y1: "3", x2: "8", y2: "13" }],
    ["line", { x1: "3", y1: "8", x2: "13", y2: "8" }],
  ],
  copy: [
    ["rect", { x: "5.5", y: "5.5", width: "8", height: "8", rx: "1.6" }],
    ["path", { d: "M10.5 5.5V4.1A1.6 1.6 0 0 0 8.9 2.5H4.1A1.6 1.6 0 0 0 2.5 4.1v4.8a1.6 1.6 0 0 0 1.6 1.6h1.4" }],
  ],
  // Folding the pane, closing an ended tab and deleting a topic. Which of
  // those it is, is the colour and the label: the one that cannot be taken
  // back carries `--danger`. Ending a session has its own drawing, `stop`.
  close: [
    ["line", { x1: "4", y1: "4", x2: "12", y2: "12" }],
    ["line", { x1: "12", y1: "4", x2: "4", y2: "12" }],
  ],
  // 送信 (#234): a paper plane, Tabler Icons `send` (MIT, NOTICE.txt) on this grid.
  send: [
    ["path", { d: "M6.667 9.333l7.333 -7.333" }],
    [
      "path",
      { d: "M14 2l-4.333 12a.367 .367 0 0 1 -.667 0l-2.333 -4.667l-4.667 -2.333a.367 .367 0 0 1 0 -.667l12 -4.333" },
    ],
  ],
  // 宛先: the `@` the button types (#222).
  at: [
    ["circle", { cx: "8", cy: "8", r: "2.6" }],
    ["path", { d: "M10.6 5.4v3.3a2 2 0 0 0 4 0V8a6.6 6.6 0 1 0-2.6 5.25" }],
  ],
  // 添付 (#223): a paper clip, slanted (#234). Tabler Icons `paperclip` (MIT,
  // NOTICE.txt) on this grid.
  attach: [
    [
      "path",
      { d: "M10 4.667l-4.333 4.333a1 1 0 0 0 2 2l4.333 -4.333a2 2 0 0 0 -4 -4l-4.333 4.333a3 3 0 0 0 6 6l4.333 -4.333" },
    ],
  ],
  start: [["path", { d: "M5 3.2v9.6L12.6 8Z" }]],
  // 終了 (#239): a square, the other end of the triangle above, on the row in
  // the column 開始 stands in and in the row's menu. Tabler Icons `player-stop`
  // (MIT, NOTICE.txt) on this grid.
  stop: [
    [
      "path",
      {
        d:
          "M3.333 4.667a1.333 1.333 0 0 1 1.333 -1.333h6.667a1.333 1.333 0 0 1 1.333 1.333" +
          "v6.667a1.333 1.333 0 0 1 -1.333 1.333h-6.667a1.333 1.333 0 0 1 -1.333 -1.333Z",
      },
    ],
  ],
  // A gear: 編集 in the row's menu, the window an account's settings are made in.
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
  // 最新の発言へ (#243): a chevron pointing down, to the room's foot. Tabler
  // Icons `chevron-down` (MIT, NOTICE.txt) on this grid.
  "chevron-down": [["path", { d: "M4 6l4 4l4 -4" }]],
  // The window's own three (#252), in the shapes the standard frame drew them:
  // a bar, a square, and two squares with the one in front at the lower left.
  // 閉じる is `close` above, the same ✕ as every other one on the screen.
  // `restore` is not `copy` turned over by accident: the frame puts its front
  // square at the lower left, `copy` puts it at the lower right.
  minimize: [["line", { x1: "3.5", y1: "8", x2: "12.5", y2: "8" }]],
  maximize: [["rect", { x: "3.5", y: "3.5", width: "9", height: "9", rx: "1.2" }]],
  restore: [
    ["rect", { x: "3", y: "5.5", width: "7.5", height: "7.5", rx: "1.2" }],
    ["path", { d: "M5.5 5.5V4.2A1.2 1.2 0 0 1 6.7 3h5.1A1.2 1.2 0 0 1 13 4.2v5.1a1.2 1.2 0 0 1 -1.2 1.2H10.5" }],
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
const scrollLatestEl = document.getElementById("scroll-latest") as HTMLButtonElement;
const roomBusyEl = document.getElementById("room-busy") as HTMLElement;

/**
 * Whether the room is on its way down to its foot by a glide it was sent on
 * (#307). The glide is the stylesheet's (`scroll-behavior` on `#room`), so for
 * as long as it runs the room is between its last place and its foot, and the
 * distance alone would read it as read back: a second line arriving mid-glide
 * would not follow it, and 最新の発言へ would blink up under it. Counted as at
 * the foot until the glide lands there or is taken over by the person's own
 * scroll (`settleGlide`).
 */
let roomGliding = false;

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
const accountMenuEl = document.getElementById("account-menu") as HTMLElement;
/** Where the input box's editor is mounted (#354); it carries the room's font size. */
const inputEl = document.getElementById("input") as HTMLElement;
const sendEl = document.getElementById("send") as HTMLButtonElement;
const mentionEl = document.getElementById("mention") as HTMLButtonElement;
const composerEl = document.getElementById("composer") as HTMLElement;
const composerBoxEl = document.getElementById("composer-box") as HTMLElement;
const attachEl = document.getElementById("attach") as HTMLButtonElement;
const attachInputEl = document.getElementById("attach-input") as HTMLInputElement;
const attachmentsEl = document.getElementById("attachments") as HTMLUListElement;
const viewerEl = document.getElementById("viewer") as HTMLDialogElement;
const viewerImageEl = document.getElementById("viewer-image") as HTMLImageElement;
const mentionListEl = document.getElementById("mention-list") as HTMLUListElement;
/**
 * The input box (#354): an editor whose ``` turns a line into a code block,
 * handing the room the same Markdown text a textarea held. Its keys and its
 * paste are wired here to what the screen already did with the textarea: the
 * `@` list's keys first, Enter to 送信, a paste of files to the chips.
 */
const composer = createComposer(inputEl, {
  onSubmit: () => void send(),
  onKeyDown: (event) => mentionKey(event),
  // `@` opens the list of who can be addressed (#204). It follows the caret,
  // so a click or an arrow key that moves it off the `@` shuts it.
  onChange: () => refreshMentions(),
  onBlur: () => closeMentions(),
  onPasteFiles: (files) => attachFiles(files),
});
const statusEl = document.getElementById("status") as HTMLElement;
const diagnosticsEl = document.getElementById("diagnostics") as HTMLElement;
const toggleEl = document.getElementById("toggle-diagnostics") as HTMLButtonElement;
const socketStateEl = document.getElementById("socket-state") as HTMLElement;
const factsMoreEl = document.querySelector("#participants .facts-more") as HTMLDetailsElement;
const terminalEl = document.getElementById("terminal") as HTMLElement;
const diagnosticsCloseEl = document.getElementById("diagnostics-close") as HTMLButtonElement;
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
 * colour is the same one, so this circle and the one beside the name in the
 * panel still match — the panel draws it with the same function (#253).
 *
 * The screen person's own line is a bubble at the right edge (#185) and carries
 * neither the circle nor the name: where it stands and its tint already say
 * whose it is. Its clock is at its foot, and a run of them shows only the last
 * one's (src/styles.css, `.message.mine`).
 */
function roomLine(line: {
  speaker: string;
  /** The account it was said as, or null when none was declared — what the
   *  circle's image is looked up by (#236). */
  account: string | null;
  /** Said by the app itself: the circle carries the app's icon (#340). */
  app: boolean;
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
    article.appendChild(avatar(line.speaker, line.account, line.app));
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

  // What was attached is drawn rather than written out (#318): the paths under
  // `添付:` become pictures and chips below the words, and the words are what
  // is left. Only the screen reads it so; the post itself is unchanged.
  const { text, paths } = splitAttachments(line.content);
  const said: HTMLElement[] = [];
  if (text || !paths.length) {
    const body = document.createElement("div");
    body.className = "body";
    // ``` fences are drawn as code blocks (#348); the words around them as
    // before.
    appendPostWords(body, text);
    said.push(body);
  }
  if (paths.length) said.push(postAttachments(paths));

  if (line.mine) {
    // The clock goes to the bubble's foot, so a run of them can keep only the
    // last one's. A head is drawn only when it has an addressee to carry.
    if (head.childElementCount) article.appendChild(head);
    article.append(...said, time);
  } else {
    head.appendChild(time);
    article.append(head, ...said);
  }
  return article;
}

/**
 * The circle that says who a line is from (#225): the name's first character on
 * a tint of the speaker's colour, taken from `--speaker` on the element it
 * stands in. Hidden from a screen reader, which reads the name beside it.
 *
 * The account's image instead of the character, when the line names an account
 * that carries one (#236). The account rather than the name, because two
 * accounts may share a name and an image is one account's: a speaker with no
 * account, and an account with no image, keep the character. The circle
 * remembers which account it is for, so an image that arrives or changes after
 * the line was drawn reaches it (`setAvatarImage`).
 *
 * The app's icon instead, when the line is the app's own (#340). Decided by
 * the mark the room put on the post, never by the name: a participant may
 * answer to `Pullcept`, and their line keeps its character. The app speaks as
 * no account, so nothing repaints this circle afterwards.
 */
function avatar(name: string, accountId: string | null = null, app = false): HTMLElement {
  const mark = document.createElement("span");
  mark.className = "avatar";
  mark.setAttribute("aria-hidden", "true");
  mark.dataset.initial = initialOf(name);
  if (app) {
    mark.classList.add("app");
    drawAvatar(mark, APP_ICON_URL);
    return mark;
  }
  if (accountId) mark.dataset.account = accountId;
  paintAvatar(mark);
  return mark;
}

/**
 * The app's icon, as the app's own lines carry it (#340): the same drawing the
 * title bar shows and every size in src-tauri/icons/ is made from (#246), so
 * one picture is the app wherever it appears. Resolved here so the build
 * bundles it the way it bundles the title bar's.
 */
const APP_ICON_URL = new URL("../src-tauri/app-icon.svg", import.meta.url).href;

/** The character a circle carries for `name`: its first, upper-cased. */
function initialOf(name: string): string {
  return (Array.from(name.trim())[0] ?? "?").toUpperCase();
}

/** Draw a circle from the account it names: the image if there is one. */
function paintAvatar(mark: HTMLElement): void {
  const account = mark.dataset.account;
  drawAvatar(mark, account ? avatarImages.get(account) : undefined);
}

/**
 * Put `url`'s image in a circle, or its character when there is no image.
 *
 * An image that will not load is dropped for the character without a word
 * (#236): the file may have been removed or broken behind the app's back, and
 * the character is what the circle said before there was an image at all.
 */
function drawAvatar(mark: HTMLElement, url: string | undefined): void {
  const initial = () => {
    mark.classList.remove("image");
    mark.textContent = mark.dataset.initial ?? "?";
  };
  if (!url) {
    initial();
    return;
  }
  const image = document.createElement("img");
  image.alt = "";
  image.addEventListener("error", initial, { once: true });
  image.src = url;
  mark.classList.add("image");
  mark.replaceChildren(image);
}

/**
 * The images accounts carry, as object URLs by account id (#236).
 *
 * Read from the app once — when the config is read, and when an image is
 * picked — rather than with every line drawn: a room read back is many lines
 * from few speakers. An account is here only while its image is: one without an
 * image, one whose file would not read, and one deleted draw the character.
 */
const avatarImages = new Map<string, string>();

/**
 * Set or drop one account's image, and redraw every circle on the screen that
 * is that account's — lines drawn before the image arrived included.
 */
function setAvatarImage(accountId: string, image: Blob | null): void {
  const old = avatarImages.get(accountId);
  if (old) URL.revokeObjectURL(old);
  if (image) avatarImages.set(accountId, URL.createObjectURL(image));
  else avatarImages.delete(accountId);
  for (const mark of document.querySelectorAll<HTMLElement>(".avatar[data-account]")) {
    if (mark.dataset.account === accountId) paintAvatar(mark);
  }
}

/**
 * Read one account's image from the app (`account_avatar`). A file that is not
 * there or will not read leaves the account on its character, silently
 * (#236): the flag on the account is not corrected for it, and the next 選ぶ
 * writes the file again.
 */
async function loadAvatarImage(accountId: string): Promise<void> {
  let bytes: ArrayBuffer;
  try {
    bytes = await invoke<ArrayBuffer>("account_avatar", { accountId });
  } catch {
    setAvatarImage(accountId, null);
    return;
  }
  setAvatarImage(accountId, new Blob([bytes], { type: "image/png" }));
}

/** Read the image of every account that carries one (#236). */
function loadAvatarImages(): void {
  for (const account of accounts) {
    if (account.avatar) void loadAvatarImage(account.id);
  }
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

function foldOf(speaker: string, account: string | null | undefined): Fold | null {
  if (account) {
    const owner = accounts.find((one) => one.id === account);
    if (owner?.kind !== "mcp") return null;
    return {
      key: owner.id,
      account: owner.id,
      label: owner.name,
      colour: speakerColor(owner.name, owner.hue, false),
    };
  }
  if (speaker === LEGACY_NOTICE_SPEAKER) {
    return {
      key: `legacy:${speaker}`,
      account: null,
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
    summary.append(avatar(fold.label, fold.account), label);
    box.appendChild(summary);
    roomEl.appendChild(box);
  }
  box.appendChild(line);
  const count = box.querySelectorAll(":scope > .message").length;
  box.querySelector("summary > .label")!.textContent = `${fold.label} ${count} 件`;
}

/**
 * Whether the room is at its foot: within 40px of the bottom. One judgment for
 * two things — an arriving line follows the room down only from here, and the
 * button back to the newest line is shown only away from here (#243).
 */
function roomAtBottom(): boolean {
  return roomGliding || roomEl.scrollHeight - roomEl.scrollTop - roomEl.clientHeight < 40;
}

/** Send the room to its foot: gliding, or at once for a topic just opened. */
function glideRoomToFoot(behavior: "smooth" | "instant"): void {
  const distance = roomEl.scrollHeight - roomEl.scrollTop - roomEl.clientHeight;
  // Already there: no scroll happens, so nothing would ever end the glide.
  if (distance < 2) return;
  // `smooth` here is "whatever `#room` says": the stylesheet glides only where
  // motion is not reduced, and moves at once where it is.
  roomGliding = behavior === "smooth";
  roomEl.scrollTo({ top: roomEl.scrollHeight, behavior: behavior === "smooth" ? "auto" : "instant" });
}

/**
 * End the glide once it has landed, or once the person took the scroll over —
 * a wheel or a drag during a glide cancels it, and its `scrollend` comes where
 * they stopped.
 */
function settleGlide(event: Event): void {
  if (!roomGliding) return;
  const distance = roomEl.scrollHeight - roomEl.scrollTop - roomEl.clientHeight;
  if (event.type === "scrollend" || distance < 2) roomGliding = false;
}

/**
 * Show 最新の発言へ while the room is being read back, and hide it at the foot
 * (#243). Run wherever the distance to the foot can change: a scroll, the
 * room's own size, a line drawn, a fold opened, the conversation's text size.
 */
function syncScrollLatest(): void {
  scrollLatestEl.hidden = roomAtBottom();
}

/**
 * To the room's foot, where the conversation continues (#243). Gliding when
 * 最新の発言へ is pressed, at once when a topic is opened (#307).
 */
function scrollRoomToLatest(behavior: "smooth" | "instant"): void {
  glideRoomToFoot(behavior);
  syncScrollLatest();
}

function appendMessage(message: RoomMessage): void {
  // The room is scrolled to the bottom only when it already was, so reading
  // back through the log is not yanked away by an arriving message.
  const atBottom = roomAtBottom();

  const fold = foldOf(message.speaker, message.account);
  placeDay(message.ts);
  const line = roomLine({
    speaker: message.speaker,
    account: message.account,
    app: message.from_app,
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
  });
  // A line that has just been said fades in (#307). Only here: a topic opened
  // draws what was already said, and that is not arriving.
  line.classList.add("arriving");
  placeLine(line, fold);
  // On the glass, so it is what this screen can declare having seen. Own posts
  // included: the room does not hold a speaker's own posts against them, and
  // carrying the newest id either way keeps this one value rather than two.
  lastSeenId = message.message_id;
  drawnIds.add(message.message_id);

  // Gliding down, and only from the foot (#307): a person reading further up
  // is left where they are, as before (#243).
  if (atBottom) glideRoomToFoot("smooth");
  syncScrollLatest();
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
  forgetAttachmentUrls();
  roomDay = "";
  lastSeenId = posts[posts.length - 1]?.message_id ?? null;
  drawnIds.clear();

  for (const post of posts) {
    const fold = foldOf(post.speaker, post.account);
    placeDay(post.ts);
    placeLine(
      roomLine({
        speaker: post.speaker,
        account: post.account ?? null,
        // Read back as the app's when the log says it was (#340).
        app: post.from_app ?? false,
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

  // The permission prompts still waiting in this topic (#336). They are not
  // posts and are in no log, so they are put back after the log's lines.
  permissionEls.clear();
  for (const card of permissionCards.values()) {
    if (card.topic_id === shownTopicId()) roomEl.appendChild(permissionLine(card));
  }

  // Opened at the end, which is where the conversation continues. At once: a
  // topic opened is a place arrived at, not a line arriving (#307).
  roomGliding = false;
  scrollRoomToLatest("instant");
}

/** The prompts still waiting on the room, by id, in every topic. */
const permissionCards = new Map<string, PermissionCard>();
/** The cards drawn in the room on the glass, waiting or ended, by id. */
const permissionEls = new Map<string, HTMLElement>();

/** What an ended card says, by how it ended. */
const PERMISSION_OUTCOMES: Record<PermissionResolved["outcome"], string> = {
  deny: "部屋から「拒否」を返しました",
  allow: "部屋から「一度だけ許可」を返しました",
  always: "部屋から「常に許可」を返しました",
  elsewhere: "端末で答えたか、待ちが終わりました",
  closed: "セッションが答えを待つのをやめました",
  timeout: "部屋での受付を終えました。端末から答えてください",
};

/** The buttons, in the order Claude Desktop's card puts them. */
const PERMISSION_BUTTONS: { decision: "deny" | "always" | "allow"; label: string }[] = [
  { decision: "deny", label: "拒否" },
  { decision: "always", label: "常に許可" },
  { decision: "allow", label: "一度だけ許可" },
];

/**
 * One permission card in the room (#336), modelled on Claude Desktop's: who
 * asks, for which tool — an MCP tool as its server and its own name — what the
 * input says, and 拒否 / 常に許可 / 一度だけ許可. 常に許可 is drawn only when the
 * request carries a rule to add, and the rule is shown beside it.
 */
function permissionLine(card: PermissionCard): HTMLElement {
  const account = accounts.find((one) => one.id === card.account_id);
  const name = account?.name.trim() || card.account_id;
  const article = document.createElement("article");
  article.className = "message permission";
  article.dataset.permission = card.id;
  article.style.setProperty("--speaker", speakerColor(name, account?.hue ?? null, false));
  article.appendChild(avatar(name, card.account_id));

  const head = document.createElement("div");
  head.className = "meta";
  const speaker = document.createElement("span");
  speaker.className = "speaker";
  speaker.textContent = name;
  const kind = document.createElement("span");
  kind.className = "permission-kind";
  kind.textContent = card.agent_type ? `許可の確認・サブエージェント（${card.agent_type}）` : "許可の確認";
  const time = document.createElement("time");
  time.className = "ts";
  time.dateTime = card.at;
  time.textContent = shortTime(card.at);
  time.title = fullDateTime(card.at);
  head.append(speaker, kind, time);

  const body = document.createElement("div");
  body.className = "permission-body";
  const title = document.createElement("div");
  title.className = "permission-tool";
  title.title = card.tool_name;
  if (card.server) {
    const server = document.createElement("span");
    server.className = "permission-server";
    server.textContent = card.server;
    title.append(server, " / ");
  }
  const tool = document.createElement("strong");
  tool.textContent = card.tool;
  title.append(tool, " を使ってよいですか？");
  body.appendChild(title);

  if (card.fields.length) {
    const list = document.createElement("dl");
    list.className = "permission-input";
    for (const field of card.fields) {
      const term = document.createElement("dt");
      term.textContent = field.name;
      const value = document.createElement("dd");
      value.textContent = field.cut ? `${field.value}…（以下略）` : field.value;
      list.append(term, value);
    }
    body.appendChild(list);
  }
  if (card.more_fields > 0) {
    const more = document.createElement("div");
    more.className = "permission-note";
    more.textContent = `ほか ${card.more_fields} 項目は省きました`;
    body.appendChild(more);
  }
  if (card.can_always && card.always_rules.length) {
    const rules = document.createElement("div");
    rules.className = "permission-note";
    rules.textContent = `常に許可で足す規則：${card.always_rules.join("、")}`;
    body.appendChild(rules);
  }

  const actions = document.createElement("div");
  actions.className = "permission-actions";
  for (const button of PERMISSION_BUTTONS) {
    if (button.decision === "always" && !card.can_always) continue;
    const press = document.createElement("button");
    press.type = "button";
    press.className = `permission-${button.decision}`;
    press.textContent = button.label;
    press.addEventListener("click", (event) => void answerPermission(card, button.decision, article, event));
    actions.appendChild(press);
  }
  const state = document.createElement("div");
  state.className = "permission-state";
  state.textContent = "端末からも答えられます";
  body.append(actions, state);

  article.append(head, body);
  permissionEls.set(card.id, article);
  return article;
}

/** Turn a card into its ended form: no buttons, and how it ended. */
function closePermissionLine(article: HTMLElement, text: string): void {
  article.classList.add("ended");
  article.querySelector(".permission-actions")?.remove();
  const state = article.querySelector(".permission-state");
  if (state) state.textContent = text;
}

/**
 * Send the person's press to the held hook. Only this screen can: the command
 * is the webview's, and the room reads no post as an answer (#336). What is
 * said back is what the room sent, not what the session did — the terminal
 * may have answered first.
 *
 * How the click reached the button goes with it, for the app's log only
 * (#346): the pointer's kind (empty for Enter or Space on a focused button),
 * the click count (0 for a key) and whether the browser made the event.
 */
async function answerPermission(
  card: PermissionCard,
  decision: "deny" | "always" | "allow",
  article: HTMLElement,
  event: MouseEvent,
): Promise<void> {
  const buttons = article.querySelectorAll<HTMLButtonElement>(".permission-actions button");
  for (const button of buttons) button.disabled = true;
  const press = {
    pointer_type: "pointerType" in event ? String((event as PointerEvent).pointerType ?? "") : "",
    detail: event.detail,
    trusted: event.isTrusted,
  };
  let delivered: boolean;
  try {
    delivered = await invoke<boolean>("permission_answer", { id: card.id, decision, press });
  } catch (err) {
    for (const button of buttons) button.disabled = false;
    status(`許可の答えを送れませんでした: ${err}`, "error");
    return;
  }
  if (!delivered) {
    permissionCards.delete(card.id);
    closePermissionLine(article, "もう答えられません。端末で答えたか、待ちが終わりました");
  }
}

/** A prompt arrived to be held for the room: draw it if its topic is shown. */
function showPermission(card: PermissionCard): void {
  if (permissionCards.has(card.id)) return;
  permissionCards.set(card.id, card);
  if (card.topic_id !== shownTopicId()) return;
  const atBottom = roomAtBottom();
  const line = permissionLine(card);
  line.classList.add("arriving");
  roomEl.appendChild(line);
  if (atBottom) glideRoomToFoot("smooth");
  syncScrollLatest();
}

/** A held prompt ended, however it did: its card stops being pressable. */
function endPermission(resolved: PermissionResolved): void {
  permissionCards.delete(resolved.id);
  const article = permissionEls.get(resolved.id);
  if (!article) return;
  closePermissionLine(article, PERMISSION_OUTCOMES[resolved.outcome] ?? "受付を終えました");
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
  return [...sessions.allSeats()].some((seat) => seat.topic_id === topicId);
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
  if (leaving !== "") sessions.rememberSelection(leaving);
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
    held = [...sessions.allSeats()];
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
    for (const view of [...sessions.allViews()]) {
      if (view.topicId === topic.topic_id) discardView(view);
    }
    if (moved) {
      await enterTopic(moved);
      drawTopic([]);
    }
    // After the move, which records what the topic being left was showing.
    rosters.delete(topic.topic_id);
    awaiting.delete(topic.topic_id);
    sessions.forgetSelection(topic.topic_id);
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
      from_app: one.from_app ?? false,
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

/** Which group a row falls in, and the heading it is drawn under. */
const GROUPS: { kind: AccountKind | "guest"; label: string }[] = [
  { kind: "admin", label: "admin" },
  // One heading per launched kind (#156). A group with nobody in it is not
  // drawn, so a screen whose accounts are all one kind reads as it did before
  // the split — the second heading appears when a second kind does.
  { kind: "claude_code", label: "Claude Code" },
  { kind: "codex_cli", label: "Codex CLI" },
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
 * The word a row in the room says about what its session is doing, or no word
 * when the row says something else or nothing (#82).
 *
 * The branches `memberRow` takes before this one are taken here first, in its
 * order: a row that is oneself, a server, or a launch not back yet does not say
 * this. The
 * row's badge and the line under the room (`renderRoomBusy`, #307) both read it
 * here, so the line cannot name a seat the badge does not.
 */
function rowActivity(row: Member): RowWord {
  if (!row.participant || row.participant.own) return NO_WORD;
  if (row.account?.kind === "mcp" && mcpServerOf(row.account)) return NO_WORD;
  const view = row.account ? sessions.getView(seatKey(shownTopicId(), row.account.id)) : undefined;
  if (view != null && view.ended === null && view.ptyId === "") return NO_WORD;
  return activityNote(view, awaiting.get(view?.topicId ?? "")?.has(memberName(row)) ?? false);
}

/**
 * A pulse's start set back to the page's own clock, so a badge or a line drawn
 * anew mid-pulse carries on in the phase the one it replaced was in, rather than
 * every redraw of the panel restarting the pulse (#307).
 */
function pulsePhase(): string {
  return `${-Math.round(performance.now())}ms`;
}

/** What the line under the room says now, so an unchanged one is not redrawn. */
let roomBusyKey = "";

/**
 * The line under the room naming who is 考え中… or 出力中 now (#307):
 * 「Claude Lin が考え中…」, and the next one after 、.
 *
 * The same word, from the same reading, as each row's badge (`rowActivity`) —
 * this line adds no signal of its own and no word the badge does not say. It
 * keeps its row whether or not it says anything, so the room above it does not
 * grow and shrink with every burst of output. Redrawn only when what it says
 * changes, so its dots keep moving through the panel's redraws.
 */
function renderRoomBusy(rows: Member[]): void {
  const busy = rows.flatMap((row) => {
    const said = rowActivity(row);
    if (!said.kind) return [];
    const name = memberName(row);
    return [{ name, word: said.line, colour: speakerColor(name, row.participant!.hue, false) }];
  });
  const key = busy.map((one) => `${one.name}\u0000${one.word}\u0000${one.colour}`).join("\u0001");
  if (key === roomBusyKey) return;
  roomBusyKey = key;
  roomBusyEl.replaceChildren();
  if (!busy.length) return;

  const dots = document.createElement("span");
  dots.className = "busy-dots";
  dots.setAttribute("aria-hidden", "true");
  for (let i = 0; i < 3; i += 1) dots.appendChild(document.createElement("i"));
  dots.style.setProperty("--pulse-phase", pulsePhase());
  roomBusyEl.appendChild(dots);

  const text = document.createElement("span");
  text.className = "busy-text";
  busy.forEach((one, index) => {
    if (index > 0) text.append("、");
    const who = document.createElement("span");
    who.className = "who";
    who.style.setProperty("--speaker", one.colour);
    who.textContent = one.name;
    text.append(who, `が${one.word}`);
  });
  roomBusyEl.appendChild(text);
}

/**
 * Draw one line of the participant list.
 *
 * The colour is the one that participant's lines carry in the room, which is
 * what makes the panel a legend for the conversation rather than a second copy
 * of the same names. It is on the name itself here, not only on the circle.
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
  const view = row.account ? sessions.getView(seatKey(shownTopicId(), row.account.id)) : undefined;
  // The server an `mcp` account is. It takes no seat, so what says it is here
  // is the server running, not the roster (#193).
  const server = row.account?.kind === "mcp" ? mcpServerOf(row.account) : null;

  const entry = document.createElement("li");
  entry.className = "member";
  entry.style.setProperty("--speaker", speakerColor(name, hue, own));
  if (!row.participant && server?.view?.state?.state !== "running") {
    entry.classList.add("offline");
  }

  // The circle the room draws for this speaker, drawn by the same function so
  // the two cannot drift (#253): the account's image where it has one, the
  // initial on its colour otherwise. The account is the seat's when the row is
  // in the room — what the room's lines carry — and the list's otherwise; a
  // guest declared none and keeps the initial, as its lines do. It replaces the
  // dot and takes over its part as the legend: the circle is the line's colour.
  const mark = avatar(name, row.participant ? row.participant.account : (row.account?.id ?? null));

  const who = document.createElement("span");
  who.className = "who";
  who.textContent = name;
  who.title = name;

  // Asked for and not back yet. The view exists from the moment 開始 is pressed
  // and its pty id arrives with the session, so this pair is exactly that
  // window (`startSession`).
  const launching = view != null && view.ended === null && view.ptyId === "";
  const failure = row.account
    ? sessions.launchFailure(seatKey(shownTopicId(), row.account.id))
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
    const said = rowActivity(row);
    noteText = said.word;
    // 待機 and 制限中 both stand where an utterance has ended, so both stay the
    // ground colour; the coloured words are the ones saying one is still under
    // way (#148 / #149), or stopped on the person (#326).
    noteKind = said.kind;
    // A Codex app-server seat's longer form, when the badge had to be shorter
    // than it (#326): ツール on the badge, the tool's name here.
    if (said.line !== said.word) noteTitle = said.line;
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
    button.setAttribute("aria-pressed", String(view.accountId === sessions.selectedAccount()));
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
    if (view.accountId === sessions.selectedAccount()) entry.classList.add("shown");
    pick = button;
  } else {
    pick = document.createElement("div");
    pick.className = "pick static";
  }
  pick.append(mark, who);
  if (noteText) {
    const note = document.createElement("span");
    note.className = "note";
    note.textContent = noteText;
    // A state is a badge, tinted by its kind (#225). 「（あなた）」 is not a state
    // but who the row is, and stays plain beside the name.
    if (!own) note.classList.add("badge");
    if (noteKind) note.dataset.kind = noteKind;
    // The two busy words pulse (#307), in one phase across redraws.
    if (noteKind === "active") note.style.setProperty("--pulse-phase", pulsePhase());
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
  // Two fixed columns: the session's lifecycle, then the menu. The first holds
  // 開始 on a row that is not running and 終了 in the same place on one that is
  // (#239): the two ends of one session stand where each other stood. Both are
  // in the menu as well (`openAccountMenu`), beside 端末を開く, 編集 and
  // 色を変える. The slot is emitted whether or not it holds a button, so a row
  // with neither keeps the column open rather than sliding its menu left of
  // every other row's.
  const lifecycle = document.createElement("span");
  lifecycle.className = "lifecycle";
  const running = view != null && view.ended === null;
  if (row.account && running && !launching) {
    // Only once the launch has returned an id: a kill aimed at an empty id
    // reports success having done nothing (#57).
    lifecycle.appendChild(stopButton(row.account));
  } else if (row.account && launches(row.account)) {
    // A launched kind only. An `admin` account is a person and there is no CLI
    // under a person to spawn; `start_session` refuses one and that refusal is
    // the authority, but a refusal is the wrong way for the person to find out
    // (#59). Their row keeps the empty column, and their menu with it. The
    // launch that has not come back yet keeps the pressed button, dead, on the
    // row that was pressed (#62).
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
 * 終了 on the row (#239), in the column 開始 stands in on a row that is not
 * running. It asks, as 終了 in the menu does: the click opens `#end-dialog`
 * and nothing ends until that is answered (#71).
 */
function stopButton(account: Account): HTMLButtonElement {
  const stop = document.createElement("button");
  stop.type = "button";
  stop.className = "stop";
  stop.appendChild(icon("stop"));
  const label = `${account.name} のセッションを終了する`;
  stop.title = label;
  stop.setAttribute("aria-label", label);
  stop.addEventListener("click", () => endFromMenu(account.id));
  return stop;
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
 * Open one account's menu at a point: 端末を開く, 編集, 色を変える, and 開始 or 終了.
 *
 * An item that has nothing to act on is left out, not greyed: 端末を開く on an
 * account with no terminal in this topic (the row has no terminal operation
 * then either, #57), 開始 on an account that is running or is not a launched
 * kind (#59), and 終了 on an account with no session to end. So 開始 and 終了
 * are never both drawn, and each is drawn exactly when the row's first column
 * holds it as a live button (#239). While a launch is out neither is: 開始 has
 * been pressed, and 終了 is left out before the launch has returned an id — a
 * kill aimed at an empty id reports success having done nothing (#57).
 *
 * 終了 is the one item that cannot be taken back, so it stands apart below a
 * divider in the danger colour, last, where a slip down the list does not land
 * on it. It opens the same question it always did (`#end-dialog`); the menu
 * moves where it is asked from, not whether it is asked.
 *
 * 開始 takes the same place, below the same divider and last (#241): the two
 * ends of one session stand where each other stood, as they do in the row's
 * first column. Only the place is shared, not the danger colour — 開始 can be
 * taken back by the 終了 that replaces it. With neither drawn, while a launch
 * is out, the divider is left out too, having nothing below it.
 *
 * 色を変える is not a colour picker of its own. The colour is a field of the
 * account's form and nothing else writes it, so the item opens that form on
 * that field (`openAccountDialog`).
 */
function openAccountMenu(account: Account, at: { x: number; y: number }): void {
  const view = sessions.getView(seatKey(shownTopicId(), account.id));
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
  const canStart = launches(account) && (!view || view.ended !== null);
  const canEnd = view != null && view.ended === null && view.ptyId !== "";
  if (canStart || canEnd) {
    const divider = document.createElement("div");
    divider.setAttribute("role", "separator");
    accountMenuEl.appendChild(divider);
  }
  if (canStart) {
    item("開始", icon("start"), () => startFromMenu(account.id));
  }
  if (canEnd) {
    item("終了", icon("stop"), () => endFromMenu(account.id)).classList.add("danger");
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
  const view = sessions.getView(seatKey(shownTopicId(), accountId));
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

/** 開始 from the menu: the row's ▶, for the account as it is now. */
function startFromMenu(accountId: string): void {
  const account = accounts.find((one) => one.id === accountId);
  if (!account || !launches(account)) return;
  // The session may have been started, from the row or another menu, while
  // the menu stood. `startSession` refuses a held seat as well; this keeps the
  // press from reaching that refusal.
  const view = sessions.getView(seatKey(shownTopicId(), accountId));
  if (view && view.ended === null) return;
  void startSession(account);
}

/**
 * 終了 from the row or the menu: the question `#end-dialog` asks, about the
 * session as it is now.
 */
function endFromMenu(accountId: string): void {
  const key = seatKey(shownTopicId(), accountId);
  const view = sessions.getView(key);
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
  // The line under the room reads the same rows, at the same moments (#307).
  renderRoomBusy(rows);

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
  sessions.receiveRoster(topicId, joined);
  if (topicId === shownTopicId()) renderPanel();
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
      // The initial, until an image is picked on the form (#236).
      avatar: false,
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
  // Read on the caret's line (#354): the box's text is lines and code blocks,
  // and an `@` is looked for in the one the caret is in, up to the caret.
  const { from, to, lineStart, before } = composer.selection();
  if (from !== to) return null;
  const at = Math.max(before.lastIndexOf("@"), before.lastIndexOf("＠"));
  if (at < 0) return null;
  if (at > 0 && !/\s/.test(before[at - 1])) return null;
  const query = before.slice(at + 1);
  if (query.includes("\n")) return null;
  return { at: lineStart + at, query };
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
  const { from, to, before } = composer.selection();
  const at = before === "" || /\s$/.test(before) ? "@" : " @";
  composer.replace(from, to, at);
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
    // mousedown rather than click: by the time a click lands the box has
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
  composer.replace(mentionAt, composer.selection().from, `@${one.name} `);
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

let attachments: Attachment[] = [];

/** Let go of the pictures the chips that are gone were showing. */
function dropPreviews(gone: Attachment[]): void {
  for (const one of gone) {
    if (one.preview) URL.revokeObjectURL(one.preview);
    one.preview = null;
  }
}

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
      preview: imageType(file.name || "image.png") ? URL.createObjectURL(file) : null,
    })),
  );
}

function attachPaths(paths: string[]): void {
  attach(
    paths.map((path) => ({ name: baseName(path), source: { path }, saved: null, preview: null })),
  );
}

function renderAttachments(): void {
  attachmentsEl.replaceChildren();
  attachments.forEach((one, at) => {
    const chip = document.createElement("li");
    let name: HTMLElement;
    if (one.preview) {
      // A picture in place of the name (#318), opened larger by a press. The
      // name is still its label and its tooltip.
      const url = one.preview;
      chip.classList.add("image");
      const thumb = document.createElement("button");
      thumb.type = "button";
      thumb.className = "thumb";
      thumb.setAttribute("aria-label", `${one.name} を大きく見る`);
      const image = document.createElement("img");
      image.alt = "";
      image.src = url;
      thumb.appendChild(image);
      // The box keeps its focus through the press, so closing the larger
      // picture hands the focus back to it.
      thumb.addEventListener("mousedown", (event) => event.preventDefault());
      thumb.addEventListener("click", () => openViewer(url, one.name));
      name = thumb;
    } else {
      name = document.createElement("span");
      name.className = "name";
      name.textContent = one.name;
    }
    name.title = one.name;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.setAttribute("aria-label", `${one.name} を外す`);
    remove.title = "外す";
    remove.appendChild(icon("close"));
    // The box keeps its focus and caret through the press, as it does
    // for 宛先.
    remove.addEventListener("mousedown", (event) => event.preventDefault());
    remove.addEventListener("click", () => {
      dropPreviews([one]);
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
  const block = [ATTACHMENT_HEAD, ...paths].join("\n");
  return text ? `${text}\n\n${block}` : block;
}

/** The line a post's attachments are listed under (`withAttachments`). */
const ATTACHMENT_HEAD = "添付:";

/**
 * Where this room's attachments are saved, as the paths in the posts spell it
 * (`room_attachments_dir`, #318). Null until it is read, and when it could not
 * be: then every post is drawn as its text.
 */
let attachmentsRoot: string | null = null;

/**
 * A post's words and the attachments it lists, read back out of its text
 * (#318): the `添付:` block `withAttachments` put at its end.
 *
 * Read as attachments only when every line of the block is a path inside this
 * room's attachments folder. A block naming anything else — an AI writing out
 * a path of its own, a post quoting one — is left as the text it is, so a path
 * the screen will not draw is not hidden either. This decides how the text is
 * drawn and nothing more: what may be loaded is decided again by the app, on
 * the resolved path (`room_attachment`).
 */
function splitAttachments(content: string): { text: string; paths: string[] } {
  const asText = { text: content, paths: [] };
  const root = attachmentsRoot;
  if (!root) return asText;
  const marker = `\n\n${ATTACHMENT_HEAD}\n`;
  const at = content.lastIndexOf(marker);
  let text: string;
  let block: string;
  if (at >= 0) {
    text = content.slice(0, at);
    block = content.slice(at + marker.length);
  } else if (content.startsWith(`${ATTACHMENT_HEAD}\n`)) {
    text = "";
    block = content.slice(ATTACHMENT_HEAD.length + 1);
  } else {
    return asText;
  }
  // What was written inside a ``` fence stays code (#348): a `添付:` block that
  // an unclosed fence runs over is part of the code, not attachments.
  const head = at >= 0 ? text.split("\n").length + 1 : 0;
  if (lineInFence(findCodeFences(content.split("\n")), head)) return asText;
  const paths = block.split("\n");
  const inside = (path: string) =>
    path.length > root.length + 1 &&
    path.startsWith(root) &&
    (path[root.length] === "\\" || path[root.length] === "/");
  if (!paths.every(inside)) return asText;
  return { text, paths };
}

/**
 * A post's words into its body (#348): prose as it has always been drawn, each
 * fenced block as code. Text only — nothing is read as HTML.
 */
function appendPostWords(body: HTMLElement, text: string): void {
  for (const piece of splitCodeFences(text)) {
    if (piece.kind === "text") appendProse(body, piece.text);
    else body.appendChild(codeBlock(piece.code, piece.lang));
  }
}

/**
 * Words outside a fence: text, with its URLs and paths as links (#349). The one
 * place prose is put into a post's body; code blocks do not pass through it, so
 * nothing inside a fence is a link.
 */
function appendProse(parent: HTMLElement, text: string): void {
  parent.append(...linkifyText(text));
}

/**
 * One fenced block (#348): the code in monospace, its line breaks and spaces as
 * written, inside a frame. The language, when the fence names one, is a small
 * label over the code and never part of it.
 */
function codeBlock(code: string, lang: string): HTMLElement {
  const block = document.createElement("div");
  block.className = "code-block";
  if (lang) {
    const label = document.createElement("div");
    label.className = "code-lang";
    label.textContent = lang;
    block.appendChild(label);
  }
  const pre = document.createElement("pre");
  const inner = document.createElement("code");
  // Coloured as the input box colours it (#354): the fence's language, or a guess.
  inner.append(...highlightedCode(code, lang));
  pre.appendChild(inner);
  block.appendChild(pre);
  return block;
}

/**
 * Where a link may begin, and how far it runs before its end is trimmed
 * (#349): an `http://` / `https://` URL, a drive path (`C:\…`, `D:/…`) or a
 * UNC path (`\\server\share\…`). A relative path is not a link, and no other
 * scheme is.
 *
 * A link is one run of text: it stops at whitespace, at what a path cannot
 * hold, at a backquote (a path written as code), and at the Japanese
 * punctuation that closes a phrase — 、。「」『』【】 — which a name almost
 * never carries and a sentence written straight after a path always does.
 * Other full-width characters are part of a path. Not preceded by a letter or
 * a digit, so a drive path is not found inside a word, nor inside a URL or a
 * longer path.
 */
const LINK_PATTERN =
  /(?<![\w])https?:\/\/[^\s<>"`、。「」『』【】]+|(?<![\w/\\])[a-z]:[\\/][^\s<>"|*?:`、。「」『』【】]*|(?<![\w/\\])\\\\[^\s\\/<>"|*?:`、。「」『』【】]+\\[^\s<>"|*?:`、。「」『』【】]+/gi;

/** What a link does not end on: the punctuation after it in a sentence. */
const LINK_TRAILING = new Set([...".,;:!?'\"*>。、，．：；！？〉》」』】"]);

/** A closing bracket, by the opening one it is kept with. */
const LINK_CLOSERS: Record<string, string> = { ")": "(", "）": "（", "]": "[", "］": "［", "}": "{", "｝": "｛" };

/**
 * A link with the punctuation after it taken off (#349). A closing bracket is
 * taken off only when it closes nothing in the link, so `…/Foo_(bar)` keeps its
 * own and `（C:\資料）` gives back the sentence's.
 */
function trimLinkEnd(candidate: string): string {
  let end = candidate.length;
  while (end > 0) {
    const last = candidate[end - 1];
    if (LINK_TRAILING.has(last)) {
      end--;
      continue;
    }
    const open = LINK_CLOSERS[last];
    if (!open) break;
    const kept = candidate.slice(0, end);
    if (kept.split(open).length >= kept.split(last).length) break;
    end--;
  }
  return candidate.slice(0, end);
}

/**
 * The words of a post as text and links, in order (#349): the pure half of
 * `linkifyText`. Joined back, the pieces are `text` exactly.
 */
function linkPieces(text: string): TextPiece[] {
  const pieces: TextPiece[] = [];
  const say = (kind: TextPiece["kind"], part: string) => {
    if (!part) return;
    const last = pieces[pieces.length - 1];
    if (kind === "text" && last?.kind === "text") last.text += part;
    else pieces.push({ kind, text: part });
  };
  let from = 0;
  for (const match of text.matchAll(LINK_PATTERN)) {
    const at = match.index ?? 0;
    const link = trimLinkEnd(match[0]);
    const url = /^https?:\/\//i.test(link);
    // Trimmed down to no host, or to a server with no share, it is no link;
    // nor is `\\.\`, which names a device and not a server.
    const whole = url ? /^https?:\/\/[^/\\]/i.test(link) : !link.startsWith("\\\\") || /^\\\\(?!\.\\)[^\\]+\\[^\\]/.test(link);
    if (!whole) continue;
    say("text", text.slice(from, at));
    say(url ? "url" : "path", link);
    from = at + link.length;
  }
  say("text", text.slice(from));
  return pieces;
}

/**
 * A post's words as the nodes the screen draws them with (#349): text as text,
 * and every URL and Windows path as a link a press opens — a URL in the
 * default browser, a folder in Explorer, a file in the app its type opens
 * with. What is opened and how is decided again by the app
 * (`open_post_url` / `open_post_path`), since a post is anyone's writing.
 *
 * Takes any run of plain text and gives it back unchanged but for the links,
 * newlines and spaces included, so a part of a post drawn some other way can
 * pass the rest of its words through here.
 *
 * A press is a click and nothing else. A drag that selects text across a link
 * opens nothing.
 */
function linkifyText(text: string): Node[] {
  return linkPieces(text).map((piece) => {
    if (piece.kind === "text") return document.createTextNode(piece.text);
    const link = document.createElement("a");
    link.className = "link";
    link.setAttribute("role", "link");
    link.textContent = piece.text;
    link.addEventListener("click", () => {
      if (!(window.getSelection()?.isCollapsed ?? true)) return;
      openLink(piece);
    });
    return link;
  });
}

/** Open what a link names, saying on the status line when it cannot (#349). */
async function openLink(piece: TextPiece): Promise<void> {
  try {
    if (piece.kind === "url") await invoke("open_post_url", { url: piece.text });
    else await invoke("open_post_path", { path: piece.text });
  } catch (err) {
    status(String(err), "error");
  }
}

/** What a file is called after its last dot, lower-cased; empty without one. */
function extensionOf(name: string): string {
  const at = name.lastIndexOf(".");
  return at > 0 ? name.slice(at + 1).toLowerCase() : "";
}

/** The pictures drawn as pictures (#318), by extension, with their types. */
const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
};

/** The type a file is drawn as a picture under, or undefined for any other. */
function imageType(name: string): string | undefined {
  return IMAGE_TYPES[extensionOf(name)];
}

/**
 * The pictures of the attachments on the glass, as object URLs by path (#318).
 * Read once per path rather than once per line: a post drawn again draws the
 * same picture. A path that would not read is kept as null, and its line shows
 * the file's chip. Emptied when a topic is drawn afresh (`drawTopic`), which
 * takes every line that was showing one.
 */
const attachmentUrls = new Map<string, Promise<string | null>>();

function attachmentUrl(path: string, type: string): Promise<string | null> {
  let url = attachmentUrls.get(path);
  if (!url) {
    url = invoke<ArrayBuffer>("room_attachment", { path }).then(
      (bytes) => URL.createObjectURL(new Blob([bytes], { type })),
      () => null,
    );
    attachmentUrls.set(path, url);
  }
  return url;
}

function forgetAttachmentUrls(): void {
  for (const url of attachmentUrls.values()) {
    void url.then((one) => {
      if (one) URL.revokeObjectURL(one);
    });
  }
  attachmentUrls.clear();
}

/** A post's attachments, in the order its block lists them (#318). */
function postAttachments(paths: string[]): HTMLElement {
  const list = document.createElement("div");
  list.className = "attachments";
  for (const path of paths) {
    const name = baseName(path);
    const type = imageType(name);
    list.appendChild(type ? postImage(path, name, type) : fileChip(name));
  }
  return list;
}

/** A file that is not drawn as a picture: its name, and its type beside it. */
function fileChip(name: string): HTMLElement {
  const chip = document.createElement("span");
  chip.className = "file";
  chip.title = name;
  const label = document.createElement("span");
  label.className = "name";
  label.textContent = name;
  const kind = document.createElement("span");
  kind.className = "kind";
  const ext = extensionOf(name);
  kind.textContent = ext ? ext.toUpperCase() : "ファイル";
  chip.append(label, kind);
  return chip;
}

/**
 * An attached picture, small, opened larger by a press (#318). The tile has
 * its full size before the picture arrives, so a line does not grow under the
 * reader when it does. A picture that will not read or will not decode gives
 * way to the file's chip, without a word.
 */
function postImage(path: string, name: string, type: string): HTMLElement {
  const thumb = document.createElement("button");
  thumb.type = "button";
  thumb.className = "thumb";
  thumb.title = name;
  thumb.setAttribute("aria-label", `${name} を大きく見る`);
  const image = document.createElement("img");
  image.alt = "";
  thumb.appendChild(image);
  const giveWay = () => thumb.replaceWith(fileChip(name));
  void attachmentUrl(path, type).then((url) => {
    if (!url) {
      giveWay();
      return;
    }
    image.addEventListener("error", giveWay, { once: true });
    image.src = url;
  });
  thumb.addEventListener("click", () => {
    if (image.src) openViewer(image.src, name);
  });
  return thumb;
}

/** Open a picture larger, over the dimmed window (#318). */
function openViewer(url: string, name: string): void {
  viewerImageEl.src = url;
  viewerImageEl.alt = name;
  viewerEl.setAttribute("aria-label", name);
  if (!viewerEl.open) viewerEl.showModal();
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
  const text = composer.text().trim();
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
  composer.setText("");
  closeMentions();
  // Off the composer at once with the text, so a second Enter while the files
  // are being saved does not send them twice. Put back with it below on
  // anything but a delivery.
  attachments = [];
  renderAttachments();
  const putBack = (): void => {
    composer.setText(text);
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
    // Delivered: the chips are gone for good, and their pictures with them.
    dropPreviews(pending);
    status("");
  } catch (err) {
    // Put the text back rather than losing what was typed.
    putBack();
    status(`発言を送れませんでした: ${err}`, "error");
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
  const view = sessions.getView(key);
  if (view) void endSession(view);
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
    held = [...sessions.allSeats()];
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
 * The window's three buttons on the title bar (#252), which stand where the
 * standard frame's stood until the frame was taken off.
 *
 * 閉じる is `close`, not `destroy`. `close` raises the same `CloseRequested`
 * the frame's ✕ raised (tauri-runtime-wry 2.11.4, `on_close_requested`;
 * `@tauri-apps/api/window` says so of `close`), so it lands in
 * `onQuitRequested` above and the question about running sessions is asked
 * exactly as before. `destroy` would take the window without asking.
 *
 * 最大化 is one button with two names. Its name and drawing are read back from
 * the window whenever the window's size changes, rather than flipped on the
 * click: a window is also maximized by a double click on the bar, by Win+↑ and
 * by dragging it to the top of the screen, and none of those passes through
 * this button.
 *
 * A call the window refuses is said on the status line. Nothing else would show
 * it: the click is the only witness, and it already happened.
 */
async function wireWindowControls(): Promise<void> {
  const appWindow = getCurrentWindow();
  const minimizeEl = document.getElementById("window-minimize") as HTMLButtonElement;
  const maximizeEl = document.getElementById("window-maximize") as HTMLButtonElement;
  const closeEl = document.getElementById("window-close") as HTMLButtonElement;

  const refused = (err: unknown) => status(`窓を操作できませんでした: ${err}`, "error");
  minimizeEl.addEventListener("click", () => void appWindow.minimize().catch(refused));
  maximizeEl.addEventListener("click", () => void appWindow.toggleMaximize().catch(refused));
  closeEl.addEventListener("click", () => void appWindow.close().catch(refused));

  const showMaximized = async () => {
    const maximized = await appWindow.isMaximized();
    const label = maximized ? "元に戻す" : "最大化";
    maximizeEl.setAttribute("aria-label", label);
    maximizeEl.title = label;
    maximizeEl.replaceChildren(icon(maximized ? "restore" : "maximize"));
  };
  // Caught here rather than left to the caller: this runs early in start-up,
  // and a throw would stop everything after it for the sake of one tooltip.
  // The three buttons above are already wired by now and work regardless.
  try {
    await appWindow.onResized(() => void showMaximized().catch(refused));
    await showMaximized();
  } catch (err) {
    refused(err);
  }
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

async function main(): Promise<void> {
  // Before anything else is drawn, so no button is ever on screen empty.
  fillIcons();

  // The pane is one container holding every session's terminal, so the observer
  // is on the container and the fit lands on whichever one is showing.
  new ResizeObserver(() => fitShown()).observe(terminalEl);

  // 最新の発言へ (#243): shown only while the room is read back from its foot.
  // `toggle` does not bubble, so a fold opened in the room is caught on the way
  // down.
  roomEl.addEventListener(
    "scroll",
    (event) => {
      settleGlide(event);
      syncScrollLatest();
    },
    { passive: true },
  );
  roomEl.addEventListener("scrollend", (event) => {
    settleGlide(event);
    syncScrollLatest();
  });
  roomEl.addEventListener("toggle", syncScrollLatest, true);
  new ResizeObserver(() => syncScrollLatest()).observe(roomEl);
  // Pressed without taking the focus, as the composer's buttons are: what is
  // being written stays where it was. The button goes once pressed, so a press
  // from the keyboard hands the focus to the text rather than to nothing.
  scrollLatestEl.addEventListener("mousedown", (event) => event.preventDefault());
  scrollLatestEl.addEventListener("click", () => {
    const hadFocus = document.activeElement === scrollLatestEl;
    scrollRoomToLatest("smooth");
    if (hadFocus) composer.focus();
  });
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
  // Right after the close is held, for the same reason: the window has no
  // frame of its own (#252), so these are the pointer's only way to minimize
  // or close it, and 閉じる needs the listener above to be the close that
  // asks.
  await wireWindowControls();

  // The account's colour is the account's, so nothing is restored into this
  // picker — the form fills it from whichever account it was opened on.
  accountDialog.initializeHue();

  // Restored before anything is drawn, so the first line to arrive is already
  // at the size this screen reads at rather than jumping once it lands.
  //
  // The UI's multiple goes first: the conversation's size is written against it.
  displaySettings.initialize();

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
  // A Claude Code seat's permission prompt held for the room's answer, and
  // its end (#336). Screen-only: neither is a post.
  await listen<PermissionCard>("permission-request", (event) => showPermission(event.payload));
  await listen<PermissionResolved>("permission-resolved", (event) => endPermission(event.payload));
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
  await listen<SessionStats>("session-stats", (event) => sessions.receiveStats(event.payload));
  // A Codex app-server seat (#326) or a Claude Code seat's hooks (#331) said
  // what it is doing. Keyed on the seat
  // like the report above, dropped for a seat this screen has no terminal for,
  // and drawn only when the word, its longer form, the thread's status (#329)
  // or the connection changed —
  // the server sends one of these on each change, not on every notification.
  await listen<SeatActivity>("seat-activity", (event) => sessions.receiveActivity(event.payload));
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
  displaySettings.wireDialog();

  // ── the local MCP servers (#172 / #193) ─────────────────────────────────────
  //
  // Read again whenever a server moves — a state, a log line — whether or not a
  // window is open: each server's row says where its run is.
  await accountDialog.wireMcp();

  // The one thing that draws a topic boundary, and the head of the list it
  // appears in (#125, 決定1). `renderTopics` is what draws it as picked; this is
  // only the press.
  topicNewEl.addEventListener("click", () => void startNewTopic());

  sendEl.addEventListener("click", () => void send());
  // mousedown is stopped so the box keeps its focus and its caret through
  // the press; the click is what types.
  mentionEl.addEventListener("mousedown", (event) => event.preventDefault());
  mentionEl.addEventListener("click", () => typeMention());
  // The box's own keys, its changes and its blur are wired where it is made
  // (`composer`, #354).

  // ── attachments (#223) ──────────────────────────────────────────────────────
  //
  // The larger picture (#318) closes on its ✕ and on a press anywhere around
  // the picture, which is the dim; Esc closes it as it closes any modal dialog.
  viewerEl.addEventListener("click", (event) => {
    if (event.target !== viewerImageEl) viewerEl.close();
  });
  viewerEl.addEventListener("close", () => viewerImageEl.removeAttribute("src"));
  //
  // 添付 opens the system's file dialog through the hidden picker. The picker
  // is emptied after each choice, so picking the same file again is a change.
  attachEl.addEventListener("mousedown", (event) => event.preventDefault());
  attachEl.addEventListener("click", () => attachInputEl.click());
  attachInputEl.addEventListener("change", () => {
    if (attachInputEl.files) attachFiles(attachInputEl.files);
    attachInputEl.value = "";
    composer.focus();
  });
  // Ctrl+V of an image is the box's paste (`composer`, `onPasteFiles`).
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
        composer.focus();
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
  accountDialog.wireNewAccount();

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
  sessions.wireSessionIdCopy();
  accountDialog.wireForm();

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
    // Not awaited: a line drawn before its account's image has arrived is
    // redrawn when it does (#236).
    loadAvatarImages();
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
  // Where the attachments are, before any line is drawn: a post's `添付:` block
  // is read against it (#318). Without it the posts are drawn as their text.
  try {
    attachmentsRoot = await invoke<string>("room_attachments_dir");
  } catch {
    attachmentsRoot = null;
  }
  if (currentTopic) {
    try {
      drawTopic(await invoke<LoggedPost[]>("room_topic_log", { topicId: currentTopic.topic_id }));
    } catch (err) {
      status(`トピックの発言を読めませんでした: ${err}`, "error");
    }
  }
  // Permission prompts already held when this screen loaded — after a reload
  // (#336). One that ends meanwhile is ended by its event as any other.
  try {
    for (const card of await invoke<PermissionCard[]>("permission_requests")) showPermission(card);
  } catch {
    // Nothing held is drawn; the terminals still ask.
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

const sessions = createSessionController({
  invoke, listen,
  createTerminal: (options) => new Terminal(options),
  createFitAddon: () => new FitAddon(),
  useWebglRenderer: (term) => {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // No WebGL here. The DOM renderer is already drawing; nothing to undo.
    }
  },
  shownTopicId: (...args) => shownTopicId(...args),
  seatKey: (...args) => seatKey(...args),
  get accounts() { return accounts; },
  get diagnosticsEl() { return diagnosticsEl; },
  renderPanel: (...args) => renderPanel(...args),
  speakerColor: (...args) => speakerColor(...args),
  icon: (...args) => icon(...args),
  renderTopics: (...args) => renderTopics(...args),
  revealDiagnostics: (...args) => revealDiagnostics(...args),
  status: (...args) => status(...args),
  shortTime: (...args) => shortTime(...args),
  get topics() { return topics; },
  members: (...args) => members(...args),
  launches: (...args) => launches(...args),
  memberName: (...args) => memberName(...args),
  get terminalEl() { return terminalEl; },
  openAccountDialog: (...args) => openAccountDialog(...args),
});
const { seatedAnywhere, viewName, fitShown, showTopicTerminals, renderTerminalTabs, refreshSeats, renderSessionFacts, renderSessionId, endSession, discardView, showView, startSession } = sessions;

const displaySettings = createDisplaySettings({
  get roomEl() { return roomEl; },
  get inputEl() { return inputEl; },
  get participantsEl() { return participantsEl; },
  syncScrollLatest: (...args) => syncScrollLatest(...args),
  setTerminalFontSize: (size) => sessions.setTerminalFontSize(size),
  fitTerminal: () => sessions.fitShown(),
});

const accountDialog = createAccountDialog({
  invoke, listen,
  get accounts() { return accounts; },
  set accounts(value) { accounts = value; },
  shownTopicId: (...args) => shownTopicId(...args),
  joinArgs: (...args) => joinArgs(...args),
  get homeDir() { return homeDir; },
  get localAccountId() { return localAccountId; },
  speakerColor: (...args) => speakerColor(...args),
  declaredHue: (...args) => declaredHue(...args),
  initialOf: (...args) => initialOf(...args),
  get avatarImages() { return avatarImages; },
  drawAvatar: (...args) => drawAvatar(...args),
  asciiJson: (...args) => asciiJson(...args),
  setAvatarImage: (...args) => setAvatarImage(...args),
  saveConfig: (...args) => saveConfig(...args),
  renderPanel: (...args) => renderPanel(...args),
  status: (...args) => status(...args),
  seatedAnywhere: (...args) => seatedAnywhere(...args),
  renderSessionFacts: (...args) => renderSessionFacts(...args),
  join: (...args) => join(...args),
  allViews: () => sessions.allViews(),
  discardView: (...args) => discardView(...args),
  fillHues: (...args) => fillHues(...args),
});
const { openAccountDialog, mcpServerOf, mcpNote, refreshMcpServers } = accountDialog;

void main();
