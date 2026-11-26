import type { SeatedAccount, Account, SessionView } from "../contracts";
import { invoke } from "@tauri-apps/api/core";
import type { CloseRequestedEvent } from "@tauri-apps/api/window";
import type { createSessionController } from "../session-controller";

export interface Dependencies {
  endDialogEl: HTMLDialogElement;
  quitDialogEl: HTMLDialogElement;
  accounts: Account[];
  endingAccount: string | null;
  sessions: ReturnType<typeof createSessionController>;
  endSession: (view: SessionView) => Promise<void>;
}

export function createEndConfirm(deps: Dependencies) {

  const endMessageEl = document.getElementById("end-dialog-message") as HTMLElement;
  const quitMessageEl = document.getElementById("quit-dialog-message") as HTMLElement;
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
    deps.endingAccount = key;
    endMessageEl.textContent = `${name} のセッションを終了します。よろしいですか？`;
    deps.endDialogEl.showModal();
  }

  /** Leave the question unanswered. Escape lands here too, by the close handler. */
  function closeEndDialog(): void {
    deps.endingAccount = null;
    if (deps.endDialogEl.open) deps.endDialogEl.close();
  }

  /** The answer that acts. */
  function confirmEndDialog(): void {
    const key = deps.endingAccount;
    closeEndDialog();
    if (key === null) return;
    // Resolved now, not when the dialog opened: the session may have ended on its
    // own while the question stood, and there is then nothing left to end.
    const view = deps.sessions.getView(key);
    if (view) void deps.endSession(view);
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
      held = [...deps.sessions.allSeats()];
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
            deps.accounts.find((one) => one.id === seat.account_id)?.name.trim() || seat.account_id,
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
      deps.quitDialogEl.showModal();
    });
  }

  /** Answer the standing question, once. Escape lands here too, as 取消. */
  function answerQuit(confirmed: boolean): void {
    const answer = quitAnswer;
    // Cleared before the close, because closing fires the handler that calls this
    // again: that second call finds nothing standing and does nothing. Without it
    // the answer would be delivered twice, and the second one is always 取消.
    quitAnswer = null;
    if (deps.quitDialogEl.open) deps.quitDialogEl.close();
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

  return { openEndDialog, answerQuit, onQuitRequested, closeEndDialog, confirmEndDialog };
}
