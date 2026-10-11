import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";
import { tsImport } from "tsx/esm/api";
const { accountName, speakerLabel } = await tsImport("../../src/account-name.ts", import.meta.url);
const source = ts.createSourceFile("main.ts", readFileSync(new URL("../../src/main.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
const wanted = ["roomLine", "trackAddress", "pruneAwaiting"];
const statements = source.statements.filter(s => ts.isFunctionDeclaration(s) && wanted.includes(s.name?.text));
assert.equal(statements.length, wanted.length);
const compiled = ts.transpileModule(statements.map(s => s.getText(source)).join("\n"), {compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
class Element {
  textContent = ""; children = []; classList = {add(){}}; style = {setProperty(){}};
  appendChild(one) {this.children.push(one); return one;} append(...ones) {this.children.push(...ones);}
  get childElementCount() {return this.children.length;}
}
const awaiting = new Map(); const rosters = new Map();
const context = vm.createContext({speakerLabel, awaiting, rosters, shownTopicId: () => "topic", renderPanel(){},
  document:{createElement:()=>new Element()}, avatar:()=>new Element(), fullDateTime:()=>"time",
  splitAttachments:content=>({text:content, paths:[]}), appendPostWords(){}, MINE_SPEAKER:"blue"});
vm.runInContext(compiled, context);
function header(line) {
 const article = context.roomLine({speaker:"Luna", full_name:"Codex Luna", account:"a", app:false, colour:"blue", to:[], ts:"t", stamp:"t", content:"hi", past:true, mine:false, ...line});
 return article.children.flatMap(e=>e.children).find(e=>e.className === "speaker")?.textContent;
}
test("conversation headers show the posting nickname in history and new messages", () => {
 assert.equal(header({}), "Luna");
 assert.equal(header({speaker:"Moon", full_name:"New Name", past:false}), "Moon");
 assert.equal(header({full_name:undefined}), "Luna");
 assert.equal(header({speaker:"Codex Luna", full_name:"Codex Luna"}), "Codex Luna");
 assert.equal(accountName({name:"Codex Luna", nickname:" Luna "}), "Luna");
 for (const nickname of [undefined, null, "", "  "]) assert.equal(accountName({name:"Codex Luna", nickname}), "Codex Luna");
});
test("awaiting replies follows addressing nickname and drops renamed/disconnected call-names", () => {
 rosters.set("topic", [{name:"Luna", full_name:"Codex Luna", own:false}, {name:"Luna", full_name:"Other Luna", own:false}]);
 context.trackAddress({topic_id:"topic", speaker:"Master", to:["Codex Luna"]});
 assert.equal(awaiting.get("topic").size, 0);
 context.trackAddress({topic_id:"topic", speaker:"Master", to:["Luna"]});
 assert.equal(awaiting.get("topic").has("Luna"), true);
 context.trackAddress({topic_id:"topic", speaker:"Luna", full_name:"Codex Luna", to:[]});
 assert.equal(awaiting.get("topic").size, 0);
 context.trackAddress({topic_id:"topic", speaker:"Master", to:["Luna"]});
 rosters.set("topic", [{name:"Moon", full_name:"Codex Luna", own:false}]);
 context.pruneAwaiting("topic");
 assert.equal(awaiting.get("topic").size, 0);
});
