import { test } from "node:test";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { spawn } from "node:child_process";
import { copyFile, mkdtemp, rm, writeFile } from "node:fs/promises";
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
  // An example gets no embedded manifest, and without Common Controls 6 the
  // tauri tree stops before main with STATUS_ENTRYPOINT_NOT_FOUND. A manifest
  // beside the executable is read when none is embedded.
  await writeFile("src-tauri/target/x86_64-pc-windows-gnu/debug/examples/html-preview-check.exe.manifest", `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency></assembly>
`);
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
  const doc = htmlPreviewDocument('<style>h1{color:rgb(12,34,56)}</style><h1>Native report</h1><script>document.body.dataset.ran="yes";parent.pwned=true</script><a href="https://probe.invalid/">link</a>', true);
  const opened = context.waitForEvent("page");
  await invoke(main, doc);
  const host = await opened;
  await host.frameLocator("iframe").getByRole("heading", { name: "Native report" }).waitFor();
  assert.equal(new URL(host.url()).pathname, "/html-preview.html");
  assert.equal(await host.locator("iframe").getAttribute("sandbox"), "allow-scripts");
  assert.equal(await host.frameLocator("iframe").locator("h1").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
  // WebView2 injects Tauri's scripts into every frame, so the frame does see
  // __TAURI_INTERNALS__ (#443). Its replies return to the top frame only; that
  // an invoke from the frame is denied by the ACL is checked on the real app.
  assert.equal(await host.frameLocator("iframe").locator("body").evaluate(() => document.body.dataset.ran ?? "none"), "yes", "inline script runs in the separate window's frame");
  assert.equal(await host.frameLocator("iframe").locator("body").evaluate(() => { try { parent.document; return false; } catch { return true; } }), true);
  assert.equal(await host.evaluate(() => window.pwned), undefined);
  // The separate window holds no capability: the ACL refuses before the command's own check.
  await assert.rejects(() => invoke(host, doc), /not allowed|main window/);
  // Its own title bar (#468): no standard frame, the file's name, and each
  // window operation carried out by the navigation guard, not the window API.
  const windowState = what => main.evaluate(what => window.__TAURI_INTERNALS__.invoke(`plugin:window|${what}`, { label: "html-preview" }), what);
  const until = async (what, expected) => {
    for (let attempt = 0; attempt < 100; attempt++) {
      if (await windowState(what) === expected) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.equal(await windowState(what), expected, what);
  };
  assert.equal(await windowState("is_decorated"), false, "the window draws its own title bar");
  await main.evaluate(document => window.__TAURI_INTERNALS__.invoke("html_preview", { document, open: false, name: "report.html", code: "<h1>Native report</h1>" }), doc);
  await host.locator("header strong", { hasText: "report.html" }).waitFor();
  assert.equal(await windowState("title"), "report.html - Pullcept", "the taskbar names the file");
  assert.equal(await host.locator("#code").textContent(), "<h1>Native report</h1>");
  await host.locator("#window-maximize").dispatchEvent("click");
  await until("is_maximized", true);
  await host.locator("#window-maximize[aria-label='元に戻す']").waitFor({ state: "attached" });
  // A double click on the bar restores; the page itself never navigates.
  await host.locator("header strong").dispatchEvent("mousedown", { button: 0, detail: 2 });
  await until("is_maximized", false);
  await host.locator("#window-maximize[aria-label='最大化']").waitFor({ state: "attached" });
  await host.locator("header strong").dispatchEvent("mousedown", { button: 0, detail: 1 });
  await host.locator("#window-minimize").dispatchEvent("click");
  await until("is_minimized", true);
  // Back from the taskbar's side: maximize, then restore.
  await host.locator("#window-maximize").dispatchEvent("click");
  await until("is_maximized", true);
  await until("is_minimized", false);
  await host.locator("#window-maximize").dispatchEvent("click");
  await until("is_maximized", false);
  assert.equal(new URL(host.url()).pathname, "/html-preview.html", "window operations leave the host page in place");
  await host.frameLocator("iframe").getByRole("heading", { name: "Native report" }).waitFor();
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
  // 閉じる on the window's own title bar closes it (#468).
  const titled = context.waitForEvent("page");
  await invoke(main, doc);
  const titledHost = await titled;
  await titledHost.frameLocator("iframe").locator("h1").waitFor();
  const closedByBar = titledHost.waitForEvent("close");
  await titledHost.locator("#window-close").dispatchEvent("click");
  await closedByBar;
  await invoke(main, doc, false);
  assert.equal(context.pages().length, 1, "source update does not reopen a closed detached window");
  // The side pane's child view (#466): a second webview inside the main window.
  // Not run locally yet (see docs/5-development.md); the real app check is #466's.
  const paned = context.waitForEvent("page");
  const place = (document, bounds) => main.evaluate(pane => window.__TAURI_INTERNALS__.invoke("html_preview", { document: null, open: false, pane }), { document, bounds });
  await place(doc, { x: 0, y: 0, width: 400, height: 300 });
  const paneView = await paned;
  // The page event can come before the child view's first navigation commits.
  await paneView.waitForURL(url => url.search === "?pane");
  assert.equal(new URL(paneView.url()).search, "?pane");
  await paneView.frameLocator("iframe").locator("h1").waitFor();
  assert.equal(await paneView.locator("iframe").getAttribute("sandbox"), "allow-scripts");
  assert.equal(await paneView.frameLocator("iframe").locator("body").evaluate(() => document.body.dataset.ran ?? "none"), "yes", "inline script runs in the child view's frame");
  await assert.rejects(() => paneView.evaluate(() => window.__TAURI_INTERNALS__.invoke("html_preview", { document: null, open: false })), "the child view matches no capability");
  await paneView.evaluate(() => { location.href = "https://probe.invalid/navigation"; });
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(new URL(paneView.url()).pathname, "/html-preview.html", "child view top-level navigation denied");
  await place("", null);
  // The main webview keeps its commands while its window holds two webviews.
  await invoke(main, doc, false);
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
