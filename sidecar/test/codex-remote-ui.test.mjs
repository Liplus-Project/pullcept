import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";
const { openCodexRemote } = await tsImport("../../src/codex-remote.ts", import.meta.url);

class Element {
  children = []; listeners = new Map(); textContent = ""; disabled = false;
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  addEventListener(event, fn) { this.listeners.set(event, fn); }
  removeEventListener(event) { this.listeners.delete(event); }
  setAttribute() {} showModal() {} remove() { this.removed = true; }
  emit(event) { this.listeners.get(event)?.(); }
  close() { this.emit("close"); }
  querySelectorAll(name) { return this.children.filter(c => c.tag === name); }
}
const settle = () => new Promise(resolve => setImmediate(resolve));
function fixture(t, invoke, renderQr = async () => {}) {
  const previous = { document: globalThis.document, setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval };
  const timers = new Map(); let next = 0; let live = true;
  const body = new Element();
  globalThis.document = Object.assign(new Element(), { body, hidden: false, createElement(tag) { return Object.assign(new Element(), {tag}); }});
  globalThis.setInterval = fn => { timers.set(++next, fn); return next; };
  globalThis.clearInterval = id => timers.delete(id);
  t.after(() => Object.assign(globalThis, previous));
  openCodexRemote("pty", "Luna", () => live, {invoke, renderQr});
  const dialog = body.children[0];
  const buttons = dialog.children[5].children;
  return { dialog, privateArea: dialog.children[4], buttons, timers,
    tick: () => [...timers.values()].forEach(fn => fn()), end: () => { live = false; },
    hidden() { document.hidden = true; document.emit("visibilitychange"); } };
}
const status = (epoch="seat-1") => ({epoch, status:"connected", error:null, pairing:null});
const pair = () => ({...status(), status:null, pairing:{pairingCode:"private-qr",manualPairingCode:"private-pin",expiresAt: Math.floor(Date.now()/1000)+60}});

test("relay screen discards a pending private reply after close", async t => {
  let answer;
  const f = fixture(t, async (_method, {action}) => action === "pair" ? new Promise(r => {answer=r;}) : status());
  await settle(); f.buttons[2].emit("click"); await settle();
  f.dialog.close(); answer(pair()); await settle();
  assert.equal(f.privateArea.children.length,0);
  assert.equal(f.timers.size,0); assert.equal(f.dialog.removed,true);
});

test("relay screen renders locally and erases codes on expiry, hiding and seat exit", async t => {
  let now=Date.now(); t.mock.method(Date,"now",()=>now);
  const qr=[];
  const f=fixture(t,async (_method,{action})=>action==="pair"?pair():status(),async (canvas,url)=>qr.push({canvas,url}));
  await settle(); f.buttons[2].emit("click"); await settle();
  assert.equal(qr.length,1); assert.equal(new URL(qr[0].url).origin,"https://chatgpt.com");
  assert.equal(new URL(qr[0].url).searchParams.get("pairing_code"),"private-qr");
  assert.match(f.privateArea.children[1].textContent,/private-pin/);
  now+=61_000; f.tick(); await settle();
  assert.equal(f.privateArea.children.length,0); assert.equal(qr[0].canvas.width,0);
  f.buttons[2].emit("click"); await settle(); f.hidden();
  assert.equal(f.privateArea.children.length,0);
  document.hidden=false; f.buttons[2].emit("click"); await settle(); f.end(); f.tick();
  assert.equal(f.privateArea.children.length,0); assert.equal(f.timers.size,0);
  assert.equal(f.buttons[0].disabled,true); f.dialog.close();
});

test("late QR completion and seat generation mismatch cannot restore secrets", async t => {
  let finishQr; let epoch="seat-1";
  const f=fixture(t,async (_method,{action})=>action==="pair"?pair():status(epoch),()=>new Promise(r=>{finishQr=r;}));
  await settle(); f.buttons[2].emit("click"); await settle();
  const canvas=f.privateArea.children[0]; f.dialog.close(); canvas.width=999; finishQr(); await settle();
  assert.equal(canvas.width,0); assert.equal(f.privateArea.children.length,0);
  const g=fixture(t,async()=>status(epoch)); await settle(); epoch="seat-2"; g.tick(); await settle();
  assert.match(g.dialog.children[2].textContent,/終了/); assert.equal(g.buttons[2].disabled,true); g.dialog.close();
});

test("unsupported remote API leaves the screen usable for retry without codes", async t => {
  let calls=0;
  const f=fixture(t,async()=>{calls++; return {...status(),status:null,error:"この CLI は非対応です"};});
  await settle(); assert.match(f.dialog.children[3].textContent,/非対応/);
  assert.equal(f.privateArea.children.length,0); assert.equal(f.buttons[0].disabled,false);
  f.buttons[0].emit("click"); await settle(); assert.equal(calls,2); f.dialog.close();
});


test("expiry while QR rendering is pending prevents the late canvas restoring codes", async t => {
  let now=Date.now(); t.mock.method(Date,"now",()=>now); let finishQr;
  const f=fixture(t,async (_method,{action})=>action==="pair"?pair():status(),()=>new Promise(r=>{finishQr=r;}));
  await settle(); f.buttons[2].emit("click"); await settle();
  const canvas=f.privateArea.children[0]; now+=61_000; f.tick();
  assert.equal(f.privateArea.children.length,0);
  canvas.width=999; finishQr(); await settle();
  assert.equal(canvas.width,0); assert.equal(f.privateArea.children.length,0); f.dialog.close();
});


test("shared connection names its scope and requires a second explicit stop click", async t => {
  const calls=[];
  const f=fixture(t,async (_method,args)=>{calls.push(args);return {...status(),shared:true};});
  await settle();
  assert.match(f.dialog.children[0].textContent,/共通/);
  assert.match(f.dialog.children[1].textContent,/再ペアリング/);
  assert.match(f.dialog.children[1].textContent,/会話を閉じて/);
  f.buttons[1].emit("click");await settle();
  assert.equal(calls.filter(c=>c.action==="disable").length,0);
  assert.match(f.buttons[1].textContent,/全席/);
  assert.match(f.dialog.children[3].textContent,/すべての席/);
  f.buttons[1].emit("click");await settle();
  assert.equal(calls.filter(c=>c.action==="disable").length,1);
  assert.equal(calls.find(c=>c.action==="disable").confirmShared,true);
  f.dialog.close();
});

test("unknown shared identity displays dedicated-connection fallback notice", async t => {
  const f=fixture(t,async()=>({...status(),shared:false,notice:"共通接続を利用できないため従来の席別接続"}));
  await settle();assert.match(f.dialog.children[1].textContent,/席別接続/);
  assert.doesNotMatch(f.dialog.children[0].textContent,/共通/);
  f.dialog.close();
});
