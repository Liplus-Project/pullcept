import type { AccountKind, Member, RowWord, Account, Participant, IconName, SessionView, McpServerView } from "../contracts";
import { accountName } from "../account-name";
import { NO_WORD, activityNote } from "../seat-status";
import { openCodexRemote } from "../codex-remote";
import { invoke } from "@tauri-apps/api/core";
import type { createParticipantDetails } from "../participant-details";
import type { createSessionController } from "../session-controller";

export interface Dependencies {
  launches: (account: Account) => boolean;
  HUES: { label: string; hue: number; }[];
  icon: (name: IconName) => SVGSVGElement;
  rosterEl: HTMLElement;
  accountMenuEl: HTMLElement;
  mentionListEl: HTMLUListElement;
  diagnosticsEl: HTMLElement;
  participantDetails: ReturnType<typeof createParticipantDetails>;
  accounts: Account[];
  rosters: Map<string, Participant[]>;
  localAccountId: string;
  awaiting: Map<string, Set<string>>;
  seatKey: (topicId: string, accountId: string) => string;
  shownTopicId: () => string;
  shownRoster: () => Participant[];
  revealDiagnostics: () => void;
  localAccount: () => Account | null;
  saveConfig: () => Promise<boolean>;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  avatar: (name: string, accountId?: string | null, app?: boolean) => HTMLElement;
  paintMine: () => void;
  renderRoomTitle: () => void;
  pruneAwaiting: (topicId: string) => void;
  renderRoomBusy: (rows: Member[]) => void;
  menuAccountId: string | null;
  refreshMentions: () => void;
  sessions: ReturnType<typeof createSessionController>;
  renderTerminalTabs: () => void;
  showView: (accountId: string | null) => void;
  startSession: (account: Account) => Promise<void>;
  viewName: (view: SessionView) => string;
  mcpNote: (view: McpServerView | undefined) => { text: string; kind: string; title: string; };
  mcpServerOf: (account: Account) => { name: string; view: McpServerView | undefined; };
  openAccountDialog: (account: Account | null, field?: "name" | "hue") => void;
  openEndDialog: (key: string, name: string) => void;
}

export function createParticipants(deps: Dependencies) {

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

    for (const account of deps.accounts) {
      // Plural on purpose. One account holding two seats is refused by the app
      // that launches it (`RoomSeats`), and this list is not the place to enforce
      // that: something joining from elsewhere could carry the same id, and
      // dropping the second one would hide a participant who is genuinely there.
      const matches = deps.shownRoster().filter((one) => one.account === account.id);
      for (const participant of matches) {
        placed.add(participant.id);
        rows.push({ account, participant });
      }
      if (!matches.length) rows.push({ account, participant: null });
    }

    for (const participant of deps.shownRoster()) {
      if (!placed.has(participant.id)) rows.push({ account: null, participant });
    }
    return rows;
  }

  /** What a row is called: the room's name while it is in the room. */
  function memberName(row: Member): string {
    return row.participant?.name ?? (row.account ? accountName(row.account) : "");
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
    if (row.account?.kind === "mcp" && deps.mcpServerOf(row.account)) return NO_WORD;
    const view = row.account ? deps.sessions.getView(deps.seatKey(deps.shownTopicId(), row.account.id)) : undefined;
    if (view != null && view.ended === null && view.ptyId === "") return NO_WORD;
    return activityNote(view, deps.awaiting.get(view?.topicId ?? "")?.has(memberName(row)) ?? false);
  }

  /**
   * A pulse's start set back to the page's own clock, so a badge or a line drawn
   * anew mid-pulse carries on in the phase the one it replaced was in, rather than
   * every redraw of the panel restarting the pulse (#307).
   */
  function pulsePhase(): string {
    return `${-Math.round(performance.now())}ms`;
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
    const view = row.account ? deps.sessions.getView(deps.seatKey(deps.shownTopicId(), row.account.id)) : undefined;
    // The server an `mcp` account is. It takes no seat, so what says it is here
    // is the server running, not the roster (#193).
    const server = row.account?.kind === "mcp" ? deps.mcpServerOf(row.account) : null;

    const entry = document.createElement("li");
    entry.className = "member";
    entry.style.setProperty("--speaker", deps.speakerColor(name, hue, own));
    if (!row.participant && server?.view?.state?.state !== "running") {
      entry.classList.add("offline");
    }

    // The circle the room draws for this speaker, drawn by the same function so
    // the two cannot drift (#253): the account's image where it has one, the
    // initial on its colour otherwise. The account is the seat's when the row is
    // in the room — what the room's lines carry — and the list's otherwise; a
    // guest declared none and keeps the initial, as its lines do. It replaces the
    // dot and takes over its part as the legend: the circle is the line's colour.
    const mark = deps.avatar(name, row.participant ? row.participant.account : (row.account?.id ?? null));

    const who = document.createElement("span");
    who.className = "who";
    who.textContent = name;
    const original = row.participant?.full_name ?? row.account?.name;
    if (original && original !== name) {
      const full = document.createElement("small");
      full.className = "original-name";
      full.textContent = original;
      who.appendChild(full);
    }
    who.title = name;

    // Asked for and not back yet. The view exists from the moment 開始 is pressed
    // and its pty id arrives with the session, so this pair is exactly that
    // window (`startSession`).
    const launching = view != null && view.ended === null && view.ptyId === "";
    const failure = row.account
      ? deps.sessions.launchFailure(deps.seatKey(deps.shownTopicId(), row.account.id))
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
      ({ text: noteText, kind: noteKind, title: noteTitle } = deps.mcpNote(server.view));
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

    // A row selects its work and, when present, its existing terminal (#513).
    const button = document.createElement("button");
    button.type = "button"; button.className = "pick";
    const detailId = row.account?.id ?? row.participant?.id ?? null;
    button.setAttribute("aria-pressed", String(deps.participantDetails.selected() === detailId));
    button.title = `${name} の作業と判断待ち`;
    button.addEventListener("click", () => {
      deps.participantDetails.select(detailId, name, row.account?.server ?? undefined, row.account?.kind);
      if (view) {
        const folded = deps.diagnosticsEl.hidden;
        deps.showView(view.accountId);
        if (!folded) view.term.focus();
      } else renderPanel();
    });
    if (detailId != null && deps.participantDetails.selected() === detailId) entry.classList.add("detail-selected");
    const pick: HTMLElement = button;
    pick.append(mark, who);
    const notificationCount = detailId ? deps.participantDetails.count(detailId, row.account?.server ?? undefined, row.account?.kind) : undefined;
    if (notificationCount != null && notificationCount > 0) {
      const count = document.createElement("span"); count.className = "notification-count";
      count.textContent = String(notificationCount);
      count.title = `${row.account?.kind === "mcp" ? "未処理" : "未完了"} ${notificationCount} 件`;
      count.setAttribute("aria-label", count.title); pick.append(count);
    }
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
    if (view?.accountId === deps.sessions.selectedAccount()) entry.classList.add("shown");

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
    } else if (row.account && deps.launches(row.account)) {
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
    start.appendChild(deps.icon("start"));
    start.disabled = launching;
    const label = launching
      ? `${account.name} を起動しています`
      : `${account.name} のセッションを開始する`;
    start.title = label;
    start.setAttribute("aria-label", label);
    start.addEventListener("click", () => void deps.startSession(account));
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
    stop.appendChild(deps.icon("stop"));
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
    more.appendChild(deps.icon("more"));
    more.title = `${account.name} の操作`;
    more.setAttribute("aria-label", `${account.name} の操作`);
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-expanded", String(deps.menuAccountId === account.id));
    more.addEventListener("click", () => {
      // A second press on the button whose menu is open closes it, the way a
      // menu button does.
      if (deps.menuAccountId === account.id) {
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
    const view = deps.sessions.getView(deps.seatKey(deps.shownTopicId(), account.id));
    deps.accountMenuEl.replaceChildren();

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
      deps.accountMenuEl.appendChild(button);
      return button;
    };

    if (view) {
      item("端末を開く", deps.icon("terminal"), () => openTerminalOf(account.id));
    }
    if (view && view.ended === null && view.ptyId && account.kind === "codex_cli") {
      item("スマホから接続", deps.icon("terminal"), () => openCodexRemote(view.ptyId, accountName(account), () => view.ended === null && deps.sessions.getView(deps.seatKey(view.topicId, account.id)) === view));
    }
    item("編集", deps.icon("edit"), () => openAccountFrom(account.id, "name"));
    // A swatch in the account's own colour where the others have a drawing: it
    // says what the item changes by showing it.
    const swatch = document.createElement("span");
    swatch.className = "swatch";
    swatch.setAttribute("aria-hidden", "true");
    item("色を変える", swatch, () => openAccountFrom(account.id, "hue"));
    const canStart = deps.launches(account) && (!view || view.ended !== null);
    const canEnd = view != null && view.ended === null && view.ptyId !== "";
    if (canStart || canEnd) {
      const divider = document.createElement("div");
      divider.setAttribute("role", "separator");
      deps.accountMenuEl.appendChild(divider);
    }
    if (canStart) {
      item("開始", deps.icon("start"), () => startFromMenu(account.id));
    }
    if (canEnd) {
      item("終了", deps.icon("stop"), () => endFromMenu(account.id)).classList.add("danger");
    }

    // The colour the row draws the account in, so the swatch is the row's own.
    const row = document.querySelector<HTMLElement>(
      `#roster button.more[data-account="${CSS.escape(account.id)}"]`,
    )?.closest<HTMLElement>(".member");
    deps.accountMenuEl.style.setProperty(
      "--speaker",
      row?.style.getPropertyValue("--speaker") || "var(--accent)",
    );
    deps.accountMenuEl.setAttribute("aria-label", `${account.name} の操作`);

    deps.menuAccountId = account.id;
    deps.accountMenuEl.hidden = false;
    // Placed after it is shown, because a hidden menu has no size to keep inside
    // the window. Pulled back from the right and bottom edges rather than
    // flipped, which keeps the menu's corner at the point wherever there is room.
    const width = deps.accountMenuEl.offsetWidth;
    const height = deps.accountMenuEl.offsetHeight;
    const x = Math.max(0, Math.min(at.x, window.innerWidth - width - 4));
    const y = Math.max(0, Math.min(at.y, window.innerHeight - height - 4));
    deps.accountMenuEl.style.left = `${x}px`;
    deps.accountMenuEl.style.top = `${y}px`;
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
    for (const more of deps.rosterEl.querySelectorAll<HTMLButtonElement>("button.more")) {
      more.setAttribute("aria-expanded", String(more.dataset.account === deps.menuAccountId));
    }
  }

  /** The menu's items, in the order they are drawn. */
  function menuItems(): HTMLButtonElement[] {
    return [...deps.accountMenuEl.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
  }

  /**
   * Close the menu. With `restore`, the focus goes back to the row's menu button,
   * which is what Escape does; a click elsewhere or a chosen item leaves the focus
   * where that put it.
   */
  function closeAccountMenu(restore: boolean): void {
    if (deps.menuAccountId === null) return;
    const id = deps.menuAccountId;
    deps.menuAccountId = null;
    deps.accountMenuEl.hidden = true;
    deps.accountMenuEl.replaceChildren();
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
    const view = deps.sessions.getView(deps.seatKey(deps.shownTopicId(), accountId));
    if (!view) return;
    deps.showView(accountId);
    deps.revealDiagnostics();
    view.term.focus();
  }

  /** 編集 and 色を変える: the account's form, on the field the item names. */
  function openAccountFrom(accountId: string, field: "name" | "hue"): void {
    const account = deps.accounts.find((one) => one.id === accountId);
    if (account) deps.openAccountDialog(account, field);
  }

  /** 開始 from the menu: the row's ▶, for the account as it is now. */
  function startFromMenu(accountId: string): void {
    const account = deps.accounts.find((one) => one.id === accountId);
    if (!account || !deps.launches(account)) return;
    // The session may have been started, from the row or another menu, while
    // the menu stood. `startSession` refuses a held seat as well; this keeps the
    // press from reaching that refusal.
    const view = deps.sessions.getView(deps.seatKey(deps.shownTopicId(), accountId));
    if (view && view.ended === null) return;
    void deps.startSession(account);
  }

  /**
   * 終了 from the row or the menu: the question `#end-dialog` asks, about the
   * session as it is now.
   */
  function endFromMenu(accountId: string): void {
    const key = deps.seatKey(deps.shownTopicId(), accountId);
    const view = deps.sessions.getView(key);
    // The session may have ended, or the topic changed, while the menu stood.
    if (!view || view.ended !== null || view.ptyId === "") return;
    deps.openEndDialog(key, deps.viewName(view));
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
    const selection = deps.participantDetails.selection();
    const ai = selection && selection.id != null && selection.kind !== "admin" && selection.kind !== "mcp" && selection.id !== deps.localAccountId && !selection.server;
    (document.getElementById("terminal-facts") as HTMLElement).hidden = selection != null && !ai;
    (document.getElementById("terminal-diagnostics") as HTMLElement).hidden = selection != null && (!ai || !deps.sessions.getView(deps.seatKey(deps.shownTopicId(), selection.id!)));
    deps.sessions.renderSessionFacts();
    // The title bar's count is this roster's length (#225).
    deps.renderRoomTitle();
    // The tabs are redrawn here rather than on their own schedule. Both surfaces
    // read `views` and both mark the same selection, so drawing them from one call
    // is what makes "they move together" true by construction (#68).
    deps.renderTerminalTabs();
    deps.rosterEl.replaceChildren();
    const rows = members();
    // The line under the room reads the same rows, at the same moments (#307).
    deps.renderRoomBusy(rows);

    if (!rows.length) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = "参加者なし";
      deps.rosterEl.appendChild(empty);
      if (!deps.mentionListEl.hidden) deps.refreshMentions();
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
      deps.rosterEl.appendChild(heading);

      for (const row of inGroup) deps.rosterEl.appendChild(memberRow(row));
    }

    // An open list follows the room: someone who left is not offered, someone who
    // came is. A shut one stays shut — it opens on `@`, not on the roster.
    if (!deps.mentionListEl.hidden) deps.refreshMentions();
  }

  /**
   * Take the room's roster and redraw the panel around it.
   *
   * The one door the roster comes in by, the event and the first read alike. A
   * second assignment to `participants` elsewhere would be a roster that arrived
   * without the two readings below happening to it.
   */
  function renderRoster(topicId: string, joined: Participant[]): void {
    deps.rosters.set(topicId, joined);
    // Against the roster that just arrived, before it is drawn: who the room is
    // waiting on is only meaningful about someone who is in it (#82).
    deps.pruneAwaiting(topicId);
    // The other reading of the same roster: which sessions have arrived at all.
    // Kept on the view rather than asked at the exit, because by then the
    // connection is gone and the roster no longer remembers it was there (#127).
    // This room's terminals only: the same account arriving in another topic is
    // another session arriving (#141).
    deps.sessions.receiveRoster(topicId, joined);
    if (topicId === deps.shownTopicId()) renderPanel();
  }

  /** The name this screen posts under, and is listed in the roster under. */
  function localName(): string {
    const account = deps.localAccount();
    return account ? accountName(account) : "human";
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
    const account = deps.localAccount();
    // On the glass first: the lines already drawn are this screen's to repaint,
    // and they do not wait on the room answering (#189).
    deps.paintMine();
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
      deps.accounts.find((one) => one.id === stored && one.kind === "admin") ??
      deps.accounts.find((one) => one.kind === "admin") ??
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
        hue: savedHue !== null && deps.HUES.some(({ hue }) => String(hue) === savedHue)
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
      deps.accounts.push(account);
      deps.saveConfig();
    }

    deps.localAccountId = account.id;
    localStorage.setItem(LOCAL_KEY, account.id);
    // Before a topic is read back, which at startup is ahead of `join` (#189).
    deps.paintMine();
  }

  return { renderPanel, localName, rowActivity, memberName, pulsePhase, renderRoster, onAccountMenuKey, closeAccountMenu, resolveLocalAccount, join, members };
}
