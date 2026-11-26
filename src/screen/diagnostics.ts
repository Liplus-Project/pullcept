export interface Dependencies {
  diagnosticsEl: HTMLElement;
  toggleEl: HTMLButtonElement;
  socketStateEl: HTMLElement;
  factsMoreEl: HTMLDetailsElement;
  fitShown: () => void;
}

export function createDiagnostics(deps: Dependencies) {

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
    deps.diagnosticsEl.hidden = false;
    deps.toggleEl.setAttribute("aria-expanded", "true");
    rememberDiagnostics(true);
    // The container has no size while hidden, so the fit has to wait for layout.
    requestAnimationFrame(() => deps.fitShown());
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
    deps.diagnosticsEl.hidden = true;
    deps.toggleEl.setAttribute("aria-expanded", "false");
    rememberDiagnostics(false);
  }

  function renderSocket(port: number | null, error?: string): void {
    // The socket's row is folded under 詳細 (#225), and a fold must not hide that
    // the room is not listening: the summary takes the error colour with it.
    deps.factsMoreEl.dataset.kind = error || port === null ? "error" : "";
    if (error) {
      deps.socketStateEl.textContent = error;
      deps.socketStateEl.dataset.kind = "error";
      return;
    }
    if (port === null) {
      deps.socketStateEl.textContent = "未待受";
      deps.socketStateEl.dataset.kind = "error";
      return;
    }
    // The address alone. It is an address, and the panel column is one line
    // wide; that it is being listened on is what the accent colour says.
    deps.socketStateEl.textContent = `127.0.0.1:${port}`;
    deps.socketStateEl.dataset.kind = "ok";
  }

  return { revealDiagnostics, hideDiagnostics, renderSocket, diagnosticsWereOpen };
}
