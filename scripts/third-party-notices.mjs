// Writes THIRD-PARTY-NOTICES.txt (#354): the copyright notices and licence
// texts of the libraries the input box's editor brings into the app, and of
// everything they depend on, at the versions package-lock.json holds.
//
//   node scripts/third-party-notices.mjs          write the file
//   node scripts/third-party-notices.mjs --check  fail when the file is stale
//
// The roots are the packages src/composer.ts, src/codex-remote.ts and src/markdown.ts (#431) import. Their dependencies are
// followed through package-lock.json, peers included, and each package's
// licence is read from the LICENSE file it ships in node_modules. A package
// with no licence file, or with a licence outside ALLOWED, stops the run: it is
// a package to look at before it is shipped, not one to list.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = join(dirname(fileURLToPath(import.meta.url)), "..");
export const NOTICES = join(REPO, "THIRD-PARTY-NOTICES.txt");

/** Browser libraries imported by composer and the local pairing screen. */
export const ROOTS = [
  "@tiptap/core",
  "@tiptap/pm",
  "@tiptap/extension-code-block",
  "@tiptap/extension-code-block-lowlight",
  "@tiptap/extension-document",
  "@tiptap/extension-paragraph",
  "@tiptap/extension-text",
  "@tiptap/extensions",
  "lowlight",
  "highlight.js",
  "qrcode",
  "marked",
  "mermaid",
];

/** Permissive licences only (#354: nothing GPL-like goes into the app). */
const ALLOWED = new Set(["MIT", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Apache-2.0", "0BSD", "Unlicense"]);

/**
 * Packages whose package.json declares no licence (#431: khroma, pulled in by
 * mermaid), with the licence their own licence file states. The file is checked
 * to name it, so a changed licence still stops the run.
 */
const UNDECLARED = { khroma: "MIT" };

/**
 * Packages that ship their licence only as the README's `## License` section
 * (#431: fastdom, pulled in by mermaid). Named one by one, so any other package
 * without a licence file still stops the run.
 */
const README_LICENCE = new Set(["fastdom"]);

/** The README's `## License` section, for a package in README_LICENCE. */
function readmeLicence(name, version, dir) {
  if (!README_LICENCE.has(name)) throw new Error(`${name}@${version} ships no licence file`);
  const readme = readFileSync(join(dir, "README.md"), "utf8").replace(/\r\n?/g, "\n");
  const section = /^#+ *Licen[cs]e *\n([\s\S]*?)(?=^#+ |(?![\s\S]))/im.exec(readme);
  if (!section || !/copyright/i.test(section[1])) throw new Error(`${name}@${version}: no licence section in README.md`);
  return section[1].trim();
}

/**
 * Whether a licence expression may ship (#431): a single allowed id, or an
 * SPDX `(A OR B)` choice with at least one allowed alternative (dompurify,
 * MPL-2.0 OR Apache-2.0, is taken under Apache-2.0). `AND` is not accepted.
 */
function allowed(license) {
  if (typeof license !== "string") return false;
  if (ALLOWED.has(license)) return true;
  const choice = /^\((.+)\)$/.exec(license);
  if (!choice || / AND /.test(choice[1])) return false;
  return choice[1].split(" OR ").some((one) => ALLOWED.has(one.trim()));
}

/** Every package the roots reach, by name, with its version and licence, sorted. */
export function collect() {
  const lock = JSON.parse(readFileSync(join(REPO, "package-lock.json"), "utf8")).packages;
  const found = new Map();
  const queue = [...ROOTS];
  while (queue.length) {
    const name = queue.shift();
    if (found.has(name)) continue;
    const entry = lock[`node_modules/${name}`];
    if (!entry) throw new Error(`${name} is not in package-lock.json at the top level`);
    found.set(name, entry);
    // qrcode's browser entry imports only dijkstrajs; pngjs and yargs are
    // server/CLI dependencies and are absent from Vite's browser bundle.
    // fastdom's strictdom is used only by fastdom-strict.js, which mermaid does not
    // import (it imports fastdom and fastdom-promised, #431).
    const dependencies =
      name === "qrcode" ? ["dijkstrajs"] : name === "fastdom" ? [] : Object.keys(entry.dependencies ?? {});
    queue.push(...dependencies, ...Object.keys(entry.peerDependencies ?? {}));
  }
  return [...found.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, entry]) => {
      const license = entry.license ?? UNDECLARED[name];
      if (!allowed(license)) throw new Error(`${name}@${entry.version}: licence ${license} is not allowed`);
      const dir = join(REPO, "node_modules", name);
      const installed = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version;
      if (installed !== entry.version) throw new Error(`${name}: node_modules has ${installed}, the lock ${entry.version}`);
      const file = readdirSync(dir).find((one) => /^(licen[cs]e|copying)(\.(md|txt))?$/i.test(one));
      const text = file
        ? readFileSync(join(dir, file), "utf8").replace(/\r\n?/g, "\n").trim()
        : readmeLicence(name, entry.version, dir);
      if (!entry.license && !text.includes(`${license} License`)) throw new Error(`${name}@${entry.version}: licence file is not ${license}`);
      return { name, version: entry.version, license, text };
    });
}

/** The file's text. */
export function render(packages) {
  const rule = "=".repeat(78);
  const head = [
    "Pullcept: third-party notices",
    "",
    "Pullcept includes the following libraries in its window (the input box's",
    "editor, code colouring, local pairing QR and post Markdown / diagrams, #354 /",
    "#419 / #431). Each has the version",
    "shipped, its licence, and the licence text and copyright notice it is",
    "distributed under. Generated by scripts/third-party-notices.mjs from",
    "package-lock.json and the packages' own licence files.",
    "",
    ...packages.map((one) => `- ${one.name} ${one.version} (${one.license})`),
  ];
  const bodies = packages.map((one) => [rule, `${one.name} ${one.version} (${one.license})`, rule, "", one.text].join("\n"));
  return [head.join("\n"), ...bodies].join("\n\n") + "\n";
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const text = render(collect());
  if (process.argv.includes("--check")) {
    const current = readFileSync(NOTICES, "utf8").replace(/\r\n?/g, "\n");
    if (current !== text) {
      console.error("THIRD-PARTY-NOTICES.txt is stale: run node scripts/third-party-notices.mjs");
      process.exit(1);
    }
  } else {
    writeFileSync(NOTICES, text);
  }
}
