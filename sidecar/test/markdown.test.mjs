// A post's words drawn as Markdown (#431): the actual src/markdown.ts, run
// against a small stand-in for the DOM. Raw HTML in a post is never an element;
// tables, emphasis, lists and code are; ```mermaid starts as its code.
import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";

class FakeNode {
  constructor(tag, text = "") {
    this.tag = tag;
    this.text = text;
    this.children = [];
    this.attrs = {};
    this.style = {};
    this.className = "";
  }
  append(...nodes) { this.children.push(...nodes); }
  appendChild(node) { this.children.push(node); return node; }
  replaceChildren(...nodes) { this.children = nodes; }
  setAttribute(name, value) { this.attrs[name] = value; }
  set textContent(value) { this.children = [new FakeNode("#text", value)]; }
  get textContent() { return this.tag === "#text" ? this.text : this.children.map(c => c.textContent).join(""); }
  set innerHTML(_) { throw new Error("innerHTML is not used for a post's words"); }
}
globalThis.document = {
  createElement: tag => new FakeNode(tag),
  createTextNode: text => new FakeNode("#text", text),
  getElementById: () => null,
};

const { appendMarkdown } = await tsImport("../../src/markdown.ts", import.meta.url);

const hooks = {
  prose: text => [new FakeNode("#text", text)],
  link: (href, label) => { const a = new FakeNode("a"); a.attrs.href = href; a.append(...label); return [a]; },
  code: (code, lang) => { const c = new FakeNode("code-block"); c.attrs.lang = lang; c.textContent = code; return c; },
};

function draw(text) {
  const body = new FakeNode("div");
  appendMarkdown(body, text, hooks);
  return body;
}

/** Every element tag under a node, in document order. */
function tags(node) {
  return node.children.flatMap(c => c.tag === "#text" ? [] : [c.tag, ...tags(c)]);
}

test("raw HTML is drawn as its text, never as an element", () => {
  for (const text of ["<script>alert(1)</script>", "前 <img src=x onerror=alert(1)> 後", "<div>\n<b>x</b>\n</div>", "<iframe src=\"https://e\"></iframe>"]) {
    const body = draw(text);
    const found = tags(body);
    for (const bad of ["script", "img", "iframe", "b", "div"]) assert.ok(!found.includes(bad), `${bad} in ${text}`);
    assert.ok(body.textContent.includes("<"), `the tag is shown as text: ${text}`);
  }
});

test("an image is its alt text; nothing is fetched", () => {
  const body = draw("![図の説明](https://example.com/x.png)");
  assert.ok(!tags(body).includes("img"));
  assert.equal(body.textContent, "図の説明");
});

test("a table becomes a table with its header and rows", () => {
  const body = draw("| 名前 | 値 |\n|:--|--:|\n| a | 1 |\n| b | **2** |");
  const found = tags(body);
  assert.deepEqual(found.filter(t => ["table", "thead", "tbody", "th", "td", "strong"].includes(t)),
    ["table", "thead", "th", "th", "tbody", "td", "td", "td", "td", "strong"]);
});

test("emphasis, inline code, lists, quotes and headings are elements", () => {
  const found = tags(draw("# 見出し\n\n*強調* と `code` と ~~消し~~\n\n- 一\n- 二\n\n1. a\n2. b\n\n> 引用\n\n---"));
  for (const tag of ["h1", "p", "em", "code", "del", "ul", "ol", "li", "blockquote", "hr"]) assert.ok(found.includes(tag), tag);
});

test("a single newline stays a newline in the words", () => {
  assert.equal(draw("一行目\n二行目").textContent, "一行目\n二行目");
});

test("code fences go to the screen's code block with their language", () => {
  const body = draw("前\n\n```ts\nconst a = 1;\n```\n\n後");
  const block = body.children.find(c => c.tag === "code-block");
  assert.equal(block.attrs.lang, "ts");
  assert.equal(block.textContent, "const a = 1;");
});

test("a mermaid fence is held as its code until the diagram is drawn", () => {
  const body = draw("```mermaid\ngraph TD; A-->B\n```");
  const holder = body.children[0];
  assert.equal(holder.className, "mermaid-holder");
  assert.equal(holder.children[0].tag, "code-block");
  assert.equal(holder.children[0].textContent, "graph TD; A-->B");
});

test("a link goes through the screen's link hook with its address", () => {
  const body = draw("[見て](https://example.com/a) と [悪い](javascript:alert(1))");
  const links = body.children[0].children.filter(c => c.tag === "a");
  assert.deepEqual(links.map(a => a.attrs.href), ["https://example.com/a", "javascript:alert(1)"]);
});

test("a UNC path and a drive path keep their backslashes", () => {
  assert.equal(draw("\\\\server\\share\\a.txt と C:\\Users\\hal").textContent, "\\\\server\\share\\a.txt と C:\\Users\\hal");
});
