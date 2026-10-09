// Synthetic files and hook processes only; no CLI, room or scheduled task.
import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync,writeFileSync,readFileSync,rmSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";
import {fileURLToPath} from "node:url";
import {spawnSync} from "node:child_process";
const installer=fileURLToPath(new URL("../../scripts/install-notify-stop-migration.ps1",import.meta.url));
function run(script,args=[],input="",env={}) {
  const result=spawnSync("pwsh",["-NoProfile","-NonInteractive","-File",script,...args],{
    input,encoding:"utf8",env:{...process.env,PULLCEPT_LIMIT_NOTICE_OWNER:"",PULLCEPT_CLAUDE_LAUNCH:"",...env},timeout:20000});
  assert.ifError(result.error); assert.equal(result.status,0,result.stderr); return result;
}
test("migration backs up bytes, suppresses new owned limits and only exact legacy resume scopes",()=>{
  const dir=mkdtempSync(join(tmpdir(),"pullcept-notice-"));
  try {
    const hook=join(dir,"notify-stop.ps1"),receipt=join(dir,"receipt.json"),scopes=join(dir,"scopes.json");
    const legacy=`param([string]$Resume,[string]$ProjectDir,[string]$TaskName)
      function Register-ResumeTask { $server='s';$dir='p';$name='t'; return '{0}|{1}|{2}|{3}' -f $PSCommandPath, $server, $dir, $name }
      [IO.File]::WriteAllText($env:SYNTHETIC_RECEIPT, (@{scheduled=Register-ResumeTask;resume=$Resume;project=$ProjectDir;task=$TaskName;body=[Console]::In.ReadToEnd()}|ConvertTo-Json))`;
    writeFileSync(hook,legacy); writeFileSync(scopes,JSON.stringify([{task:"task-one",project:dir,server:"pullcept-room-one"}]));
    run(installer,["-HookPath",hook,"-SuppressedResumeFile",scopes]);
    const config=JSON.parse(readFileSync(hook+".pullcept.json","utf8"));
    assert.equal(readFileSync(config.backup_path,"utf8"),legacy);
    const env={SYNTHETIC_RECEIPT:receipt};
    const stop=JSON.stringify({hook_event_name:"StopFailure",error:"rate_limit"});
    run(hook,[],stop,env); assert.equal(JSON.parse(readFileSync(receipt,"utf8")).scheduled,`${hook}|s|p|t`); assert.equal(JSON.parse(JSON.parse(readFileSync(receipt,"utf8")).body).error,"rate_limit");
    rmSync(receipt);
    run(hook,[],stop,{...env,PULLCEPT_LIMIT_NOTICE_OWNER:"app-v1",PULLCEPT_CLAUDE_LAUNCH:"synthetic-launch"});
    assert.throws(()=>readFileSync(receipt));
    run(hook,[],JSON.stringify({hook_event_name:"StopFailure",error:"overloaded"}),{...env,PULLCEPT_LIMIT_NOTICE_OWNER:"app-v1",PULLCEPT_CLAUDE_LAUNCH:"synthetic-launch"});
    assert.equal(JSON.parse(JSON.parse(readFileSync(receipt,"utf8")).body).error,"overloaded"); rmSync(receipt);
    run(hook,[],JSON.stringify({hook_event_name:"Notification",notification_type:"permission_prompt"}),env); assert.throws(()=>readFileSync(receipt));
    run(hook,["-Resume","pullcept-room-one","-ProjectDir",dir,"-TaskName","task-one"],"",env); assert.throws(()=>readFileSync(receipt));
    for (const [server,project,task] of [["pullcept-room-other",dir,"task-one"],["pullcept-room-one",join(dir,"other"),"task-one"],["pullcept-room-one",dir,"other-task"]]) {
      run(hook,["-Resume",server,"-ProjectDir",project,"-TaskName",task],"",env);
      const received=JSON.parse(readFileSync(receipt,"utf8")); assert.equal(received.resume,server); assert.equal(received.task,task); rmSync(receipt);
    }
    // A second invocation refuses to overwrite the original backup.
    const again=spawnSync("pwsh",["-NoProfile","-NonInteractive","-File",installer,"-HookPath",hook],{encoding:"utf8"});
    assert.notEqual(again.status,0); assert.equal(readFileSync(config.backup_path,"utf8"),legacy);
  } finally {rmSync(dir,{recursive:true,force:true});}
});
