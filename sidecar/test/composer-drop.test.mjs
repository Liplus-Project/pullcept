import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

// #605: with Tauri's `unstable` feature (needed for the HTML preview's child
// view, #466) the main webview is built as a child of its window, and its file
// drops reach webview listeners only. A window listener hears none of them,
// with or without the child view.
test("the composer's file drop is heard on the webview while tauri is unstable", async () => {
  const cargo = await readFile(new URL("../../src-tauri/Cargo.toml", import.meta.url), "utf8");
  const main = await readFile(new URL("../../src/main.ts", import.meta.url), "utf8");
  const tauri = cargo.split(/\r?\n/).find(line => /^tauri\s*=/.test(line)) ?? "";
  if (!/"unstable"/.test(tauri)) return;
  assert.match(main, /getCurrentWebview\(\)\.onDragDropEvent\(/);
  assert.doesNotMatch(main, /getCurrentWindow\(\)\.onDragDropEvent\(/);
});
