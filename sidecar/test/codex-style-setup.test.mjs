import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, realpathSync, existsSync} from "node:fs";
import {join, resolve} from "node:path";
import {tmpdir} from "node:os";
import {execFileSync} from "node:child_process";
import {createHash} from "node:crypto";
import {projectTrustEdits, verifyOwnHook, approveStyle} from "../src/codex-style-setup.mjs";
import {resolveOutputStyle} from "../src/codex-style.mjs";

test("trust edits preserve existing project spelling and target only trust_level leaves", () => {
  const root = process.platform === "win32" ? "C:\\Work\\Folder" : "/work/folder";
  const equivalent = process.platform === "win32" ? "c:\\work\\folder" : root;
  const config = {projects:{[equivalent]:{trust_level:"untrusted", extra:"keep"}, elsewhere:{trust_level:"untrusted"}}};
  const edits = projectTrustEdits(config, root);
  assert.deepEqual(edits, [{keyPath:`projects.${JSON.stringify(equivalent)}.trust_level`, value:"trusted", mergeStrategy:"upsert"}]);
  assert.equal(config.projects[equivalent].extra, "keep");
  if (process.platform === "win32") {
    assert.deepEqual(projectTrustEdits({projects:{"c:/work/folder":{trust_level:"untrusted", extra:"keep"}}}, root), [
      {keyPath:'projects."c:/work/folder"', value:null, mergeStrategy:"replace"},
      {keyPath:`projects.${JSON.stringify(root)}`, value:{trust_level:"trusted", extra:"keep"}, mergeStrategy:"upsert"},
    ]);
  }
});

test("approval rejects modified bundled bytes, foreign commands/sources and session flag index collisions", async () => {
  const root = realpathSync.native(mkdtempSync(join(tmpdir(), "style559-own-")));
  try {
    mkdirSync(join(root, ".codex/hooks"), {recursive:true});
    const helper = join(root, ".codex/hooks/codex-output-style.py"), file = join(root, ".codex/hooks.json");
    writeFileSync(helper, "BUNDLED_FIXTURE"); writeFileSync(file, "{}");
    const hash = createHash("sha256").update("BUNDLED_FIXTURE").digest("hex");
    const command = `${process.platform === "win32" ? "python" : "python3"} "${root.replaceAll("\\", "/")}/.codex/hooks/codex-output-style.py" hook --root "${root.replaceAll("\\", "/")}"`;
    const hook = {source:"project", sourcePath:file, key:`${file}:session_start:0:0`, command, matcher:"startup|resume|clear|compact", async:false, additionalContextLimit:0, enabled:true, currentHash:`sha256:${"a".repeat(64)}`};
    assert.equal((await verifyOwnHook([hook], root, false, hash)).key, hook.key);
    for (const change of [{source:"user"}, {command:command + " --unknown"}, {matcher:"startup"}, {async:true}, {currentHash:""}, {enabled:false}])
      await assert.rejects(verifyOwnHook([{...hook, ...change}], root, false, hash));
    const flags = process.platform === "win32" ? "C:\\<session-flags>\\config.toml" : "/<session-flags>/config.toml";
    await assert.rejects(verifyOwnHook([{...hook, source:"sessionFlags", sourcePath:flags, key:`${flags}:session_start:1:0`}], root, true, hash));
    writeFileSync(helper, "MODIFIED");
    await assert.rejects(verifyOwnHook([hook], root, false, hash), /同梱版/);
  } finally { rmSync(root, {recursive:true, force:true}); }
});

const bin = process.env.PULLCEPT_TEST_CODEX_BIN;
const installer = process.env.PULLCEPT_TEST_STYLE_INSTALLER;
test("native isolated on-save preserves config and hooks, approves exact project/home loader and delivers selected body", {skip:!bin || !installer, timeout:60000}, async () => {
  const scratch = realpathSync.native(mkdtempSync(join(tmpdir(), "style559-native-")));
  const previous = process.env.CODEX_HOME;
  const reports = [];
  try {
    for (const scope of ["project", "home", "home-shim"]) {
      const cwd = join(scratch, scope), home = scope === "project" ? join(scratch, "isolated-home") : join(cwd, ".codex");
      mkdirSync(cwd); mkdirSync(home, {recursive:true}); process.env.CODEX_HOME = home;
      const variant = process.platform === "win32" ? cwd.toLowerCase().replaceAll("\\", "/") : cwd;
      const config = `# retain user comment\nmodel_verbosity='low'\n[projects.${JSON.stringify(variant)}]\ntrust_level='untrusted'\nextra='KEEP_PROJECT_FIELD'\n[projects.other]\ntrust_level='untrusted'\n[hooks.state.unrelated]\nenabled=false\ntrusted_hash='KEEP_OTHER_TRUST'\n`;
      writeFileSync(join(home, "config.toml"), config);
      const profile = "# keep named profile\nproject_root_markers=['.git']\n[features]\napps=false\n";
      if (scope === "home-shim") writeFileSync(join(home, "fixture.config.toml"), profile);
      const userHook = {hooks:{SessionStart:[{hooks:[{type:"command",command:"echo unrelated",async:false}]}]}, extra:"KEEP_HOOK_FILE"};
      writeFileSync(join(home, "hooks.json"), JSON.stringify(userHook));
      const options = ["--model", "model-fixture", "-c", "features.apps=false", ...(scope === "home-shim" ? ["--profile", "fixture"] : [])];
      const setup = JSON.parse(execFileSync(resolve(installer), [], {input:JSON.stringify({cwd, home, options}), encoding:"utf8", windowsHide:true}));
      const actualBin = scope === "home-shim" && process.platform === "win32" ? (process.env.PULLCEPT_TEST_CODEX_SHIM ?? "codex") : bin;
      const approved = await approveStyle(actualBin, cwd, setup.options, cwd, setup.hash, setup.scope);
      assert.equal(approved.approved, true);
      assert.equal(approved.source, scope === "project" ? "project" : "sessionFlags");
      const after = readFileSync(join(home, "config.toml"), "utf8");
      for (const retained of ["# retain user comment", "KEEP_PROJECT_FIELD", "KEEP_OTHER_TRUST", "[projects.other]", "enabled=false"])
        assert.ok(after.includes(retained), retained + " remains");
      assert.equal((after.match(/trust_level = "trusted"/g) ?? []).length, 1);
      assert.deepEqual(JSON.parse(readFileSync(join(home, "hooks.json"), "utf8")), userHook);
      if (scope === "home-shim") assert.equal(readFileSync(join(home, "fixture.config.toml"), "utf8"), profile);
      const discover = () => JSON.parse(execFileSync(process.execPath, ["sidecar/src/codex-project.mjs", actualBin, cwd, JSON.stringify(setup.options)], {encoding:"utf8", windowsHide:true}));
      const discovery = discover(); assert.ok(discovery.styleHooks.every(h => h.trust === "trusted"));
      mkdirSync(join(cwd, ".codex/output-styles"), {recursive:true});
      writeFileSync(join(cwd, ".codex/output-styles/Luna.md"), "---\nname: Luna\n---\nNATIVE_BODY & | %PATH%\n");
      const body = await resolveOutputStyle(discovery, cwd, "Luna", process.env, true, true);
      assert.match(body.body, /NATIVE_BODY & \| %PATH%/); assert.deepEqual(body.hook_keys, [approved.key]);
      await assert.rejects(resolveOutputStyle(discovery, cwd, "Missing", process.env, true, true), /本文ファイル/);
      const beforeRetry = readFileSync(join(home, "config.toml"));
      await approveStyle(actualBin, cwd, setup.options, cwd, setup.hash, setup.scope);
      assert.deepEqual(readFileSync(join(home, "config.toml")), beforeRetry);
      const helper = join(cwd, ".codex/hooks/codex-output-style.py");
      writeFileSync(helper, readFileSync(helper, "utf8") + "\n# changed\n");
      await assert.rejects(approveStyle(actualBin, cwd, setup.options, cwd, setup.hash, setup.scope), /同梱版/);
      assert.deepEqual(readFileSync(join(home, "config.toml")), beforeRetry);
      reports.push({scope, source:approved.source, key:approved.key, preserved:true, bodyVerified:true, repeatedSaveStable:true, modifiedRejected:true, modelCalls:0});
    }
    if (process.env.PULLCEPT_STYLE_EVIDENCE) {
      mkdirSync(process.env.PULLCEPT_STYLE_EVIDENCE, {recursive:true});
      writeFileSync(join(process.env.PULLCEPT_STYLE_EVIDENCE, "native-setup.json"), JSON.stringify({codex:bin, reports, modelCalls:0}, null, 2));
    }
  } finally {
    if (previous === undefined) delete process.env.CODEX_HOME; else process.env.CODEX_HOME = previous;
    rmSync(scratch, {recursive:true, force:true, maxRetries:10, retryDelay:100});
  }
});
