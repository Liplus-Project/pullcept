import {test} from "node:test";
import assert from "node:assert/strict";
import {spawn,execFileSync} from "node:child_process";
import {mkdtempSync,mkdirSync,writeFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {fileURLToPath} from "node:url";
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
