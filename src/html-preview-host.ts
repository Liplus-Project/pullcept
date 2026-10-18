import { previewFrame } from "./html-preview-document";

declare global { interface Window { __htmlPreviewDocument?: string } }
// Only the trusted native host writes this property/event. Posted markup is
// always srcdoc of an opaque frame, never this window's document. Its inline
// scripts may run in that frame (#443): this page is shown by the separate
// window and, with `?pane`, by the side pane's child view (#466). Neither holds
// a capability, so nothing reached from the frame can run an app command.
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
