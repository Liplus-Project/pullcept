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

const roomEl = document.getElementById("room") as HTMLElement;
const rosterEl = document.getElementById("roster") as HTMLElement;
const accountNewEl = document.getElementById("account-new") as HTMLButtonElement;
const inputEl = document.getElementById("input") as HTMLTextAreaElement;
const sendEl = document.getElementById("send") as HTMLButtonElement;
const toEl = document.getElementById("to-select") as HTMLSelectElement;
const statusEl = document.getElementById("status") as HTMLElement;
const diagnosticsEl = document.getElementById("diagnostics") as HTMLElement;
const toggleEl = document.getElementById("toggle-diagnostics") as HTMLButtonElement;
const socketStateEl = document.getElementById("socket-state") as HTMLElement;
const sessionStateEl = document.getElementById("session-state") as HTMLElement;
const transportEl = document.getElementById("session-transport") as HTMLElement;
const commandEl = document.getElementById("session-command") as HTMLElement;
const dirEl = document.getElementById("session-dir") as HTMLElement;
const startedEl = document.getElementById("session-started") as HTMLElement;
const windowEl = document.getElementById("session-window") as HTMLElement;
const terminalEl = document.getElementById("terminal") as HTMLElement;
const dialogEl = document.getElementById("account-dialog") as HTMLDialogElement;
const dialogFormEl = document.getElementById("account-form") as HTMLFormElement;
const dialogTitleEl = document.getElementById("account-dialog-title") as HTMLElement;
const dialogNameEl = document.getElementById("dialog-name") as HTMLInputElement;
const dialogKindEl = document.getElementById("dialog-kind") as HTMLSelectElement;
const dialogHueEl = document.getElementById("dialog-hue") as HTMLSelectElement;
const dialogLaunchEl = document.getElementById("dialog-launch") as HTMLElement;
const dialogCwdEl = document.getElementById("dialog-cwd") as HTMLInputElement;
const dialogOptionsEl = document.getElementById("dialog-options") as HTMLInputElement;
const dialogPreviewEl = document.getElementById("dialog-preview") as HTMLElement;
const dialogErrorEl = document.getElementById("dialog-error") as HTMLElement;
const dialogDeleteEl = document.getElementById("dialog-delete") as HTMLButtonElement;
const dialogCancelEl = document.getElementById("dialog-cancel") as HTMLButtonElement;

let accounts: Account[] = [];
/**
 * The accounts holding a seat in the room, by id.
 *
 * Ids, never names: this is matched against the account list to decide who is
 * offline, and a name match would tie the wrong account as soon as two share a
 * name — which they may, now that a name is an editable attribute (#53). The
 * app is the authority (`seated_accounts`); the screen re-reads it rather than
 * keeping a count of its own launches.
 */
let seated = new Set<string>();
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

/**
 * The emulator options every session's terminal is opened with.
 *
 * One set for all of them, so that two sessions on this screen are two of the
 * same kind of thing and a difference between their panes says something about
 * the sessions rather than about the panes.
 */
const TERMINAL_OPTIONS = {
  cursorBlink: true,
  fontSize: 13,
  fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, monospace',
  // The CLI is a full-screen TUI: it moves the cursor, clears regions and
  // repaints. Anything less than an emulator turns that into debris, which is
  // what the previous line-appending pane did (#24).
  convertEol: false,
  scrollback: 5000,
};

/** How long a 終了 stays armed before it falls back to its resting state. */
const END_ARM_MS = 4000;
/**
 * How long an armed 終了 refuses to act.
 *
 * The armed button is wider than the resting one and grows under the pointer,
 * so a double-click lands both clicks on it: without this, one slip of the
 * finger arms and ends in a single gesture, which is the shape the two clicks
 * exist to rule out. Short enough that a deliberate second click never waits.
 */
const END_SETTLE_MS = 400;

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
}

/** The terminals this screen holds, by account id, in launch order. */
const views = new Map<string, SessionView>();
/** The account whose terminal is on the glass, or null when none is. */
let shownAccount: string | null = null;
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
/** The account whose 終了 is armed. Its next click is the one that ends it. */
let armedEnd: string | null = null;
let armedTimer = 0;
let armedAt = 0;

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
  // oneself says nothing: being in the list is what it would have said.
  let noteText = "";
  let noteKind = "";
  if (own) noteText = "（あなた）";
  else if (launching) noteText = "起動中";
  else if (row.participant) noteText = "";
  else if (failure) {
    noteText = "起動失敗";
    noteKind = "error";
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
 * One click, where 終了 beside it takes two. The asymmetry is the difference
 * between the two acts: ending a session cannot be taken back, and starting one
 * is undone by the button that replaces this one. An arm here would charge
 * every deliberate start a second click to guard a mistake that undoes itself.
 *
 * It carries its own launch. Pressed, it goes dead until the launch comes back,
 * on the row that was pressed — the launcher's button held that state for
 * whichever account its picker was on, and could not say which one (#62). What
 * the state *is* stays in the note beside the name, where 未起動 and 終了 and
 * 起動失敗 are: the button says what can be done, the note says what is so.
 */
function startButton(account: Account, launching: boolean): HTMLButtonElement {
  const start = document.createElement("button");
  start.type = "button";
  start.className = "start";
  start.textContent = "開始";
  start.disabled = launching;
  start.title = launching
    ? `${account.name} を起動しています`
    : `${account.name} のセッションを開始する`;
  start.addEventListener("click", () => void startSession(account));
  return start;
}

/** The control that opens one account's form. */
function editButton(account: Account): HTMLButtonElement {
  const edit = document.createElement("button");
  edit.type = "button";
  edit.className = "edit";
  edit.textContent = "編集";
  edit.title = `${account.name} の設定`;
  edit.addEventListener("click", () => openAccountDialog(account));
  return edit;
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
    seated = new Set(await invoke<string[]>("seated_accounts"));
  } catch {
    // The panel keeps the last answer rather than declaring everyone offline
    // on a failed read. It still redraws: what failed is this one value, and
    // whatever else moved since the last draw is not held back by it.
  }
  renderPanel();
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
 * The 終了 control, which takes two clicks.
 *
 * Ending a session cannot be undone, and this control sits in a list whose
 * other click merely changes which pane is showing. One plain click away from a
 * harmless neighbour is how a slip ends a session that was mid-answer, so the
 * first click only arms: the button changes what it says and how it looks, and
 * the second click within a few seconds is the one that acts.
 *
 * Two clicks rather than `window.confirm`, which the account delete beside this
 * uses. That dialog answers on the host's terms, and the two ways it can fail
 * here are both wrong: a host that answers nothing either makes the button
 * silently dead or — the bias the delete chose — makes a single click destroy.
 * An arm state held on this side has neither failure, and the button says which
 * state it is in rather than a modal saying it elsewhere.
 */
function endButton(view: SessionView, name: string): HTMLButtonElement {
  const armed = armedEnd === view.accountId;
  const end = document.createElement("button");
  end.type = "button";
  end.className = armed ? "end armed" : "end";
  end.textContent = armed ? "本当に終了" : "終了";
  end.title = armed
    ? `もう一度押すと ${name} のセッションを終了します。`
    : `${name} のセッションを終了する`;
  end.addEventListener("click", () => {
    if (!armed) {
      armEnd(view.accountId);
      return;
    }
    // A click that arrives inside the settle window is the tail of the gesture
    // that armed it, not an answer to it. Ignored, arm left standing.
    if (Date.now() - armedAt < END_SETTLE_MS) return;
    void endSession(view);
  });
  return end;
}

/** Put one account's 終了 into its armed state, and let the arm lapse. */
function armEnd(accountId: string): void {
  window.clearTimeout(armedTimer);
  armedEnd = accountId;
  armedAt = Date.now();
  // The arm expires on its own. A button left saying 「本当に終了」 for the rest
  // of a session is one that an unrelated click, minutes later, fires.
  armedTimer = window.setTimeout(() => {
    armedEnd = null;
    renderPanel();
  }, END_ARM_MS);
  renderPanel();
}

function disarmEnd(): void {
  window.clearTimeout(armedTimer);
  armedEnd = null;
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
  disarmEnd();
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
 * Give an account its own terminal and put it on the glass.
 *
 * Made before the launch, because the CLI's first paint is laid out for the
 * size this pane reports and there is nothing else to measure.
 */
function openView(account: Account): SessionView {
  // A relaunch replaces the previous run's pane. Two panes for one account
  // would be two rows under one name, and the row is what the operations hang
  // on; the scrollback that goes with it is the one the person just decided to
  // start over from.
  discardView(views.get(account.id));

  const host = document.createElement("div");
  host.className = "term";
  terminalEl.appendChild(host);

  const term = new Terminal({ ...TERMINAL_OPTIONS });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);
  term.options.theme = terminalTheme();

  const view: SessionView = {
    accountId: account.id,
    ptyId: "",
    name: account.name,
    command: account.command,
    cwd: account.cwd,
    startedAt: "",
    term,
    fit,
    host,
    unlisten: [],
    ended: null,
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
 * A terminal whose session has ended is the exception, and it is discarded here
 * rather than at the moment it died — what a session printed on its way out is
 * the only account of why, and choosing another account is the person saying
 * they have read it (#57).
 */
function showView(accountId: string | null): void {
  disarmEnd();
  for (const view of [...views.values()]) {
    if (view.ended !== null && view.accountId !== accountId) discardView(view);
  }
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

  // Both listeners are this view's, and are dropped with it. The shared
  // terminal subscribed once per launch and unsubscribed never, which is how
  // every running session ended up writing into one pane (#57).
  view.unlisten.push(
    await listen<string>(`pty-data-${started.pty_id}`, (event) => view.term.write(event.payload)),
  );
  view.unlisten.push(
    await listen<number | null>(`pty-exit-${started.pty_id}`, (event) => {
      const code = event.payload;
      const detail = code === null ? "終了コード不明" : `終了コード ${code}`;
      view.ended = detail;
      // Nothing more will arrive on this pty. The view lives on for what it
      // has already printed, not for anything it is still waiting to hear.
      for (const off of view.unlisten) off();
      view.unlisten = [];
      const name = viewName(view);
      status(`${name} が終了しました（${detail}）。診断を確認してください。`, "error");
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
/** True once 削除 has been armed, in the same shape 終了 uses. */
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
 * The app merges its own channel entry into whatever is typed, so the line
 * written here is not the line that launches; showing the result is cheaper
 * than explaining the merge. The entry names this account's own server, which
 * follows the account id — so the preview holds still while the name in the
 * field above it is edited. Holding still is the point: the identity being
 * launched is the account, and renaming it does not make it something else
 * (#53).
 */
async function refreshDialogPreview(): Promise<void> {
  if (!draft) return;
  const id = draft.id;
  try {
    const parsed = await invoke<string[]>("parse_launch_options", {
      text: dialogOptionsEl.value,
    });
    const merged = await invoke<string[]>("preview_launch_args", { args: parsed, accountId: id });
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
      };

  dialogTitleEl.textContent = account ? "アカウントの編集" : "アカウントの追加";
  dialogNameEl.value = draft.name;
  dialogKindEl.value = draft.kind;
  dialogHueEl.value = draft.hue === null ? "" : String(draft.hue);
  dialogCwdEl.value = draft.cwd ?? "";
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
  // Only for a session. A person's working directory and options would be two
  // values nothing ever reads, kept alive by an edit that once set them.
  const args =
    kind === "ai"
      ? await invoke<string[]>("parse_launch_options", { text: dialogOptionsEl.value })
      : [];

  const settled: Account = {
    ...settling,
    name,
    kind,
    hue: declaredHue(dialogHueEl),
    cwd: kind === "ai" ? cwd || null : null,
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
 * — the bias `confirm` defaults to — destructive on one click (#57).
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

  // The account's colour is the account's, so nothing is restored into this
  // picker — the form fills it from whichever account it was opened on.
  fillHues(dialogHueEl, null);

  toggleEl.addEventListener("click", () => {
    if (diagnosticsEl.hidden) {
      revealDiagnostics();
    } else {
      diagnosticsEl.hidden = true;
      toggleEl.setAttribute("aria-expanded", "false");
    }
  });

  await listen<RoomMessage>("room-message", (event) => appendMessage(event.payload));
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
  // Anything but the second click of 削除 disarms it: an arm left standing is
  // one that an unrelated click fires later.
  for (const field of [dialogNameEl, dialogKindEl, dialogHueEl, dialogCwdEl, dialogOptionsEl]) {
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

  try {
    await join();
    participants = await invoke<Participant[]>("room_participants");
    await refreshSeats();
    const port = await invoke<number | null>("room_port");
    if (port !== null) renderSocket(port);
  } catch (err) {
    renderPanel();
    renderSocket(null, `取得できませんでした: ${err}`);
  }
}

void main();
