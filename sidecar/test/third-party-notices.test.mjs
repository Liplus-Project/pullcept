// The licences shipped with the input box's editor (#354): THIRD-PARTY-NOTICES.txt
// matches the versions the lock holds, every library src/composer.ts imports
// is covered, and the file goes into the installer.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { collect, render, NOTICES, ROOTS } from "../../scripts/third-party-notices.mjs";

// LF, whatever the checkout wrote.
const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n?/g, "\n");

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


test("local QR's browser renderer and browser dependency carry notices", () => {
  const packages=collect().map(p=>p.name);
  assert.ok(ROOTS.includes("qrcode"));
  assert.ok(packages.includes("qrcode"));
  assert.ok(packages.includes("dijkstrajs"));
  assert.ok(!packages.includes("yargs"));
  assert.ok(!packages.includes("pngjs"));
});

test("the post renderer's libraries are roots of the notices and dependencies of the app (#431)", () => {
  const dependencies = JSON.parse(read("../../package.json")).dependencies;
  const imported = [...read("../../src/markdown.ts").matchAll(/(?:from |import\()"([^".][^"]*)"/g)].map(([, spec]) => spec);
  assert.deepEqual([...new Set(imported)].sort(), ["marked", "mermaid"]);
  for (const name of ["marked", "mermaid"]) {
    assert.ok(ROOTS.includes(name), `${name} is imported but not in ROOTS`);
    assert.ok(name in dependencies, `${name} is imported but not a dependency`);
  }
  const packages = collect().map(p => p.name);
  assert.ok(packages.includes("dompurify"));
  assert.ok(!packages.includes("elkjs"), "EPL-2.0 elkjs is not shipped");
  assert.ok(!packages.includes("strictdom"), "fastdom-strict is not imported");
});
