import { test } from "node:test";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { existsSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";

const edge = process.env.PREVIEW_BROWSER ?? [
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome",
].find(existsSync);
if (!edge) throw new Error("Install Chromium/Edge or set PREVIEW_BROWSER to its executable (no skipped boundary tests)");

test("real Chromium preview boundary and lifecycle", { timeout: 60000 }, async t => {
  const server = await createServer({ server: { host: "127.0.0.1", port: 0, strictPort: false }, logLevel: "error" });
  await server.listen();
  const base = server.resolvedUrls.local[0].replace(/\/$/, "");
  const browser = await chromium.launch({ executablePath: edge, headless: true });
  t.after(async () => { await browser.close(); await server.close(); });
  const context = await browser.newContext();
  const page = await context.newPage();
  // An isolated harness uses the production renderer and controller. No live
  // app, account, seat, browser profile or runtime checkout is involved.
  await page.route(`${base}/preview-test`, route => route.fulfill({ contentType: "text/html", body: `<!doctype html><link rel="stylesheet" href="/src/styles.css"><div id="body"><div id="conversation"><div id="post"></div></div></div><script type="module">
    import { createHtmlPreview } from '/src/html-preview.ts';
    import { appendMarkdown } from '/src/markdown.ts';
    import { htmlPreviewDocument } from '/src/html-preview-document.ts';
    window.createHtmlPreview = createHtmlPreview;
    window.detachedCalls = [];
    window.detachedSources = [];
    window.paneCalls = [];
    window.preview = createHtmlPreview(document.querySelector('#conversation'), async (doc, open, source) => {
      window.detachedCalls.push(doc);
      window.detachedSources.push(source);
      if (window.detachDelay) await window.detachDelay;
      if (window.detachFail && doc !== null) throw new Error('native open failed');
    }, async placement => { window.paneCalls.push(placement); });
    window.render = text => appendMarkdown(document.querySelector('#post'), text, {
      prose: text => [document.createTextNode(text)], link: (_, label) => label,
      code: (source, lang, info) => { if (lang.toLowerCase() === 'html') return window.preview.fileBlock(source, info); const block = document.createElement('pre'); block.textContent = source; return block; }
    });
    window.htmlPreviewDocument = htmlPreviewDocument;
    window.__TAURI_INTERNALS__ = { secret: 'parent-only' };
    window.ready = true;
  </script>` }));
  await page.goto(`${base}/preview-test`);
  await page.waitForFunction(() => window.ready);
  await page.waitForFunction(() => window.detachedCalls.length === 1 && window.detachedCalls[0] === null);
  await page.evaluate(() => { window.detachedCalls = []; window.detachedSources = []; });
  // The side pane's child view (#466) is a native view; this harness records
  // what the controller asks of it. Startup empties and hides a surviving one.
  const lastPane = async (check, what) => {
    for (let attempt = 0; attempt < 100; attempt++) {
      const last = await page.evaluate(() => window.paneCalls.at(-1));
      if (last && check(last)) return last;
      await page.waitForTimeout(50);
    }
    throw new Error(`child view never reached: ${what}; calls ${JSON.stringify(await page.evaluate(() => window.paneCalls))}`);
  };
  const contentBounds = () => page.evaluate(() => {
    const rect = document.querySelector(".html-preview-content").getBoundingClientRect();
    const scale = devicePixelRatio;
    const x = Math.round(rect.left * scale), y = Math.round(rect.top * scale);
    return { x, y, width: Math.round(rect.right * scale) - x, height: Math.round(rect.bottom * scale) - y };
  });
  assert.deepEqual(await page.evaluate(() => window.paneCalls), [{ document: "", bounds: null }]);
  const requests = [];
  const failures = new Map();
  context.on("requestfailed", request => failures.set(request.url(), request.failure()?.errorText));
  // Observe BEFORE source parsing, including CSS and initial DOM creation.
  context.on("request", request => { if (!request.url().startsWith(base + "/")) requests.push(request.url()); });
  let popups = 0;
  context.on("page", () => popups++);
  const source = `<!doctype html><html class="report-root" style="background-image:url(https://probe.invalid/root);border-top:3px solid rgb(1,2,3)"><head>
    <meta http-equiv="refresh" content="0;url=https://probe.invalid/refresh">
    <meta http-equiv="Content-Security-Policy" content="default-src * 'unsafe-inline' 'unsafe-eval'">
    <base href="https://probe.invalid/" target="_top">
    <link rel="stylesheet" href="https://probe.invalid/css"><link rel="prefetch" href="https://probe.invalid/prefetch">
    <style>@import url(https://probe.invalid/import); @font-face{font-family:probe;src:url(https://probe.invalid/font)}
    #card{color:rgb(12, 34, 56);background-image:url(https://probe.invalid/background);font-family:probe,serif}
    html.report-root #card{text-decoration:underline}body::after{content:'static CSS'}</style></head>
    <body style="background-color:rgb(230, 240, 250)" onload="parent.pwned=true">
    <h1 id="card" onclick="parent.pwned=true">Static report</h1><p style="font-weight:700">inline CSS</p>
    <script>parent.pwned=true</script>
    <script>document.getElementById('card').dataset.ran='inline';document.body.append(Object.assign(document.createElement('div'),{id:'made',textContent:'made by script'}))</script>
    <script>const mark=(k,v)=>document.body.dataset[k]=v;
      try{parent.document.body.innerHTML='bad';mark('parent','reached')}catch{mark('parent','opaque')}
      try{top.document.title='bad';mark('top','reached')}catch{mark('top','opaque')}
      try{mark('parentTauri',typeof parent.__TAURI_INTERNALS__)}catch{mark('parentTauri','denied')}
      mark('tauri',typeof window.__TAURI_INTERNALS__);
      fetch('https://probe.invalid/fetch').then(()=>mark('fetch','reached'),()=>mark('fetch','blocked'));
      new Image().src='https://probe.invalid/js-image';
      document.body.append(Object.assign(document.createElement('iframe'),{src:'https://probe.invalid/js-frame'}));
      try{window.open('https://probe.invalid/js-popup');}catch{}</script>
    <script type="module">document.body.dataset.module='ran'</script>
    <script src="https://probe.invalid/external.js">document.body.dataset.external='ran'</script>
    <script type="importmap">{"imports":{"x":"https://probe.invalid/map.js"}}</script>
    <script type="speculationrules">{"prefetch":[{"source":"list","urls":["https://probe.invalid/speculation"]}]}</script>
    <script><!--<script>document.body.dataset.escaped='ran'</script>--></script><h2 id="after-script">after script</h2>
    <button id="handler" onclick="this.dataset.clicked='yes'">handler</button>
    <img src="https://probe.invalid/image" srcset="https://probe.invalid/set 2x" onerror="parent.pwned=true">
    <iframe src="https://probe.invalid/frame"></iframe><object data="file:///C:/secret"></object><embed src="https://probe.invalid/embed">
    <svg><a href="https://probe.invalid/svg"><text>svg</text></a><image href="https://probe.invalid/svg-image"/></svg>
    <math><mtext><img src="https://probe.invalid/mutation"></mtext></math>
    <form action="https://probe.invalid/form"><button formaction="https://probe.invalid/button">submit</button></form>
    <a id="nav" href="https://probe.invalid/self" ping="https://probe.invalid/ping">navigate</a>
    <a id="top" href="https://probe.invalid/top" target="_top">top</a>
    <a id="popup" href="https://probe.invalid/pop" target="_blank">popup</a>
    <a id="js" href="java&#x73;cript:parent.pwned=true">js</a>
    <a id="file" href="file:///C:/secret">file</a><a id="app" href="pullcept://network-probe">app</a>
    <a id="data" href="data:text/html,<script>parent.pwned=true</script>">data</a>
    <a id="download" href="https://probe.invalid/download" download>download</a>
    <div style="background:url(file:///C:/secret);cursor:url(pullcept://network-probe),auto">local CSS</div>
    </body></html>`;
  await page.evaluate(source => window.render('Raw <img src="https://probe.invalid/raw">\n\n```html report.html\n' + source + '\n```\n\n```css\np{}\n```'), source);
  assert.equal(await page.locator("#post img").count(), 0);
  assert.equal(await page.locator(".html-preview iframe").count(), 0, "arrival never opens a preview");
  const open = name => page.getByRole("button", { name: `${name} のプレビューを開く`, exact: true });
  assert.equal(await open("report.html").count(), 1, "the fence's first line names the file");
  assert.equal(await page.locator("#post .html-file .html-file-name").textContent(), "report.html", "room shows one file-name card, not code");
  assert.equal(await page.locator("#post .html-file").textContent(), "report.html開く ›", "card carries only the name and the open hint");
  assert.equal((await page.locator("#post").textContent()).includes("probe.invalid/css"), false);
  await open("report.html").click();
  assert.equal(await page.locator(".html-preview header strong").textContent(), "report.html");
  // No frame in the main page (#466): the prepared document and the content
  // area's bounds, in physical pixels, go to the child view.
  const prepared = await page.evaluate(source => window.htmlPreviewDocument(source), source);
  const shown = await lastPane(call => call.document === prepared && call.bounds, "document and bounds");
  assert.deepEqual(shown.bounds, await contentBounds());
  assert.ok(shown.bounds.width > 0 && shown.bounds.height > 0);
  assert.equal(await page.locator(".html-preview iframe").count(), 0, "the main page holds no preview frame");
  assert.ok((await page.locator(".html-preview > p").first().textContent()).includes("JavaScript は、この表示の中だけで動きます"));
  // The child view loads the separate window's host page with `?pane`: the frame only.
  const paneHost = await context.newPage();
  await paneHost.goto(`${base}/html-preview.html?pane`);
  popups = 0;
  assert.equal(await paneHost.getByRole("button", { name: "メインウィンドウで開く" }).isVisible(), false);
  assert.equal(await paneHost.locator("body > p").isVisible(), false);
  // The separate window's title bar and code view are not drawn in the side pane (#468).
  assert.equal(await paneHost.locator("header").isVisible(), false);
  assert.equal(await paneHost.locator("#code").isVisible(), false);
  await paneHost.evaluate(doc => { window.__htmlPreviewDocument = doc; window.dispatchEvent(new Event("html-preview-update")); }, shown.document);
  const frame = paneHost.frameLocator("iframe");
  await frame.locator("#card").waitFor();
  assert.equal(await paneHost.locator("iframe").getAttribute("sandbox"), "allow-scripts", "the child view's frame adds allow-scripts only");
  await frame.locator("#made").waitFor();
  await frame.locator("body[data-fetch]").waitFor();
  assert.equal(await frame.locator("#card").getAttribute("data-ran"), "inline");
  assert.deepEqual(await frame.locator("body").evaluate(el => ({ ...el.dataset })), {
    parent: "opaque", top: "opaque", parentTauri: "denied", tauri: "undefined", fetch: "blocked", module: "ran",
  }, "scripts run in the child view's frame; fetch stops at CSP; parent/top stay out of reach");
  await frame.locator("#handler").click();
  assert.equal(await frame.locator("#handler").getAttribute("data-clicked"), "yes");
  assert.equal(await frame.locator("#after-script").textContent(), "after script");
  assert.equal(await frame.locator("#card").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
  assert.equal(await frame.locator("body").evaluate(el => getComputedStyle(el).backgroundColor), "rgb(230, 240, 250)");
  assert.equal(await frame.locator("p").evaluate(el => getComputedStyle(el).fontWeight), "700");
  assert.equal(await frame.locator("#card").evaluate(el => getComputedStyle(el).textDecorationLine), "underline");
  assert.equal(await frame.locator("html").evaluate(el => getComputedStyle(el).borderTopWidth), "3px");
  assert.equal(await frame.locator("iframe:not([src='https://probe.invalid/js-frame']),object,embed,meta[http-equiv=refresh],base,form,link,img,svg,math").count(), 0);
  assert.equal(await frame.locator("script[src],script[type=importmap],script[type=speculationrules]").count(), 0);
  for (const id of ["card", "nav", "top", "popup", "js", "file", "app", "data", "download"]) {
    await frame.locator(`#${id}`).click();
  }
  await paneHost.waitForTimeout(600);
  assert.equal(paneHost.url(), `${base}/html-preview.html?pane`);
  assert.equal(paneHost.frames()[1].url(), "about:srcdoc");
  assert.equal(await paneHost.evaluate(() => window.pwned), undefined);
  assert.equal(await page.evaluate(() => window.pwned), undefined);
  const attempted = requests.map(url => [url, failures.get(url)]);
  for (const css of ["https://probe.invalid/root", "https://probe.invalid/background"]) {
    assert.ok(attempted.some(([url, failure]) => url === css && failure === "csp"), `${css} blocked by CSP`);
  }
  assert.ok(attempted.every(([url, failure]) => failure === "csp" || (url === "file:///C:/secret" && failure === "other")),
    `CSP blocks network/app schemes, opaque frame denies local files: ${JSON.stringify(attempted)}`);
  assert.equal(popups, 0, "window.open in the frame opens nothing");
  await paneHost.close();
  const codeToggle = page.getByRole("button", { name: "コード", exact: true });
  await codeToggle.click();
  assert.equal(await codeToggle.getAttribute("aria-pressed"), "true");
  const previewToggle = page.getByRole("button", { name: "プレビュー", exact: true });
  assert.equal(await previewToggle.getAttribute("aria-pressed"), "false", "segment pressed states are exclusive");
  assert.ok((await page.locator(".html-preview-code").textContent()).includes(source), "code is read in the preview");
  assert.equal(await page.locator(".html-preview-content").isVisible(), false);
  // The code view is this page's DOM: the child view hides, keeping its document.
  assert.deepEqual(await lastPane(call => call.bounds === null, "hidden for code"), { bounds: null });
  await previewToggle.click();
  assert.equal(await page.locator(".html-preview-content").isVisible(), true);
  assert.deepEqual(await lastPane(call => call.bounds !== null, "shown again"), { bounds: await contentBounds() });
  // A dialog over the page hides the native view, which would otherwise sit above it.
  await page.evaluate(() => { const d = document.createElement("dialog"); d.id = "overlay"; d.textContent = "over"; document.body.append(d); d.showModal(); });
  await lastPane(call => call.bounds === null, "hidden under a dialog");
  await page.evaluate(() => { const d = document.getElementById("overlay"); d.close(); d.remove(); });
  await lastPane(call => call.bounds !== null, "shown after the dialog");
  // Window size changes move the view with the content area.
  await page.setViewportSize({ width: 1000, height: 640 });
  const resized = await contentBounds();
  await lastPane(call => JSON.stringify(call.bounds) === JSON.stringify(resized), "follows the resize");
  assert.equal((await page.evaluate(() => window.paneCalls)).filter(call => "document" in call && call.document !== "").length, 1, "the document is sent once");

  await page.getByRole("button", { name: "新しいウィンドウで開く" }).click();
  await page.waitForFunction(() => window.detachedCalls.length === 1);
  // Success hides the side pane but does not close the separate window (#463).
  await page.waitForFunction(() => document.querySelector('.html-preview').hidden);
  await lastPane(call => call.bounds === null, "hidden after detach");
  assert.equal(await page.evaluate(() => window.detachedCalls.includes(null)), false);
  // Exercise the shipped trusted detached host document with the exact same
  // prepared source; native window creation/ownership is separately cargo-checked.
  const host = await context.newPage();
  await host.goto(`${base}/html-preview.html`);
  await host.evaluate(doc => { window.__htmlPreviewDocument = doc; window.dispatchEvent(new Event("html-preview-update")); }, await page.evaluate(() => window.detachedCalls[0]));
  const hostFrame = host.frameLocator("iframe");
  await hostFrame.locator("#card").waitFor();
  assert.equal(await host.locator("iframe").getAttribute("sandbox"), "allow-scripts", "the separate window's frame adds allow-scripts only");
  // Inline scripts and handlers run inside the separate window's opaque frame only (#443).
  await hostFrame.locator("#made").waitFor();
  await hostFrame.locator("body[data-fetch]").waitFor();
  assert.equal(await hostFrame.locator("#card").getAttribute("data-ran"), "inline");
  assert.deepEqual(await hostFrame.locator("body").evaluate(el => ({ ...el.dataset })), {
    parent: "opaque", top: "opaque", parentTauri: "denied", tauri: "undefined", fetch: "blocked", module: "ran",
  }, "fetch stops at CSP; parent/top documents stay out of reach");
  assert.equal(await hostFrame.locator("#after-script").textContent(), "after script", "a script cannot swallow following markup");
  assert.equal(await hostFrame.locator("script[src],script[type=importmap],script[type=speculationrules]").count(), 0);
  await hostFrame.locator("#handler").click();
  assert.equal(await hostFrame.locator("#handler").getAttribute("data-clicked"), "yes", "on* handlers run in the frame");
  assert.equal(await hostFrame.locator("html").evaluate(el => [...el.attributes].some(a => a.name.startsWith("on"))), false);
  assert.equal(await hostFrame.locator("iframe:not([src='https://probe.invalid/js-frame']),object,embed,meta[http-equiv=refresh],base,form,link,img,svg,math").count(), 0, "posted markup never carries these; only the script-made frame exists");
  assert.equal(await hostFrame.locator("#card").evaluate(el => getComputedStyle(el).color), "rgb(12, 34, 56)");
  await hostFrame.locator("#nav").click();
  await host.waitForTimeout(300);
  assert.equal(host.frames()[1].url(), "about:srcdoc");
  assert.equal(await host.evaluate(() => window.pwned), undefined);
  assert.equal(popups, 1, "only the test's own host page opened; window.open is blocked");
  // A script can navigate its own frame; the host page's frame-src (about: only)
  // stops that before any request, for external and same-origin targets alike.
  for (const target of ["https://probe.invalid/host-self-nav", "/html-preview.html", "/index.html"]) {
    const navDoc = await page.evaluate(target => window.htmlPreviewDocument(`<p id="nav-start">nav</p><script>location.href=${JSON.stringify(target)}</script>`, true), target);
    await host.evaluate(doc => { window.__htmlPreviewDocument = doc; window.dispatchEvent(new Event("html-preview-update")); }, navDoc);
    await host.waitForTimeout(500);
    assert.equal(host.frames().length, 2);
    // Chromium leaves its error document in a frame whose navigation CSP blocked.
    assert.ok(["about:srcdoc", "chrome-error://chromewebdata/"].includes(host.frames()[1].url()), `self-navigation to ${target} is blocked`);
  }
  assert.equal(requests.some(url => url.includes("self-nav")), false, "self-navigation never reaches the network");
  assert.ok(requests.every(url => failures.get(url) === "csp" || (url === "file:///C:/secret" && failures.get(url) === "other")),
    "every script-made attempt fails at CSP");

  // The separate window's own title bar (#468): the side pane's header row
  // (name, プレビュー｜コード, メインウィンドウで開く) and the window's three,
  // with no "HTML プレビュー - Pullcept" anywhere on the page.
  const sent = await page.evaluate(() => window.detachedSources[0]);
  assert.deepEqual(sent, { name: "report.html", code: source }, "the separate window gets the file name and the posted source");
  await host.evaluate(({ name, code }) => { window.__htmlPreviewName = name; window.__htmlPreviewCode = code; window.dispatchEvent(new Event("html-preview-update")); }, sent);
  const bar = host.locator("header");
  assert.equal(await bar.locator("strong").textContent(), "report.html");
  assert.deepEqual(await bar.getByRole("button").evaluateAll(els => els.map(el => el.getAttribute("aria-label") ?? el.textContent)),
    ["プレビュー", "コード", "メインウィンドウで開く", "最小化", "最大化", "閉じる"]);
  assert.equal((await host.locator("body").textContent()).includes("HTML プレビュー - Pullcept"), false);
  assert.equal(await host.title(), "Pullcept");
  // The code view is text: the posted scripts appear as characters and run nothing.
  await host.getByRole("button", { name: "コード" }).click();
  assert.equal(await host.locator("#code").isVisible(), true);
  assert.equal(await host.locator("#preview").isVisible(), false);
  assert.equal(await host.locator("#code").textContent(), source);
  assert.equal(await host.locator("#code *").count(), 0);
  assert.equal(await host.getByRole("button", { name: "コード" }).getAttribute("aria-pressed"), "true");
  await host.getByRole("button", { name: "プレビュー" }).click();
  assert.equal(await host.locator("#code").isVisible(), false);
  assert.equal(await host.locator("#preview iframe").isVisible(), true);
  // Each window operation is a fixed navigation the native guard blocks and
  // performs (src-tauri/src/html_preview.rs); no window API is called here.
  // A 204 answer leaves the page in place, as the guard's refusal does.
  // On a page of its own: a navigation the test answers stays pending to Chromium.
  const bare = await context.newPage();
  await bare.goto(`${base}/html-preview.html`);
  const bareBar = bare.locator("header");
  const asked = [];
  await bare.route("**/html-preview-window/**", route => { asked.push(new URL(route.request().url()).pathname); return route.fulfill({ status: 204 }); });
  const askedNext = async (what, act) => {
    const before = asked.length;
    await act();
    for (let attempt = 0; attempt < 50 && asked.length === before; attempt++) await bare.waitForTimeout(20);
    assert.deepEqual(asked.slice(before), [what]);
  };
  await askedNext("/html-preview-window/minimize", () => bare.getByRole("button", { name: "最小化" }).dispatchEvent("click"));
  await askedNext("/html-preview-window/toggle-maximize", () => bare.getByRole("button", { name: "最大化" }).dispatchEvent("click"));
  await askedNext("/html-preview-window/close", () => bare.getByRole("button", { name: "閉じる" }).dispatchEvent("click"));
  // Grabbing the bar (the name included) drags; a double click maximizes or restores.
  await askedNext("/html-preview-window/drag", () => bareBar.locator("strong").dispatchEvent("mousedown", { button: 0, detail: 1 }));
  await askedNext("/html-preview-window/toggle-maximize", () => bareBar.dispatchEvent("mousedown", { button: 0, detail: 2 }));
  // A button on the bar is a button, not a handle; the right button does nothing.
  const quiet = asked.length;
  await bare.getByRole("button", { name: "コード" }).dispatchEvent("mousedown", { button: 0, detail: 1 });
  await bareBar.locator("strong").dispatchEvent("mousedown", { button: 2, detail: 1 });
  await bare.waitForTimeout(200);
  assert.equal(asked.length, quiet);
  assert.equal(new URL(bare.url()).pathname, "/html-preview.html");
  // 最大化 names what pressing it does now, from the native side's report.
  await bare.evaluate(() => { window.__htmlPreviewMaximized = true; window.dispatchEvent(new Event("html-preview-window")); });
  assert.equal(await bare.getByRole("button", { name: "元に戻す" }).isVisible(), true);
  await bare.evaluate(() => { window.__htmlPreviewMaximized = false; window.dispatchEvent(new Event("html-preview-window")); });
  assert.equal(await bare.getByRole("button", { name: "最大化" }).isVisible(), true);
  await bare.close();

  await page.evaluate(() => window.render('```HTML\n<title>  Second\n report </title><h2>Second report</h2>\n```\n\n```html\n<p>untitled</p>\n```'));
  assert.equal(await open("HTML").count(), 1, "no first-line name and no title falls back to HTML");
  await open("Second report").click();
  await lastPane(call => call.document?.includes("Second report") && call.bounds, "second document shown");
  await page.waitForFunction(() => window.detachedCalls.length === 2);
  // "メインウィンドウで開く" asks the native guard through a fixed navigation (#463).
  await page.getByRole("button", { name: "新しいウィンドウで開く" }).click();
  await page.waitForFunction(() => window.detachedCalls.length === 3 && document.querySelector('.html-preview').hidden);
  const back = host.getByRole("button", { name: "メインウィンドウで開く" });
  assert.equal(await back.locator("svg").count(), 1);
  const returned = host.waitForRequest(r => new URL(r.url()).pathname === "/html-preview-return");
  await host.route("**/html-preview-return", route => route.abort());
  await back.click();
  await returned;
  await page.evaluate(() => window.preview.returnToMain());
  assert.equal(await page.locator(".html-preview").isVisible(), true, "the same document returns to the side pane");
  assert.equal(await page.getByRole("button", { name: "プレビューを閉じて会話に戻る" }).evaluate(el => el === document.activeElement), true);
  await lastPane(call => call.bounds !== null, "shown again after return");
  await page.getByRole("button", { name: "プレビューを閉じて会話に戻る" }).click();
  assert.equal(await page.locator(".html-preview").isVisible(), false);
  assert.deepEqual(await lastPane(call => call.bounds === null, "emptied on close"), { document: "", bounds: null });
  assert.equal(await open("Second report").evaluate(el => el === document.activeElement), true);
  await page.waitForFunction(() => window.detachedCalls.at(-1) === null);

  await page.setViewportSize({ width: 400, height: 700 });
  await open("Second report").click();
  assert.equal(await page.getByRole("button", { name: "プレビューを閉じて会話に戻る" }).isVisible(), true);
  await page.getByRole("button", { name: "新しいウィンドウで開く" }).click();
  await page.waitForFunction(() => document.querySelector('.html-preview').hidden);
  await page.evaluate(() => window.preview.clear()); // enterTopic uses this same boundary
  await page.waitForFunction(() => window.detachedCalls.at(-1) === null);
  assert.equal(await page.locator(".html-preview iframe").count(), 0);

  // A failed render cannot erase source or leave the previous preview active.
  await page.evaluate(() => { document.querySelector('#post').append(window.preview.fileBlock(Symbol('invalid source'), 'html broken.html')); });
  await open("broken.html").click();
  assert.equal(await page.locator(".html-preview [role=status]").textContent(), "表示できませんでした。コードをご覧ください。");
  assert.equal(await page.locator(".html-preview iframe").count(), 0);
  assert.ok((await page.locator(".html-preview-code").textContent()).includes("invalid source"), "failed render leaves code readable");
  await page.evaluate(() => window.preview.clear());
  assert.ok(requests.every(url => failures.get(url) === "csp" || (url === "file:///C:/secret" && failures.get(url) === "other")));

  // A failed separate window keeps the side pane and the error text (#463).
  await page.evaluate(() => { window.detachFail = true; });
  await open("Second report").click();
  await page.getByRole("button", { name: "新しいウィンドウで開く" }).click();
  await page.waitForFunction(() => document.querySelector('.html-preview [role=status]').textContent === "別ウィンドウを開けませんでした。コードまたは横の表示をご覧ください。");
  assert.equal(await page.locator(".html-preview").isVisible(), true);
  await page.evaluate(() => { window.detachFail = false; window.preview.clear(); });

  // Main-webview reload/recreation starts without knowledge of a surviving
  // native window. Startup and topic clears must still close that window.
  await page.evaluate(() => {
    window.nativeDoc = 'old-topic native document';
    document.querySelector('.html-preview').remove();
    window.paneCalls = [];
    window.preview = window.createHtmlPreview(document.querySelector('#conversation'), async doc => {
      window.nativeDoc = doc;
      window.detachedCalls.push(doc);
    }, async placement => { window.paneCalls.push(placement); });
  });
  await page.waitForFunction(() => window.nativeDoc === null);
  assert.deepEqual(await page.evaluate(() => window.paneCalls), [{ document: "", bounds: null }], "a recreated controller empties a surviving child view");
  await page.evaluate(() => {
    window.nativeDoc = 'unknown surviving native document';
    window.preview.clear();
  });
  await page.waitForFunction(() => window.nativeDoc === null);

  if (process.env.PREVIEW_SCREENSHOTS) {
    await mkdir(process.env.PREVIEW_SCREENSHOTS, { recursive: true });
    await page.setViewportSize({ width: 1250, height: 850 });
    await page.bringToFront();
    await page.addStyleTag({ content: "html,body,#body{height:100%;margin:0}#conversation{padding:20px;overflow:auto}#post pre{white-space:pre-wrap;overflow-wrap:anywhere}.html-preview{flex-basis:56%}" });
    const report = `<style>body{background:#f5f7fc;color:#202844;line-height:1.6}.eyebrow{color:#657294;font-size:12px;letter-spacing:2px}h1{font-size:30px;margin:8px 0}.cards{display:flex;gap:12px;margin:24px 0}.card{background:white;padding:18px;border-radius:12px;flex:1}.value{font-size:28px;color:#355bc9;font-weight:bold}table{width:100%;border-collapse:collapse;background:white}th,td{text-align:left;padding:12px;border-bottom:1px solid #e8edf6}th{color:#657294;font-size:12px}.badge{color:#20805b}</style><p class="eyebrow">WEEKLY REPORT / 2026.10.17</p><h1>Project overview</h1><p>A quiet space for the work taking shape.</p><section class="cards"><div class="card">Completed<div class="value">24</div></div><div class="card">In progress<div class="value">6</div></div><div class="card">On track<div class="value">92%</div></div></section><h2>This week</h2><table><thead><tr><th>Workstream</th><th>Status</th></tr></thead><tbody><tr><td>Conversation layout</td><td class="badge">Ready</td></tr><tr><td>Static report preview</td><td class="badge">Ready</td></tr><tr><td>Documentation</td><td>In review</td></tr></tbody></table>`;
    await page.evaluate(report => { document.querySelector('#post').replaceChildren(); window.render('A static report from the room.\n\n```html weekly-report.html\n' + report + '\n```'); }, report);
    await page.getByRole("button", { name: "weekly-report.html のプレビューを開く" }).click();
    await lastPane(call => call.document?.includes("Project overview") && call.bounds, "report shown");
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    // The main page's side pane: header and the area the child view covers.
    await page.screenshot({ path: join(process.env.PREVIEW_SCREENSHOTS, "side-panel.png") });
    await host.evaluate(doc => { window.__htmlPreviewDocument = doc; window.dispatchEvent(new Event("html-preview-update")); }, await page.evaluate(report => window.htmlPreviewDocument(report), report));
    await host.setViewportSize({ width: 900, height: 800 });
    await host.bringToFront();
    await host.frameLocator("iframe").getByRole("heading", { name: "Project overview" }).waitFor();
    await host.screenshot({ path: join(process.env.PREVIEW_SCREENSHOTS, "detached-host.png") });
  }
});
