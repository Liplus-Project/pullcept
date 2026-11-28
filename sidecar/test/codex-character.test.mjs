import {test} from "node:test";
import assert from "node:assert/strict";
import {extractInstructions, promptOptions} from "../src/codex-character.mjs";
import {mkdtempSync, writeFileSync, chmodSync, rmSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {fileURLToPath} from "node:url";
import {execFileSync} from "node:child_process";
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
const tools = (...names) => `<tools>\nDeferred tool namespaces:\n${names.map(name => `- ${name}: tools`).join("\n")}\n</tools>`;
test("deferred tool namespaces may differ between the two calls (#622)", () => {
  const full = tools("mcp__codex_apps", "mcp__github_webhook_mcp"), partial = tools("mcp__codex_apps");
  assert.equal(extractInstructions([message("IDENTITY", "SKILLS", full)], [message("sentinel", "SKILLS", partial)], "sentinel"), "IDENTITY");
  assert.equal(extractInstructions([message("SKILLS", partial)], [message("sentinel", "SKILLS", full)], "sentinel"), null);
  // Instructions shaped like the block sit in the sentinel's slot and are still read.
  assert.equal(extractInstructions([message(full, "SKILLS", partial)], [message("sentinel", "SKILLS", full)], "sentinel"), full);
  // A block only pairs with a block: one standing in for other text still fails.
  assert.throws(() => extractInstructions([message(full, "SKILLS", partial)], [message("SKILLS", "sentinel", full)], "sentinel"));
});
test("a difference outside the volatile block still fails, with a fixed code", () => {
  const block = tools("mcp__codex_apps");
  const code = error => error.diagnostic_code === "comparison" && !error.message.includes("SECRET");
  assert.throws(() => extractInstructions([message("IDENTITY", "SKILLS SECRET", block)], [message("sentinel", "SKILLS", block)], "sentinel"), code);
  assert.throws(() => extractInstructions([message("IDENTITY", "SKILLS")], [message("sentinel", "SKILLS", block)], "sentinel"), code);
  // Only the CLI's own block shape is volatile: other developer text is compared as is.
  assert.throws(() => extractInstructions([message("IDENTITY", "<tools>\nA\n</tools> SECRET")], [message("sentinel", "<tools>\nB\n</tools> SECRET")], "sentinel"), code);
  const user = text => ({...message(text), role:"user"});
  assert.throws(() => extractInstructions([message("IDENTITY"), user(block)], [message("sentinel"), user(tools("x"))], "sentinel"), code);
  assert.throws(() => extractInstructions([message("IDENTITY")], [message("SKILLS")], "sentinel"), error => error.diagnostic_code === "sentinel");
  assert.throws(() => extractInstructions({}, [message("sentinel")], "sentinel"), error => error.diagnostic_code === "prompt_shape");
  assert.throws(() => promptOptions(["--profile"]), error => error.diagnostic_code === "options");
});
test("the CLI reply names the failing call and stage and the elapsed time, never prompt text", () => {
  const scratch = mkdtempSync(join(tmpdir(), "pullcept-character-stage-"));
  try {
    // A stand-in CLI: `mode` picks what each call prints; every prompt carries SECRET text.
    writeFileSync(join(scratch, "fake.mjs"), `import {readFileSync} from "node:fs";
const mode = readFileSync(${JSON.stringify(join(scratch, "mode"))}, "utf8");
const sentinel = process.argv.map(arg => /^developer_instructions='(.*)'$/.exec(arg)?.[1]).find(Boolean);
const call = sentinel ? "call2" : "call1";
const tools = names => "<tools>\\nDeferred tool namespaces:\\n" + names.map(name => "- " + name + ": SECRET").join("\\n") + "\\n</tools>";
const prompt = (first, names, skills = "SKILLS SECRET") => [{type:"message", role:"developer", content:[first, skills, tools(names)].map(text => ({type:"input_text", text}))}];
if (mode === call + "_exit") { process.stdout.write("SECRET"); process.exit(3); }
if (mode === call + "_json") { process.stdout.write("SECRET not JSON"); process.exit(0); }
const skills = mode === "comparison" && sentinel ? "SKILLS CHANGED" : undefined;
process.stdout.write(JSON.stringify(sentinel ? prompt(sentinel, ["apps"], skills) : prompt("INSTRUCTIONS SECRET", ["apps", "webhook"])));
`);
    let bin;
    if (process.platform === "win32") { bin = join(scratch, "fake.cmd"); writeFileSync(bin, `@echo off\r\n"${process.execPath}" "${join(scratch, "fake.mjs")}" %*\r\n`); }
    else { bin = join(scratch, "fake"); writeFileSync(bin, `#!/bin/sh\nexec "${process.execPath}" "${join(scratch, "fake.mjs")}" "$@"\n`); chmodSync(bin, 0o755); }
    const run = mode => {
      writeFileSync(join(scratch, "mode"), mode);
      try { return {ok:true, reply:JSON.parse(execFileSync(process.execPath, [fileURLToPath(new URL("../src/codex-character.mjs", import.meta.url)), bin, scratch, "[]"], {encoding:"utf8", windowsHide:true}))}; }
      catch (error) { return {ok:false, raw:error.stdout, reply:JSON.parse(error.stdout)}; }
    };
    assert.deepEqual(run("volatile"), {ok:true, reply:{instructions:"INSTRUCTIONS SECRET"}});
    for (const code of ["call1_exit", "call1_json", "call2_exit", "call2_json", "comparison"]) {
      const {ok, raw, reply} = run(code);
      assert.equal(ok, false, code);
      assert.deepEqual(Object.keys(reply).sort(), ["diagnostic_code", "elapsed_ms", "error"]);
      assert.equal(reply.diagnostic_code, code);
      assert.ok(Number.isInteger(reply.elapsed_ms) && reply.elapsed_ms >= 0);
      assert.ok(!raw.includes("SECRET") && !raw.includes("INSTRUCTIONS"), code);
    }
  } finally { rmSync(scratch, {recursive:true, force:true}); }
});
