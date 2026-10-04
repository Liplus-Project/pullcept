// Opt-in real CLI / real-model evidence; never part of the network-free CI suite.
import {createServer} from "node:http";
import {spawn,execFileSync} from "node:child_process";
import {mkdirSync,copyFileSync,writeFileSync,readFileSync,existsSync} from "node:fs";
import {join,resolve} from "node:path";
import {randomUUID} from "node:crypto";
import {WebSocketServer} from "ws";
const repo=resolve("."), root=resolve(process.env.PULLCEPT_PROBE_ROOT ?? ".codex-live-evidence");
const home=join(root,"home"),workspace=join(root,"workspace");mkdirSync(workspace,{recursive:true});mkdirSync(home,{recursive:true});
if (!existsSync(join(home,"auth.json")) && process.env.PULLCEPT_PROBE_AUTH) copyFileSync(process.env.PULLCEPT_PROBE_AUTH,join(home,"auth.json"));
if (!existsSync(join(home,"config.toml"))) writeFileSync(join(home,"config.toml"),'model="gpt-6-luna"\nmodel_reasoning_effort="low"\n[features]\nmemories=false\n');
const evidence={version:null,callbacks:[],hello:[],posts:[],history:[],events:[]};
const flush=()=>writeFileSync(join(root,"evidence.json"),JSON.stringify(evidence,null,2));
const native=createServer((req,res)=>{let body="";req.on("data",b=>body+=b);req.on("end",()=>{
  evidence.callbacks.push(JSON.parse(body));flush();res.writeHead(200);res.end("{}");
});});await new Promise(r=>native.listen(0,"127.0.0.1",r));
const room=new WebSocketServer({host:"127.0.0.1",port:0});await new Promise(r=>room.once("listening",r));
room.on("connection",ws=>ws.on("message",data=>{const v=JSON.parse(data);evidence.events.push(v.type);
  if(v.type==="hello") {evidence.hello.push(v);}
  if(v.type==="post") {evidence.posts.push(v);ws.send(JSON.stringify({type:"post_result",message_id:v.message_id,delivered:true,missed:[]}));}
  if(v.type==="history") {evidence.history.push(v);ws.send(JSON.stringify({type:"history_result",request_id:v.request_id,posts:[{message_id:"history-id",speaker:"Master",content:"history-secret-42",ts:new Date().toISOString()}],has_more:false}));}
  flush();
}));
const env={...process.env,TERM:"xterm-256color",CODEX_HOME:home,PULLCEPT_PROBE_WORKSPACE:workspace,PULLCEPT_ROOM_URL:`ws://127.0.0.1:${room.address().port}`,PULLCEPT_ROOM_TOKEN:randomUUID(),PULLCEPT_LAUNCHED_AS:"codex-test",PULLCEPT_LAUNCHED_ROOM:"codex-topic",PULLCEPT_ROOM_ID:"codex-topic",PULLCEPT_LAUNCH_ID:randomUUID(),PULLCEPT_NATIVE_URL:`http://127.0.0.1:${native.address().port}/hooks/codex-session`};
delete env.OPENAI_API_KEY;delete env.CODEX_API_KEY;
Object.assign(env,{PULLCEPT_AGENT_NAME:"Codex Test",PULLCEPT_ACCOUNT_ID:"codex-test",PULLCEPT_UNSEEN_HISTORY:"1"});
if(process.env.PULLCEPT_PROBE_RESUME)env.PULLCEPT_PROBE_RESUME=process.env.PULLCEPT_PROBE_RESUME;
const prepared=execFileSync('cargo',['run','--quiet','--manifest-path','crates/mcp-config/Cargo.toml','--example','codex_probe'],{env,cwd:repo,encoding:"utf8",windowsHide:true});
const bin=process.env.PULLCEPT_PROBE_BIN;if(!bin)throw Error("PULLCEPT_PROBE_BIN is required");
evidence.version=execFileSync(bin,["--version"],{encoding:"utf8",windowsHide:true}).trim();flush();
console.log("Luna: isolated live CLI starts; evidence contains only room traffic/native ID, never auth or terminal output.");
const child=spawn(bin,JSON.parse(prepared),{cwd:workspace,env,stdio:"inherit",windowsHide:true});
child.once("exit",code=>{evidence.exit=code;flush();for(const client of room.clients)client.close();room.close();native.close();});
