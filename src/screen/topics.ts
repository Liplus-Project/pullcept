import type { Topic, LoggedPost, TopicRef, Participant, SeatedAccount, Account, IconName, SessionView } from "../contracts";
import { invoke } from "@tauri-apps/api/core";
import type { createParticipantDetails } from "../participant-details";
import type { createHtmlPreview } from "../html-preview";
import type { createSessionController } from "../session-controller";

export interface Dependencies {
  icon: (name: IconName) => SVGSVGElement;
  topicListEl: HTMLElement;
  topicNewEl: HTMLButtonElement;
  topicMenuEl: HTMLElement;
  participantDetails: ReturnType<typeof createParticipantDetails>;
  topicDeleteDialogEl: HTMLDialogElement;
  accounts: Account[];
  rosters: Map<string, Participant[]>;
  topics: Topic[];
  currentTopic: TopicRef | null;
  deletingTopic: Topic | null;
  awaiting: Map<string, Set<string>>;
  shownTopicId: () => string;
  status: (text: string, kind?: "info" | "error") => void;
  dayLabel: (iso: string) => string;
  fullDateTime: (iso: string) => string;
  drawTopic: (posts: LoggedPost[]) => void;
  renderRoomTitle: () => void;
  renderRoster: (topicId: string, joined: Participant[]) => void;
  htmlPreview: ReturnType<typeof createHtmlPreview>;
  sessions: ReturnType<typeof createSessionController>;
  discardView: (view: SessionView | undefined) => void;
  refreshSeats: (restoreDiagnostics?: boolean) => Promise<void>;
  showTopicTerminals: () => void;
}

export function createTopics(deps: Dependencies) {

  const topicDeleteMessageEl = document.getElementById("topic-delete-message") as HTMLElement;
  const topicDeleteSessionsEl = document.getElementById("topic-delete-sessions") as HTMLElement;

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
    deps.topicListEl.replaceChildren();

    const listed = [...deps.topics].reverse();
    const current = deps.currentTopic;
    const unlisted = current !== null && !listed.some((one) => one.topic_id === current.topic_id);
    deps.topicNewEl.classList.toggle("current", unlisted);
    // A launch realises its topic, so a session running in a topic the list does
    // not carry is the moment between ▶ and the launch answering. 新規 is where
    // that topic is drawn, so the mark goes there for that moment (#141).
    deps.topicNewEl.classList.toggle("running", unlisted && current !== null && runsIn(current.topic_id));
    deps.renderRoomTitle();

    if (listed.length === 0) {
      const empty = document.createElement("li");
      empty.className = "empty";
      empty.textContent = "トピックなし";
      deps.topicListEl.appendChild(empty);
      return;
    }

    for (const topic of listed) deps.topicListEl.appendChild(topicRow(topic));
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
    if (topic.topic_id === deps.currentTopic?.topic_id) row.classList.add("current");

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
    when.textContent = deps.dayLabel(topic.created_at);
    when.title = deps.fullDateTime(topic.created_at);
    pick.appendChild(when);

    pick.addEventListener("click", () => void openTopic(topic));
    // Renamed where it is read. The generated title is a starting point in an
    // editable field, the same thing an account's name is (#115, decision 9).
    pick.addEventListener("dblclick", (event) => {
      event.preventDefault();
      beginRename(row, topic);
    });
    row.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      openTopicMenu(topic, { x: event.clientX, y: event.clientY }, () => beginRename(row, topic));
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
    return [...deps.sessions.allSeats()].some((seat) => seat.topic_id === topicId);
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
    remove.appendChild(deps.icon("close"));
    remove.title = `「${topicName(topic)}」を削除する`;
    remove.setAttribute("aria-label", `トピック「${topicName(topic)}」を削除する`);
    remove.addEventListener("click", () => void openTopicDeleteDialog(topic));
    return remove;
  }

  /**
   * Rename one topic in place.
   *
   * Escape and losing focus both mean 取消, Enter means 決定 — the three ways out
   * the account form has. `done` puts back what the field stood over on every
   * path, so none of them leaves half an edit applied.
   *
   * The list's rows and the title bar's heading (#469) both open this one field:
   * two places that edit one name answer the same three keys.
   */
  function renameField(topic: Topic, done: () => void): HTMLInputElement {
    const field = document.createElement("input");
    field.type = "text";
    field.className = "rename";
    field.value = topic.title;
    field.spellcheck = false;

    let settled = false;
    const cancel = (): void => {
      if (settled) return;
      settled = true;
      done();
    };
    const commit = (): void => {
      if (settled) return;
      settled = true;
      const title = field.value.trim();
      if (!title || title === topic.title) {
        done();
        return;
      }
      void invoke("room_rename_topic", { topicId: topic.topic_id, title })
        .then(() => deps.status(`トピックを「${title}」にしました。`))
        .catch((err) => deps.status(`トピック名を変えられませんでした: ${err}`, "error"))
        // The list arrives on `room-topics` when the rename lands. This redraw is
        // for the path where it did not: a field left standing over a failed
        // rename is an edit that looks like it took.
        .finally(done);
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
    return field;
  }

  /** Rename one topic from its row in the list. */
  function beginRename(row: HTMLLIElement, topic: Topic): void {
    const field = renameField(topic, () => renderTopics());
    row.replaceChildren(field);
    field.focus();
    field.select();
  }

  /**
   * A topic's right-click menu (#479): 名前を変更, for the row in the list and
   * for the title bar's heading alike. `rename` is the one each place opens its
   * own field with.
   *
   * Its own element beside `#account-menu`, drawn the same way, because that
   * menu's state is an account's — what it marks and where Escape returns the
   * focus are a roster row's.
   */
  function openTopicMenu(topic: Topic, at: { x: number; y: number }, rename: () => void): void {
    const item = document.createElement("button");
    item.type = "button";
    item.setAttribute("role", "menuitem");
    item.tabIndex = -1;
    item.append(deps.icon("edit"), "名前を変更");
    item.addEventListener("click", () => {
      closeTopicMenu();
      rename();
    });
    deps.topicMenuEl.replaceChildren(item);
    deps.topicMenuEl.setAttribute("aria-label", `トピック「${topicName(topic)}」の操作`);
    deps.topicMenuEl.hidden = false;
    const x = Math.max(0, Math.min(at.x, window.innerWidth - deps.topicMenuEl.offsetWidth - 4));
    const y = Math.max(0, Math.min(at.y, window.innerHeight - deps.topicMenuEl.offsetHeight - 4));
    deps.topicMenuEl.style.left = `${x}px`;
    deps.topicMenuEl.style.top = `${y}px`;
    item.focus();
  }

  function closeTopicMenu(): void {
    if (deps.topicMenuEl.hidden) return;
    deps.topicMenuEl.hidden = true;
    deps.topicMenuEl.replaceChildren();
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
    if (topic.topic_id === deps.currentTopic?.topic_id) return;
    try {
      await invoke("room_select_topic", {
        topicId: topic.topic_id,
        createdAt: topic.created_at,
      });
      await enterTopic({ topic_id: topic.topic_id, created_at: topic.created_at });
      deps.drawTopic(await invoke<LoggedPost[]>("room_topic_log", { topicId: topic.topic_id }));
      deps.status(`トピック「${topicName(topic)}」を開きました。`);
    } catch (err) {
      deps.status(`トピックを開けませんでした: ${err}`, "error");
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
    deps.htmlPreview.clear();
    // The pane being watched in the topic being left, kept for coming back.
    const leaving = deps.shownTopicId();
    if (leaving !== "") deps.sessions.rememberSelection(leaving);
    deps.currentTopic = topic;
    deps.participantDetails.topicChanged();
    void deps.participantDetails.refresh();
    try {
      deps.renderRoster(
        topic.topic_id,
        await invoke<Participant[]>("room_participants", { topicId: topic.topic_id }),
      );
    } catch {
      // What this screen last heard of that room stands, or nothing: a roster
      // that failed to read is not a reason to leave the topic half opened.
    }
    deps.showTopicTerminals();
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
    if (deps.deletingTopic) return;
    deps.deletingTopic = topic;
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
    deps.topicDeleteDialogEl.showModal();
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
      held = [...deps.sessions.allSeats()];
    }
    return held
      .filter((seat) => seat.session !== null && seat.topic_id === topicId)
      .map(
        (seat) =>
          deps.accounts.find((one) => one.id === seat.account_id)?.name.trim() || seat.account_id,
      );
  }

  /** Leave the question unanswered. Escape lands here too, by the close handler. */
  function closeTopicDeleteDialog(): void {
    deps.deletingTopic = null;
    if (deps.topicDeleteDialogEl.open) deps.topicDeleteDialogEl.close();
  }

  /** The answer that acts. */
  function confirmTopicDeleteDialog(): void {
    const topic = deps.deletingTopic;
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
      for (const view of [...deps.sessions.allViews()]) {
        if (view.topicId === topic.topic_id) deps.discardView(view);
      }
      if (moved) {
        await enterTopic(moved);
        deps.drawTopic([]);
      }
      // After the move, which records what the topic being left was showing.
      deps.rosters.delete(topic.topic_id);
      deps.awaiting.delete(topic.topic_id);
      deps.sessions.forgetSelection(topic.topic_id);
      renderTopics();
      deps.status(`トピック「${topicName(topic)}」を削除しました。`);
    } catch (err) {
      deps.status(`トピックを削除できませんでした: ${err}`, "error");
    }
    await deps.refreshSeats();
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
      deps.drawTopic([]);
      deps.status("新しいトピックを始めました。");
    } catch (err) {
      deps.status(`新しいトピックを始められませんでした: ${err}`, "error");
    }
  }

  /** Say on the topic list why it has nothing to show. */
  function topicsFailed(reason: string): void {
    deps.topicListEl.replaceChildren();
    const line = document.createElement("li");
    line.className = "empty";
    line.textContent = `トピックを読めませんでした: ${reason}`;
    deps.topicListEl.appendChild(line);
  }

  return { topicName, renameField, openTopicMenu, renderTopics, startNewTopic, closeTopicMenu, closeTopicDeleteDialog, confirmTopicDeleteDialog, topicsFailed };
}
