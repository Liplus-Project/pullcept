import { htmlPreviewDocument, htmlTitle, previewFrame } from "./html-preview-document";

export type DetachedPreview = (document: string | null, open: boolean) => Promise<void>;

/**
 * The name an html fence shows in the room (#436): the opening line's words
 * after `html`, else the document's `<title>`, else "HTML".
 */
export function htmlFenceName(source: string, info = ""): string {
  const named = info.trim().replace(/^\S+\s*/, "").trim();
  if (named) return named;
  try { if (htmlTitle(source)) return htmlTitle(source); } catch { /* fall through */ }
  return "HTML";
}

/** One explicitly opened preview per topic; the room shows a file-name line and the code is read here. */
export function createHtmlPreview(conversation: HTMLElement, detached: DetachedPreview,
  renderCode: (code: string) => Node[] = code => [document.createTextNode(code)]) {
  const pane = document.createElement("aside");
  pane.className = "html-preview";
  pane.setAttribute("aria-label", "HTML プレビュー");
  pane.hidden = true;
  const header = document.createElement("header");
  const title = document.createElement("strong");
  title.textContent = "HTML プレビュー";
  const separate = document.createElement("button");
  separate.type = "button";
  separate.className = "html-preview-separate";
  // Window outline with an up-right arrow, then the label (#441).
  const svgNs = "http://www.w3.org/2000/svg";
  const icon = document.createElementNS(svgNs, "svg");
  icon.setAttribute("viewBox", "0 0 16 16");
  icon.setAttribute("aria-hidden", "true");
  icon.setAttribute("focusable", "false");
  for (const d of ["M7 2.5H3.5a1 1 0 0 0-1 1v9a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1V9", "M9.5 2.5h4v4", "M13.5 2.5 8 8"]) {
    const path = document.createElementNS(svgNs, "path");
    path.setAttribute("d", d);
    icon.append(path);
  }
  const separateLabel = document.createElement("span");
  separateLabel.textContent = "新しいウィンドウで開く";
  separate.append(icon, separateLabel);
  // "プレビュー｜コード" segmented switch (#439); each side keeps aria-pressed.
  const views = document.createElement("div");
  views.className = "html-preview-views";
  views.setAttribute("role", "group");
  views.setAttribute("aria-label", "表示の切り替え");
  const previewToggle = document.createElement("button");
  previewToggle.type = "button";
  previewToggle.textContent = "プレビュー";
  previewToggle.setAttribute("aria-pressed", "true");
  const codeToggle = document.createElement("button");
  codeToggle.type = "button";
  codeToggle.textContent = "コード";
  codeToggle.setAttribute("aria-pressed", "false");
  views.append(previewToggle, codeToggle);
  const close = document.createElement("button");
  close.type = "button";
  close.className = "html-preview-close";
  close.textContent = "×";
  close.title = "閉じる";
  close.setAttribute("aria-label", "プレビューを閉じて会話に戻る");
  const note = document.createElement("p");
  note.textContent = "静的な表示です。JavaScript や外部通信を使う内容には対応しません。";
  const error = document.createElement("p");
  error.setAttribute("role", "status");
  const content = document.createElement("div");
  content.className = "html-preview-content";
  const codeView = document.createElement("pre");
  codeView.className = "html-preview-code";
  codeView.hidden = true;
  codeView.tabIndex = 0;
  header.append(title, views, separate, close);
  pane.append(header, note, error, content, codeView);
  conversation.after(pane);
  let doc: string | null = null;
  let opener: HTMLButtonElement | null = null;
  let detachedOpen = false;
  // Serialize native operations: a slow create must finish before a topic's
  // clear/destroy, and before a subsequent source replaces it.
  let pending = Promise.resolve();
  /** Resolves true only once the native side reports success. */
  function syncDetached(value: string | null, open = false): Promise<boolean> {
    const run = pending.catch(() => {}).then(() => detached(value, open)).then(() => true, () => {
      error.textContent = "別ウィンドウを開けませんでした。コードまたは横の表示をご覧ください。";
      return false;
    });
    pending = run.then(() => {});
    return run;
  }
  function showCode(on: boolean) {
    codeView.hidden = !on;
    content.hidden = on;
    codeToggle.setAttribute("aria-pressed", String(on));
    previewToggle.setAttribute("aria-pressed", String(!on));
  }
  previewToggle.addEventListener("click", () => showCode(false));
  codeToggle.addEventListener("click", () => showCode(true));
  function clear(restoreFocus = false) {
    doc = null;
    pane.hidden = true;
    title.textContent = "HTML プレビュー";
    content.replaceChildren();
    codeView.replaceChildren();
    showCode(false);
    error.textContent = "";
    // A native window can survive a main-webview reload. Local state cannot
    // prove that no detached view exists, so every topic clear closes it.
    syncDetached(null);
    detachedOpen = false;
    if (restoreFocus && opener?.isConnected) opener.focus();
    opener = null;
  }
  close.addEventListener("click", () => clear(true));
  pane.addEventListener("keydown", e => { if (e.key === "Escape") clear(true); });
  separate.addEventListener("click", () => {
    if (doc === null) return;
    detachedOpen = true;
    const shown = doc;
    // Hide the side pane only after the separate window has opened (#463);
    // on failure the pane and its error stay. The window itself stays open.
    void syncDetached(shown, true).then(ok => {
      if (!ok || doc !== shown || pane.hidden) return;
      pane.hidden = true;
      if (opener?.isConnected) opener.focus();
    });
  });
  // Reclaim ownership after startup/reload before accepting any new opens.
  syncDetached(null);
  return {
    clear,
    /** The room's single line for an html fence: its name, which opens the preview. */
    fileBlock(source: string, info = ""): HTMLElement {
      const name = htmlFenceName(source, info);
      const block = document.createElement("div");
      block.className = "html-file";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "html-preview-open";
      // Card: icon, name, "開く ›" (#439). The name is drawn only via textContent.
      const icon = document.createElement("span");
      icon.className = "html-file-icon";
      icon.setAttribute("aria-hidden", "true");
      const label = document.createElement("span");
      label.className = "html-file-name";
      label.textContent = name;
      const action = document.createElement("span");
      action.className = "html-file-action";
      action.setAttribute("aria-hidden", "true");
      action.textContent = "開く ›";
      button.append(icon, label, action);
      button.title = `${name} のプレビューを開く`;
      button.setAttribute("aria-label", `${name} のプレビューを開く`);
      button.addEventListener("click", () => {
        opener = button;
        pane.hidden = false;
        title.textContent = name;
        let code: Node[];
        try { code = renderCode(source); } catch { code = [document.createTextNode(String(source))]; }
        codeView.replaceChildren(...code);
        try {
          const next = htmlPreviewDocument(source);
          const frame = previewFrame(next);
          frame.addEventListener("error", () => { error.textContent = "表示できませんでした。コードをご覧ください。"; showCode(true); });
          content.replaceChildren(frame);
          doc = next;
          error.textContent = "";
          showCode(false);
          if (detachedOpen) syncDetached(doc);
        } catch {
          // The code stays readable in the pane even if parsing or rendering fails.
          doc = null;
          content.replaceChildren();
          error.textContent = "表示できませんでした。コードをご覧ください。";
          showCode(true);
        }
        close.focus();
      });
      block.append(button);
      return block;
    },
  };
}
