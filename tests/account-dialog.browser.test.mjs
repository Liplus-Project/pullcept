import {test} from "node:test";
import assert from "node:assert/strict";
import {chromium} from "playwright-core";
import {createServer} from "vite";
import {existsSync} from "node:fs";
const edge=process.env.PREVIEW_BROWSER ?? ["C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe","C:/Program Files/Microsoft/Edge/Application/msedge.exe","/usr/bin/chromium"].find(existsSync);
const entry = `import {createAccountDialog} from './account-dialog';
import {codexPlanEnabled,withCodexPlan} from './codex-plan';
const accounts=[]; const avatars=new Map();
const invoke=async(cmd,args)=>window.invokeTest(cmd,args);
const dialog=createAccountDialog({invoke,listen:async()=>()=>{},accounts,shownTopicId:()=>'',joinArgs:args=>args.map(a=>/\\s/.test(a)?JSON.stringify(a):a).join(' '),homeDir:'',localAccountId:'human',speakerColor:()=>'',declaredHue:()=>null,initialOf:()=>'',avatarImages:avatars,drawAvatar:()=>{},asciiJson:JSON.stringify,setAvatarImage:()=>{},saveConfig:async()=>{await invoke('save_config',{accounts:structuredClone(accounts)});return true;},renderPanel:()=>{},status:(text,kind)=>window.statuses.push({text,kind}),seatedAnywhere:()=>false,renderSessionFacts:()=>{},join:async()=>{},allViews:()=>[],discardView:()=>{},fillHues:()=>{}});
dialog.wireForm(); window.test={accounts,avatars,open:(account=null)=>dialog.openAccountDialog(account),codexPlanEnabled,withCodexPlan};`;
test('account save stays open, retries preserve identity and Codex options', {timeout:60000}, async t=>{
 const server=await createServer({server:{host:'127.0.0.1',port:0},logLevel:'error',plugins:[{name:'account-test',enforce:'pre',transform(code,id){if(id.endsWith('/src/main.ts'))return entry;}}]});await server.listen();t.after(()=>server.close());
 const browser=await chromium.launch({executablePath:edge,headless:true});t.after(async()=>{await browser.close();});const page=await browser.newPage();
 const errors=[];page.on('pageerror',e=>errors.push(String(e)));
 await page.addInitScript(()=>{
  window.calls=[];window.statuses=[];window.failConfig=false;window.created=null;
  window.invokeTest=async(cmd,args)=>{
   window.calls.push({cmd,args});
   if(cmd==='parse_launch_options')return (args.text.match(/"(?:\\.|[^"\\])*"|\S+/g)||[]).map(v=>v.startsWith('"')?JSON.parse(v):v);
   if(cmd==='account_env_text')return (args.env??[]).map(e=>e.key+'='+e.value).join('\n');
   if(cmd==='seal_account_env')return [{key:'CHECK',value:args.text.split('=')[1]}];
   if(cmd==='preview_launch_args')return {args:args.args};
   if(cmd==='save_config'){if(window.delayConfig)await new Promise(r=>window.releaseConfig=r);if(window.failConfig)throw Error('disk unavailable');window.saved=args.accounts;return;}
   if(cmd==='create_mcp_server'){window.created={name:'server',command:args.command,args:args.args,env:args.env,listed:true,state:null,log:[]};return {server:'server',account_id:'mcp-id'};}
   if(cmd==='mcp_servers')return {file:'fixture',error:null,servers:window.created?[window.created]:[]};
   if(cmd==='delete_account_avatar'&&window.failAvatar)throw Error('avatar unavailable');
   if(cmd==='save_mcp_server'){Object.assign(window.created,args);return;}
   if(cmd==='open_character_file')return {body:'original',exists:true,folder_exists:true,path:'fixture.md',stamp:'first',name_in_file:'persona'};
   if(cmd==='character_file_wearers')return [];
   if(cmd==='save_character_file')return {written:true,opened:{body:args.body,exists:true,folder_exists:true,path:'fixture.md',stamp:'second'}};
   return null;
  };
 });
 await page.goto(server.resolvedUrls.local[0]);await page.waitForFunction(()=>window.test);
 const save=async()=>{const n=await page.evaluate(()=>window.calls.filter(c=>c.cmd==='save_config').length);await page.locator('#dialog-commit').click();await page.waitForFunction(n=>window.calls.filter(c=>c.cmd==='save_config').length>n,n,{timeout:4000}).catch(async e=>{throw Error(e.message+' '+await page.locator('#dialog-error').innerText())});await page.waitForFunction(()=>!document.querySelector('#dialog-commit').closest('form').inert);};
 await page.evaluate(()=>window.test.open());await page.locator('#dialog-kind').selectOption('codex_cli');await page.locator('#dialog-name').fill('one');
 await page.locator('[data-section-tab="launch"]').click();
 assert.equal(await page.locator('#dialog-codex-plan').isChecked(),false);
 await page.locator('#dialog-options').fill('--model model -c tools.update_plan.enabled=false --config=tools.update_plan.enabled=true -c unrelated=true');
 await page.waitForFunction(()=>document.querySelector('#dialog-codex-plan').checked);
 await page.locator('#dialog-codex-plan').uncheck();await save();
 assert.equal(await page.locator('#account-dialog').evaluate(e=>e.open),true);
 const first=await page.evaluate(()=>window.test.accounts[0]);assert.equal(first.args.filter(a=>a.startsWith('tools.update_plan.enabled=')).length,1);assert.ok(first.args.includes('tools.update_plan.enabled=false'));assert.ok(first.args.includes('unrelated=true'));
 await page.locator('#dialog-codex-plan').check();await save();assert.ok(await page.evaluate(()=>window.test.accounts[0].args.includes('tools.update_plan.enabled=true')));assert.equal(await page.locator('#account-dialog').evaluate(e=>e.open),true);await page.locator('#dialog-cancel').click();await page.evaluate(()=>window.test.open(window.test.accounts[0]));await page.locator('[data-section-tab="launch"]').click();assert.equal(await page.locator('#dialog-codex-plan').isChecked(),true);
 await page.locator('[data-section-tab="basic"]').click();await page.locator('#dialog-name').fill('two');await save();assert.equal(await page.evaluate(()=>window.test.accounts.length),1);assert.equal(await page.evaluate(()=>window.test.accounts[0].id),first.id);
 await page.locator('#dialog-name').fill('unsaved');await page.locator('#dialog-cancel').click();assert.equal(await page.evaluate(()=>window.test.accounts[0].name),'two');
 await page.evaluate(()=>{window.test.open(window.test.accounts[0]);window.failConfig=true;});await page.locator('#dialog-name').fill('failed');await save();assert.match(await page.locator('#dialog-error').innerText(),/disk unavailable/);assert.equal(await page.evaluate(()=>window.test.accounts[0].name),'two');
 await page.evaluate(()=>{window.failConfig=false;window.delayConfig=true;});await page.locator('#dialog-commit').click();await page.waitForFunction(()=>window.releaseConfig);await page.keyboard.press('Escape');assert.equal(await page.locator('#account-dialog').evaluate(e=>e.open),true);await page.evaluate(()=>{window.delayConfig=false;window.releaseConfig();});await page.waitForFunction(()=>!document.querySelector('#dialog-commit').closest('form').inert);await page.locator('#dialog-cancel').click();
 // The character write refreshes its stamp/body baseline for a second save.
 await page.evaluate(()=>window.test.open(window.test.accounts[0]));await page.locator('[data-section-tab="character"]').click();await page.locator('#dialog-character').fill('persona');await page.waitForFunction(()=>!document.querySelector('#dialog-character-body').disabled);await page.locator('#dialog-character-body').fill('changed');await save();await save();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='save_character_file').length),1);await page.locator('#dialog-cancel').click();
 await page.evaluate(()=>window.test.open(window.test.accounts[0]));await page.locator('[data-section-tab="env"]').click();await page.locator('#dialog-env').fill('CHECK=value');await save();assert.equal(await page.locator('#dialog-env').inputValue(),'CHECK=value');assert.equal(await page.locator('#dialog-env').evaluate(e=>e.readOnly),false);await save();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='seal_account_env').length),1);await page.locator('#dialog-cancel').click();
 // Avatar failure remains editable; the successful baseline avoids repeat deletion.
 await page.evaluate(()=>{window.test.accounts[0].avatar=true;window.test.avatars.set(window.test.accounts[0].id,"fixture.png");window.test.open(window.test.accounts[0]);window.failAvatar=true;});
 await page.locator('#dialog-avatar-clear').click();await page.locator('#dialog-commit').click();await page.waitForFunction(()=>!document.querySelector('#dialog-commit').closest('form').inert);assert.match(await page.locator('#dialog-error').innerText(),/avatar unavailable/);
 await page.evaluate(()=>window.failAvatar=false);await save();await save();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='delete_account_avatar').length),2);assert.equal(await page.evaluate(()=>window.test.accounts[0].avatar),false);await page.locator('#dialog-cancel').click();
 // A new MCP entry survives a failed account-config write without being created twice.
 await page.evaluate(()=>{window.test.open();window.failConfig=true;});await page.locator('#dialog-kind').selectOption('mcp');await page.locator('[data-section-tab="server"]').click();await page.locator('#mcp-command').fill('fixture-server');await save();assert.equal(await page.evaluate(()=>window.test.accounts.length),1);await page.evaluate(()=>window.failConfig=false);await save();await save();assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='create_mcp_server').length),1);assert.equal(await page.evaluate(()=>window.test.accounts.filter(a=>a.id==='mcp-id').length),1);assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='restart_mcp_server').length),1);
 for(const id of ['dialog-commit','dialog-cancel']){assert.equal(await page.locator('#'+id).evaluate(e=>getComputedStyle(e).backgroundColor),'rgba(0, 0, 0, 0)');await page.locator('#'+id).focus();assert.equal(await page.locator('#'+id).evaluate(e=>document.activeElement===e),true);}
 assert.equal(await page.evaluate(()=>window.calls.some(c=>['start_session','start_codex_session'].includes(c.cmd))),false);
 assert.deepEqual(await page.evaluate(()=>{const a=['--model','x','--config=tools.update_plan.enabled=false','-ctools.update_plan.enabled=true','--','tools.update_plan.enabled=prompt'];return [window.test.codexPlanEnabled(a),window.test.withCodexPlan(a,false)];}),[true,['--model','x','-c','tools.update_plan.enabled=false','--','tools.update_plan.enabled=prompt']]);
 assert.deepEqual(errors,[]);
});
