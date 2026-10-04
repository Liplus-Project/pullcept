import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { fileURLToPath } from "node:url";
const helper = fileURLToPath(new URL("../src/codex-session.mjs", import.meta.url));
const id = "12345678-1234-1234-1234-123456789abc";
function run(env, input) {
  return new Promise((resolve, reject) => {
    const child=spawn(process.execPath,[helper],{env:{PATH:process.env.PATH,...env},windowsHide:true});
    let out="",err="";child.stdout.on("data",b=>out+=b);child.stderr.on("data",b=>err+=b);
    child.once("error",reject);child.once("exit",code=>resolve({code,out,err}));child.stdin.end(JSON.stringify(input));
  });
}
test("root hook carries correlated native ID, retries startup race, and ignores parent thread env",async()=>{
  const received=[];let attempts=0;
  const server=createServer((req,res)=>{let raw="";req.on("data",b=>raw+=b);req.on("end",()=>{
    received.push({auth:req.headers.authorization,body:JSON.parse(raw)});res.writeHead(++attempts===1?409:200);res.end("{}");
  });});await new Promise(r=>server.listen(0,"127.0.0.1",r));
  try {
    const env={PULLCEPT_NATIVE_URL:`http://127.0.0.1:${server.address().port}/hooks/codex-session`,PULLCEPT_ROOM_TOKEN:"secret",PULLCEPT_LAUNCH_ID:"nonce",PULLCEPT_LAUNCHED_AS:"a",PULLCEPT_LAUNCHED_ROOM:"topic",CODEX_THREAD_ID:"wrong-parent"};
    assert.deepEqual(await run(env,{hook_event_name:"SessionStart",session_id:id}),{code:0,out:"",err:""});
    assert.equal(received.length,2);assert.equal(received[1].auth,"Bearer secret");
    assert.deepEqual(received[1].body,{session_id:id,hook_event_name:"SessionStart",launch_id:"nonce",account_id:"a",room_id:"topic"});
    for(const event of [{hook_event_name:"SubagentStart",session_id:id},{hook_event_name:"SessionStart",session_id:"not-id"}]) await run(env,event);
    await run({...env,PULLCEPT_LAUNCH_ID:""},{hook_event_name:"SessionStart",session_id:id});
    assert.equal(received.length,2);
  } finally { await new Promise(r=>server.close(r)); }
});
