import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";

// Import the actual owners: importing must not touch the DOM or register events.
const { createSessionController } = await tsImport("../../src/session-controller.ts", import.meta.url);
const { createAccountDialog } = await tsImport("../../src/account-dialog.ts", import.meta.url);
const { createDisplaySettings } = await tsImport("../../src/display-settings.ts", import.meta.url);

class Element {
  value = ""; textContent = ""; hidden = false; open = false;
  dataset = {}; children = []; listeners = new Map();
  style = { setProperty() {}, removeProperty() {} };
  classList = { add() {}, remove() {}, toggle() {} };
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  emit(name) { this.listeners.get(name)?.({preventDefault() {}}); }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  remove() { this.removed = true; }
  setAttribute() {} removeAttribute() {} focus() {} select() {}
  querySelector() { return new Element(); } querySelectorAll() { return []; }
  closest() { return null; }
  showModal() { this.open = true; }
  close() { this.open = false; this.emit("close"); }
}

function dom(t) {
  const elements = new Map();
  const get = id => { if (!elements.has(id)) elements.set(id, new Element()); return elements.get(id); };
  const previous = {document: globalThis.document, window: globalThis.window, getComputedStyle: globalThis.getComputedStyle, clearTimeout: globalThis.clearTimeout};
  globalThis.document = {getElementById: get, createElement: () => new Element(), documentElement: new Element()};
  const timers = new Map(); let nextTimer = 1;
  globalThis.window = {setTimeout(fn) { const id = nextTimer++; timers.set(id, fn); return id; }, clearTimeout(id) { timers.delete(id); }, addEventListener() {}};
  globalThis.clearTimeout = id => timers.delete(id);
  globalThis.getComputedStyle = () => ({getPropertyValue: () => "#000"});
  t.after(() => Object.assign(globalThis, previous));
  return {get, timers};
}

function terminal() {
  return {options: {}, cols: 80, rows: 24, writes: [],
    loadAddon() {}, open() {}, onData(fn) { this.input = fn; }, attachCustomKeyEventHandler() {},
    write(text) { this.writes.push(text); }, refresh() {}, focus() {},
    dispose() { this.disposed = true; }};
}
const account = {id: "one", name: "One", command: "claude", args: [], cwd: "D:/work", hue: null,
  kind: "claude_code", character: null, resume_command: null, env: [], server: null, avatar: false};
const noop = () => {};

test("sessions subscribe in order, keep topics separate, and release listeners/timers/emulator on discard", async t => {
  const {get, timers} = dom(t); let topic = "first";
  const callbacks = new Map(), order = [], off = [];
  let resolveLaunch, resolveSubscribe, holdSubscription = false;
  const controller = createSessionController({
    invoke: async command => command === "start_session" ? await new Promise(resolve => { resolveLaunch = resolve; }) : [],
    listen: async (event, callback) => { order.push(event); callbacks.set(event, callback); const unsubscribe = () => { off.push(event); callbacks.delete(event); }; return holdSubscription ? await new Promise(resolve => { resolveSubscribe = () => resolve(unsubscribe); }) : unsubscribe; },
    createTerminal: terminal, createFitAddon: () => ({fit() {}}), useWebglRenderer: noop,
    shownTopicId: () => topic, seatKey: (topic, account) => `${topic}\n${account}`,
    accounts: [account], topics: [], diagnosticsEl: Object.assign(get("diagnostics"), {hidden: true}), terminalEl: get("terminal"),
    renderPanel: noop, renderTopics: noop, revealDiagnostics: noop, status: noop, speakerColor: () => "red",
    icon: () => new Element(), shortTime: text => text, members: () => [], launches: () => true, memberName: () => "One", openAccountDialog: noop,
  });
  const launching = controller.startSession(account);
  const view = controller.getView("first\none");
  assert.equal(view.ptyId, "");
  topic = "second"; controller.showTopicTerminals();
  resolveLaunch({pty_id: "pty-one", started_at: "now", resumed_from: null, dropped_resume: null, mcp_config: "config"});
  await launching;
  assert.equal(controller.selectedAccount(), null);
  assert.equal(view.host.hidden, true);
  assert.deepEqual(order, ["pty-data-pty-one", "pty-exit-pty-one"]);
  assert.equal(timers.size, 1);
  const oldData = callbacks.get("pty-data-pty-one"), oldExit = callbacks.get("pty-exit-pty-one");
  oldData({payload: "bytes"});
  controller.receiveStats({topic_id: "first", account_id: "one", pty_id: "old-pty", limited: true});
  controller.receiveActivity({topic_id: "first", account_id: "one", pty_id: "old-pty", word: "old"});
  assert.equal(view.stats, null);
  assert.equal(view.activity, null);
  assert.equal(view.outputting, true);
  assert.deepEqual(view.term.writes, ["bytes"]);
  topic = "first"; controller.showTopicTerminals();
  assert.equal(controller.shownView(), view);
  controller.discardView(view);
  assert.deepEqual(off, order);
  assert.equal(timers.size, 0);
  assert.equal(view.term.disposed, true);
  assert.equal(view.host.removed, true);
  assert.equal(controller.getView("first\none"), undefined);
  assert.equal(controller.selectedAccount(), null);
  oldData({payload: "late bytes"});
  oldExit({payload: {code: 1, requested: false}});
  assert.deepEqual(view.term.writes, ["bytes"]);
  assert.equal(view.ended, null);
  assert.equal(timers.size, 0);

  // Disposal can also happen while Tauri is still returning the first unlisten.
  holdSubscription = true; topic = "third";
  const pendingLaunch = controller.startSession(account);
  const pendingView = controller.getView("third\none");
  resolveLaunch({pty_id: "pty-third", started_at: "now", resumed_from: null, dropped_resume: null, mcp_config: "config"});
  await new Promise(resolve => setImmediate(resolve));
  controller.discardView(pendingView);
  resolveSubscribe();
  await pendingLaunch;
  assert.equal(callbacks.has("pty-data-pty-third"), false);
  assert.equal(callbacks.has("pty-exit-pty-third"), false);
  assert.equal(timers.size, 0);
});

test("account cancellation rejects a pending environment read and leaves stored account unchanged", async t => {
  const {get} = dom(t); const original = structuredClone(account); const accounts = [original];
  let resolveEnv;
  const dialog = createAccountDialog({
    invoke: async command => command === "account_env_text" ? await new Promise(resolve => { resolveEnv = resolve; }) : "",
    listen: async () => noop, accounts, shownTopicId: () => "first", joinArgs: args => args.join(" "), homeDir: "D:/work", localAccountId: "human",
    speakerColor: () => "red", declaredHue: () => null, initialOf: name => name[0], avatarImages: new Map(), drawAvatar: noop,
    asciiJson: text => text, setAvatarImage: noop, saveConfig: async () => true, renderPanel: noop, status: noop,
    seatedAnywhere: () => false, renderSessionFacts: noop, join: async () => {}, allViews: () => [], discardView: noop, fillHues: noop,
  });
  dialog.wireForm();
  assert.equal(get("account-dialog").open, false);
  dialog.openAccountDialog(original);
  get("dialog-name").value = "Uncommitted";
  get("dialog-cancel").emit("click");
  assert.equal(get("account-dialog").open, false);
  resolveEnv("SECRET=late");
  await new Promise(resolve => setImmediate(resolve));
  assert.notEqual(get("dialog-env").value, "SECRET=late");
  assert.deepEqual(accounts, [account]);
});

test("display initialization restores fonts before wiring changes without writing defaults", t => {
  const {get} = dom(t); const before = globalThis.localStorage; const stored = new Map([["pullcept.terminal-font-size", "18"]]); const writes = [];
  globalThis.localStorage = {getItem: key => stored.get(key) ?? null, setItem: (key, value) => writes.push([key, value])};
  t.after(() => { globalThis.localStorage = before; });
  const sizes = [];
  const display = createDisplaySettings({roomEl: get("room"), inputEl: get("input"), participantsEl: get("participants"), syncScrollLatest: noop, setTerminalFontSize: size => sizes.push(size)});
  assert.deepEqual(sizes, []);
  display.initialize();
  assert.deepEqual(sizes, [18]);
  assert.deepEqual(writes, []);
  get("settings-terminal-font-size").value = "16"; get("settings-terminal-font-size").emit("change");
  assert.deepEqual(sizes, [18, 16]);
  assert.deepEqual(writes, [["pullcept.terminal-font-size", "16"]]);
});
