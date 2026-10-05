import {test} from "node:test";
import assert from "node:assert/strict";
import {spawn,execFileSync} from "node:child_process";
import {mkdtempSync,mkdirSync,writeFileSync,rmSync,copyFileSync,readFileSync,realpathSync,renameSync,symlinkSync,unlinkSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {fileURLToPath} from "node:url";
import {fileModeRoot,resolveOutputStyle} from "../src/codex-style.mjs";
import {discoverInstructions} from "../src/codex-character.mjs";
const bin=process.env.PULLCEPT_TEST_CODEX_BIN;
const helper=fileURLToPath(new URL("../src/codex-project.mjs",import.meta.url));
async function discover(home,cwd,options=[],command=bin) {
  return new Promise((resolve,reject)=>{
    const env={...process.env,CODEX_HOME:home};delete env.OPENAI_API_KEY;delete env.CODEX_API_KEY;
    const child=spawn(process.execPath,[helper,command,cwd,JSON.stringify(options)],{env,windowsHide:true});
    let out="";child.stdout.on("data",b=>out+=b);child.stderr.resume();
    child.once("error",reject);child.once("exit",()=>{try{resolve(JSON.parse(out))}catch{reject(Error("No discovery response"))}});
  });
}
test("real Codex 0.160 discovers standalone cwd, Git root, nested cwd and profile project markers without a model",{skip:!bin},async()=>{
  const scratch=mkdtempSync(join(tmpdir(),"pullcept-native-discovery-"));
  try{
    const home=join(scratch,"home"),plain=join(scratch,"standalone"),repo=join(scratch,"repo"),nested=join(repo,"nested");
    for(const dir of [home,plain,repo,nested])mkdirSync(join(dir,".codex"),{recursive:true});
    for(const dir of [plain,repo,nested])writeFileSync(join(dir,".codex/config.toml"),"# native project fixture\n");
    writeFileSync(join(home,"config.toml"),"[features]\nmemories=false\n");
    writeFileSync(join(home,"custom.v2.config.toml"),"project_root_markers=['.pullcept-test-root']\n[features]\nmemories=false\n");
    let result=await discover(home,plain);assert.ok(!result.error);assert.equal(result.projects[0].folder.toLowerCase(),join(plain,".codex").toLowerCase());
    execFileSync("git",["init","--quiet",repo],{windowsHide:true});
    result=await discover(home,repo);assert.equal(result.projects[0].folder.toLowerCase(),join(repo,".codex").toLowerCase());
    result=await discover(home,nested);assert.ok(result.projects.some(p=>p.folder.toLowerCase()===join(repo,".codex").toLowerCase()));
    assert.equal(result.projects[0].folder.toLowerCase(),join(nested,".codex").toLowerCase());
    writeFileSync(join(repo,".pullcept-test-root"),"");
    const profile=JSON.parse(execFileSync("cargo",["run","--quiet","--manifest-path","crates/mcp-config/Cargo.toml","--example","codex_discovery","--",JSON.stringify(["--profile","custom.v2"])],{env:{...process.env,CODEX_HOME:home},encoding:"utf8",windowsHide:true}));
    result=await discover(home,nested,profile);assert.ok(!result.error,JSON.stringify(result));assert.ok(result.projects.length>0);
    for(const options of [["-C",repo],["--cd",repo],[`--cd=${repo}`],[`-C=${repo}`]]) {
      result=await discover(home,plain,options);assert.ok(!result.error,JSON.stringify(result));
      assert.equal(result.projects[0].folder.toLowerCase(),join(repo,".codex").toLowerCase());
    }
    if(process.platform==="win32") {
      const shim=join(scratch,"native shim.cmd");writeFileSync(shim,`@echo off\r\n"${bin}" %*\r\n`);
      result=await discover(home,plain,["-c","features.codex_hooks=true"],shim);
      assert.ok(!result.error,JSON.stringify(result));
      assert.equal(result.projects[0].folder.toLowerCase(),join(plain,".codex").toLowerCase());
    }
  }finally{rmSync(scratch,{recursive:true,force:true});}
});

test("native layers and real installed Li+ v1 peer preserve common instructions and reject untrusted delivery", {skip:!bin || !process.env.PULLCEPT_TEST_STYLE_HELPER}, async () => {
  const scratch = realpathSync(mkdtempSync(join(tmpdir(), "pullcept actual style 日本語 ")));
  try {
    const home = join(scratch, "home"), root = join(scratch, "project"), cwd = join(root, "nested");
    mkdirSync(home); mkdirSync(cwd, {recursive:true}); mkdirSync(join(root, ".codex/hooks"), {recursive:true});
    const styles = join(root, ".codex/output-styles"); mkdirSync(styles);
    execFileSync("git", ["init", "--quiet", root], {windowsHide:true});
    const installed = join(root, ".codex/hooks/codex-output-style.py");
    copyFileSync(process.env.PULLCEPT_TEST_STYLE_HELPER, installed);
    const path = root.replaceAll("\\", "/");
    writeFileSync(join(root, ".codex/hooks.json"), JSON.stringify({hooks:{SessionStart:[{matcher:"startup|resume|clear|compact", hooks:[{
      type:"command", command:`python3 "${path}/.codex/hooks/codex-output-style.py" hook --root "${path}"`,
      commandWindows:`python "${path}/.codex/hooks/codex-output-style.py" hook --root "${path}"`,
      timeout:30, additionalContextLimit:0,
    }]}]}}));
    writeFileSync(join(root, ".codex/config.toml"), 'developer_instructions="COMMON_FIXTURE"\n');
    writeFileSync(join(styles, "character_instance.md"), "DEFAULT_ONLY\n");
    const body = "選択本文\r\n".repeat(4000);
    writeFileSync(join(styles, "character_codex_luna.md"), `---\nname: character_codex_luna\nkeep-coding-instructions: true\n---\n${body}`);
    writeFileSync(join(home, "config.toml"), "[features]\nmemories=false\n");
    let data = await discover(home, cwd, ["-c", "features.codex_hooks=true"]);
    assert.equal(await fileModeRoot(data.projects, cwd), null, "untrusted project cannot supply a style root");
    writeFileSync(join(home, "config.toml"), `[features]\nmemories=false\n[projects.${JSON.stringify(root.toLowerCase())}]\ntrust_level='trusted'\n`);
    data = await discover(home, cwd, ["-c", "features.codex_hooks=true"]);
    assert.equal(await fileModeRoot(data.projects, cwd), root);
    assert.equal(data.styleHooks.length, 1); assert.equal(data.styleHooks[0].trust, "untrusted");
    const env = {...process.env, CODEX_HOME:home, LI_PLUS_OUTPUT_STYLE:"SHOULD_NOT_WIN"};
    await assert.rejects(resolveOutputStyle(data, cwd, "character_codex_luna", env));
    // Protocol interoperability fixture only: do not grant native trust on disk.
    data.styleHooks[0].trust = "trusted";
    const account = await resolveOutputStyle(data, cwd, "character_codex_luna", env);
    assert.equal(account.name, "character_codex_luna"); assert.equal(account.byte_count, Buffer.byteLength(body));
    assert.equal((await resolveOutputStyle(data, cwd, "", env)).name, "character_instance");
    rmSync(join(styles, "character_instance.md"));
    await assert.rejects(resolveOutputStyle(data, cwd, "", env));
    assert.equal((await resolveOutputStyle(data, cwd, "character_codex_luna", env)).name, "character_codex_luna");
    writeFileSync(join(styles, "character_instance.md"), "DEFAULT_ONLY\n");
    const outside = join(scratch, "outside-styles"); mkdirSync(outside);
    writeFileSync(join(outside, "character_codex_luna.md"), "OUTSIDE\n");
    const savedStyles = join(root, ".codex/saved-styles"); renameSync(styles, savedStyles);
    try {
      symlinkSync(outside, styles, process.platform === "win32" ? "junction" : "dir");
      await assert.rejects(resolveOutputStyle(data, cwd, "character_codex_luna", env));
    } finally { unlinkSync(styles); renameSync(savedStyles, styles); }
    writeFileSync(join(root, ".codex/config.toml"), 'developer_instructions="COMMON_FIXTURE"\n[liplus]\noutput_style=false\n');
    data = await discover(home, scratch, ["--cd", cwd, "-c", "features.codex_hooks=true"]);
    assert.equal(data.projects.find(p => p.outputStyle === false)?.folder.toLowerCase(), join(root, ".codex").toLowerCase());
    data.styleHooks[0].trust = "trusted";
    assert.equal((await resolveOutputStyle(data, cwd, "", env)).mode, "disabled");
    assert.equal((await resolveOutputStyle(data, cwd, "character_codex_luna", env)).mode, "file");
    const saved = readFileSync(join(root, ".codex/config.toml"));
    for (const bytes of [Buffer.from(""), Buffer.from([255]), Buffer.from("---\nname: wrong\n---\nBODY\n"), Buffer.from("---\nname: character_codex_luna\nBODY\n"), Buffer.alloc(140000, 65)]) {
      writeFileSync(join(styles, "character_codex_luna.md"), bytes);
      await assert.rejects(resolveOutputStyle(data, cwd, "character_codex_luna", env));
    }
    writeFileSync(join(styles, "character_codex_luna.md"), body);
    await assert.rejects(resolveOutputStyle(data, cwd, "Character_codex_luna", env));
    await assert.rejects(resolveOutputStyle(data, cwd, "../character_codex_luna", env));
    rmSync(join(styles, "character_codex_luna.md"));
    await assert.rejects(resolveOutputStyle(data, cwd, "character_codex_luna", env));
    const previous = process.env.CODEX_HOME;
    try {
      process.env.CODEX_HOME = home;
      assert.equal(await discoverInstructions(bin, scratch, ["--cd", cwd]), "COMMON_FIXTURE");
      assert.equal(await discoverInstructions(bin, scratch, ["--cd", cwd, "-c", 'developer_instructions="MANUAL_COMMON"']), "MANUAL_COMMON");
    } finally { if (previous === undefined) delete process.env.CODEX_HOME; else process.env.CODEX_HOME = previous; }
    assert.deepEqual(readFileSync(join(root, ".codex/config.toml")), saved);
  } finally { rmSync(scratch, {recursive:true, force:true}); }
});
