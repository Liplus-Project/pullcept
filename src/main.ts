import { CLAUDE_PLAN_ENV, codexPlanEnabled } from "./codex-plan";
import { createParticipantDetails } from "./participant-details";
import { accountName } from "./account-name";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import type { TopicStatus, RoomMessage, LoggedPost, Topic, TopicRef, Participant, Roster, SessionStats, SeatActivity, Account, PanelState, AppConfig, IconName, IconShape, PermissionCard, PermissionResolved } from "./contracts";
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
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@xterm/xterm/css/xterm.css";
import { createComposer, highlightedCode } from "./composer";
import { createHtmlPreview } from "./html-preview";
import { resizableSidebar } from "./sidebar-resize";
import { createEndConfirm } from "./screen/end-confirm";
import { createTopics } from "./screen/topics";
import { createParticipants } from "./screen/participants";
import { createComposerScreen } from "./screen/composer";
import { createMessages } from "./screen/messages";
import { createRoom } from "./screen/room";

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
 * The hues a participant can declare.
 *
 * Hue only. Lightness and chroma use the theme's shared display tokens, so
 * declared colours share one weight on the page. The vivid pair (#397) is
 * independent of accent; the hue-only contract stays (#43). Offering a full
 * colour picker would ask for two values and silently discard them.
 *
 * A short list rather than a continuous dial: what this has to buy is that any
 * two participants can be told apart, and sixteen positions spread round the
 * wheel buy it without asking anyone to judge degrees.
 */
const HUES: { label: string; hue: number }[] = [
  { label: "紅", hue: 7.5 },
  { label: "赤", hue: 25 },
  { label: "朱", hue: 40 },
  { label: "橙", hue: 55 },
  { label: "山吹", hue: 75 },
  { label: "黄", hue: 95 },
  { label: "黄緑", hue: 120 },
  { label: "緑", hue: 145 },
  { label: "ミント", hue: 170 },
  { label: "青緑", hue: 195 },
  { label: "水色", hue: 222.5 },
  { label: "青", hue: 250 },
  { label: "藍", hue: 275 },
  { label: "紫", hue: 300 },
  { label: "藤", hue: 325 },
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
  // one is this drawing turned over in src/styles/titlebar.css.
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
// Each side panel's inner edge drags to resize it (#530).
resizableSidebar(historyEl, "left", "トピック");
resizableSidebar(participantsEl, "right", "アカウント");
const rosterEl = document.getElementById("roster") as HTMLElement;
const topicListEl = document.getElementById("topic-list") as HTMLElement;
const topicNewEl = document.getElementById("topic-new") as HTMLButtonElement;
const roomTitleEl = document.getElementById("room-title") as HTMLElement;
const topicMenuEl = document.getElementById("topic-menu") as HTMLElement;
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
const participantDetails = createParticipantDetails({
  planEnabled: id => { const account = accounts.find(a => a.id === id); if (account?.kind === "claude_code") return account.env.some(v => v.name === CLAUDE_PLAN_ENV); return account?.kind !== "codex_cli" || codexPlanEnabled(account.args); },
  element: document.getElementById("participant-details") as HTMLElement,
  topic: () => shownTopicId(), human: () => localAccountId,
  read: (topicId, accountId) => invoke<TopicStatus>("topic_status", {topicId,accountId}),
  update: (topicId, operation, answer) => invoke("update_topic_work", {topicId,operation,answer}),
  history: (topicId) => invoke<LoggedPost[]>("room_topic_log", {topicId}),
  openUrl: (url) => { void invoke("open_post_url", {url}); },
  changed: () => renderPanel(),
});
const factsMoreEl = document.querySelector("#terminal-diagnostics") as HTMLDetailsElement;
const terminalEl = document.getElementById("terminal") as HTMLElement;
const diagnosticsCloseEl = document.getElementById("diagnostics-close") as HTMLButtonElement;
const endDialogEl = document.getElementById("end-dialog") as HTMLDialogElement;
const endCancelEl = document.getElementById("end-cancel") as HTMLButtonElement;
const endCommitEl = document.getElementById("end-commit") as HTMLButtonElement;
const quitDialogEl = document.getElementById("quit-dialog") as HTMLDialogElement;
const quitCancelEl = document.getElementById("quit-cancel") as HTMLButtonElement;
const quitCommitEl = document.getElementById("quit-commit") as HTMLButtonElement;
const topicDeleteDialogEl = document.getElementById(
  "topic-delete-dialog",
) as HTMLDialogElement;
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

/**
 * Whether the pane was open, kept across a reload of this screen (#454).
 *
 * In `sessionStorage` because the question is only ever asked by the same
 * window coming back: a reload keeps it, and an app restart has no seats to
 * re-adopt, so it never reads it. Nothing stored reads as folded, which is how
 * the page opens.
 */
const DIAGNOSTICS_OPEN_KEY = "pullcept.diagnostics-open";

function rememberDiagnostics(open: boolean): void {
  try {
    sessionStorage.setItem(DIAGNOSTICS_OPEN_KEY, open ? "1" : "0");
  } catch {
    // Unremembered, the pane comes back folded after a reload.
  }
}

function diagnosticsWereOpen(): boolean {
  try {
    return sessionStorage.getItem(DIAGNOSTICS_OPEN_KEY) === "1";
  } catch {
    return false;
  }
}

function revealDiagnostics(): void {
  diagnosticsEl.hidden = false;
  toggleEl.setAttribute("aria-expanded", "true");
  rememberDiagnostics(true);
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
  rememberDiagnostics(false);
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
 * Declared and derived colours use the theme's shared display lightness and
 * chroma; only hue turns. Undeclared oneself keeps accent. One ladder:
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
  roomTitleNameEl.title = topic ? `${name}（クリックで名前を変える）` : name;
  roomTitleNameEl.classList.toggle("unnamed", topic !== undefined && !topic.title);
  roomTitleCountEl.textContent = current === null ? "" : `${shownRoster().length} 人`;
}

/**
 * Rename the open topic from the title bar's heading (#469).
 *
 * A topic the index does not carry yet — 新しいトピック — has no row to rename,
 * so the heading offers nothing then. The field stands in the name's place and
 * the count stays beside it.
 */
function beginTitleRename(): void {
  const current = currentTopic;
  const topic = current ? topics.find((one) => one.topic_id === current.topic_id) : undefined;
  if (!topic || roomTitleEl.querySelector("input.rename")) return;
  const field = renameField(topic, () => {
    field.remove();
    roomTitleNameEl.hidden = false;
    renderRoomTitle();
  });
  roomTitleNameEl.hidden = true;
  roomTitleNameEl.after(field);
  // As wide as what is written in it (#479), measured rather than left to
  // `field-sizing`, which fell about two characters short on the device and
  // scrolled the start of the name out of the field (#484). The extra is the
  // field's own padding and border, and room for the caret.
  const measure = document.createElement("canvas").getContext("2d");
  const fit = (): void => {
    if (!measure) return;
    const style = getComputedStyle(field);
    measure.font = style.font;
    const chrome =
      parseFloat(style.paddingLeft) + parseFloat(style.paddingRight) +
      parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth);
    field.style.width = `${Math.ceil(measure.measureText(field.value).width + chrome) + 8}px`;
  };
  field.addEventListener("input", fit);
  fit();
  field.focus();
  field.select();
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
 * Where this room's attachments are saved, as the paths in the posts spell it
 * (`room_attachments_dir`, #318). Null until it is read, and when it could not
 * be: then every post is drawn as its text.
 */
let attachmentsRoot: string | null = null;

/**
 * One fenced block (#348): the code in monospace, its line breaks and spaces as
 * written, inside a frame. The language, when the fence names one, is a small
 * label over the code and never part of it.
 */
const htmlPreview = createHtmlPreview(document.getElementById("conversation")!,
  // The separate window's title bar and code view take the file's name and source (#468).
  (document, open, source) => invoke("html_preview", { document, open, name: source?.name, code: source?.code }),
  // The side pane's child view (#466) goes through the same command.
  (pane) => invoke("html_preview", { document: null, open: false, pane }),
  (code) => highlightedCode(code, "html"));
void listen("html-preview-return", () => htmlPreview.returnToMain());

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

  // The topic's name renames on a click (#469, #479): a press released where
  // it was pressed opens the field, and a press that moves grabs the window
  // like the bar around it. The name carries `data-tauri-drag-region="false"`,
  // which takes it out of Tauri's drag script — that script starts the drag on
  // the press itself, before it can be told from a click — so the drag is
  // started here once the pointer has moved a few pixels with the button held.
  roomTitleNameEl.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    const fromX = event.clientX;
    const fromY = event.clientY;
    const off = (): void => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    const onMove = (move: MouseEvent): void => {
      if (Math.abs(move.clientX - fromX) + Math.abs(move.clientY - fromY) < 4) return;
      off();
      void appWindow.startDragging().catch(refused);
    };
    const onUp = (): void => {
      off();
      beginTitleRename();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  roomTitleNameEl.addEventListener("contextmenu", (event) => {
    const current = currentTopic;
    const topic = current ? topics.find((one) => one.topic_id === current.topic_id) : undefined;
    if (!topic) return;
    event.preventDefault();
    openTopicMenu(topic, { x: event.clientX, y: event.clientY }, () => beginTitleRename());
  });

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
  // in src/styles/layout.css), so the way back has to be somewhere that is always
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
  await listen<{topic_id:string}>("work-status-changed", (event) => {
    if (event.payload.topic_id === shownTopicId()) void participantDetails.refresh();
  });
  await listen("mcp-servers-changed", () => { void participantDetails.refresh(); });
  setInterval(() => { if (!participantsEl.hidden) void participantDetails.refresh(); }, 15000);
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
  await listen("session-seats-changed", () => void refreshSeats());
  await listen<{ status: string; error?: string }>("session-operation", (event) => {
    if (event.payload.error) status(`セッション操作 ${event.payload.status}: ${event.payload.error}`, "error");
  });
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
  // nothing. Heard on the webview, not the window (#605): the child view of
  // the HTML preview (#466) needs Tauri's `unstable` feature, under which the
  // main webview is built as a child of its window too, so its drops come as
  // webview events and reach webview listeners only — with one webview in
  // the window or two (tauri-runtime-wry 2.11.4 `create_window` and the drag
  // handler in `create_webview`; tauri 2.11.5 `emit_to_webview`).
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
  // The topic menu closes the same ways, and on Escape.
  document.addEventListener(
    "pointerdown",
    (event) => {
      if (event.target instanceof Node && topicMenuEl.contains(event.target)) return;
      closeTopicMenu();
    },
    true,
  );
  topicMenuEl.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeTopicMenu();
    }
  });
  window.addEventListener("resize", () => closeTopicMenu());
  window.addEventListener("blur", () => closeTopicMenu());
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
  // The pane comes back as it was before the reload, not forced open (#454):
  // the notice it was opened for is on the status line as well.
  await refreshSeats(diagnosticsWereOpen());

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
    void participantDetails.refresh();
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
  selectFactsAccount: (accountId) => {
    const account = accounts.find(one => one.id === accountId);
    if (account) participantDetails.select(accountId, accountName(account), account.server ?? undefined, account.kind);
  },
  factsSelection: () => {
    const selection = participantDetails.selection();
    return selection ? { accountId: selection.id, session: participantDetails.session() } : null;
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
  get composerBoxEl() { return composerBoxEl; },
  get participantsEl() { return participantsEl; },
  syncScrollLatest: (...args) => syncScrollLatest(...args),
  setTerminalFontSize: (size) => sessions.setTerminalFontSize(size),
  setTerminalCursorStyle: (style) => sessions.setTerminalCursorStyle(style),
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

const endConfirm = createEndConfirm({
  get endDialogEl() { return endDialogEl; },
  get quitDialogEl() { return quitDialogEl; },
  get accounts() { return accounts; },
  get endingAccount() { return endingAccount; },
  set endingAccount(value) { endingAccount = value; },
  get sessions() { return sessions; },
  get endSession() { return endSession; },
});
const { openEndDialog, answerQuit, onQuitRequested, closeEndDialog, confirmEndDialog } = endConfirm;

const topicsScreen = createTopics({
  icon: (...args) => icon(...args),
  get topicListEl() { return topicListEl; },
  get topicNewEl() { return topicNewEl; },
  get topicMenuEl() { return topicMenuEl; },
  get participantDetails() { return participantDetails; },
  get topicDeleteDialogEl() { return topicDeleteDialogEl; },
  get accounts() { return accounts; },
  get rosters() { return rosters; },
  get topics() { return topics; },
  get currentTopic() { return currentTopic; },
  set currentTopic(value) { currentTopic = value; },
  get deletingTopic() { return deletingTopic; },
  set deletingTopic(value) { deletingTopic = value; },
  get awaiting() { return awaiting; },
  shownTopicId: (...args) => shownTopicId(...args),
  status: (...args) => status(...args),
  dayLabel: (...args) => dayLabel(...args),
  fullDateTime: (...args) => fullDateTime(...args),
  drawTopic: (...args) => drawTopic(...args),
  renderRoomTitle: (...args) => renderRoomTitle(...args),
  renderRoster: (...args) => renderRoster(...args),
  get htmlPreview() { return htmlPreview; },
  get sessions() { return sessions; },
  get discardView() { return discardView; },
  get refreshSeats() { return refreshSeats; },
  get showTopicTerminals() { return showTopicTerminals; },
});
const { topicName, renameField, openTopicMenu, renderTopics, startNewTopic, closeTopicMenu, closeTopicDeleteDialog, confirmTopicDeleteDialog, topicsFailed } = topicsScreen;

const participantsScreen = createParticipants({
  launches: (...args) => launches(...args),
  get HUES() { return HUES; },
  icon: (...args) => icon(...args),
  get rosterEl() { return rosterEl; },
  get accountMenuEl() { return accountMenuEl; },
  get mentionListEl() { return mentionListEl; },
  get diagnosticsEl() { return diagnosticsEl; },
  get participantDetails() { return participantDetails; },
  get accounts() { return accounts; },
  get rosters() { return rosters; },
  get localAccountId() { return localAccountId; },
  set localAccountId(value) { localAccountId = value; },
  get awaiting() { return awaiting; },
  seatKey: (...args) => seatKey(...args),
  shownTopicId: (...args) => shownTopicId(...args),
  shownRoster: (...args) => shownRoster(...args),
  revealDiagnostics: (...args) => revealDiagnostics(...args),
  localAccount: (...args) => localAccount(...args),
  saveConfig: (...args) => saveConfig(...args),
  speakerColor: (...args) => speakerColor(...args),
  avatar: (...args) => avatar(...args),
  paintMine: (...args) => paintMine(...args),
  renderRoomTitle: (...args) => renderRoomTitle(...args),
  pruneAwaiting: (...args) => pruneAwaiting(...args),
  renderRoomBusy: (...args) => renderRoomBusy(...args),
  get menuAccountId() { return menuAccountId; },
  set menuAccountId(value) { menuAccountId = value; },
  refreshMentions: (...args) => refreshMentions(...args),
  get sessions() { return sessions; },
  get renderTerminalTabs() { return renderTerminalTabs; },
  get showView() { return showView; },
  get startSession() { return startSession; },
  get viewName() { return viewName; },
  get mcpNote() { return mcpNote; },
  get mcpServerOf() { return mcpServerOf; },
  get openAccountDialog() { return openAccountDialog; },
  get openEndDialog() { return openEndDialog; },
});
const { renderPanel, localName, rowActivity, memberName, pulsePhase, renderRoster, onAccountMenuKey, closeAccountMenu, resolveLocalAccount, join, members } = participantsScreen;

const composerScreen = createComposerScreen({
  icon: (...args) => icon(...args),
  get composerEl() { return composerEl; },
  get composerBoxEl() { return composerBoxEl; },
  get attachmentsEl() { return attachmentsEl; },
  get mentionListEl() { return mentionListEl; },
  get composer() { return composer; },
  get lastSeenId() { return lastSeenId; },
  shownTopicId: (...args) => shownTopicId(...args),
  shownRoster: (...args) => shownRoster(...args),
  status: (...args) => status(...args),
  speakerColor: (...args) => speakerColor(...args),
  drawMissed: (...args) => drawMissed(...args),
  get ATTACHMENT_HEAD() { return ATTACHMENT_HEAD; },
  imageType: (...args) => imageType(...args),
  openViewer: (...args) => openViewer(...args),
  get localName() { return localName; },
});
const { send, mentionKey, refreshMentions, closeMentions, attachFiles, baseName, typeMention, markDrop, overComposer, attachPaths, asciiJson } = composerScreen;

const messagesScreen = createMessages({
  get roomEl() { return roomEl; },
  get roomGliding() { return roomGliding; },
  set roomGliding(value) { roomGliding = value; },
  get viewerEl() { return viewerEl; },
  get viewerImageEl() { return viewerImageEl; },
  get accounts() { return accounts; },
  get rosters() { return rosters; },
  get lastSeenId() { return lastSeenId; },
  set lastSeenId(value) { lastSeenId = value; },
  get drawnIds() { return drawnIds; },
  get awaiting() { return awaiting; },
  shownTopicId: (...args) => shownTopicId(...args),
  shownRoster: (...args) => shownRoster(...args),
  status: (...args) => status(...args),
  localAccount: (...args) => localAccount(...args),
  speakerColor: (...args) => speakerColor(...args),
  shortTime: (...args) => shortTime(...args),
  dayKey: (...args) => dayKey(...args),
  dayLabel: (...args) => dayLabel(...args),
  fullDateTime: (...args) => fullDateTime(...args),
  roomAtBottom: (...args) => roomAtBottom(...args),
  glideRoomToFoot: (...args) => glideRoomToFoot(...args),
  syncScrollLatest: (...args) => syncScrollLatest(...args),
  scrollRoomToLatest: (...args) => scrollRoomToLatest(...args),
  get attachmentsRoot() { return attachmentsRoot; },
  get htmlPreview() { return htmlPreview; },
  get localName() { return localName; },
  get renderPanel() { return renderPanel; },
  get baseName() { return baseName; },
});
const { appendMessage, trackAddress, showPermission, endPermission, loadAvatarImages, drawTopic, initialOf, avatarImages, drawAvatar, setAvatarImage, avatar, paintMine, pruneAwaiting, drawMissed, ATTACHMENT_HEAD, imageType, openViewer } = messagesScreen;

const roomScreen = createRoom({
  get roomEl() { return roomEl; },
  get scrollLatestEl() { return scrollLatestEl; },
  get roomBusyEl() { return roomBusyEl; },
  get roomGliding() { return roomGliding; },
  set roomGliding(value) { roomGliding = value; },
  speakerColor: (...args) => speakerColor(...args),
  get memberName() { return memberName; },
  get pulsePhase() { return pulsePhase; },
  get rowActivity() { return rowActivity; },
});
const { settleGlide, syncScrollLatest, scrollRoomToLatest, renderRoomBusy, roomAtBottom, glideRoomToFoot } = roomScreen;

void main();
