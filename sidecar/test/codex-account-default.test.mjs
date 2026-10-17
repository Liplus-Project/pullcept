import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";
const { createAccountDialog } = await tsImport("../../src/account-dialog.ts", import.meta.url);

class Element {
  value = ""; checked = false; hidden = false; disabled = false; textContent = "";
  dataset = {}; listeners = new Map(); classList = { add(){}, remove(){}, toggle(){} };
  style = { setProperty(){} };
  addEventListener(name, fn) { this.listeners.set(name, fn); }
  emit(name) { this.listeners.get(name)?.({ preventDefault(){} }); }
  querySelectorAll() { return []; }
  querySelector() { return new Element(); }
  setAttribute() {} removeAttribute() {} focus() {} select() {} showModal() {} close() { this.emit("close"); }
}
const settle = () => new Promise(resolve => setTimeout(resolve, 320));
function fixture(t, account) {
  const old = {document:globalThis.document, window:globalThis.window};
  const nodes = new Map();
  const el = id => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); };
  globalThis.document = {getElementById:el}; globalThis.window = globalThis;
  t.after(() => Object.assign(globalThis, old));
  const calls = [], accounts = account ? [account] : [];
  const dialog = createAccountDialog({accounts, homeDir:"C:/project", avatarImages:new Map(),
    invoke:async (name,args) => {
      calls.push({name,args});
      if (name === "parse_launch_options") return [];
      if (name === "account_env_text") return "";
      if (name === "launch_field_report") return {character:true, options:true, command:true, resume:true};
      if (name === "preview_launch_args") return {args:[],server_args:args.codexAppServer ? [] : null};
      throw new Error(`Unexpected invoke: ${name}`);
    }, joinArgs:()=>"", shownTopicId:()=>"topic", declaredHue:()=>null,
    drawAvatar(){}, speakerColor:()=>"blue", initialOf:()=>"C", saveConfig:async()=>true,
    renderPanel(){}, renderSessionFacts(){}, status(){}, seatedAnywhere:()=>false });
  dialog.wireForm(); dialog.openAccountDialog(account ?? null);
  return {el, calls, accounts, dialog};
}
const account = (kind, flag) => ({id:"a", name:"Codex", command:kind === "codex_cli" ? "codex" : "claude", args:[], cwd:"C:/project", hue:null, kind, character:null, resume_command:null, env:[], server:null, avatar:false, ...(flag === undefined ? {} : {codex_app_server:flag})});

for (const [label, original, expected] of [
  ["missing setting", account("codex_cli"), true],
  ["explicit hook", account("codex_cli", false), false],
  ["explicit app-server", account("codex_cli", true), true],
  ["new account switched to Codex", null, true],
  ["existing Claude switched to Codex", account("claude_code", false), true],
]) {
  test(`Codex dialog preview and save agree for ${label}`, async t => {
    const f = fixture(t, original);
    if (!original || original.kind !== "codex_cli") {
      f.el("dialog-kind").value = "codex_cli"; f.el("dialog-kind").emit("change");
    }
    await settle();
    assert.equal(f.el("dialog-codex-app-server").checked, expected);
    assert.equal(f.el("dialog-codex-remote").checked, false);
    assert.equal(f.calls.filter(c=>c.name === "preview_launch_args").at(-1).args.codexAppServer, expected);
    assert.equal(f.calls.filter(c=>c.name === "launch_field_report").at(-1).args.codexAppServer, expected);
    f.el("account-form").emit("submit"); await settle();
    assert.equal(f.accounts[0].codex_app_server, expected);
    assert.equal(f.accounts[0].codex_remote_control, false);
    f.dialog.openAccountDialog(f.accounts[0]); await settle();
    assert.equal(f.el("dialog-codex-app-server").checked, expected);
    f.el("account-dialog").close();
  });
}

test("Codex hook switch persists across kind round trip and edit", async t => {
  const f = fixture(t, account("codex_cli", true));
  f.el("dialog-codex-app-server").checked = false;
  f.el("dialog-codex-app-server").emit("change");
  f.el("dialog-kind").value = "cli"; f.el("dialog-kind").emit("change");
  f.el("dialog-kind").value = "codex_cli"; f.el("dialog-kind").emit("change");
  await settle();
  assert.equal(f.calls.filter(c=>c.name === "preview_launch_args").at(-1).args.codexAppServer, false);
  f.el("account-form").emit("submit"); await settle();
  assert.equal(f.accounts[0].codex_app_server, false);
  f.dialog.openAccountDialog(f.accounts[0]); await settle();
  assert.equal(f.el("dialog-codex-app-server").checked, false);
  f.el("account-dialog").close();
});

