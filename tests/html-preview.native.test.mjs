import { test } from "node:test";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { spawn } from "node:child_process";
import { copyFile, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer as netServer } from "node:net";
import { tsImport } from "tsx/esm/api";

test("native WebView2 trusted host creation, isolation, reuse and destruction", { timeout: 30000 }, async t => {
  assert.equal(process.platform, "win32", "native probe requires Windows/WebView2");
  const { htmlPreviewDocument } = await tsImport("../src/html-preview-document.ts", import.meta.url);
  const socket = netServer();
  await new Promise(resolve => socket.listen(0, "127.0.0.1", resolve));
  const port = socket.address().port;
  await new Promise(resolve => socket.close(resolve));
  const profile = await mkdtemp(join(tmpdir(), "pullcept-preview-check-"));
  // Cargo examples need the loader beside their executable too; do not resolve
  // a different checkout's DLL through the developer's PATH.
  await copyFile("src-tauri/target/x86_64-pc-windows-gnu/debug/WebView2Loader.dll", "src-tauri/target/x86_64-pc-windows-gnu/debug/examples/WebView2Loader.dll");
  const server = await createServer({ server: { host: "127.0.0.1", port: 0, strictPort: false }, logLevel: "error" });
  await server.listen();
  const base = server.resolvedUrls.local[0];
  const child = spawn("src-tauri/target/x86_64-pc-windows-gnu/debug/examples/html-preview-check.exe", [], {
    windowsHide: true, stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, PREVIEW_CHECK_URL: base, WEBVIEW2_USER_DATA_FOLDER: profile, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` },
  });
  let stderr = "";
  child.stderr.on("data", data => { stderr += data; });
  t.after(async () => {
    if (child.exitCode === null) child.kill();
    await server.close();
    await rm(profile, { recursive: true, force: true, maxRetries: 8, retryDelay: 300 });
  });
  let browser;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw new Error(`native harness exited ${child.exitCode}: ${stderr}`);
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`); break; } catch { await new Promise(resolve => setTimeout(resolve, 100)); }
  }
  assert.ok(browser, stderr);
  const context = browser.contexts()[0];
  const main = context.pages()[0];
  await main.waitForFunction(() => window.__TAURI_INTERNALS__);
  const invoke = (page, document, open = true) => page.evaluate(({ document, open }) => window.__TAURI_INTERNALS__.invoke("html_preview", { document, open }), { document, open });
  const doc = htmlPreviewDocument('<style>h1{color:rgb(12,34,56)}</style><h1>Native report</h1><script>parent.pwned=true</script><a href="https://probe.invalid/">link</a>');
  const opened = context.waitForEvent("page");
  await invoke(main, doc);
  const host = await opened;
  await host.frameLocator("iframe").getByRole("heading", { name: "Native report" }).waitFor();
  assert.equal(new URL(host.url()).pathname, "/html-preview.html");
  assert.equal(await host.locator("iframe").getAttribute("sandbox"), "");
  assert.equal(await host.frameLocator("iframe").locator("h1").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
  assert.equal(await host.frameLocator("iframe").locator("body").evaluate(() => typeof window.__TAURI_INTERNALS__), "undefined");
  assert.equal(await host.frameLocator("iframe").locator("body").evaluate(() => { try { parent.document; return false; } catch { return true; } }), true);
  assert.equal(await host.evaluate(() => window.pwned), undefined);
  await assert.rejects(() => invoke(host, doc), /main window/);
  await invoke(main, htmlPreviewDocument("<h2>Updated native report</h2>"), false);
  await host.frameLocator("iframe").getByRole("heading", { name: "Updated native report" }).waitFor();
  await invoke(main, doc);
  await host.frameLocator("iframe").getByRole("heading", { name: "Native report" }).waitFor();
  assert.equal(context.pages().length, 2, "repeated explicit open reuses one native window");
  await host.evaluate(() => { location.href = "https://probe.invalid/navigation"; });
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(new URL(host.url()).pathname, "/html-preview.html", "native top-level navigation denied");
  const closed = host.waitForEvent("close");
  await invoke(main, null, false);
  const next = context.waitForEvent("page");
  // No timer or close-event wait between the clear acknowledgment and reopen.
  await invoke(main, doc);
  await closed;
  const reopened = await next;
  await reopened.frameLocator("iframe").locator("h1").waitFor();
  const closedAgain = reopened.waitForEvent("close");
  await invoke(main, null, false);
  await closedAgain;
  await invoke(main, doc, false);
  assert.equal(context.pages().length, 1, "source update does not reopen a closed detached window");
  const last = context.waitForEvent("page");
  await invoke(main, doc);
  const lastHost = await last;
  await lastHost.frameLocator("iframe").locator("h1").waitFor();
  const destroyed = lastHost.waitForEvent("close");
  await main.evaluate(() => window.__TAURI_INTERNALS__.invoke("preview_test_close_main")).catch(() => {});
  await destroyed;
  await new Promise(resolve => child.exitCode !== null ? resolve() : child.once("exit", resolve));
  assert.equal(child.exitCode, 0, stderr);
});
