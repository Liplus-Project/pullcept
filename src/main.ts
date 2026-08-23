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
import { listen } from "@tauri-apps/api/event";
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
  own: boolean;
}

interface TabConfig {
  id: string;
  /** The tab's label — which CLI it launches. Not the name a session takes in
   *  the room: that is chosen per launch, in the launcher. */
  name: string;
  command: string;
  args: string[];
  cwd: string | null;
  cli_kind: string;
}

interface AppConfig {
  tabs: TabConfig[];
}

interface StartedSession {
  pty_id: string;
  mcp_config: string;
  /** When the session was launched, stamped by the room's own clock. */
  started_at: string;
}

const NAME_KEY = "liplus-chat.display-name";
const HUE_KEY = "liplus-chat.display-hue";
const SESSION_NAME_KEY = "liplus-chat.session-name";
const SESSION_HUE_KEY = "liplus-chat.session-hue";

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
const nameEl = document.getElementById("display-name") as HTMLInputElement;
const hueEl = document.getElementById("display-hue") as HTMLSelectElement;
const tabEl = document.getElementById("tab-select") as HTMLSelectElement;
const sessionNameEl = document.getElementById("session-name") as HTMLInputElement;
const sessionHueEl = document.getElementById("session-hue") as HTMLSelectElement;
const startEl = document.getElementById("start-session") as HTMLButtonElement;
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
const cwdEl = document.getElementById("session-cwd") as HTMLInputElement;
const optionsEl = document.getElementById("launch-options") as HTMLInputElement;
const previewEl = document.getElementById("launch-preview") as HTMLElement;

let tabs: TabConfig[] = [];
/** Everyone in the room, this screen's person included. */
let participants: Participant[] = [];
/** The session the terminal is attached to, once one is running. */
let activePtyId: string | null = null;
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

const terminal = new Terminal({
  cursorBlink: true,
  fontSize: 13,
  fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, monospace',
  // The CLI is a full-screen TUI: it moves the cursor, clears regions and
  // repaints. Anything less than an emulator turns that into debris, which is
  // what the previous line-appending pane did (#24).
  convertEol: false,
  scrollback: 5000,
});
const fitAddon = new FitAddon();

function status(text: string, kind: "info" | "error" = "info"): void {
  statusEl.textContent = text;
  statusEl.dataset.kind = kind;
}

function revealDiagnostics(): void {
  diagnosticsEl.hidden = false;
  toggleEl.setAttribute("aria-expanded", "true");
  // The container has no size while hidden, so the fit has to wait for layout.
  requestAnimationFrame(() => fitTerminal());
}

function fitTerminal(): void {
  if (diagnosticsEl.hidden) return;
  try {
    fitAddon.fit();
  } catch {
    // A fit against a zero-sized container is not worth a message.
    return;
  }
  if (activePtyId !== null) {
    showWindowSize();
    void invoke("resize_pty", {
      id: activePtyId,
      cols: terminal.cols,
      rows: terminal.rows,
    }).catch(() => {
      // The session may have exited between the fit and the call.
    });
  }
}

/**
 * The size the CLI is laid out for.
 *
 * A TUI that is drawing at the wrong size looks like a broken TUI, and the
 * number it was given is the one thing that says which of the two it is.
 */
function showWindowSize(): void {
  windowEl.textContent = `${terminal.cols}×${terminal.rows}`;
}

/** Render saved arguments back into an editable line. */
function joinArgs(args: string[]): string {
  return args.map((arg) => (arg === "" || arg.includes(" ") ? `"${arg}"` : arg)).join(" ");
}

/**
 * Show the command that will actually run.
 *
 * The app merges its own channel entry into whatever is typed here, so the
 * line the person wrote is not the line that launches. Showing the result is
 * cheaper than explaining the merge.
 */
async function refreshPreview(): Promise<void> {
  const tab = tabs.find((candidate) => candidate.id === tabEl.value);
  if (!tab) {
    previewEl.textContent = "";
    return;
  }
  try {
    const parsed = await invoke<string[]>("parse_launch_options", { text: optionsEl.value });
    // The channel entry names this session's own server, which follows the
    // name being launched under, so the preview moves as that field is typed.
    const merged = await invoke<string[]>("preview_launch_args", {
      args: parsed,
      name: sessionName(),
    });
    previewEl.textContent = `${tab.command} ${joinArgs(merged)}`;
  } catch {
    previewEl.textContent = "";
  }
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
 * Draw the roster into the panel.
 *
 * The list is the room's roster and nothing else: the screen keeps no second
 * list of who is present, so a name on this panel is a name a post can be
 * addressed to. Each entry carries the colour that participant's lines carry
 * in the room, which is what makes the panel a legend for the conversation
 * rather than a second copy of the same names.
 */
function renderRoster(joined: Participant[]): void {
  participants = joined;
  rosterEl.replaceChildren();

  if (!joined.length) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = "参加者なし";
    rosterEl.appendChild(empty);
  }

  for (const participant of joined) {
    // `own` comes from the room, decided on the connection. A name test here
    // would mark every participant answering to this screen's name as oneself.
    const entry = document.createElement("li");
    entry.style.setProperty(
      "--speaker",
      speakerColor(participant.name, participant.hue, participant.own),
    );

    const dot = document.createElement("span");
    dot.className = "dot";

    const who = document.createElement("span");
    who.className = "who";
    who.textContent = participant.name;
    who.title = participant.name;

    entry.append(dot, who);
    if (participant.own) {
      const you = document.createElement("span");
      you.className = "self";
      you.textContent = "（あなた）";
      entry.appendChild(you);
    }
    rosterEl.appendChild(entry);
  }

  renderAddressees();
}

/** The name this screen posts under, and is listed in the roster under. */
function localName(): string {
  return nameEl.value.trim() || "human";
}

/** The name the next session will join under. */
function sessionName(): string {
  return sessionNameEl.value.trim();
}

/**
 * Take a seat in the room under the current name and colour.
 *
 * Being in the room is not the same as having spoken in it: without this the
 * roster would list only the sessions, and nobody could address someone who
 * had not spoken yet.
 *
 * The colour goes with the name because they are one declaration, and because
 * a seat carries both — sending the name alone on a rename would withdraw a
 * colour nobody withdrew.
 */
async function join(): Promise<void> {
  try {
    await invoke("room_join", { name: localName(), hue: declaredHue(hueEl) });
  } catch {
    // Failing to seat is not worth interrupting anything: the first post
    // seats the name anyway.
  }
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
 * roster no longer separates them.
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
 * Follow a launched session until it dies.
 *
 * A session that exits on startup is the failure mode with no other witness:
 * the room simply stays empty. Without this the screen is identical whether
 * the CLI is running or was never there.
 */
async function followSession(
  tab: TabConfig,
  name: string,
  started: StartedSession,
): Promise<void> {
  sessionStateEl.textContent = `${name} 起動中`;
  sessionStateEl.dataset.kind = "ok";
  activePtyId = started.pty_id;

  // How this session was launched, on the panel rather than in the person's
  // memory: a session that is answering nothing is read against the directory
  // and the command it actually got, not against the ones that were intended.
  // They stay on screen after the session exits — the question is what ran.
  transportEl.textContent = "PTY";
  commandEl.textContent = tab.command;
  dirEl.textContent = tab.cwd ?? "—";
  dirEl.title = tab.cwd ?? "";
  startedEl.textContent = shortTime(started.started_at);
  showWindowSize();

  await listen<string>(`pty-data-${started.pty_id}`, (event) => terminal.write(event.payload));
  await listen<number | null>(`pty-exit-${started.pty_id}`, (event) => {
    const code = event.payload;
    const detail = code === null ? "終了コード不明" : `終了コード ${code}`;
    sessionStateEl.textContent = `${name} 終了（${detail}）`;
    sessionStateEl.dataset.kind = "error";
    status(`${name} が終了しました（${detail}）。診断を確認してください。`, "error");
    activePtyId = null;
    revealDiagnostics();
  });

  // The first thing a session shows is a question, so the pane that carries
  // the answer opens with it rather than waiting for a failure.
  revealDiagnostics();
  terminal.focus();
}

async function startSession(): Promise<void> {
  const tab = tabs.find((candidate) => candidate.id === tabEl.value);
  if (!tab) {
    status("起動するセッションが選ばれていません。", "error");
    return;
  }

  // The identity this session joins under. Refused rather than defaulted: an
  // unnamed session is the state this whole surface exists to end, and quietly
  // falling back to the tab's label is how every session came to answer to
  // `Claude Code` (#40).
  const name = sessionName();
  if (!name) {
    status("セッションの名前を入力してください。部屋での名乗りになります。", "error");
    sessionNameEl.focus();
    return;
  }
  const hue = declaredHue(sessionHueEl);

  const cwd = cwdEl.value.trim();
  if (!cwd) {
    status("作業ディレクトリを入力してください。", "error");
    cwdEl.focus();
    return;
  }
  // The directory is the person's choice, so it is carried on the tab and
  // saved. Falling back to whatever directory the app process happens to sit
  // in is what put a session in src-tauri (#20).
  const args = await invoke<string[]>("parse_launch_options", { text: optionsEl.value });
  const launching: TabConfig = { ...tab, cwd, args };

  // Size the PTY to the terminal that will display it, so the CLI's first
  // paint is not laid out for a window it does not have.
  revealDiagnostics();
  fitAddon.fit();

  startEl.disabled = true;
  status(`${name} を起動しています…`);
  try {
    const started = await invoke<StartedSession>("start_session", {
      tab: launching,
      name,
      hue,
      cols: terminal.cols,
      rows: terminal.rows,
    });
    tab.cwd = cwd;
    tab.args = args;
    void invoke("save_config", { config: { tabs } }).catch(() => {
      // A directory that fails to persist is worth one line, not a failed
      // launch: the session is already up.
      status("作業ディレクトリを保存できませんでした。", "error");
    });
    // Remembered next to this screen's own name and colour, and in the same
    // place — the identity a launch declares is not a property of the tab, and
    // storing it there is what fixed every launch to one name.
    localStorage.setItem(SESSION_NAME_KEY, name);
    localStorage.setItem(SESSION_HUE_KEY, sessionHueEl.value);
    status(`${name} を起動しました。${started.mcp_config} に登録済み。`);
    await followSession(tab, name, started);
  } catch (err) {
    status(`${name} を起動できませんでした: ${err}`, "error");
    sessionStateEl.textContent = `${name} 起動失敗`;
    sessionStateEl.dataset.kind = "error";
    revealDiagnostics();
  } finally {
    startEl.disabled = false;
  }
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

function setUpTerminal(): void {
  terminal.loadAddon(fitAddon);
  terminal.open(terminalEl);

  const style = getComputedStyle(document.documentElement);
  terminal.options.theme = {
    background: style.getPropertyValue("--bg").trim() || "#17171a",
    foreground: style.getPropertyValue("--fg").trim() || "#e8e8ea",
  };

  terminal.onData((data) => {
    if (activePtyId === null) return;
    void invoke("write_pty", { id: activePtyId, data }).catch((err) => {
      status(`セッションへ送れませんでした: ${err}`, "error");
    });
  });

  // The webview does not deliver a native paste to xterm, so Ctrl+V is bridged
  // explicitly. preventDefault stops the input arriving twice.
  terminal.attachCustomKeyEventHandler((event) => {
    const isPaste =
      event.type === "keydown" &&
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      (event.key === "v" || event.key === "V");
    if (!isPaste) return true;

    event.preventDefault();
    void readText().then((text) => {
      if (text && activePtyId !== null) void invoke("write_pty", { id: activePtyId, data: text });
    });
    return false;
  });

  new ResizeObserver(() => fitTerminal()).observe(terminalEl);
}

async function main(): Promise<void> {
  setUpTerminal();

  nameEl.value = localStorage.getItem(NAME_KEY) ?? "human";
  fillHues(hueEl, localStorage.getItem(HUE_KEY));
  fillHues(sessionHueEl, localStorage.getItem(SESSION_HUE_KEY));

  // One handler for both halves of the declaration: a rename and a recolour are
  // the same act on the same seat, and the room takes them together.
  const redeclare = (): void => {
    localStorage.setItem(NAME_KEY, localName());
    localStorage.setItem(HUE_KEY, hueEl.value);
    // The roster entry follows the name, and the addressee list follows the
    // roster: a rename must not leave the old name sitting in either.
    void join().then(() => renderAddressees());
  };
  nameEl.addEventListener("change", redeclare);
  hueEl.addEventListener("change", redeclare);

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
  startEl.addEventListener("click", () => void startSession());

  let home = "";
  try {
    home = await invoke<string>("home_dir");
  } catch {
    // Only the prefill is lost; the field is still typed into by hand.
  }

  const showTab = (): void => {
    const tab = tabs.find((candidate) => candidate.id === tabEl.value);
    cwdEl.value = tab?.cwd ?? home;
    optionsEl.value = joinArgs(tab?.args ?? []);
    void refreshPreview();
  };
  optionsEl.addEventListener("input", () => void refreshPreview());
  // The launched line carries this session's own channel entry, which follows
  // the name, so the preview has to follow it too.
  sessionNameEl.addEventListener("input", () => void refreshPreview());

  try {
    const config = await invoke<AppConfig>("load_config");
    tabs = config.tabs;
    for (const tab of tabs) {
      const option = document.createElement("option");
      option.value = tab.id;
      option.textContent = tab.name;
      tabEl.appendChild(option);
    }
    // A prefill, not a default. The tab's label is a legible starting point for
    // the first launch; what makes it safe is that it is sitting in an editable
    // field on screen, which is exactly what it was not before (#40).
    sessionNameEl.value =
      localStorage.getItem(SESSION_NAME_KEY) ?? tabs[0]?.name ?? "";
    showTab();
    tabEl.addEventListener("change", showTab);
  } catch (err) {
    status(`設定を読み込めませんでした: ${err}`, "error");
  }

  try {
    await join();
    renderRoster(await invoke<Participant[]>("room_participants"));
    const port = await invoke<number | null>("room_port");
    if (port !== null) renderSocket(port);
  } catch (err) {
    renderSocket(null, `取得できませんでした: ${err}`);
  }
}

void main();
