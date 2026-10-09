// The ``` fences of a post's words (#348): the actual frontend functions, run
// without Tauri or a DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const names = ["findCodeFences", "splitCodeFences", "lineInFence", "inOpenFence", "splitAttachments", "ATTACHMENT_HEAD"];
const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const statements = source.statements.filter(s =>
  (ts.isFunctionDeclaration(s) && names.includes(s.name?.text)) ||
  (ts.isVariableStatement(s) && s.declarationList.declarations.some(d => names.includes(d.name.getText(source)))));
assert.equal(statements.length, names.length);
const compiled = ts.transpileModule(statements.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
// `attachmentsRoot` is read off the global, as the screen's `let` is.
const ROOT = "C:\\rooms\\r1\\attachments";
const context = vm.createContext({attachmentsRoot: ROOT});
vm.runInContext(compiled, context);
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

test("the language is the first word after the fence and never part of the code", () => {
  assert.deepEqual(split("```python title\nx\n```"), [{kind:"code", code:"x", lang:"python"}]);
  assert.deepEqual(split("```\nx\n```"), [{kind:"code", code:"x", lang:""}]);
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
  const fences = plain(context.findCodeFences(["a", "```x", "b", "```", "```", "c"]));
  assert.deepEqual(fences, [{open:1, close:3, lang:"x"}, {open:4, close:null, lang:""}]);
});

/** inOpenFence with the caret where `|` stands; the `|` is not part of the text. */
const openAt = marked => context.inOpenFence(marked.replace("|", ""), marked.indexOf("|"));

test("Enter breaks the line only while the caret is inside a fence", () => {
  assert.equal(openAt("hello|"), false);
  assert.equal(openAt("```ts|"), true);
  assert.equal(openAt("```ts\nconst a|"), true);
  assert.equal(openAt("```ts\nconst a\n```|"), false);
  assert.equal(openAt("say ``` here|"), false);
  assert.equal(openAt("before|\n```ts\nx"), false);
  assert.equal(openAt("```\nx\n```\nafter|"), false);
});

test("a fence is judged on the whole text, not on what is before the caret", () => {
  // "``` suffix" is no close: the fence stays open wherever the caret is on it.
  assert.equal(openAt("```\ncode\n```| suffix"), true);
  assert.equal(openAt("```\ncode\n``` suffix|"), true);
  // A caret above the close is inside; the close line itself is not.
  assert.equal(openAt("```\nco|de\n```"), true);
  assert.equal(openAt("```\ncode\n```  |"), false);
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
