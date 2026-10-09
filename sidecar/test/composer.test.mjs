// The input box's editor (#354), away from a page: the text it hands the room,
// the text it reads back, its paste and copy, and its keys, run as the
// ProseMirror commands the box binds them to.
import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";

const composer = await tsImport("../../src/composer.ts", import.meta.url);
const { EditorState, TextSelection } = await tsImport("@tiptap/pm/state", import.meta.url);
const schema = composer.composerSchema();

/**
 * A document from blocks — ["p", words] or ["code", text, language?] — with
 * the caret where `|` stands in one of them; the `|` is not part of the text.
 */
function stateOf(blocks) {
  let caret = null;
  let pos = 0;
  const nodes = blocks.map(([kind, marked, language = null]) => {
    const at = marked.indexOf("|");
    const text = marked.replace("|", "");
    if (at >= 0) caret = pos + 1 + at;
    const node = schema.node(kind === "code" ? "codeBlock" : "paragraph", kind === "code" ? { language } : null, text ? [schema.text(text)] : []);
    pos += node.nodeSize;
    return node;
  });
  const doc = schema.node("doc", null, nodes);
  return EditorState.create({ doc, selection: TextSelection.create(doc, caret ?? 1) });
}

/** A state back as blocks, the caret marked with `|`. */
function blocksOf(state) {
  const head = state.selection.head;
  const out = [];
  state.doc.forEach((node, offset) => {
    let text = node.textContent;
    if (head > offset && head < offset + node.nodeSize) {
      const at = head - offset - 1;
      text = text.slice(0, at) + "|" + text.slice(at);
    }
    out.push(node.type.name === "codeBlock" ? ["code", text, node.attrs.language] : ["p", text]);
  });
  return out;
}

/** Run a command; the blocks after it, or null when it did not take the key. */
function run(command, blocks, ...rest) {
  let state = stateOf(blocks);
  const took = command(state, (tr) => (state = state.apply(tr)), ...rest);
  return took ? blocksOf(state) : null;
}

const edge = (value) => () => value;

// ── the text sent and read back ──────────────────────────────────────────────

const textOf = (text) => composer.textFromBlocks(composer.docFromText(schema, text).content);

test("prose lines and code blocks are sent as lines and ``` fences", () => {
  const state = stateOf([["p", "見て"], ["code", "const a = 1;\n\n  b();", "ts"], ["p", "以上"]]);
  assert.equal(composer.textFromBlocks(state.doc.content), "見て\n```ts\nconst a = 1;\n\n  b();\n```\n以上");
  // No language picked (自動): a bare fence, and the room guesses as it draws.
  assert.equal(composer.textFromBlocks(stateOf([["code", "x"]]).doc.content), "```\nx\n```");
  assert.equal(composer.textFromBlocks(stateOf([["code", ""]]).doc.content), "```\n\n```");
});

test("a fence is longer than any line of the code that would close it", () => {
  const sent = composer.textFromBlocks(stateOf([["code", "```\nnot a close\n````  "]]).doc.content);
  assert.equal(sent, "`````\n```\nnot a close\n````  \n`````");
  // Read back, the code is whole.
  const back = composer.docFromText(schema, sent);
  assert.equal(back.childCount, 1);
  assert.equal(back.firstChild.type.name, "codeBlock");
  assert.equal(back.firstChild.textContent, "```\nnot a close\n````  ");
  // A line that only starts with backticks closes nothing, so it does not lengthen the fence.
  assert.equal(composer.textFromBlocks(stateOf([["code", "```ts"]]).doc.content), "```\n```ts\n```");
});

test("text reads back into lines and code blocks, as the room draws it (#348, #352)", () => {
  const blocks = (text) => blocksOf(EditorState.create({ doc: composer.docFromText(schema, text) })).map((one) => one.map((part) => (typeof part === "string" ? part.replace("|", "") : part)));
  assert.deepEqual(blocks("a\n\n```ts\nx\n\ny\n```\nb"), [["p", "a"], ["p", ""], ["code", "x\n\ny", "ts"], ["p", "b"]]);
  assert.deepEqual(blocks("```あ。。。\nb\n```"), [["code", "あ。。。\nb", null]]);
  assert.deepEqual(blocks("see\n```\nopen to the end"), [["p", "see"], ["code", "open to the end", null]]);
  assert.deepEqual(blocks(""), [["p", ""]]);
  assert.deepEqual(blocks("a\r\nb"), [["p", "a"], ["p", "b"]]);
});

test("what the box sends reads back to the same text", () => {
  for (const text of ["hello", "a\n\nb", "```ts\nconst a = 1;\n```", "x\n```\n\n\n```\ny", "````\n```\n````", "@Lin 見て\n```sh\nnpm ci\n```"]) {
    assert.equal(textOf(text), text, text);
  }
});

// ── paste and copy ───────────────────────────────────────────────────────────

const paste = (blocks, text) => {
  const state = stateOf(blocks);
  return blocksOf(state.apply(composer.pasteText(state, text)));
};

test("pasted text with fences arrives as code blocks, its blank lines kept", () => {
  assert.deepEqual(paste([["p", "前|後"]], "a\n\n```js\nx()\n```\nb"), [
    ["p", "前a"], ["p", ""], ["code", "x()", "js"], ["p", "b|後"],
  ]);
  assert.deepEqual(paste([["p", "|"]], "one line"), [["p", "one line|"]]);
});

test("text pasted into a code block is code, fences and all", () => {
  assert.deepEqual(paste([["code", "a|"]], "\n```\nb\r\n"), [["code", "a\n```\nb\n|", null]]);
});

test("a copy within one line or one block is its words; across blocks, the text sent", () => {
  const state = stateOf([["p", "ab"], ["code", "x\ny", "ts"], ["p", "cd"]]);
  const slice = (from, to) => state.doc.slice(from, to);
  assert.equal(composer.textFromSlice(slice(1, 3)), "ab");
  assert.equal(composer.textFromSlice(slice(5, 8)), "x\ny");
  assert.equal(composer.textFromSlice(slice(0, state.doc.content.size)), "ab\n```ts\nx\ny\n```\ncd");
});

// ── the keys ─────────────────────────────────────────────────────────────────

test("Shift+Enter is a line of the code inside a block and a new line outside it", () => {
  assert.deepEqual(run(composer.lineBreak, [["code", "a|b"]]), [["code", "a\n|b", null]]);
  assert.deepEqual(run(composer.lineBreak, [["code", "a\n|"]]), [["code", "a\n\n|", null]]);
  assert.deepEqual(run(composer.lineBreak, [["p", "a|b"]]), [["p", "a"], ["p", "|b"]]);
});

test("Down on a block's last line leaves it, onto the line below or a new one", () => {
  assert.deepEqual(run(composer.leaveCodeDown, [["code", "a\nb|"]], edge(true)), [["code", "a\nb", null], ["p", "|"]]);
  assert.deepEqual(run(composer.leaveCodeDown, [["code", "a|"], ["p", "next"]], edge(true)), [["code", "a", null], ["p", "|next"]]);
  // Not on the last line, and not in a block: Down is the browser's.
  assert.equal(run(composer.leaveCodeDown, [["code", "a|\nb"]], edge(false)), null);
  assert.equal(run(composer.leaveCodeDown, [["p", "a|"]], edge(true)), null);
});

test("Up on the line under a block goes back into it, at the end of its last line", () => {
  assert.deepEqual(run(composer.enterCodeUp, [["code", "a\nb"], ["p", "c|d"]], edge(true)), [["code", "a\nb|", null], ["p", "cd"]]);
  assert.equal(run(composer.enterCodeUp, [["code", "a"], ["p", "c|d"]], edge(false)), null);
  assert.equal(run(composer.enterCodeUp, [["p", "a"], ["p", "|b"]], edge(true)), null);
});

test("Up on the first line of a block that opens the text makes a line above it", () => {
  assert.deepEqual(run(composer.enterCodeUp, [["code", "a|"]], edge(true)), [["p", "|"], ["code", "a", null]]);
  assert.equal(run(composer.enterCodeUp, [["p", "x"], ["code", "a|"]], edge(true)), null);
});

test("Backspace on a block's empty last line takes the line out of the block, with the caret", () => {
  assert.deepEqual(run(composer.backspaceInCode, [["code", "a\n|"]]), [["code", "a", null], ["p", "|"]]);
  assert.deepEqual(run(composer.backspaceInCode, [["code", "a\n|"], ["p", "next"]]), [["code", "a", null], ["p", "|"], ["p", "next"]]);
  // The code's empty lines above stay: only the caret's line leaves.
  assert.deepEqual(run(composer.backspaceInCode, [["code", "a\n\n|", "ts"]]), [["code", "a\n", "ts"], ["p", "|"]]);
});

test("Backspace in a block with nothing in it takes the block away", () => {
  assert.deepEqual(run(composer.backspaceInCode, [["p", "x"], ["code", "|"]]), [["p", "x"], ["p", "|"]]);
  assert.deepEqual(run(composer.backspaceInCode, [["code", "|"]]), [["p", "|"]]);
});

test("Backspace at the start of a block keeps the frame", () => {
  assert.deepEqual(run(composer.backspaceInCode, [["p", ""], ["code", "|a"]]), [["code", "|a", null]]);
  assert.deepEqual(run(composer.backspaceInCode, [["p", "x"], ["code", "|a"]]), [["p", "x"], ["code", "|a", null]]);
  assert.deepEqual(run(composer.backspaceInCode, [["code", "|a"]]), [["code", "|a", null]]);
});

test("Backspace anywhere else is Backspace as usual", () => {
  assert.equal(run(composer.backspaceInCode, [["code", "ab|"]]), null);
  assert.equal(run(composer.backspaceInCode, [["code", "a\n|\nb"]]), null);
  assert.equal(run(composer.backspaceInCode, [["p", "|"]]), null);
});

// ── languages ────────────────────────────────────────────────────────────────

test("a language is shown by its name, whichever alias the fence wrote", () => {
  assert.equal(composer.codeLanguageName("ts"), "TypeScript");
  assert.equal(composer.codeLanguageName("typescript"), "TypeScript");
  assert.equal(composer.codeLanguageName("CPP"), "C++");
  assert.equal(composer.codeLanguageName("sh"), "Bash");
  assert.equal(composer.codeLanguageName("nosuch"), "nosuch");
});

test("the box's document starts as one empty line, never as a code block", () => {
  assert.equal(schema.topNodeType.createAndFill().firstChild.type.name, "paragraph");
});
