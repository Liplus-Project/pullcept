import { htmlPreviewDocument, previewFrame } from "./html-preview-document";

export type DetachedPreview = (document: string | null, open: boolean) => Promise<void>;

/** One explicitly opened preview per topic, with code left in the room. */
export function createHtmlPreview(conversation: HTMLElement, detached: DetachedPreview) {
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
  header.append(title, separate, close);
  pane.append(header, note, error, content);
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
  function clear(restoreFocus = false) {
    doc = null;
    pane.hidden = true;
    content.replaceChildren();
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
    addButton(block: HTMLElement, source: string, lang: string) {
      if (lang.toLowerCase() !== "html") return;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "html-preview-open";
      button.textContent = "プレビュー";
      button.addEventListener("click", () => {
        try {
          const next = htmlPreviewDocument(source);
          const frame = previewFrame(next);
          frame.addEventListener("error", () => { error.textContent = "表示できませんでした。元のコードをご覧ください。"; });
          content.replaceChildren(frame);
          doc = next;
          opener = button;
          pane.hidden = false;
          error.textContent = "";
          if (detachedOpen) syncDetached(doc);
          close.focus();
        } catch {
          clear();
          // The source block stays readable even if parsing or rendering fails.
          button.textContent = "表示できませんでした（コードをご覧ください）";
        }
      });
      block.append(button);
    },
  };
}
