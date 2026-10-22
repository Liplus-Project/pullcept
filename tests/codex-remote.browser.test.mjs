import {test} from "node:test";
import assert from "node:assert/strict";
import {chromium} from "playwright-core";
import {createServer} from "vite";
import {existsSync} from "node:fs";
const edge=process.env.PREVIEW_BROWSER ?? ["C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe","C:/Program Files/Microsoft/Edge/Application/msedge.exe","/usr/bin/chromium"].find(existsSync);
test("shared remote dialog confirms global stop in the app without native confirm",{timeout:60000},async t=>{
 const server=await createServer({server:{host:"127.0.0.1",port:0},logLevel:"error"});await server.listen();t.after(()=>server.close());
 const browser=await chromium.launch({executablePath:edge,headless:true});t.after(()=>browser.close());const page=await browser.newPage();
 const errors=[];page.on("pageerror",error=>errors.push(String(error)));
 const base=server.resolvedUrls.local[0].replace(/\/$/,"");
 await page.route(`${base}/remote-fixture`,route=>route.fulfill({contentType:"text/html; charset=utf-8",body:`<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/src/styles.css"><script type="module">
 import {openCodexRemote} from '/src/codex-remote.ts';
 window.calls=[];window.confirm=()=>{throw new Error('native confirm must not run');};
 window.openRemote=(shared=true)=>openCodexRemote('fixture-pty','fixture',()=>true,{invoke:async(_method,args)=>{window.calls.push(args);return {epoch:'fixture-launch',shared,status:'connected',pairing:null,error:null,notice:shared?null:'共通接続を利用できないため従来の席別接続'};},renderQr:async()=>{}});
 window.ready=true;</script>`}));
 await page.goto(`${base}/remote-fixture`);await page.waitForFunction(()=>window.ready);await page.evaluate(()=>window.openRemote());
 await page.getByRole('heading',{name:'共通の Codex スマホ接続'}).waitFor();
 await page.getByRole('button',{name:'共有接続を停止',exact:true}).click();
 assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.action==='disable').length),0);
 await page.getByRole('button',{name:'全席の共有接続を停止する',exact:true}).click();
 await page.waitForFunction(()=>window.calls.some(c=>c.action==='disable'&&c.confirmShared===true));
 await page.getByRole('button',{name:'閉じる',exact:true}).click();
 await page.locator('.codex-remote-dialog').waitFor({state:'detached'});
 await page.evaluate(()=>window.openRemote(false));await page.getByRole('heading',{name:'fixture — スマホから接続'}).waitFor();
 await page.getByText('共通接続を利用できないため従来の席別接続',{exact:true}).waitFor();
 assert.match(await page.locator('.codex-remote-dialog p').first().innerText(),/従来の席別接続/);
 await page.getByRole('button',{name:'閉じる',exact:true}).click();assert.deepEqual(errors,[]);
});
