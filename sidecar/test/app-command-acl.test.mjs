// The app's own commands go through the ACL (#449): src-tauri/build.rs lists
// them in the app manifest, and only the main window's capability grants them.
// A command registered in `invoke_handler` but missing from the manifest would
// not build into a permission; one missing from the capability would be denied
// to main. Both lists and the capability are kept equal here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

// LF, whatever the checkout wrote.
const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n?/g, "\n");

const registered = () => {
  const body = read("../../src-tauri/src/lib.rs").match(/generate_handler!\[([\s\S]*?)\]/)[1];
  return body
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((path) => path.split("::").pop());
};

const manifest = () => {
  const body = read("../../src-tauri/build.rs").match(/const APP_COMMANDS: &\[&str\] = &\[([\s\S]*?)\];/)[1];
  return [...body.matchAll(/"([^"]+)"/g)].map(([, name]) => name);
};

const appPermission = (command) => `allow-${command.replace(/_/g, "-")}`;
// A plugin or core permission carries its namespace; an app permission does not.
const isAppPermission = (id) => !id.includes(":");

const capabilities = () =>
  readdirSync(new URL("../../src-tauri/capabilities/", import.meta.url))
    .filter((name) => name.endsWith(".json"))
    .map((name) => [name, JSON.parse(read(`../../src-tauri/capabilities/${name}`))]);

test("every command in invoke_handler is in the app manifest, and nothing else is", () => {
  const handler = registered();
  assert.ok(handler.length > 0);
  assert.equal(new Set(handler).size, handler.length);
  assert.deepEqual([...manifest()].sort(), [...handler].sort());
});

test("the main capability grants exactly the manifest's commands", () => {
  const main = capabilities().find(([name]) => name === "default.json")[1];
  assert.deepEqual(main.windows, ["main"]);
  assert.equal(main.webviews, undefined);
  assert.equal(main.remote, undefined);
  const granted = main.permissions.filter((p) => typeof p === "string" && isAppPermission(p));
  assert.deepEqual([...granted].sort(), manifest().map(appPermission).sort());
});

test("no other capability grants an app command", () => {
  for (const [name, capability] of capabilities()) {
    if (name === "default.json") continue;
    const appGrants = (capability.permissions ?? []).filter((p) =>
      isAppPermission(typeof p === "string" ? p : p.identifier),
    );
    assert.deepEqual(appGrants, [], `${name} grants app commands`);
  }
});
