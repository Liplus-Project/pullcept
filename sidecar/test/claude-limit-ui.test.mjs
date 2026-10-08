// Execute the actual frontend functions, without starting Tauri or a CLI.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const functions = source.statements.filter(s => ts.isFunctionDeclaration(s) && ["activityNote", "limitedByUsage"].includes(s.name?.text));
assert.equal(functions.length, 2);
const compiled = ts.transpileModule(functions.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
const context = vm.createContext({NO_WORD:{word:"",line:"",kind:""},awaiting:new Map()});
vm.runInContext(compiled, context);
function view(source, limited) {
  return {ended:null,topicId:"topic",outputting:true,silent:false,
    stats:{limited,limited_source:source,five_hour:100,seven_day:100},
    activity:{word:"委任中",line:"child still listed",waiting:false,connected:true}};
}
test("confirmed Claude parent rejection outranks stale delegation and terminal repaint without removing it", () => {
  const seat = view("claude-parent", true);
  assert.equal(context.activityNote("name", seat).word, "制限中");
  assert.equal(seat.activity.word, "委任中");
  seat.stats.limited = false;
  assert.equal(context.activityNote("name", seat).word, "委任中");
  seat.activity = null;
  assert.equal(context.activityNote("name", seat).word, "出力中");
});
test("Codex reported work, permission waiting and output keep their existing priority", () => {
  const seat = view("codex", true);
  assert.equal(context.activityNote("name", seat).word, "委任中");
  seat.activity.word = "許可待ち"; seat.activity.waiting = true;
  assert.equal(context.activityNote("name", seat).word, "許可待ち");
  seat.activity = null;
  assert.equal(context.activityNote("name", seat).word, "出力中");
  seat.outputting = false;
  assert.equal(context.activityNote("name", seat).word, "制限中");
});
