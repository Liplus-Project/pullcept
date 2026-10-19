import { test } from "node:test";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { existsSync, readFileSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";

const edge = process.env.PREVIEW_BROWSER ?? ["C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe","C:/Program Files/Microsoft/Edge/Application/msedge.exe","/usr/bin/chromium"].find(existsSync);
if (!edge) throw new Error("Install Chromium/Edge or set PREVIEW_BROWSER");
const record = (id,kind,extra={}) => ({id,kind,owner:"ai",added_by:"Codex Luna",text:kind==="decision"?"この方針で進めますか？":"参加者の状態表示を実装",progress:"認証境界を検証中",references:["https://github.com/Liplus-Project/pullcept/issues/493","javascript:alert(1)"],source_message_id:"source",target_account_id:kind==="decision"?"human":null,created_at:"2026-10-19T00:00:00Z",updated_at:"2026-10-19T01:00:00Z",revision:1,state:"active",answer:null,...extra});
const fixture = {topic_id:"t",retrieved_at:1,participants:[],work:{active:[record("work","work",{interrupted:true,owner_running:false}),record("q","decision")],history:[record("answered","decision",{state:"answered",answer:{content:"その方針で進めて。",name:"マスター",account:"human",answered_at:"2026-10-19T01:01:00Z"}})],history_limit:50,pending_decisions:1},plans:[{account_id:"ai",source:"codex-app-server",updated_at:1,current_launch:false,steps:[{step:"認証済みの更新経路",status:"completed"}]}],mcp_servers:[{server:"webhook",connection:"running",generation:1,pending:{state:"unsupported"}}]};

test("participant details use actual index, styles and row controller without starting terminals", {timeout:60000}, async t => {
  const main = readFileSync("src/main.ts","utf8");
  const inject = `
  window.detailsTest = {
    setup(list) { accounts = list; localAccountId = 'human'; currentTopic = {topic_id:'t',created_at:'now'}; topics = [{topic_id:'t',sessions:{ai:'session-ai',other:'session-other'},created_at:'now'}]; fillIcons(); renderRoster('t',[{id:'local',name:'マスター',account:'human',own:true,hue:null}]); },
    selectedTerminal: () => sessions.selectedAccount(),
    adopt: () => sessions.refreshSeats(),
    terminal: (id) => sessions.showView(id),
    stats: (value) => sessions.receiveStats(value),
    end: (id) => { sessions.getView(seatKey('t',id)).ended = '終了コード 0'; renderPanel(); },
    select: (id,name,server) => participantDetails.select(id,name,server),
    refresh: () => participantDetails.refresh(),
    topic: (id) => { currentTopic = {topic_id:id,created_at:'now'}; participantDetails.topicChanged(); },
  };`;
  const server = await createServer({server:{host:"127.0.0.1",port:0},logLevel:"error",plugins:[{name:"participant-test-entry",enforce:"pre",transform(code,id){if(id.endsWith("/src/main.ts")) return main.replace("void main();",inject);}}]});
  await server.listen();
  const browser = await chromium.launch({executablePath:edge,headless:true});
  t.after(async()=>{await browser.close();await server.close();});
  const page = await browser.newPage({viewport:{width:1280,height:900}});
  const errors=[];page.on("pageerror",e=>errors.push(String(e)));
  await page.addInitScript(fixture => {
    window.calls=[];window.snapshot=fixture;window.callbacks=new Map();let next=1;
    window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:"main"},currentWebview:{label:"main"}},transformCallback(cb){const id=next++;window.callbacks.set(id,cb);return id;},unregisterCallback(){},invoke:async(cmd,args)=>{
      window.calls.push({cmd,args});
      if(cmd==="seated_accounts")return window.seats ?? [];
      if(cmd==="topic_status") { if(window.delayed) return new Promise(resolve=>{window.resolveRead=resolve;});return {...window.snapshot,topic_id:args.topicId}; }
      if(cmd==="room_topic_log")return [{message_id:"source",speaker:"マスター",content:"判断に必要な元の発言",ts:"now"}];
      if(cmd==="update_topic_work")return {};
      if(cmd==="plugin:event|listen")return 1;
      if(cmd==="account_avatar")return null;
      return null;
    }};
  },fixture);
  await page.goto(server.resolvedUrls.local[0]);
  await page.waitForFunction(()=>window.detailsTest);
  await page.evaluate(()=>window.detailsTest.setup([
    {id:"human",name:"マスター",kind:"admin",args:[],cwd:""},
    {id:"ai",name:"Codex Luna",kind:"codex_cli",args:[],cwd:""},
    {id:"other",name:"Other AI",kind:"codex_cli",args:[],cwd:""},
    {id:"mcp",name:"GitHub",kind:"mcp",server:"webhook",args:[],cwd:""},
  ]));
  const terminalBefore=await page.evaluate(()=>window.detailsTest.selectedTerminal());
  assert.equal(await page.locator("#participant-details").evaluate(e => e.previousElementSibling.id), "terminal-facts");
  assert.equal(await page.locator("#participant-details").evaluate(e => e.nextElementSibling.id), "terminal-diagnostics");
  await page.locator('#roster button.pick[title="マスター の作業と判断待ち"]').focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("判断待ち 1 件"));
  assert.equal(await page.locator("#terminal-facts").isVisible(),false,"human detail does not show another AI's facts");
  await page.locator("#participant-details details").evaluate(e=>e.open=true);
  assert.match(await page.locator("#participant-details").innerText(),/その方針で進めて。/);
  await page.getByRole("button",{name:"回答",exact:true}).click();
  await page.getByRole("textbox",{name:"回答",exact:true}).fill("画面から直接回答します。");
  await page.getByRole("button",{name:"保存",exact:true}).click();
  assert.equal(await page.evaluate(()=>window.calls.find(c=>c.cmd==="update_topic_work").args.answer),"画面から直接回答します。");
  await page.getByRole("button",{name:"元の発言",exact:true}).first().click();
  await page.waitForFunction(()=>document.querySelector("dialog[open]")?.textContent.includes("判断に必要な元の発言"));
  await page.keyboard.press("Escape");
  const artifacts=process.env.STATUS_UI_ARTIFACTS ?? "artifacts/status-493";await mkdir(artifacts,{recursive:true});
  await page.screenshot({path:join(artifacts,"human-normal.png")});
  await page.locator('#roster button.pick[title="Codex Luna の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("中断中"));
  assert.match(await page.locator("#participant-details").innerText(),/計画（自己申告） · 前の起動/);
  assert.match(await page.locator("#session-state").innerText(),/未起動/);
  assert.equal(await page.locator("#terminal-facts").isVisible(),true);
  assert.equal(await page.locator('#participant-details a[href^="javascript:"]').count(),0);
  await page.evaluate(async()=>{
    window.snapshot.participants=[{account_id:"other",session:{process_running:true,model:{value:"OTHER-MODEL"}}},{account_id:"ai",session:{process_running:true,model:{value:"SELECTED-MODEL"},usage:{five_hour:{used_percentage:42}}}}];
    await window.detailsTest.refresh();
  });
  assert.equal(await page.locator("#session-model").innerText(),"SELECTED-MODEL");
  assert.equal(await page.locator("#session-five-hour .fill").evaluate(e=>e.style.width),"42%");
  assert.doesNotMatch(await page.locator("#participant-details").innerText(),/SELECTED-MODEL|OTHER-MODEL|42%/);
  assert.equal(await page.locator("#terminal-diagnostics").isVisible(),false);
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),terminalBefore);
  await page.evaluate(async()=>{
    window.seats=['ai','other'].map(account_id=>({account_id,topic_id:'t',session:{pty_id:'pty-'+account_id,started_at:'2026-10-19T01:00:00Z',command:'codex '+account_id,cwd:'D:/fixture/'+account_id,topic_id:'t',resumed_from:null}}));
    await window.detailsTest.adopt();
    const now=Math.floor(Date.now()/1000);
    window.detailsTest.stats({topic_id:'t',account_id:'ai',pty_id:'pty-ai',model:'OWN-MODEL',effort:'low',login:'selected@example.test',five_hour:42,seven_day:67,context:25,limited:false,five_hour_resets_at:now+7200,seven_day_resets_at:now+172800});
    window.detailsTest.terminal('other');
  });
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),'other');
  await page.locator('#roster button.pick[title="Codex Luna の作業と判断待ち"]').click();
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),'ai');
  assert.equal(await page.locator("#diagnostics").isVisible(),false,"row selection preserves folded terminal");
  assert.equal(await page.locator(".detail-terminal").count(),0);
  assert.equal(await page.locator("#session-model").innerText(),"OWN-MODEL");
  assert.equal(await page.locator("#session-login").innerText(),"selected@example.test");
  assert.equal(await page.locator("#session-five-hour .reset").count(),1);
  assert.equal(await page.locator("#session-seven-day .reset").count(),1);
  assert.equal(await page.locator("#terminal-diagnostics").isVisible(),true);
  await page.locator("#terminal-diagnostics").evaluate(e=>e.open=true);
  assert.equal(await page.locator("#session-command").innerText(),"codex ai");
  assert.equal(await page.locator("#session-dir").innerText(),"D:/fixture/ai");
  assert.equal(await page.locator("#session-id").innerText(),"session-ai");
  await page.locator("#terminal-diagnostics").evaluate(e=>e.open=false);
  await page.setViewportSize({width:1280,height:1500});
  await page.screenshot({path:join(artifacts,"ai-bars-todo-details.png")});
  await page.setViewportSize({width:1280,height:900});
  await page.evaluate(()=>document.querySelector('#diagnostics').hidden=false);
  await page.locator('#terminal-tabs button.name[title="Other AI の端末"]').click();
  await page.waitForFunction(()=>document.querySelector('#participant-details h3')?.textContent==='Other AI');
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),'other');
  assert.equal(await page.locator('#session-model').innerText(),'OTHER-MODEL');
  assert.equal(await page.locator('#session-command').textContent(),'codex other');
  await page.locator('#roster button.pick[title="Codex Luna の作業と判断待ち"]').click();
  assert.equal(await page.locator('#terminal .xterm textarea').evaluateAll(els=>els.some(e=>e===document.activeElement)),true);
  await page.evaluate(()=>document.querySelector('#diagnostics').hidden=true);
  await page.evaluate(()=>window.detailsTest.end('ai'));
  assert.equal(await page.locator("#session-login").innerText(),"selected@example.test");
  assert.equal(await page.locator("#session-five-hour .fill").evaluate(e=>e.style.width),"42%");
  await page.locator('#roster button.pick[title="マスター の作業と判断待ち"]').click();
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),'ai');
  assert.equal(await page.locator('#terminal-facts').isVisible(),false);
  await page.locator('#roster button.pick[title="GitHub の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("未処理: 未対応"));
  assert.equal(await page.locator("#terminal-facts").isVisible(),false,"MCP detail has no AI model/usage facts");
  assert.equal(await page.evaluate(()=>window.detailsTest.selectedTerminal()),"ai");
  assert.equal(await page.evaluate(()=>window.calls.filter(c=>["start_session","spawn_pty","write_pty"].includes(c.cmd)).length),0);
  await page.setViewportSize({width:900,height:760});await page.screenshot({path:join(artifacts,"mcp-narrow.png")});
  assert.equal(await page.locator("#participants").evaluate(e=>e.scrollWidth<=e.clientWidth+1),true);
  for (const [state,label] of [["auth_required","未認証"],["timeout","タイムアウト"],["unobserved","未取得"]]) {
    await page.evaluate(async state=>{window.snapshot.mcp_servers[0].pending={state};await window.detailsTest.refresh();},state);
    assert.match(await page.locator("#participant-details").innerText(),new RegExp(`未処理: ${label}`));
    assert.doesNotMatch(await page.locator("#participant-details").innerText(),/未処理: 0 件/);
  }
  await page.evaluate(async()=>{window.snapshot.mcp_servers[0].pending={state:"observed",pending_count:3,types:{issues:2,pull_request:1},latest_received_at:"2026-10-19T01:00:00Z",observed_at:1};await window.detailsTest.refresh();});
  assert.match(await page.locator("#participant-details").innerText(),/未処理: 3 件/);
  await page.screenshot({path:join(artifacts,"mcp-observed-narrow.png")});
  await page.evaluate(async()=>{window.snapshot.mcp_servers[0].pending={state:"unsupported"};await window.detailsTest.refresh();});
  // A slow old topic response cannot enter the next topic's details.
  await page.evaluate(()=>{window.delayed=true;window.detailsTest.topic("old");window.detailsTest.select("ai","AI");});
  await page.waitForFunction(()=>window.resolveRead);
  await page.evaluate(()=>{window.delayed=false;window.detailsTest.topic("new");window.detailsTest.select("mcp","GitHub","webhook");});
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("未処理: 未対応"));
  await page.evaluate(()=>window.resolveRead({...window.snapshot,topic_id:"old",mcp_servers:[]}));
  await page.waitForTimeout(50);
  assert.match(await page.locator("#participant-details").innerText(),/未処理: 未対応/);
  await page.evaluate(async()=>{window.snapshot.participants=[];window.detailsTest.select('ai','AI');await window.detailsTest.refresh();});
  assert.equal(await page.locator('#session-model').innerText(),'—');
  assert.equal(await page.locator('#session-login').innerText(),'—');
  assert.equal(await page.locator('#terminal-diagnostics').isVisible(),false);
  assert.deepEqual(errors,[]);
  t.diagnostic(`Synthetic Chromium ${browser.version()}; no live app or user data touched.`);
});
