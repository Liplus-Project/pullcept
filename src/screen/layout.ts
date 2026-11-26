import type { PanelState } from "../contracts";

export interface Dependencies {
  historyEl: HTMLElement;
  participantsEl: HTMLElement;
  toggleHistoryEl: HTMLButtonElement;
  toggleParticipantsEl: HTMLButtonElement;
  panels: PanelState;
  saveConfig: () => Promise<boolean>;
}

export function createLayout(deps: Dependencies) {

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
    deps.historyEl.hidden = !deps.panels.history;
    deps.participantsEl.hidden = !deps.panels.participants;
    deps.toggleHistoryEl.setAttribute("aria-expanded", String(deps.panels.history));
    deps.toggleParticipantsEl.setAttribute("aria-expanded", String(deps.panels.participants));
  }

  /**
   * Fold one panel away, or bring it back, and remember which.
   *
   * The write goes to disk on every press rather than at the end of the session:
   * the app is closed by the window's `✕` and by whatever ends it less politely,
   * and a layout only saved on the way out is one the second of those loses.
   */
  function togglePanel(which: keyof PanelState): void {
    deps.panels = { ...deps.panels, [which]: !deps.panels[which] };
    applyPanels();
    deps.saveConfig();
  }

  return { togglePanel, applyPanels };
}
