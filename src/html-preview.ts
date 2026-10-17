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
  separate.textContent = "別ウィンドウで開く";
  const codeToggle = document.createElement("button");
  codeToggle.type = "button";
  codeToggle.textContent = "コード";
  codeToggle.setAttribute("aria-pressed", "false");
  const close = document.createElement("button");
  close.type = "button";
  close.textContent = "閉じる";
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
  header.append(title, codeToggle, separate, close);
  pane.append(header, note, error, content, codeView);
  conversation.after(pane);
  let doc: string | null = null;
  let opener: HTMLButtonElement | null = null;
  let detachedOpen = false;
  // Serialize native operations: a slow create must finish before a topic's
  // clear/destroy, and before a subsequent source replaces it.
  let pending = Promise.resolve();
  function syncDetached(value: string | null, open = false) {
    pending = pending.catch(() => {}).then(() => detached(value, open)).catch(() => {
      error.textContent = "別ウィンドウを開けませんでした。コードまたは横の表示をご覧ください。";
    });
  }
  function showCode(on: boolean) {
    codeView.hidden = !on;
    content.hidden = on;
    codeToggle.setAttribute("aria-pressed", String(on));
  }
  codeToggle.addEventListener("click", () => showCode(codeView.hidden));
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
    syncDetached(doc, true);
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
      button.textContent = name;
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
