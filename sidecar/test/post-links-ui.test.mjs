// Execute the screen's own reading of a post's words into text and links
// (#349), without starting Tauri or a webview.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const wanted = ["LINK_PATTERN", "LINK_TRAILING", "LINK_CLOSERS", "trimLinkEnd", "linkPieces"];
const statements = source.statements.filter(s =>
  (ts.isFunctionDeclaration(s) && wanted.includes(s.name?.text)) ||
  (ts.isVariableStatement(s) && s.declarationList.declarations.some(d => wanted.includes(d.name.getText(source)))));
assert.equal(statements.length, wanted.length);
const compiled = ts.transpileModule(statements.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
const context = vm.createContext({});
vm.runInContext(compiled, context);

/** The pieces as plain [kind, text] pairs, checked to join back to the input. */
function pieces(text) {
  // Spread into this realm: the arrays the vm makes are not deep-equal to ours.
  const read = [...context.linkPieces(text)].map(p => [p.kind, p.text]);
  assert.equal(read.map(([, part]) => part).join(""), text, "the pieces join back to the words");
  return read;
}

/** Only the links. */
function links(text) {
  return pieces(text).filter(([kind]) => kind !== "text");
}

test("http and https URLs are links, and the sentence's punctuation is not part of them", () => {
  assert.deepEqual(pieces("見て https://github.com/Liplus-Project/pullcept/issues/349。"), [
    ["text", "見て "], ["url", "https://github.com/Liplus-Project/pullcept/issues/349"], ["text", "。"],
  ]);
  assert.deepEqual(links("URL:https://example.com/a?b=1#c."), [["url", "https://example.com/a?b=1#c"]]);
  assert.deepEqual(links("HTTP://Example.com/X!"), [["url", "HTTP://Example.com/X"]]);
  assert.deepEqual(links("<https://example.com/x>"), [["url", "https://example.com/x"]]);
  assert.deepEqual(links("「https://example.com/x」と"), [["url", "https://example.com/x"]]);
  assert.deepEqual(links("`https://example.com/x`"), [["url", "https://example.com/x"]]);
});

test("a closing bracket stays when it closes something in the link", () => {
  assert.deepEqual(links("https://en.wikipedia.org/wiki/Foo_(bar)"), [["url", "https://en.wikipedia.org/wiki/Foo_(bar)"]]);
  assert.deepEqual(links("(https://en.wikipedia.org/wiki/Foo_(bar))"), [["url", "https://en.wikipedia.org/wiki/Foo_(bar)"]]);
  assert.deepEqual(links("（https://example.com/a）"), [["url", "https://example.com/a"]]);
  assert.deepEqual(links("（C:\\資料\\議事録（最終）.xlsx）"), [["path", "C:\\資料\\議事録（最終）.xlsx"]]);
});

test("no other scheme is a link, nor a path inside one", () => {
  for (const text of ["file:///C:/Windows/System32/calc.exe", "javascript:alert(1)", "ftp://example.com/", "mailto:a@example.com", "ms-settings:privacy"]) {
    assert.deepEqual(links(text), [], text);
  }
  assert.deepEqual(links("xhttps://example.com"), []);
  assert.deepEqual(links("https://"), []);
  assert.deepEqual(links("https://."), []);
});

test("drive and UNC paths are links, full-width names included", () => {
  assert.deepEqual(pieces("D:\\Users\\hal\\Code\\pullcept\\src\\main.ts を見て"), [
    ["path", "D:\\Users\\hal\\Code\\pullcept\\src\\main.ts"], ["text", " を見て"],
  ]);
  assert.deepEqual(links("D:/Users/hal/Code"), [["path", "D:/Users/hal/Code"]]);
  assert.deepEqual(links("c:\\"), [["path", "c:\\"]]);
  assert.deepEqual(links("\\\\server\\share\\dir\\f.txt、"), [["path", "\\\\server\\share\\dir\\f.txt"]]);
  assert.deepEqual(links("C:\\Users\\hal\\デスクトップ\\メモ.md"), [["path", "C:\\Users\\hal\\デスクトップ\\メモ.md"]]);
  assert.deepEqual(links("`C:\\a\\b.md` と `D:\\c`"), [["path", "C:\\a\\b.md"], ["path", "D:\\c"]]);
  assert.deepEqual(links("C:\\a、D:\\b。"), [["path", "C:\\a"], ["path", "D:\\b"]]);
  assert.deepEqual(links("「C:\\Users\\hal」を開く"), [["path", "C:\\Users\\hal"]]);
  assert.deepEqual(links("パス：C:\\x"), [["path", "C:\\x"]]);
});

test("a path stops where a name cannot go on", () => {
  assert.deepEqual(pieces("C:\\a b"), [["path", "C:\\a"], ["text", " b"]]);
  assert.deepEqual(pieces("C:\\a:b"), [["path", "C:\\a"], ["text", ":b"]]);
  assert.deepEqual(pieces("**C:\\a**"), [["text", "**"], ["path", "C:\\a"], ["text", "**"]]);
  assert.deepEqual(pieces("\"C:\\a\""), [["text", "\""], ["path", "C:\\a"], ["text", "\""]]);
});

test("relative paths, and what only looks like a path, are not links", () => {
  for (const text of ["src\\main.ts", "./src/main.ts", "..\\up", "\\rooted", "abcC:\\x", "C:", "C:relative", "1:\\x", "\\\\server", "\\\\server\\", "a/C:/b", "\\\\?\\C:\\x", "\\\\.\\PhysicalDrive0"]) {
    assert.deepEqual(links(text), [], text);
  }
});

test("newlines and spaces come back as they were", () => {
  assert.deepEqual(pieces("一行目\n  https://example.com/x\n\nC:\\y  \n"), [
    ["text", "一行目\n  "], ["url", "https://example.com/x"], ["text", "\n\n"], ["path", "C:\\y"], ["text", "  \n"],
  ]);
  assert.deepEqual(pieces(""), []);
  assert.deepEqual(pieces("リンクの無い発言"), [["text", "リンクの無い発言"]]);
});
