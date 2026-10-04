// Query only native configuration layers. No thread or model request is made.
import {spawn,execFileSync} from "node:child_process";
import {createInterface} from "node:readline";
import {resolve,isAbsolute} from "node:path";
const [rawBin,initialCwd,rawOptions] = process.argv.slice(2);
const bin=/[\\/]/.test(rawBin) && !isAbsolute(rawBin) ? resolve(initialCwd,rawBin) : rawBin;
let cwd=initialCwd;
const options=JSON.parse(rawOptions ?? "[]");
const config=[];
for(let i=0;i<options.length;i++) {
  const arg=options[i];
  if(["-c","--config","--enable","--disable"].includes(arg)) {config.push(arg,options[++i]);}
  else if(arg.startsWith("--config=") || arg.startsWith("-c=") || arg.startsWith("--enable=") || arg.startsWith("--disable="))config.push(arg);
  else if(["-p","--profile"].includes(arg) || arg.startsWith("--profile="))throw Error("Profile must be resolved by the product discovery adapter");
  else if(["-C","--cd"].includes(arg))cwd=resolve(initialCwd,options[++i]);
  else if(arg.startsWith("--cd="))cwd=resolve(initialCwd,arg.slice(5));
  else if(arg.startsWith("-C="))cwd=resolve(initialCwd,arg.slice(3));
}
const shell=process.platform==="win32" && !bin.toLowerCase().endsWith(".exe");
const words=[...config,"app-server","--listen","stdio://"];
if(shell && [bin,...words].some(word=>/[&|<>^()%\r\n]/.test(word))) throw Error("Unsupported Windows command characters");
const child=spawn(shell ? `"${bin}"` : bin,shell ? words.map(word=>`"${word.replaceAll('"','\\"')}"`) : words,{
  cwd,env:process.env,stdio:["pipe","pipe","ignore"],windowsHide:true,
  shell,
});
const lines=createInterface({input:child.stdout});
let finished=false;
let projectLayers;
function finish(value){if(finished)return;finished=true;process.stdout.write(JSON.stringify(value));child.stdin.end();}
const deadline=setTimeout(()=>{
  if(process.platform==="win32" && child.pid) {try{execFileSync("taskkill",["/PID",String(child.pid),"/T","/F"],{stdio:"ignore",windowsHide:true});}catch{child.kill();}}
  else child.kill();
  finish({error:"Codex project discovery timed out"});
},5000);
child.once("error",()=>finish({error:"Could not start Codex project discovery"}));
child.once("exit",()=>{clearTimeout(deadline);lines.close();if(!finished)finish({error:"Codex project discovery exited before config/read"});});
lines.on("line",line=>{let response;try{response=JSON.parse(line)}catch{return;}
  if(response.id===1){
    if(response.error)return finish({error:"Codex initialize failed"});
    child.stdin.write(JSON.stringify({method:"initialized"})+"\n");
    child.stdin.write(JSON.stringify({id:2,method:"config/read",params:{cwd,includeLayers:true}})+"\n");
  }
  if(response.id===2){
    if(response.error)return finish({error:"Codex config/read failed"});
    const layers=response.result?.layers ?? [];
    const projects=layers.filter(layer=>layer.name?.type==="project" || layer.source?.type==="project");
    projectLayers=projects.map(layer=>({folder:layer.name?.dotCodexFolder ?? layer.source?.dotCodexFolder,disabled:layer.disabledReason ?? null}));
    child.stdin.write(JSON.stringify({id:3,method:"hooks/list",params:{cwds:[cwd]}})+"\n");
  }
  if(response.id===3) {
    if(response.error)return finish({error:"Codex hooks/list failed"});
    const hooks=(response.result?.data ?? []).flatMap(v=>v.hooks ?? []).filter(v=>v.eventName==="sessionStart" && v.command?.includes("codex-session.mjs"));
    finish({projects:projectLayers,hooks:hooks.map(v=>({source:v.source,path:v.sourcePath,trust:v.trustStatus}))});
  }
});
child.stdin.write(JSON.stringify({id:1,method:"initialize",params:{clientInfo:{name:"pullcept_project_discovery",version:"0.1.0"},capabilities:{experimentalApi:true}}})+"\n");
