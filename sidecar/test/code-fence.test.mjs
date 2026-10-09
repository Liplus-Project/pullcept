// The ``` fences of a post's words (#348): the actual frontend functions, run
// without Tauri or a DOM. The fences are read in src/composer.ts (#354), which
// the input box shares; `splitAttachments` stays in src/main.ts and is run here
// with those functions beside it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { tsImport } from "tsx/esm/api";

const fences = await tsImport("../../src/composer.ts", import.meta.url);
const names = ["splitAttachments", "ATTACHMENT_HEAD"];
const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const statements = source.statements.filter(s =>
  (ts.isFunctionDeclaration(s) && names.includes(s.name?.text)) ||
  (ts.isVariableStatement(s) && s.declarationList.declarations.some(d => names.includes(d.name.getText(source)))));
assert.equal(statements.length, names.length);
const compiled = ts.transpileModule(statements.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
// `attachmentsRoot` is read off the global, as the screen's `let` is.
const ROOT = "C:\\rooms\\r1\\attachments";
const context = vm.createContext({
  attachmentsRoot: ROOT,
  findCodeFences: fences.findCodeFences,
  lineInFence: fences.lineInFence,
});
vm.runInContext(compiled, context);
context.splitCodeFences = fences.splitCodeFences;
const plain = value => JSON.parse(JSON.stringify(value));
const split = text => plain(context.splitCodeFences(text));
const FENCE = "```";

test("a text without a fence is one piece, itself", () => {
  assert.deepEqual(split("hello\nworld"), [{kind:"text", text:"hello\nworld"}]);
  assert.deepEqual(split(""), [{kind:"text", text:""}]);
});

test("a closed fence is cut out as code, the lines around it as prose", () => {
  assert.deepEqual(split("before\n```ts\nconst a = 1;\n  indented\n```\nafter"), [
    {kind:"text", text:"before"},
    {kind:"code", code:"const a = 1;\n  indented", lang:"ts"},
    {kind:"text", text:"after"},
  ]);
});

test("the language is one word after the fence and never part of the code", () => {
  assert.deepEqual(split("```\nx\n```"), [{kind:"code", code:"x", lang:""}]);
  for (const lang of ["ts", "c++", "c#", "objective-c", "file_name.py", "v1.2"]) {
    assert.deepEqual(split(`${FENCE}${lang}\nx\n${FENCE}`), [{kind:"code", code:"x", lang}]);
  }
  assert.deepEqual(split("```  sh  \nx\n```"), [{kind:"code", code:"x", lang:"sh"}]);
});

test("words after the fence that are not one language word are the code's first line (#352)", () => {
  assert.deepEqual(split("```あ。。。"), [{kind:"code", code:"あ。。。", lang:""}]);
  assert.deepEqual(split("```あ。。。\nnext\n```"), [{kind:"code", code:"あ。。。\nnext", lang:""}]);
  assert.deepEqual(split("```python title\nx\n```"), [{kind:"code", code:"python title\nx", lang:""}]);
  assert.deepEqual(split("``` hello world\n```"), [{kind:"code", code:"hello world", lang:""}]);
  assert.deepEqual(split("```ts!\nx\n```"), [{kind:"code", code:"ts!\nx", lang:""}]);
  assert.deepEqual(split("```日本語\r\nx\r\n```"), [{kind:"code", code:"日本語\nx", lang:""}]);
});

test("an unclosed fence runs to the end", () => {
  assert.deepEqual(split("see\n```\nline 1\n\nline 3"), [
    {kind:"text", text:"see"},
    {kind:"code", code:"line 1\n\nline 3", lang:""},
  ]);
  assert.deepEqual(split("```"), [{kind:"code", code:"", lang:""}]);
});

test("backticks inside a sentence or on one line fence nothing", () => {
  for (const text of ["type ``` to start", "```inline```", "a `b` c"]) {
    assert.deepEqual(split(text), [{kind:"text", text}]);
  }
});

test("a fence closes only on at least as many backticks with nothing after them", () => {
  assert.deepEqual(split("````\n```\nstill code\n````"), [{kind:"code", code:"```\nstill code", lang:""}]);
  assert.deepEqual(split("```\n``` not a close\n```  "), [{kind:"code", code:"``` not a close", lang:""}]);
});

test("several fences, and blank lines kept in the prose between them", () => {
  assert.deepEqual(split("```\na\n```\n\nmid\n```sh\nb\n```"), [
    {kind:"code", code:"a", lang:""},
    {kind:"text", text:"\nmid"},
    {kind:"code", code:"b", lang:"sh"},
  ]);
});

test("CRLF lines are read as fences too", () => {
  assert.deepEqual(split("```js\r\nx\r\n```\r\nend"), [
    {kind:"code", code:"x", lang:"js"},
    {kind:"text", text:"end"},
  ]);
});

test("fences are found by line index, open ones with a null close", () => {
  const fences = plain(context.findCodeFences(["a", "```x", "b", "```", "````", "c"]));
  assert.deepEqual(fences, [
    {open:1, close:3, ticks:3, lang:"x", lead:""},
    {open:4, close:null, ticks:4, lang:"", lead:""},
  ]);
});

test("a 添付: block written inside an unclosed fence stays code", () => {
  const path = `${ROOT}\\a.png`;
  const content = `see\n${FENCE}\nlog\n\n添付:\n${path}`;
  assert.deepEqual(plain(context.splitAttachments(content)), {text: content, paths: []});
  assert.deepEqual(split(content), [
    {kind:"text", text:"see"},
    {kind:"code", code:`log\n\n添付:\n${path}`, lang:""},
  ]);
});

test("attachments after a closed fence, or with no fence, are still read as attachments", () => {
  const path = `${ROOT}\\a.png`;
  const closed = `${FENCE}\nx\n${FENCE}\n\n添付:\n${path}`;
  assert.deepEqual(plain(context.splitAttachments(closed)), {text: `${FENCE}\nx\n${FENCE}`, paths: [path]});
  assert.deepEqual(plain(context.splitAttachments(`hi\n\n添付:\n${path}`)), {text: "hi", paths: [path]});
  assert.deepEqual(plain(context.splitAttachments(`添付:\n${path}`)), {text: "", paths: [path]});
});

test("a fence opened by words closes and keeps 添付: as code like any other (#352)", () => {
  assert.deepEqual(plain(context.findCodeFences(["```あ。。。", "b", "```", "c"])),
    [{open:0, close:2, ticks:3, lang:"", lead:"あ。。。"}]);
  const path = `${ROOT}\\a.png`;
  const content = `${FENCE}あ。。。\n\n添付:\n${path}`;
  assert.deepEqual(plain(context.splitAttachments(content)), {text: content, paths: []});
});
