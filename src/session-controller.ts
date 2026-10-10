import { limitedByUsage, statsForView } from "./seat-status";
import type { invoke } from "@tauri-apps/api/core";
import type { listen } from "@tauri-apps/api/event";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import type { ITerminalOptions } from "@xterm/xterm";
import type { Topic, Participant, SessionStats, SeatActivity, Account, StartedSession, RunningSession, SeatedAccount, IconName, SessionView, Member, PtyExit } from "./contracts";

export interface Dependencies {
  invoke: typeof invoke;
  listen: typeof listen;
  createTerminal: (options: ITerminalOptions) => Terminal;
  createFitAddon: () => FitAddon;
  useWebglRenderer: (term: Terminal) => void;

  shownTopicId: () => string;
  seatKey: (topicId: string, accountId: string) => string;
  accounts: Account[];
  diagnosticsEl: HTMLElement;
  renderPanel: () => void;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  icon: (name: IconName) => SVGSVGElement;
  renderTopics: () => void;
  revealDiagnostics: () => void;
  status: (text: string, kind?: "info" | "error") => void;
  shortTime: (iso: string) => string;
  topics: Topic[];
  members: () => Member[];
  launches: (account: Account) => boolean;
  memberName: (row: Member) => string;
  terminalEl: HTMLElement;
  openAccountDialog: (account: Account | null, field?: "name" | "hue") => void;
}

export function createSessionController(deps: Dependencies) {

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
    fontSize: 13,
    fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, monospace',
    // The CLI is a full-screen TUI: it moves the cursor, clears regions and
    // repaints. Anything less than an emulator turns that into debris, which is
    // what the previous line-appending pane did (#24).
    convertEol: false,
    scrollback: 5000,
  };

  /**
   * Draw a terminal with the WebGL renderer, or leave it on the DOM one.
   *
   * Every terminal is put through this, for the same reason they share
   * `TERMINAL_OPTIONS`: two panes on this screen draw the same way. The point is
   * the block elements and box-drawing characters a TUI builds its pictures from
   * (`█▛▜▐▌`, the CLI's mascot among them). The DOM renderer sets each one in the
   * font, and the glyphs do not quite meet at the cell edges, so a picture made of
   * them shows a grid of thin lines. The WebGL renderer paints those characters
   * cell by cell itself (`customGlyphs`, which the DOM renderer ignores), and the
   * cells meet (#278). Inside a cell, the rectangles of one block element meet
   * only because of the install-time patch in `scripts/patch-xterm-webgl.mjs`.
   *
   * The DOM renderer stays the floor. Where WebGL cannot start, loading throws and
   * the terminal keeps drawing as it did; where the context is lost later — the
   * GPU resets, or the webview takes back the oldest context once too many are
   * open — the addon is disposed and the terminal falls back to the DOM renderer
   * rather than going blank.
   */

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
  let terminalFontSize = TERMINAL_OPTIONS.fontSize;

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

  /** The terminals of the topic on the glass, in launch order. */
  function topicViews(): SessionView[] {
    return [...views.values()].filter((view) => view.topicId === deps.shownTopicId());
  }

  /** Whether an account holds a seat in any topic. */
  function seatedAnywhere(accountId: string): boolean {
    return [...seated.values()].some((seat) => seat.account_id === accountId);
  }

  /** The terminal currently on the glass, or null when none is. */
  function shownView(): SessionView | null {
    return shownAccount === null
      ? null
      : (views.get(deps.seatKey(deps.shownTopicId(), shownAccount)) ?? null);
  }

  /** What a view is called now — its account's current name, renames included. */
  function viewName(view: SessionView): string {
    return deps.accounts.find((account) => account.id === view.accountId)?.name || view.name;
  }

  /**
   * Lay out the terminal that is showing, and tell its session the new size.
   *
   * Only that one. A hidden pane has no size to fit against, and a session told
   * it has zero columns draws for a window it does not have.
   */
  function fitShown(): void {
    const view = shownView();
    if (!view || deps.diagnosticsEl.hidden) return;
    try {
      view.fit.fit();
    } catch {
      // A fit against a zero-sized container is not worth a message.
      return;
    }
    showWindowSize();
    if (view.ptyId === "" || view.ended !== null) return;
    void deps.invoke("resize_pty", {
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
    if (!deps.diagnosticsEl.hidden) return;
    deps.diagnosticsEl.hidden = false;
    try {
      view.fit.fit();
    } catch {
      // Same as `fitShown`: the default size is what the PTY starts at then.
    } finally {
      deps.diagnosticsEl.hidden = true;
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

  /**
   * Show the terminals of the topic on the glass, and the pane that topic was
   * last watching.
   *
   * The other topics' terminals are hidden, never discarded. Their sessions are
   * running (#141, decision 2), their output keeps arriving into their own
   * emulators, and coming back finds each scrollback where it was left.
   */
  function showTopicTerminals(): void {
    const remembered = shownByTopic.get(deps.shownTopicId()) ?? null;
    const here = topicViews();
    const pick =
      remembered !== null && here.some((view) => view.accountId === remembered)
        ? remembered
        : (here.pop()?.accountId ?? null);
    showView(pick);
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
    deps.renderPanel();
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
      deps.renderPanel();
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
   * One tab: an open terminal, named by the account it belongs to.
   *
   * The name rather than the command it was launched from. The command is on the
   * row's `title` and in the account's own form, and a strip of `claude` repeated
   * once per session tells two sessions apart by nothing at all.
   *
   * The colour is the account's, the same one its lines carry in the room and its
   * circle carries in the panel — which is what lets a tab and a row be read as one
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
    const account = deps.accounts.find((one) => one.id === view.accountId) ?? null;
    const shown = view.accountId === shownAccount;

    const tab = document.createElement("div");
    tab.className = "tab";
    // Never oneself: a terminal belongs to a session, and the person at this
    // screen is not launched (`start_session` refuses an `admin` account).
    tab.style.setProperty("--speaker", deps.speakerColor(name, account?.hue ?? null, false));
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
      close.appendChild(deps.icon("close"));
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
    deps.renderPanel();
    renderSessionFacts();
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
      const held = await deps.invoke<SeatedAccount[]>("seated_accounts");
      seated = new Map(held.map((seat) => [deps.seatKey(seat.topic_id, seat.account_id), seat]));
    } catch {
      // The panel keeps the last answer rather than declaring everyone offline
      // on a failed read. It still redraws: what failed is this one value, and
      // whatever else moved since the last draw is not held back by it.
    }
    await adoptSeats();
    deps.renderPanel();
    // The list marks the topics holding a seat, and this is the answer that
    // changed (#141, AI 判断5).
    deps.renderTopics();
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
      if (!seat.session || views.has(deps.seatKey(seat.topic_id, seat.account_id))) continue;
      const account = deps.accounts.find((one) => one.id === seat.account_id);
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
    deps.revealDiagnostics();
    deps.status(`${adopted.join("、")} の端末に繋ぎ直しました。再読み込みより前の出力は残っていません。`);
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
    startedEl.textContent = view.startedAt === "" ? "—" : deps.shortTime(view.startedAt);
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
   *
   * Floored, not rounded (#370): 100% has to mean the limit is reached. Rounded,
   * 99.5% read as 100% while the provider still had 1% left to give.
   */
  function usedPercent(value: number | null): string {
    return value === null ? "—" : `${Math.floor(value)}%`;
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
   *
   * The one exception is the reset text of the 5-hour and weekly windows (#306,
   * #320): it is kept against the clock, not against reports, and is taken off
   * once the reset passes (`resetIn`, `resetAt`, `scheduleResetTick`).
   */
  function renderSessionStats(): void {
    const stats = shownView()?.stats ?? null;
    const now = Date.now();
    statsEls.model.textContent = stats?.model ?? "—";
    statsEls.effort.textContent = stats?.effort ?? "—";
    renderUsage(
      statsEls.five_hour,
      stats?.five_hour ?? null,
      resetIn(stats?.five_hour_resets_at ?? null, now),
    );
    renderUsage(
      statsEls.seven_day,
      stats?.seven_day ?? null,
      resetAt(stats?.seven_day_resets_at ?? null, now),
    );
    renderUsage(statsEls.context, stats?.context ?? null);
    scheduleResetTick(stats, now);
  }

  /**
   * The time left until the 5-hour window resets, as its row says it beside the
   * number (#306, #320): `4時間10分後にリセット`, the form Claude Desktop shows.
   *
   * Counted in whole minutes, rounded up, so the line never says 0分 while the
   * window is still running. A day or more drops the minutes, which nobody reads
   * a weekly window that closely for.
   *
   * `null` — no line — when no reset was reported, and once the reset has
   * passed. The value is not shown stale: the window has started over, and what
   * the next report says about it is the CLI's to tell.
   */
  function resetIn(resetsAt: number | null, now: number): string | null {
    if (resetsAt === null) return null;
    const left = resetsAt * 1000 - now;
    if (!(left > 0)) return null;
    const minutes = Math.ceil(left / 60_000);
    const days = Math.floor(minutes / 1440);
    const hours = Math.floor((minutes % 1440) / 60);
    const rest = minutes % 60;
    let text: string;
    if (days > 0) text = hours > 0 ? `${days}日${hours}時間` : `${days}日`;
    else if (hours > 0) text = rest > 0 ? `${hours}時間${rest}分` : `${hours}時間`;
    else text = `${rest}分`;
    return `${text}後にリセット`;
  }

  /** The one-character weekday `resetAt` names, Sunday first as `getDay` counts. */
  const WEEKDAYS = ["日", "月", "火", "水", "木", "金", "土"];

  /**
   * When the weekly window resets, as its row says it beside the number (#320):
   * `2:00 (月) にリセット`, local time with the weekday, the form Claude Desktop
   * shows for its weekly row. The hour carries no leading zero; the minutes do.
   *
   * A time and not a countdown: a week away is read as a day and an hour, not as
   * a number of hours. The text only changes when it goes — `null`, no text, when
   * no reset was reported and once the reset has passed, as `resetIn`.
   */
  function resetAt(resetsAt: number | null, now: number): string | null {
    if (resetsAt === null) return null;
    if (!(resetsAt * 1000 - now > 0)) return null;
    const at = new Date(resetsAt * 1000);
    const minutes = String(at.getMinutes()).padStart(2, "0");
    return `${at.getHours()}:${minutes} (${WEEKDAYS[at.getDay()]}) にリセット`;
  }

  /** The timer that redraws the rows when a reset text next changes (#306). */
  let resetTick: number | null = null;

  /**
   * Redraw the rows at the next moment one of their reset lines changes: when
   * the time left of either window next crosses a whole minute, which is also
   * the moment a passed reset leaves the screen. One timer, for the pane on the
   * glass; nothing is scheduled while no reset is counting down.
   */
  function scheduleResetTick(stats: SessionStats | null, now: number): void {
    if (resetTick !== null) {
      window.clearTimeout(resetTick);
      resetTick = null;
    }
    const lefts = [stats?.five_hour_resets_at, stats?.seven_day_resets_at]
      .filter((at): at is number => typeof at === "number")
      .map((at) => at * 1000 - now)
      .filter((left) => left > 0);
    if (lefts.length === 0) return;
    const next = Math.min(...lefts.map((left) => left % 60_000 || 60_000));
    // A little past the crossing, so the redraw lands on the far side of it.
    resetTick = window.setTimeout(() => {
      resetTick = null;
      renderSessionStats();
    }, next + 50);
  }

  /**
   * One usage row: the number, and a bar under the same value (#225).
   *
   * `—` with no bar while nothing has been reported — an empty bar would say 0%,
   * and not yet knowing is not that (#155). The bar is capped at full; the number
   * is not, because a spend limit can go past 100% and the number is what says by
   * how much. At 100% or over the bar takes the danger colour, the line 制限中
   * stands on (#161).
   *
   * Laid out as Claude Desktop lays it out (#320): the reset text, when there is
   * one, and the number make one line at the right of the label; the bar is a
   * line of its own under them, across the panel (`.usage-row` in the styles).
   * `reset` is given for the 5-hour and weekly rows (`resetIn`, `resetAt`) and is
   * left out on a row reading `—`. Where the line is too narrow the reset text
   * gives way with an ellipsis, the whole of it kept in its title; the number
   * never does.
   */
  function renderUsage(cell: HTMLElement, value: number | null, reset: string | null = null): void {
    const line = document.createElement("span");
    line.className = "line";
    const text = document.createElement("span");
    text.className = "value";
    text.textContent = usedPercent(value);
    if (value === null) {
      line.appendChild(text);
      cell.replaceChildren(line);
      return;
    }
    if (reset !== null) {
      const until = document.createElement("span");
      until.className = "reset";
      until.textContent = reset;
      until.title = reset;
      line.appendChild(until);
    }
    line.appendChild(text);
    const meter = document.createElement("span");
    meter.className = "meter";
    const fill = document.createElement("span");
    fill.className = "fill";
    fill.style.width = `${Math.min(100, Math.max(0, value))}%`;
    if (value >= 100) fill.dataset.kind = "error";
    meter.appendChild(fill);
    cell.replaceChildren(line, meter);
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
      id = deps.topics.find((topic) => topic.topic_id === view.topicId)?.sessions[view.accountId] ?? null;
    } else if (!view) {
      // No pane on the glass: after a restart there is none, and the record is
      // still there to read (#144, decision 1). Whose record is `idleAccount`'s
      // answer; the topic is the one open, because no launch has named another.
      const accountId = idleAccount();
      const topic = deps.topics.find((one) => one.topic_id === deps.shownTopicId());
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
    const launched = deps.members()
      .filter((row) => row.account !== null && deps.launches(row.account))
      .sort((a, b) => deps.memberName(a).localeCompare(deps.memberName(b)))
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
      deps.status(`セッション ID をコピーしました: ${id}`);
    } catch (err) {
      deps.status(`セッション ID をコピーできませんでした: ${err}`, "error");
    }
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
      await deps.invoke("kill_pty", { id: view.ptyId });
    } catch (err) {
      deps.renderPanel();
      deps.status(`${name} を終了できませんでした: ${err}`, "error");
      return;
    }
    deps.status(`${name} を終了しました。`);
    // The exit event marks the view ended and redraws the row; this call only
    // says the kill was delivered.
    await refreshSeats();
    deps.renderPanel();
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
    const key = deps.seatKey(topicId, account.id);
    // A relaunch replaces the previous run's pane. Two panes for one account in
    // one topic would be two rows under one name, and the row is what the
    // operations hang on; the scrollback that goes with it is the one the person
    // just decided to start over from.
    discardView(views.get(key));

    const host = document.createElement("div");
    host.className = "term";
    deps.terminalEl.appendChild(host);

    // At the size this screen is set to, not at the default in the options: a
    // terminal opened after the size was changed would otherwise be the one pane
    // that is a different size from the rest.
    const term = deps.createTerminal({ ...TERMINAL_OPTIONS, fontSize: terminalFontSize });
    const fit = deps.createFitAddon();
    term.loadAddon(fit);
    term.open(host);
    deps.useWebglRenderer(term);
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
      activity: null,
      quiet: undefined,
    };

    term.onData((data) => {
      // This view's own session, never "the session that started last". The
      // terminal being typed into is the one on the glass, and the two were not
      // the same thing while one `activePtyId` stood for both (#57).
      if (view.ptyId === "" || view.ended !== null) return;
      void deps.invoke("write_pty", { id: view.ptyId, data }).catch((err) => {
        deps.status(`セッションへ送れませんでした: ${err}`, "error");
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
          void deps.invoke("write_pty", { id: view.ptyId, data: text });
        }
      });
      return false;
    });

    views.set(key, view);
    if (topicId === deps.shownTopicId()) showView(account.id);
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
    views.delete(deps.seatKey(view.topicId, view.accountId));
    if (view.topicId === deps.shownTopicId() && shownAccount === view.accountId) shownAccount = null;
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
    const topicId = deps.shownTopicId();
    for (const view of views.values()) {
      view.host.hidden = view.topicId !== topicId || view.accountId !== accountId;
    }
    deps.renderPanel();
    renderSessionFacts();
    fitShown();
    // A pane that was `display: none` kept filling its buffer and painted
    // nothing, so coming back to it has to repaint from the buffer. The fit above
    // does that only when the measured size changed, and returning to a pane the
    // same size as the one just left is exactly when it did not.
    const shown = shownView();
    if (shown) shown.term.refresh(0, shown.term.rows - 1);
  }

  /** How an ended session's end reads in a parenthesis: the code, or 終了 alone. */
  function endedNote(view: SessionView): string {
    return view.endRequested ? "終了" : view.ended ?? "";
  }

  async function attachSession(view: SessionView, ptyId: string): Promise<void> {
    const ownsSession = () => views.get(deps.seatKey(view.topicId, view.accountId)) === view && view.ptyId === ptyId && view.ended === null;
    const dataOff = await deps.listen<string>(`pty-data-${ptyId}`, (event) => {
      if (!ownsSession()) return;
      view.term.write(event.payload);
      // The bytes go to the emulator and are not looked at here. That this
      // chunk arrived is the whole of the signal (#82).
      markOutput(view);
    });
    if (!ownsSession()) { dataOff(); return; }
    view.unlisten.push(dataOff);
    // From here the silence is being timed. A session that has printed nothing
    // yet reaches 待機 after one quiet window, the same as one that stopped (#148).
    if (view.ended === null && view.quiet === undefined) armQuiet(view);
    const exitOff = await deps.listen<PtyExit>(`pty-exit-${ptyId}`, (event) => {
      if (!ownsSession()) return;
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
      if (requested) deps.status(`${name} を終了しました。`);
      else deps.status(`${name} が終了しました（${detail}）。端末を確認してください。`, "error");
      // A resume that ended on its own without the room ever having seen it
      // went back into a session that is not there. The record it went in on is
      // what every later launch into this topic will fail on the same way, so
      // it goes (#127).
      void dropDeadResume(view, event.payload, detail);
      // The seat this account held is free the moment its session ends, so the
      // panel says 未起動 again and the account can be started once more.
      void refreshSeats();
      deps.renderPanel();
      if (view === shownView()) {
        renderSessionFacts();
        // Only for an end nobody asked for: what it printed on the way out is
        // the account of why. An end asked for leaves the pane as it was — not
        // opened, and not closed either if it was already open (#121).
        if (!requested) deps.revealDiagnostics();
      }
    });
    if (!ownsSession()) { exitOff(); return; }
    view.unlisten.push(exitOff);
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
      const dropped = await deps.invoke<boolean>("room_forget_session", {
        topicId: view.topicId,
        accountId: view.accountId,
        sessionId: dead,
      });
      if (!dropped) return;
      deps.status(
        `${name} は会話へ戻れないまま終了しました（${detail}）。このトピックの再開先を外したので、次は通常の起動になります。`,
        "error",
      );
    } catch (err) {
      // The record is still there, which means the next launch fails the same
      // way. Saying so is the whole of what is left to do here — a repair that
      // failed quietly reads as a repair that happened.
      deps.status(`${name} の再開先を外せませんでした: ${err}`, "error");
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
    deps.renderPanel();
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
    if (view === shownView() && !deps.diagnosticsEl.hidden) view.term.focus();
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
      deps.status(
        "アカウントの名前を入力してください。部屋での名乗りになります。",
        "error",
      );
      deps.openAccountDialog(account);
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
    const topicId = deps.shownTopicId();
    const key = deps.seatKey(topicId, account.id);
    if (seated.has(key)) await refreshSeats();
    if (seated.has(key)) {
      deps.status(
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
      deps.status(
        `「${name}」に作業ディレクトリがありません。編集から設定してください。`,
        "error",
      );
      deps.openAccountDialog(account);
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

    deps.status(`${name} を起動しています…`);
    try {
      const started = await deps.invoke<StartedSession>("start_session", {
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
      deps.status(
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
      if (topicId === deps.shownTopicId()) {
        showView(
          previous !== null && views.has(deps.seatKey(topicId, previous))
            ? previous
            : (topicViews().pop()?.accountId ?? null),
        );
      }
      deps.status(`${name} を起動できませんでした: ${err}`, "error");
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
      deps.renderPanel();
    }
  }

  /** The page's own colours, so a terminal is not a light rectangle in the dark. */
  function terminalTheme(): { background: string; foreground: string; } {
    const style = getComputedStyle(document.documentElement);
    return {
      background: style.getPropertyValue("--bg").trim() || "#17171a",
      foreground: style.getPropertyValue("--fg").trim() || "#e8e8ea",
    };
  }

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

  const tabsEl = document.getElementById("terminal-tabs") as HTMLElement;

  function getView(key: string) { return views.get(key); }
  function allViews() { return [...views.values()]; }
  function hasView(key: string) { return views.has(key); }
  function allSeats() { return [...seated.values()]; }
  function hasSeat(key: string) { return seated.has(key); }
  function selectedAccount() { return shownAccount; }
  function rememberSelection(topicId: string) { shownByTopic.set(topicId, shownAccount); }
  function forgetSelection(topicId: string) { shownByTopic.delete(topicId); }
  function launchFailure(key: string) { return launchFailures.get(key); }
  function setTerminalFontSize(size: number) { terminalFontSize = size; for (const view of views.values()) view.term.options.fontSize = size; }
  function wireSessionIdCopy() { sessionIdCopyEl.addEventListener("click", () => void copySessionId()); }
  function receiveStats(stats: SessionStats): void {
    const view = getView(deps.seatKey(stats.topic_id, stats.account_id));
    if (!view || !statsForView(stats, view)) return;
    const was = limitedByUsage(view.stats);
    view.stats = stats;
    if (view === shownView()) renderSessionStats();
    if (limitedByUsage(view.stats) !== was) deps.renderPanel();
  }

  function receiveActivity(activity: SeatActivity): void {
    const view = getView(deps.seatKey(activity.topic_id, activity.account_id));
    if (!view) return;
    // The last run's server going down after this seat was launched again.
    if (view.ptyId !== "" && view.ptyId !== activity.pty_id) return;
    const was = view.activity;
    const now = activity;
    view.activity = now;
    if (
      was?.connected !== now.connected ||
      was?.thread_status !== now.thread_status ||
      was?.word !== now.word ||
      was?.line !== now.line ||
      was?.waiting !== now.waiting
    ) {
      deps.renderPanel();
    }
  }

  function receiveRoster(topicId: string, joined: Participant[]): void {
    for (const view of allViews()) {
      if (view.seenInRoom || view.topicId !== topicId) continue;
      if (joined.some((one) => one.account === view.accountId)) view.seenInRoom = true;
    }
  }
  return { receiveStats, receiveActivity, receiveRoster, wireSessionIdCopy, getView, allViews, hasView, allSeats, hasSeat, selectedAccount, rememberSelection, forgetSelection, launchFailure, setTerminalFontSize, seatedAnywhere, shownView, viewName, fitShown, showTopicTerminals, renderTerminalTabs, refreshSeats, renderSessionFacts, renderSessionStats, renderSessionId, endSession, discardView, showView, startSession };
}
