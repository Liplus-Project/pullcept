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
draw();
