// The ``` fences of a post's words (#348): the actual frontend functions, run
// without Tauri or a DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const names = ["findCodeFences", "splitCodeFences", "inOpenFence"];
const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const functions = source.statements.filter(s => ts.isFunctionDeclaration(s) && names.includes(s.name?.text));
assert.equal(functions.length, names.length);
const compiled = ts.transpileModule(functions.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
const context = vm.createContext({});
vm.runInContext(compiled, context);
const split = text => JSON.parse(JSON.stringify(context.splitCodeFences(text)));

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

test("Enter breaks the line only while the caret is inside an open fence", () => {
  assert.equal(context.inOpenFence("hello"), false);
  assert.equal(context.inOpenFence("```ts"), true);
  assert.equal(context.inOpenFence("```ts\nconst a"), true);
  assert.equal(context.inOpenFence("```ts\nconst a\n```"), false);
  assert.equal(context.inOpenFence("say ``` here"), false);
});

test("fences are found by line index, open ones with a null close", () => {
  const fences = JSON.parse(JSON.stringify(context.findCodeFences(["a", "```x", "b", "```", "```", "c"])));
  assert.deepEqual(fences, [{open:1, close:3, lang:"x"}, {open:4, close:null, lang:""}]);
});
