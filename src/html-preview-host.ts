import { previewFrame } from "./html-preview-document";

declare global { interface Window { __htmlPreviewDocument?: string } }
// Only the trusted native host writes this property/event. Posted markup is
// always srcdoc of an opaque, scriptless frame, never this window's document.
function draw() {
  const root = document.getElementById("preview")!;
  root.replaceChildren();
  if (window.__htmlPreviewDocument) root.append(previewFrame(window.__htmlPreviewDocument));
}
window.addEventListener("html-preview-update", draw);
// "メインウィンドウで開く" (#463). This window has no IPC access; the native
// navigation guard reads this fixed path as the request, blocks it, closes
// this window and shows the same document in the main window's side pane.
export const RETURN_PATH = "/html-preview-return";
document.getElementById("return-main")?.addEventListener("click", () => { location.assign(RETURN_PATH); });
draw();
