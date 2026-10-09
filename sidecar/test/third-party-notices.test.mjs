// The licences shipped with the input box's editor (#354): THIRD-PARTY-NOTICES.txt
// matches the versions the lock holds, every library src/composer.ts imports
// is covered, and the file goes into the installer.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { collect, render, NOTICES, ROOTS } from "../../scripts/third-party-notices.mjs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/
?/g, "
");

test("THIRD-PARTY-NOTICES.txt is what the script writes from the lock (run it after a bump)", () => {
  assert.equal(readFileSync(NOTICES, "utf8").replace(/\r\n?/g, "\n"), render(collect()));
});

test("every package src/composer.ts imports is a root of the notices and a dependency of the app", () => {
  const imported = [...read("../../src/composer.ts").matchAll(/^import .* from "([^".][^"]*)";$/gm)].map(([, spec]) => {
    const parts = spec.split("/");
    return spec.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
  });
  assert.ok(imported.length >= 10, "the imports were read");
  const dependencies = JSON.parse(read("../../package.json")).dependencies;
  for (const name of new Set(imported)) {
    assert.ok(ROOTS.includes(name), `${name} is imported but not in ROOTS`);
    assert.ok(name in dependencies, `${name} is imported but not a dependency`);
  }
});

test("the installer carries the notices", () => {
  const resources = JSON.parse(read("../../src-tauri/tauri.conf.json")).bundle.resources;
  assert.equal(resources["../THIRD-PARTY-NOTICES.txt"], "THIRD-PARTY-NOTICES.txt");
  assert.equal(resources["../NOTICE.txt"], "NOTICE.txt");
});
