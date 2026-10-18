// Dry run against a synthetic clone only: no process is stopped and tauri dev is not started.
import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {spawnSync} from "node:child_process";
const script=fileURLToPath(new URL("../../scripts/restart-pullcept.ps1",import.meta.url));
function run(args) {
  const result=spawnSync("pwsh",["-NoProfile","-NonInteractive","-File",script,...args],{encoding:"utf8",timeout:60000});
  assert.ifError(result.error); return result;
}
test("dry run resolves the clone's target directory, logs and touches nothing",()=>{
  const dir=mkdtempSync(join(tmpdir(),"pullcept-restart-"));
  try {
    mkdirSync(join(dir,"src-tauri",".cargo"),{recursive:true});
    writeFileSync(join(dir,"src-tauri","tauri.conf.json"),"{}");
    writeFileSync(join(dir,"src-tauri",".cargo","config.toml"),"[build]\ntarget-dir = \"C:/pullcept-synthetic-target\"\n");
    const result=run(["-RepoRoot",dir,"-DryRun"]);
    assert.equal(result.status,0,result.stderr);
    const log=readFileSync(join(dir,"restart-pullcept.log"),"utf8");
    assert.ok(log.includes("target=C:\\pullcept-synthetic-target\\"),log);
    assert.match(log,/no running Pullcept from this clone/);
    assert.match(log,/dry run: nothing stopped or started/);
  } finally { rmSync(dir,{recursive:true,force:true}); }
});
test("refuses a directory that is not a Pullcept clone",()=>{
  const dir=mkdtempSync(join(tmpdir(),"pullcept-restart-"));
  try {
    const result=run(["-RepoRoot",dir,"-DryRun"]);
    assert.notEqual(result.status,0);
    assert.match(result.stderr+result.stdout,/Not a Pullcept clone/);
  } finally { rmSync(dir,{recursive:true,force:true}); }
});
