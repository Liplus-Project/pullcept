import {test} from "node:test";
import assert from "node:assert/strict";
import {pythonCommand} from "../src/codex-style.mjs";
const apps = "C:\\Users\\me\\AppData\\Local\\Microsoft\\WindowsApps";
const real = "C:\\Users\\me\\AppData\\Local\\Python\\bin";
test("the App Installer alias in WindowsApps is passed over for the real python.exe (#361)", () => {
  const on = new Set([`${apps}\\python.exe`, `${real}\\python.exe`]);
  assert.equal(pythonCommand({Path:`${apps};${real}`}, "win32", path => on.has(path)), `${real}\\python.exe`);
  assert.equal(pythonCommand({PATH:`${apps}\\;${real}`}, "win32", path => on.has(path)), `${real}\\python.exe`);
});
test("with python.exe only in WindowsApps, plain python is kept (a Store install)", () => {
  assert.equal(pythonCommand({Path:apps}, "win32", path => path === `${apps}\\python.exe`), "python");
  assert.equal(pythonCommand({}, "win32", () => false), "python");
});
test("off Windows the command stays python3", () => {
  assert.equal(pythonCommand({PATH:"/usr/bin"}, "linux", () => true), "python3");
});
