import {test} from "node:test";
import assert from "node:assert/strict";
import {discoverInstructions} from "../src/codex-character.mjs";
import {execFileSync} from "node:child_process";
import {mkdtempSync, mkdirSync, writeFileSync, rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
const bin = process.env.PULLCEPT_TEST_CODEX_BIN;
function delivery(common, character) {
  return JSON.parse(execFileSync("cargo", ["run", "--quiet", "--manifest-path", "crates/mcp-config/Cargo.toml", "--example", "codex_delivery"],
    {input:JSON.stringify({mode:"file", common, character, room:"ROOM"}), encoding:"utf8", windowsHide:true}));
}
test("native common instructions reach file-mode start and resume without a model (#303)", {skip:!bin}, async () => {
  const scratch = mkdtempSync(join(tmpdir(), "pullcept-delivery-native-"));
  const previous = process.env.CODEX_HOME;
  const roomEnv = Object.entries(process.env).filter(([key]) => /^PULLCEPT_/i.test(key));
  try {
    for (const [key] of roomEnv) delete process.env[key];
    const home = join(scratch, "home"), cwd = join(scratch, "repo");
    mkdirSync(home); mkdirSync(cwd);
    process.env.CODEX_HOME = home;
    const common = '共通指示\r\n引用 "quote" & %PATH% 😀';
    const character = "# Luna\r\n選択本文\r\n";
    writeFileSync(join(home, "config.toml"), `developer_instructions=${JSON.stringify(common)}\n[features]\nmemories=false\n`);
    const effective = await discoverInstructions(bin, cwd, []);
    assert.equal(effective, common);
    for (const text of [effective, await discoverInstructions(bin, cwd, ["-c", 'developer_instructions="MANUAL_COMMON"'])]) {
      const params = delivery(text, character);
      assert.equal(params.start.developerInstructions, `${text}\n\n${character}\n\nROOM`);
      assert.equal(params.resume.developerInstructions, params.start.developerInstructions);
    }
    writeFileSync(join(home, "config.toml"), "[features]\nmemories=false\n");
    assert.equal(await discoverInstructions(bin, cwd, []), null);
    assert.equal(delivery(null, character).start.developerInstructions, `${character}\n\nROOM`);
    writeFileSync(join(home, "config.toml"), "invalid toml [");
    await assert.rejects(discoverInstructions(bin, cwd, []));
  } finally {
    if (previous === undefined) delete process.env.CODEX_HOME; else process.env.CODEX_HOME = previous;
    for (const [key, value] of roomEnv) process.env[key] = value;
    rmSync(scratch, {recursive:true, force:true});
  }
});
