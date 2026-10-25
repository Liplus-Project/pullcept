import {test} from "node:test";
import assert from "node:assert/strict";
import {extractInstructions, promptOptions} from "../src/codex-character.mjs";
const message = (...text) => ({type:"message", id:"random-id", role:"developer", content:text.map(text=>({type:"input_text", text}))});
test("sentinel matches exactly one instruction replacement, or its absence", () => {
  assert.equal(extractInstructions([message("IDENTITY", "SKILLS")], [message("sentinel", "SKILLS")], "sentinel"), "IDENTITY");
  assert.equal(extractInstructions([message("SKILLS")], [message("sentinel", "SKILLS")], "sentinel"), null);
  assert.equal(extractInstructions([], [message("sentinel")], "sentinel"), null);
  assert.equal(extractInstructions([message("", "SKILLS")], [message("sentinel", "SKILLS")], "sentinel"), "");
  for (const before of [[message("SKILLS", "PERMISSIONS")], [message("IDENTITY", "CHANGED")]]) {
    assert.throws(()=>extractInstructions(before, [message("sentinel", "SKILLS")], "sentinel"));
  }
  assert.throws(()=>extractInstructions([message("original")], [message("sentinel", "sentinel")], "sentinel"));
  assert.throws(()=>extractInstructions([{...message("original"),role:"user"}], [message("sentinel")], "sentinel"));
});
test("non-message items such as additional_tools are ignored", () => {
  const tools = {type:"additional_tools", role:"developer", tools:[{name:"x"}]};
  assert.equal(extractInstructions([tools, message("IDENTITY", "SKILLS")], [tools, message("sentinel", "SKILLS")], "sentinel"), "IDENTITY");
  assert.equal(extractInstructions([tools, message("SKILLS")], [message("sentinel", "SKILLS")], "sentinel"), null);
  assert.throws(()=>extractInstructions([tools, {...message("IDENTITY"), content:"bad"}], [tools, message("sentinel")], "sentinel"));
});
test("discovery retains native profile/cd/config values and consumes other option values", () => {
  assert.deepEqual(promptOptions(["resume", "id", "--no-alt-screen", "--model", "--cd=not-a-directory", "--profile", "custom.v2", "-C", "dir", "-c", "developer_instructions='body'", "--config=features.memories=false"]),
    ["--profile", "custom.v2", "-C", "dir", "-c", "developer_instructions='body'", "--config=features.memories=false"]);
  assert.throws(()=>promptOptions(["--profile"]));
  for (const flag of ["-pcustom", "-Csubdir", "-cdeveloper_instructions=body", "-p=work", "-C=dir", "-c=key=value"]) assert.throws(()=>promptOptions([flag]));
});
