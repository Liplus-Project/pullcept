import { test } from "node:test";
import { sidebarComparisons } from "./sidebar-comparisons.mjs";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { existsSync, readFileSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";

const edge = process.env.PREVIEW_BROWSER ?? ["C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe","C:/Program Files/Microsoft/Edge/Application/msedge.exe","/usr/bin/chromium"].find(existsSync);
if (!edge) throw new Error("Install Chromium/Edge or set PREVIEW_BROWSER");
const record = (id,kind,extra={}) => ({id,kind,owner:"ai",added_by:"Codex Luna",text:kind==="decision"?"この方針で進めますか？":"参加者の状態表示を実装し、使用率と認証境界を確認する長い作業内容。".repeat(8),progress:"認証境界を検証中",references:["https://github.com/Liplus-Project/pullcept/issues/493","javascript:alert(1)"],source_message_id:"source",target_account_id:kind==="decision"?"human":null,created_at:"2026-10-19T00:00:00Z",updated_at:"2026-10-19T01:00:00Z",revision:1,state:"active",answer:null,...extra});
const fixture = {topic_id:"t",retrieved_at:1,participants:[],work:{active:[record("work","work",{interrupted:true,owner_running:false}),record("q","decision")],history:[record("answered","decision",{state:"answered",owner_limited:true,interrupted:true,answer:{content:"その方針で進めて。",name:"マスター",account:"human",answered_at:"2026-10-19T01:01:00Z"}})],history_limit:50,pending_decisions:1},plans:[{account_id:"ai",source:"codex-app-server",updated_at:1,current_launch:false,steps:[{step:"認証済みの更新経路と別アカウントの混線防止を確認する長い計画項目".repeat(5),status:"completed"}]}],mcp_servers:[{server:"webhook",connection:"running",generation:1,pending:{state:"unsupported"}}]};

// The fold marker is a thin chevron drawn by ::before (#523): right when folded, down when open.
const chevron = (page,selector) => page.locator(selector).evaluate(e=>{
  const style=getComputedStyle(e.querySelector(':scope > summary'),'::before');
  const m=/matrix\(([^)]+)\)/.exec(style.transform); const b=m?Number(m[1].split(',')[1]):NaN;
  return {content:style.content,listStyle:getComputedStyle(e.querySelector(':scope > summary')).listStyleType,border:style.borderRightStyle,dir:b<0?'right':b>0?'down':'none'};
});

// #525: the label starts one half-width space past the chevron's own clearance (0.1em + box + 0.5em),
// the space measured in the summary's own font; the same in TODO, 完了履歴 and 詳細.
const chevronGap = (page,selector) => page.locator(selector).evaluate(e=>{
  const summary=e.querySelector(':scope > summary'),s=getComputedStyle(summary),em=parseFloat(s.fontSize);
  const label=summary.querySelector('.todo-title,.todo-history-title');
  let left; if (label) left=label.getBoundingClientRect().left; else { const r=document.createRange(); r.selectNodeContents(summary); left=[...r.getClientRects()].at(-1).left; }
  const probe=document.createElement('span'); probe.style.whiteSpace='pre'; probe.textContent=' '; summary.append(probe);
  const space=probe.getBoundingClientRect().width; probe.remove();
  const before=getComputedStyle(summary,'::before');
  const clearance=em*(0.1+0.38+0.5)+parseFloat(before.borderRightWidth);
  return {extra:left-summary.getBoundingClientRect().left-parseFloat(s.paddingLeft)-clearance, space};
});
// One line of the element's own text, for the blank-line gaps (#525).
const lineHeight = (page,selector) => page.locator(selector).evaluate(e=>{
  const probe=document.createElement('div'); probe.textContent='あ'; probe.style.margin='0'; probe.style.padding='0'; e.append(probe);
  const h=probe.getBoundingClientRect().height; probe.remove(); return {line:h, margin:parseFloat(getComputedStyle(e).marginTop)};
});

test("participant details use actual index, styles and row controller without starting terminals", {timeout:60000}, async t => {
  const main = readFileSync("src/main.ts","utf8");
  const inject = `
  window.detailsTest = {
    setup(list) { accounts = list; localAccountId = 'human'; currentTopic = {topic_id:'t',created_at:'now'}; topics = [{topic_id:'t',sessions:{ai:'session-ai',other:'session-other'},created_at:'now'}]; fillIcons(); renderRoster('t',[{id:'local',name:'マスター',account:'human',own:true,hue:null}]); },
    joinLimited: () => renderRoster('t',[{id:'local',name:'マスター',account:'human',own:true,hue:null},{id:'other-session',name:'Other AI',account:'other',own:false,hue:null}]),
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
    {id:"mcp-empty",name:"Unconfigured MCP",kind:"mcp",args:[],cwd:""},
    {id:"mcp",name:"GitHub",kind:"mcp",server:"webhook",args:[],cwd:""},
  ]));
  const terminalBefore=await page.evaluate(()=>window.detailsTest.selectedTerminal());
  assert.equal(await page.locator("#participant-details").evaluate(e => e.previousElementSibling.id), "terminal-facts");
  assert.equal(await page.locator("#participant-details").evaluate(e => e.nextElementSibling.id), "terminal-diagnostics");
  await page.locator('#roster button.pick[title="マスター の作業と判断待ち"]').focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("判断待ち1"));
  assert.equal(await page.locator("#terminal-facts").isVisible(),false,"human detail does not show another AI's facts");
  assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),false);
  await page.locator('.todo-fold .todo-refresh').click();
  assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),false,'更新 in the heading does not toggle the fold');
  await page.locator('.todo-fold > summary').click();
  // #521: TODO heading with 更新, then 判断待ち cards, the plan, the folded 完了履歴.
  assert.deepEqual(await page.locator('.todo-fold > summary').evaluate(e=>[...e.children].map(c=>c.textContent)),['TODO','更新']);
  // #523: 更新 then the refresh icon, at the right of the heading row; the chevron is at the left.
  assert.equal(await page.locator('.todo-fold .todo-refresh > svg.todo-icon-refresh:last-child').count(),1);
  assert.ok(await page.locator('.todo-fold > summary').evaluate(e=>{const s=e.getBoundingClientRect(),b=e.querySelector('.todo-refresh').getBoundingClientRect(),t=e.querySelector('.todo-title').getBoundingClientRect();return s.right-b.right<16&&b.left>t.left;}));
  // The 判断待ち count is a small rounded badge next to the heading.
  assert.deepEqual(await page.locator('.todo-decisions .todo-head .todo-count').evaluate(e=>{const s=getComputedStyle(e);return [s.borderTopLeftRadius,s.fontSize,s.paddingTop,s.paddingLeft];}),['10px','11px','2px','6px']);
  // A task item: 1px border, radius 8px, padding 9px.
  assert.deepEqual(await page.locator('.todo-decisions .todo-card').evaluate(e=>{const s=getComputedStyle(e);return [s.borderTopWidth,s.borderTopLeftRadius,s.paddingTop,s.columnGap];}),['1px','8px','9px','6px']);
  // 参照：発言を開く ↗ sits at the right of the meta row, without underline until hovered.
  assert.ok(await page.locator('.todo-decisions .todo-meta').evaluate(e=>{const m=e.getBoundingClientRect(),r=e.querySelector('.todo-ref').getBoundingClientRect();return m.right-r.right<2;}));
  assert.equal(await page.locator('.todo-decisions .todo-source').evaluate(e=>getComputedStyle(e).textDecorationLine),'none');
  assert.deepEqual(await chevron(page,'.todo-fold'),{content:'""',listStyle:'none',border:'solid',dir:'down'});
  assert.deepEqual(await chevron(page,'.todo-history'),{content:'""',listStyle:'none',border:'solid',dir:'right'});
  // #523 icons from the mockup: open circle on a decision card, ↗ after 発言を開く, bundled inline SVG only.
  assert.equal(await page.locator('.todo-decisions .todo-card .todo-mark svg.todo-icon-circle').count(),1);
  assert.equal(await page.locator('.todo-decisions .todo-source svg.todo-icon-external').count(),1);
  assert.equal(await page.locator('#participant-details svg.todo-icon').evaluateAll(els=>els.every(e=>e.getAttribute('aria-hidden')==='true' && !e.querySelector('image,use,foreignObject'))),true);
  assert.deepEqual(await page.locator('.todo-fold > :not(summary)').evaluateAll(els=>els.map(e=>e.className)),['todo-section todo-decisions','todo-history']);
  assert.match(await page.locator('.todo-decisions .todo-head').innerText(),/判断待ち\s*1/);
  assert.match(await page.locator('.todo-decisions .todo-card').innerText(),/この方針で進めますか？[\s\S]*判断待ち[\s\S]*Codex Luna[\s\S]*参照：[\s\S]*発言を開く/);
  assert.equal(await page.locator('.todo-history').evaluate(e=>e.open),false);
  assert.match(await page.locator('.todo-history > summary').innerText(),/完了履歴\s*1件\s*最大50件/);
  await page.locator('.todo-history > summary').click();
  assert.equal(await page.locator('.todo-history .todo-mark').first().getAttribute('aria-label'),'回答済み');
  assert.equal(await page.locator('.todo-history .todo-mark svg.todo-icon-check').count(),1);
  assert.equal(await page.locator('.todo-history .todo-source svg.todo-icon-external').count(),1);
  assert.deepEqual(await chevron(page,'.todo-history'),{content:'""',listStyle:'none',border:'solid',dir:'down'});
  {
    const gap=await chevronGap(page,'.todo-history');
    assert.ok(gap.space>2&&Math.abs(gap.extra-gap.space)<0.75,`.todo-history: ${JSON.stringify(gap)}`);
  }
  // With no session state shown (a human row), the rule stays as the divide from the roster (#525).
  assert.equal(await page.locator('#participant-details').evaluate(e=>getComputedStyle(e).borderTopStyle),'solid');
  // The icons take the text colour and size, so they read in both themes (the check is the ok colour).
  for (const scheme of ['light','dark']) {
    await page.emulateMedia({colorScheme:scheme});
    const [stroke,color,size,ok]=await page.locator('.todo-history .todo-mark svg').evaluate(e=>{const s=getComputedStyle(e);return [s.stroke,getComputedStyle(e.parentElement).color,e.getBoundingClientRect().width,getComputedStyle(document.documentElement).getPropertyValue('--ok').trim()];});
    assert.equal(stroke,color,scheme); assert.ok(size>8&&size<20,String(size)); assert.ok(ok,scheme);
    // #529: the right panel's ground is Claude Desktop's sidebar (#fcfcfb) in light, the dark surface in dark.
    assert.equal(await page.locator('#participants').evaluate(e=>getComputedStyle(e).backgroundColor),scheme==='light'?'rgb(252, 252, 251)':'rgb(32, 32, 38)',scheme);
  }
  await page.emulateMedia({colorScheme:'light'});
  await page.locator('.todo-history .todo-source').click();
  await page.waitForFunction(()=>document.querySelector("dialog[open]")?.textContent.includes("判断に必要な元の発言"));
  await page.keyboard.press('Escape');
  await page.locator('.todo-history .todo-item').click();
  assert.match(await page.locator('dialog[open]').innerText(),/その方針で進めて。/);
  await page.keyboard.press('Escape');
  await page.locator('.todo-decisions .todo-item').click();
  await page.getByRole("button",{name:"回答",exact:true}).click();
  await page.getByRole("textbox",{name:"回答",exact:true}).fill("画面から直接回答します。");
  await page.getByRole("button",{name:"保存",exact:true}).click();
  assert.equal(await page.evaluate(()=>window.calls.find(c=>c.cmd==="update_topic_work").args.answer),"画面から直接回答します。");
  assert.equal(await page.locator(".todo-history").evaluate(e=>e.open),true);
  await page.locator('.todo-decisions').getByRole('button',{name:'発言を開く',exact:true}).first().click();
  await page.waitForFunction(()=>document.querySelector("dialog[open]")?.textContent.includes("判断に必要な元の発言"));
  await page.keyboard.press("Escape");
  await page.locator('.todo-decisions .todo-item').click();
  await page.getByRole('button',{name:'編集',exact:true}).click();
  await page.getByRole('textbox',{name:'質問',exact:true}).fill('短い編集後の質問');
  await page.getByRole('button',{name:'保存',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.calls.find(c=>c.cmd==='update_topic_work' && c.args.operation.action==='put').args.operation.text),'短い編集後の質問');
  await page.locator('.todo-decisions .todo-item').click();
  await page.getByRole('button',{name:'取消',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.calls.some(c=>c.cmd==='update_topic_work' && c.args.operation.action==='cancel')),true);
  const artifacts=process.env.STATUS_UI_ARTIFACTS ?? "artifacts/status-516";await mkdir(artifacts,{recursive:true});
  await page.screenshot({path:join(artifacts,"human-normal.png")});
  await page.locator('#roster button.pick[title="Codex Luna の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector("#participant-details").textContent.includes("中断中"));
  assert.match(await page.locator(".todo-plan .todo-head").innerText(),/計画（自己申告）\s*前の起動/);
  assert.equal(await page.locator('.todo-plan .todo-head > svg.todo-icon-calendar:first-child').count(),1);
  // #523: a long title is clamped to two lines and wraps inside the sidebar; clicking it opens the full text.
  const clamp=await page.locator('.todo-works .todo-item').evaluate(e=>{const s=getComputedStyle(e);return {lines:Math.round(e.clientHeight/parseFloat(s.lineHeight)),clipped:e.scrollHeight>e.clientHeight+1,fits:e.getBoundingClientRect().right<=e.closest('.todo-card').getBoundingClientRect().right+1,full:e.textContent.length};});
  assert.deepEqual([clamp.lines,clamp.clipped,clamp.fits],[2,true,true],JSON.stringify(clamp));
  await page.locator('.todo-works .todo-item').click();
  assert.match(await page.locator('dialog[open]').innerText(),/(参加者の状態表示を実装し、使用率と認証境界を確認する長い作業内容。){8}/);
  await page.keyboard.press('Escape');
  assert.deepEqual(await page.locator('.todo-fold > :not(summary)').evaluateAll(els=>els.map(e=>e.className)),['todo-section todo-decisions','todo-section todo-works','todo-plan','todo-history']);
  assert.match(await page.locator('.todo-works .todo-card').innerText(),/中断中/);
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
  assert.deepEqual(await chevron(page,'#terminal-diagnostics'),{content:'""',listStyle:'none',border:'solid',dir:'right'});
  // #525: no rule between the session's state and TODO, two lines of blank space instead; one line above 詳細.
  assert.equal(await page.locator('#participant-details').evaluate(e=>getComputedStyle(e).borderTopStyle),'none');
  {
    const todo=await lineHeight(page,'#participant-details'),detail=await lineHeight(page,'#terminal-diagnostics');
    assert.ok(Math.abs(todo.margin-2*todo.line)<1,JSON.stringify(todo));
    assert.ok(Math.abs(detail.margin-detail.line)<1,JSON.stringify(detail));
    const [facts,details,diag]=await page.evaluate(()=>['#terminal-facts','#participant-details','#terminal-diagnostics'].map(s=>document.querySelector(s).getBoundingClientRect()));
    assert.ok(details.top-facts.bottom>=2*todo.line-1,'TODO sits two lines below the usage bars');
    assert.ok(diag.top-details.bottom>=detail.line-1,'詳細 sits a line below TODO');
  }
  for (const fold of ['.todo-fold','#terminal-diagnostics']) {
    const gap=await chevronGap(page,fold);
    assert.ok(gap.space>2&&Math.abs(gap.extra-gap.space)<0.75,`${fold}: ${JSON.stringify(gap)}`);
  }
  // #527: the 詳細 heading reads as the TODO heading does — same weight, size and colour.
  {
    const heading=s=>page.locator(s).evaluate(e=>{const c=getComputedStyle(e);return {weight:c.fontWeight,size:c.fontSize,color:c.color};});
    const todo=await heading('.todo-fold > summary .todo-title'),detail=await heading('#terminal-diagnostics > summary');
    if (process.env.HEADING_STYLES) console.log(JSON.stringify({todo,detail,account:await heading('#participants .panel-title')}));
    assert.deepEqual(detail,todo);
  }
  await page.locator("#terminal-diagnostics").evaluate(e=>e.open=true);
  assert.deepEqual(await chevron(page,'#terminal-diagnostics'),{content:'""',listStyle:'none',border:'solid',dir:'down'});
  if (process.env.STATUS_UI_ARTIFACTS) {
    // Every fold open, for the visual check of the headings (#527).
    for (const fold of ['.todo-fold','.todo-history']) if (!await page.locator(fold).evaluate(e=>e.open)) await page.locator(`${fold} > summary`).click();
    await page.locator('#participants').screenshot({path:join(artifacts,"panel-all-open.png")});
  }
  if (process.env.SIDEBAR_COMPARISONS) await sidebarComparisons(page, process.env.SIDEBAR_COMPARISONS);
  assert.equal(await page.locator("#session-command").innerText(),"codex ai");
  assert.equal(await page.locator("#session-dir").innerText(),"D:/fixture/ai");
  assert.equal(await page.locator("#session-id").innerText(),"session-ai");
  await page.locator("#terminal-diagnostics").evaluate(e=>e.open=false);
  await page.evaluate(()=>window.detailsTest.stats({topic_id:'t',account_id:'other',pty_id:'pty-other',model:'OTHER-MODEL',effort:'high',login:'other@example.test',five_hour:100,seven_day:100,context:80,limited:true,five_hour_resets_at:null,seven_day_resets_at:null}));
  await page.evaluate(()=>window.detailsTest.joinLimited());
  for (const viewport of [{width:1280,height:900},{width:900,height:760}]) {
    await page.setViewportSize(viewport);
    // Fold states change only by the user's toggle (#523), so close them through their headings.
    if (await page.locator('.todo-history').evaluate(e=>e.open)) await page.locator('.todo-history > summary').click();
    if (await page.locator('.todo-fold').evaluate(e=>e.open)) await page.locator('.todo-fold > summary').click();
    await page.evaluate(()=>window.detailsTest.refresh());
    assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),false);
    const foldedHeight=await page.locator('.todo-fold').evaluate(e=>e.getBoundingClientRect().height);
    assert.ok(foldedHeight<35,String(foldedHeight));
    assert.equal((await chevron(page,'.todo-fold')).dir,'right');
    assert.equal(await page.locator('.todo-fold > summary').evaluate(e=>e.getBoundingClientRect().left),await page.locator('#terminal-diagnostics > summary').evaluate(e=>e.getBoundingClientRect().left));
    await page.screenshot({path:join(artifacts,`todo-folded-${viewport.width}.png`)});
    await page.locator('#participants').screenshot({path:join(artifacts,`panel-folded-${viewport.width}.png`)});
    await page.locator('.todo-fold > summary').click();
    await page.evaluate(()=>window.detailsTest.refresh());
    assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),true);
    assert.equal(await page.locator('.todo-history').evaluate(e=>e.open),false,'history does not take the outer open state');
    assert.equal(await page.locator('#participants').evaluate(e=>e.scrollWidth<=e.clientWidth+1),true);
    assert.doesNotMatch(await page.locator('.todo-fold').innerText(),/https:\/\/|javascript:/);
    for (const row of await page.locator('.todo-fold .todo-card, .todo-fold .todo-head, .todo-history > summary, .todo-steps li').all()) {
      assert.equal(await row.evaluate(e=>e.scrollWidth<=e.clientWidth+1),true,'todo rows stay inside the right column');
    }
    assert.ok(await page.locator('.todo-item').first().evaluate(e=>e.textContent.length<40));
    for (const title of ['Codex Luna の作業と判断待ち','Other AI の作業と判断待ち']) {
      const placement=await page.locator(`button.pick[title="${title}"]`).evaluate(e=>{const name=e.querySelector('.who').getBoundingClientRect(),badge=e.querySelector('.note').getBoundingClientRect();return {right:badge.x>=name.right-1,aligned:Math.abs((badge.y+badge.height/2)-(name.y+name.height/2))<2};});
      assert.deepEqual(placement,{right:true,aligned:true});
    }
    assert.match(await page.locator('button.pick[title="Other AI の作業と判断待ち"] .note').innerText(),/制限中/);
    await page.screenshot({path:join(artifacts,`todo-expanded-${viewport.width}.png`)});
    await page.locator('#participants').screenshot({path:join(artifacts,`panel-expanded-${viewport.width}.png`)});
  }
  // The whole panel with every fold open, TODO through 詳細 (#525), then the folds as they were.
  await page.setViewportSize({width:1280,height:1600});
  await page.locator('.todo-history > summary').click();
  await page.locator('#terminal-diagnostics').evaluate(e=>e.open=true);
  await page.locator('#participants').screenshot({path:join(artifacts,'panel-all-open.png')});
  await page.locator('#terminal-diagnostics').evaluate(e=>e.open=false);
  await page.locator('.todo-history > summary').click();
  await page.setViewportSize({width:1280,height:900});
  await page.locator('.todo-works .todo-item').first().click();
  assert.ok((await page.locator('dialog[open]').innerText()).includes('長い作業内容。'.repeat(1)));
  await page.screenshot({path:join(artifacts,'record-detail.png')});
  await page.locator('dialog[open]').getByRole('button',{name:'https://github.com/Liplus-Project/pullcept/issues/493',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.calls.find(c=>c.cmd==='open_post_url').args.url),'https://github.com/Liplus-Project/pullcept/issues/493');
  assert.equal(await page.locator('dialog[open]').getByRole('button',{name:'javascript:alert(1)',exact:true}).count(),0);
  await page.keyboard.press('Escape');
  await page.locator('.todo-steps .todo-item').last().click();
  assert.doesNotMatch(await page.locator('dialog[open]').innerText(),/completed|inProgress|pending/);
  assert.match(await page.locator('dialog[open]').innerText(),/完了 · 認証済み/);
  await page.keyboard.press('Escape');
  await page.evaluate(()=>document.querySelector('#diagnostics').hidden=false);
  await page.locator('#terminal-tabs button.name[title="Other AI の端末"]').click();
  await page.waitForFunction(()=>document.querySelector('#participant-details .todo-refresh')?.getAttribute('aria-label')==='Other AI の TODO を更新');
  assert.match(await page.locator('.todo-plan').innerText(),/計画（自己申告）\s*未取得\s*計画データを取得していません/);
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
  await page.locator('#roster button.pick[title="Unconfigured MCP の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector('#participant-details').textContent.includes('未処理: 未取得'));
  assert.equal(await page.locator('.todo-fold').count(),0);
  assert.equal(await page.locator('#terminal-facts').isVisible(),false);
  assert.equal(await page.locator('#terminal-diagnostics').isVisible(),false);
  assert.match(await page.locator('#participant-details').innerText(),/接続: 未取得/);
  await page.locator('#roster button.pick[title="GitHub の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector('#participant-details').textContent.includes('未処理: 未対応'));
  assert.equal(await page.locator('.todo-fold').count(),0);
  // The fold states are the user's (#523): only a toggle changes them, so passing through
  // an MCP view, which draws no fold, and switching participants keep them as left.
  await page.locator('#roster button.pick[title="Codex Luna の作業と判断待ち"]').click();
  assert.equal(await page.locator('.todo-fold').count(),1);
  assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),true,'the fold left open on マスター stays open on Luna');
  assert.equal(await page.locator('.todo-history').evaluate(e=>e.open),false);
  await page.locator('.todo-history > summary').click();
  assert.match(await page.locator('.todo-fold').innerText(),/判断待ち/);
  for (const name of ['Codex Luna','マスター']) {
    await page.locator('#roster button.pick[title="GitHub の作業と判断待ち"]').click();
    await page.waitForFunction(()=>document.querySelector('#participant-details').textContent.includes('未処理: 未対応'));
    assert.equal(await page.locator('.todo-fold').count(),0);
    await page.locator(`#roster button.pick[title="${name} の作業と判断待ち"]`).click();
    await page.waitForFunction(()=>document.querySelector('.todo-history'));
    assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),true,`${name}: the fold stays open after the MCP view`);
    assert.equal(await page.locator('.todo-history').evaluate(e=>e.open),true,`${name}: the history stays open after the MCP view`);
  }
  await page.locator('.todo-fold > summary').click();
  await page.evaluate(()=>window.detailsTest.refresh());
  assert.equal(await page.locator('.todo-fold').evaluate(e=>e.open),false,'closing is the user toggle too');
  await page.locator('#roster button.pick[title="GitHub の作業と判断待ち"]').click();
  await page.waitForFunction(()=>document.querySelector('#participant-details').textContent.includes('未処理: 未対応'));
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
