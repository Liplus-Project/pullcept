import { previewFrame } from "./html-preview-document";

declare global {
  interface Window {
    __htmlPreviewDocument?: string;
    __htmlPreviewName?: string;
    __htmlPreviewCode?: string;
    __htmlPreviewMaximized?: boolean;
  }
}
// Only the trusted native host writes these properties/events. Posted markup is
// always srcdoc of an opaque frame, never this window's document. Its inline
// scripts may run in that frame (#443): this page is shown by the separate
// window and, with `?pane`, by the side pane's child view (#466). Neither holds
// a capability, so nothing reached from the frame can run an app command.
const nameEl = document.getElementById("file-name")!;
const previewEl = document.getElementById("preview")!;
const codeEl = document.getElementById("code")!;
const previewToggle = document.getElementById("show-preview")!;
const codeToggle = document.getElementById("show-code")!;

function draw() {
  previewEl.replaceChildren();
  if (window.__htmlPreviewDocument) previewEl.append(previewFrame(window.__htmlPreviewDocument));
  // The separate window's title bar and code view (#468): text only, never
  // parsed as HTML, so the code view runs nothing.
  const name = window.__htmlPreviewName || "HTML";
  nameEl.textContent = name;
  nameEl.title = name;
  codeEl.textContent = window.__htmlPreviewCode ?? "";
}
window.addEventListener("html-preview-update", draw);
// A document the native side handed over before this module ran (#478): its
// update event fired with no listener yet, and the side pane's first open after
// a restart stayed blank until the next document. Drawn once now from what is set.
if (window.__htmlPreviewDocument !== undefined) draw();

/** "プレビュー｜コード", as the side pane's switch (#439). */
function showCode(on: boolean) {
  codeEl.hidden = !on;
  previewEl.hidden = on;
  codeToggle.setAttribute("aria-pressed", String(on));
  previewToggle.setAttribute("aria-pressed", String(!on));
}
previewToggle.addEventListener("click", () => showCode(false));
codeToggle.addEventListener("click", () => showCode(true));

// "メインウィンドウで開く" (#463). This window has no IPC access; the native
// navigation guard reads this fixed path as the request, blocks it, closes
// this window and shows the same document in the main window's side pane.
export const RETURN_PATH = "/html-preview-return";
document.getElementById("return-main")?.addEventListener("click", () => { location.assign(RETURN_PATH); });

// The window's own title bar (#468). Without a capability this page cannot call
// the window API, so every operation is a fixed navigation the native guard
// blocks and carries out (src-tauri/src/html_preview.rs, WINDOW_PATH).
export const WINDOW_PATH = "/html-preview-window/";
export type WindowAction = "drag" | "toggle-maximize" | "minimize" | "close";
const ask = (action: WindowAction) => location.assign(WINDOW_PATH + action);
document.getElementById("window-minimize")?.addEventListener("click", () => ask("minimize"));
document.getElementById("window-maximize")?.addEventListener("click", () => ask("toggle-maximize"));
document.getElementById("window-close")?.addEventListener("click", () => ask("close"));
// Grabbing the bar outside its buttons moves the window, and a double click
// maximizes or restores it — Tauri's drag script (tauri 2.11.5,
// `window/scripts/drag.js`) on Windows, through the navigation guard instead
// of `plugin:window`.
document.getElementById("titlebar")?.addEventListener("mousedown", e => {
  if (e.button !== 0 || (e.detail !== 1 && e.detail !== 2)) return;
  if ((e.target as Element).closest("button")) return;
  e.preventDefault();
  ask(e.detail === 2 ? "toggle-maximize" : "drag");
});

/** 最大化 says 元に戻す while maximized; the native side reports every size change. */
function showMaximized() {
  const button = document.getElementById("window-maximize");
  if (!button) return;
  const maximized = window.__htmlPreviewMaximized === true;
  const label = maximized ? "元に戻す" : "最大化";
  button.classList.toggle("maximized", maximized);
  button.setAttribute("aria-label", label);
  button.title = label;
}
window.addEventListener("html-preview-window", showMaximized);
draw();
showMaximized();
