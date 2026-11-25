import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { test } from "node:test";
import { chromium } from "playwright-core";
import { createServer } from "vite";

const edge = process.env.PREVIEW_BROWSER ?? [
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/chromium", "/usr/bin/chromium-browser", "/usr/bin/google-chrome",
].find(existsSync);
if (!edge) throw new Error("Install Chromium/Edge or set PREVIEW_BROWSER to its executable");

async function openRoom(t, options) {
  const server = await createServer({
    server: { host: "127.0.0.1", port: 0 },
    logLevel: "error",
    plugins: [{
      name: "room-accessibility-test-entry",
      enforce: "pre",
      transform(code, id) {
        if (id.endsWith("/src/main.ts")) {
          return code.replace("void main();", "window.roomTest = { drawTopic, appendMessage };");
        }
      },
    }],
  });
  await server.listen();
  let browser;
  t.after(async () => {
    await browser?.close();
    await server.close();
  });
  browser = await chromium.launch({ executablePath: edge, headless: true });
  const page = await browser.newPage(options);
  await page.addInitScript(() => {
    window.calls = [];
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
      transformCallback: () => 1,
      unregisterCallback() {},
      async invoke(cmd, args) {
        window.calls.push({ cmd, args });
        if (cmd === "plugin:event|listen") return 1;
      },
    };
  });
  await page.goto(server.resolvedUrls.local[0]);
  await page.waitForFunction(() => window.roomTest);
  return page;
}

test("room posts expose readable Markdown and controls with a single selectable copy", { timeout: 30000 }, async t => {
  const page = await openRoom(t, { permissions: ["clipboard-read", "clipboard-write"] });

  const markdown = [
    "# 聞き取れる見出し",
    "Before **strong words** and a [post link](https://example.test/) after.",
    "- First item\n- Second item",
    "| Header |\n| --- |\n| Readable cell |",
    "```html fixture.html\n<h1>Preview fixture</h1>\n```",
  ].join("\n\n");
  await page.evaluate(content => {
    const post = { speaker: "Fixture speaker", ts: "2026-11-25T00:00:00Z", to: [], account: null, hue: null, own: false };
    window.roomTest.drawTopic([{ ...post, message_id: "history", content: `History post\n\n${content}` }]);
    window.roomTest.appendMessage({ ...post, message_id: "live", content: `Live post\n\n${content}` });
  }, markdown);
  const room = page.locator("#room");
  assert.equal(await room.getAttribute("aria-live"), "polite");
  const posts = await room.getByRole("article").all();
  assert.equal(posts.length, 2);
  for (const [index, post] of posts.entries()) {
    const snapshot = await post.ariaSnapshot();
    assert.match(snapshot, index === 0 ? /History post/ : /Live post/);
    assert.match(snapshot, /heading "聞き取れる見出し"/);
    assert.match(snapshot, /listitem: Second item/);
    assert.match(snapshot, /cell "Readable cell"/);
    assert.match(snapshot, /link "post link"/);
    assert.match(snapshot, /button "fixture.html のプレビューを開く"/);
    assert.equal(snapshot.match(/strong words/g)?.length, 1);
  }

  const live = posts[1];
  const paragraph = live.locator("p").filter({ hasText: "post link" });
  const selected = await paragraph.evaluate(el => {
    const selection = getSelection();
    selection.selectAllChildren(el);
    return selection.toString();
  });
  const expected = "Before strong words and a post link after.";
  assert.equal(selected, expected);
  await page.keyboard.press("ControlOrMeta+c");
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), expected);
  const richClipboard = await page.evaluate(async () => {
    const [item] = await navigator.clipboard.read();
    const html = await (await item.getType("text/html")).text();
    return new DOMParser().parseFromString(html, "text/html").body.textContent;
  });
  assert.equal(richClipboard, expected);
  await page.evaluate(() => getSelection().removeAllRanges());
  await live.getByRole("link", { name: "post link" }).click();
  await page.waitForFunction(() => window.calls.some(({ cmd, args }) => cmd === "open_post_url" && args.url === "https://example.test/"));
  await live.getByRole("button", { name: "fixture.html のプレビューを開く" }).click();
  await page.waitForFunction(() => window.calls.some(({ cmd, args }) => cmd === "html_preview" && args.pane?.document.includes("Preview fixture")));
});

test("long history and arriving posts scroll inside the room", { timeout: 30000 }, async t => {
  const page = await openRoom(t, { viewport: { width: 1000, height: 640 }, reducedMotion: "reduce" });
  const measurements = await page.evaluate(() => {
    const post = {
      speaker: "Fixture speaker", ts: "2026-11-25T00:00:00Z", to: [], account: null, hue: null, own: false,
      content: "A short paragraph with **readable Markdown**.\n\nOne more paragraph.",
    };
    const room = document.querySelector("#room");
    const composer = document.querySelector("#composer");
    const root = document.scrollingElement;
    const measure = () => ({
      pageHeight: root.clientHeight,
      pageScrollHeight: root.scrollHeight,
      roomHeight: room.clientHeight,
      roomScrollHeight: room.scrollHeight,
      composerBottom: composer.getBoundingClientRect().bottom,
    });

    window.roomTest.drawTopic(Array.from({ length: 40 }, (_, i) => ({ ...post, message_id: `history-${i}` })));
    const history = measure();
    window.roomTest.appendMessage({ ...post, message_id: "arriving" });
    return { history, arriving: measure() };
  });

  assert.deepEqual(
    Object.values(measurements).map(({ pageHeight, pageScrollHeight }) => pageScrollHeight - pageHeight),
    [0, 0],
    `The page should fit after loading history and after an append: ${JSON.stringify(measurements)}`,
  );
  for (const { pageHeight, roomHeight, roomScrollHeight, composerBottom } of Object.values(measurements)) {
    assert.ok(roomScrollHeight > roomHeight, "The room has its own scroll range");
    assert.ok(composerBottom <= pageHeight, "The composer fits in the viewport");
  }
  assert.ok(measurements.arriving.roomScrollHeight > measurements.history.roomScrollHeight, "The arriving post extends the room");
  await page.locator("#room").evaluate(room => { room.scrollTop = 0; });
  await page.locator("#room").hover();
  await page.mouse.wheel(0, 130);
  await page.waitForFunction(() => document.querySelector("#room").scrollTop > 0);
  assert.equal(await page.evaluate(() => document.scrollingElement.scrollTop), 0);
});
