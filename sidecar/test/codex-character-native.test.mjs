import {test} from "node:test";
import assert from "node:assert/strict";
import {discoverInstructions} from "../src/codex-character.mjs";
import {execFileSync} from "node:child_process";
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join, resolve} from "node:path";
const bin = process.env.PULLCEPT_TEST_CODEX_BIN;
const hazard = '日本語 & | () %PATH% "quote" \\ 😀';
const definitions = label => `COMMON ${label}\r\n# character_Codex_Lin\r\nNAME=Lin\r\n${hazard}\r\n# character_Codex_Lay\r\nNAME=Lay\r\nLay body\r\n# Shared\r\nTAIL\r\n`;
function compose(options, source, selected) {
  return JSON.parse(execFileSync("cargo", ["run", "--quiet", "--manifest-path", "crates/mcp-config/Cargo.toml", "--example", "codex_character"],
    {input:JSON.stringify({options, source, selected}), encoding:"utf8", windowsHide:true}));
}
test("native selection respects empty config, profile-v2, trusted project, cd, manual overrides and cmd shim", {skip:!bin}, async () => {
  const scratch = mkdtempSync(join(tmpdir(), "pullcept-character-native-"));
  const previous = process.env.CODEX_HOME;
  try {
    const home = join(scratch, "home"), cwd = join(scratch, "repo"), nested = join(cwd, "nested");
    mkdirSync(home); mkdirSync(join(cwd, ".codex"), {recursive:true}); mkdirSync(nested);
    process.env.CODEX_HOME = home;
    writeFileSync(join(home, "config.toml"), "[features]\nmemories=false\n");
    assert.equal(await discoverInstructions(bin, cwd, []), null);
    const global = definitions("GLOBAL"), profile = definitions("PROFILE"), project = definitions("PROJECT"), manual = definitions("MANUAL");
    const base = `developer_instructions=${JSON.stringify(global)}\n[features]\nmemories=false\n`;
    writeFileSync(join(home, "config.toml"), base);
    writeFileSync(join(home, "custom.config.toml"), `developer_instructions=${JSON.stringify(profile)}\n`);
    assert.equal(await discoverInstructions(bin, cwd, []), global);
    assert.equal(await discoverInstructions(bin, cwd, ["--profile", "custom"]), profile);
    execFileSync("git", ["init", "--quiet", cwd], {windowsHide:true});
    writeFileSync(join(cwd, ".codex/config.toml"), `developer_instructions=${JSON.stringify(project)}\n`);
    const original = readFileSync(join(cwd, ".codex/config.toml"));
    assert.equal(await discoverInstructions(bin, nested, ["--profile", "custom"]), profile, "untrusted project is excluded");
    const trust = `\n[projects.${JSON.stringify(cwd.toLowerCase())}]\ntrust_level='trusted'\n`;
    writeFileSync(join(home, "config.toml"), base + trust);
    assert.equal(await discoverInstructions(bin, nested, []), project);
    writeFileSync(join(home, "custom.config.toml"), `developer_instructions=${JSON.stringify(profile)}\n` + trust);
    assert.equal(await discoverInstructions(bin, nested, ["--profile=custom"]), project);
    assert.equal(await discoverInstructions(bin, scratch, ["-C", nested, "-p", "custom"]), project);
    const manualArgs = compose(["--cd=" + nested, "--profile", "custom", "-c", "developer_instructions=" + JSON.stringify(manual)]);
    assert.equal(await discoverInstructions(bin, scratch, manualArgs), manual);
    for (const selected of ["character_Codex_Lin", "character_Codex_Lay"]) {
      const args = compose(manualArgs, manual, selected);
      const result = await discoverInstructions(bin, scratch, args);
      assert.ok(result.includes("COMMON MANUAL\r\n")); assert.ok(result.endsWith("# Shared\r\nTAIL\r\n"));
      assert.ok(result.includes(selected.endsWith("Lin") ? hazard : "Lay body"));
      assert.ok(!result.includes(selected.endsWith("Lin") ? "NAME=Lay" : "NAME=Lin"));
    }
    if (process.platform === "win32") {
      const shim = join(scratch, "native shim.cmd"); writeFileSync(shim, `@echo off\r\n"${bin}" %*\r\n`);
      assert.equal(await discoverInstructions(shim, scratch, manualArgs), manual);
    }
    assert.deepEqual(readFileSync(join(cwd, ".codex/config.toml")), original);
    for (const flag of ["-pcustom", "-Cnested", "-cdeveloper_instructions=body", "-p=custom", "-C=nested"]) await assert.rejects(discoverInstructions(bin, cwd, [flag]));
    await assert.rejects(discoverInstructions(bin, cwd, ["--profile", "custom.v2"]));
    writeFileSync(join(home, "config.toml"), 'developer_instructions = "SECRET_FIXTURE\n');
    await assert.rejects(discoverInstructions(bin, cwd, []), error => !error.message.includes("SECRET_FIXTURE"));
  } finally {
    if (previous === undefined) delete process.env.CODEX_HOME; else process.env.CODEX_HOME = previous;
    rmSync(scratch, {recursive:true, force:true});
  }
});

test("actual patched ConPTY/cmd delivers selected Unicode and shell text to native developer instructions", {skip:!bin || process.platform !== "win32" || !process.env.PULLCEPT_TEST_CODEX_PTY}, () => {
  const scratch = mkdtempSync(join(tmpdir(), "pullcept-character-pty-"));
  try {
    const home = join(scratch, "home"); mkdirSync(home);
    writeFileSync(join(home, "config.toml"), "[features]\nmemories=false\n");
    const source = definitions("PTY");
    const selected = "character_Codex_Lin";
    const options = compose([], source, selected);
    const expected = `COMMON PTY\r\n# character_Codex_Lin\r\nNAME=Lin\r\n${hazard}\r\n# Shared\r\nTAIL\r\n`;
    execFileSync("cargo", ["build", "--quiet", "--manifest-path", "crates/codex-character-probe/Cargo.toml"], {windowsHide:true, stdio:"ignore"});
    const probe = resolve("crates/codex-character-probe/target/debug/codex-character-probe.exe");
    const shim = join(scratch, "native shim.cmd"); writeFileSync(shim, `@echo off\r\n"${bin}" %*\r\n`);
    for (const command of [bin, shim]) {
      const result = JSON.parse(execFileSync(probe, [], {input:JSON.stringify({command, options, cwd:scratch, home, expected}), encoding:"utf8", windowsHide:true}));
      assert.equal(result.equal, true); assert.equal(result.transport, "patched-ConPTY-cmd");
    }
  } finally { rmSync(scratch, {recursive:true, force:true}); }
});
