// Writes THIRD-PARTY-NOTICES-RUST.txt (#360): the copyright notices and
// licence texts of the Rust crates built into the Windows app, at the versions
// src-tauri/Cargo.lock holds.
//
//   node scripts/rust-notices.mjs   write the file (needs cargo and the crates' sources)
//
// The crates are the normal dependencies `cargo tree -e normal --target
// x86_64-pc-windows-msvc` shows, read from `cargo metadata`. Each crate's
// licence is read from the licence files it ships in the cargo registry. A
// crate with no licence file, or with a licence outside ALLOWED, stops the run.
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = join(dirname(fileURLToPath(import.meta.url)), "..");
export const RUST_NOTICES = join(REPO, "THIRD-PARTY-NOTICES-RUST.txt");
const TARGET = "x86_64-pc-windows-msvc";

/** Permissive licences only (#354). */
const ALLOWED = new Set([
  "MIT", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Apache-2.0", "0BSD", "Unlicense",
  "Zlib", "Unicode-3.0", "Unicode-DFS-2016", "BSL-1.0", "CC0-1.0", "Apache-2.0 WITH LLVM-exception",
]);

/**
 * MPL-2.0 crates accepted by name (#360, Master 2026-10-19): file-level
 * copyleft, used unmodified, so shipping the licence text is what it asks. Any
 * other MPL crate still stops the run.
 */
const MPL = new Set(["cssparser", "cssparser-macros", "dtoa-short", "option-ext", "selectors"]);

/**
 * Crates whose published package carries no licence file, with the text kept
 * in scripts/rust-licences/: each is the licence file of the crate's own
 * repository (selectors' repository has none, so it is the MPL-2.0 text
 * cssparser ships). Named one by one, so any other crate without a licence
 * file still stops the run.
 */
const UPSTREAM = {
  "alloc-stdlib": "rust-alloc-no-stdlib",
  "clipboard-win": "clipboard-win",
  "defmt-parser": "defmt",
  selectors: "mpl-2.0",
  "unic-char-property": "rust-unic",
  "unic-char-range": "rust-unic",
  "unic-common": "rust-unic",
  "unic-ucd-ident": "rust-unic",
  "unic-ucd-version": "rust-unic",
  "webview2-com": "webview2-rs",
  "webview2-com-macros": "webview2-rs",
  "webview2-com-sys": "webview2-rs",
};

/** Whether a crate's licence expression may ship: every AND term has an allowed OR choice. */
function allowed(name, license) {
  if (typeof license !== "string") return false;
  const expr = license.replace(/[()]/g, "").replace(/\//g, " OR ");
  return expr.split(/ AND /).every((term) =>
    term.split(/ OR /).some((one) => ALLOWED.has(one.trim()) || (one.trim() === "MPL-2.0" && MPL.has(name))),
  );
}

/** The licence files a crate ships, joined; empty when none. */
function licenceFiles(dir) {
  return readdirSync(dir)
    .filter((one) => /^(licen[cs]e|copying|copyright|notice)([._-][a-z0-9._-]+)?$/i.test(one))
    .sort()
    .map((one) => readFileSync(join(dir, one), "utf8").replace(/\r\n?/g, "\n").trim())
    .join("\n\n");
}

/** Every third-party crate the app's normal dependency graph reaches, sorted. */
export function collect() {
  const meta = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET], {
      cwd: join(REPO, "src-tauri"),
      maxBuffer: 1 << 28,
      encoding: "utf8",
    }),
  );
  const packages = new Map(meta.packages.map((one) => [one.id, one]));
  const nodes = new Map(meta.resolve.nodes.map((one) => [one.id, one]));
  const found = new Set();
  const queue = [meta.resolve.root];
  while (queue.length) {
    const id = queue.shift();
    if (found.has(id)) continue;
    found.add(id);
    for (const dep of nodes.get(id).deps) {
      if (dep.dep_kinds.some((kind) => kind.kind === null)) queue.push(dep.pkg);
    }
  }
  return [...found]
    .map((id) => packages.get(id))
    .filter((one) => one.source) // path crates are Pullcept's own
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
    .map((one) => {
      if (!allowed(one.name, one.license)) throw new Error(`${one.name}@${one.version}: licence ${one.license} is not allowed`);
      const shipped = licenceFiles(dirname(one.manifest_path));
      if (shipped && Object.hasOwn(UPSTREAM, one.name)) throw new Error(`${one.name}@${one.version} now ships a licence file: drop it from UPSTREAM`);
      const text = Object.hasOwn(UPSTREAM, one.name)
        ? readFileSync(join(REPO, "scripts", "rust-licences", `${UPSTREAM[one.name]}.txt`), "utf8").replace(/\r\n?/g, "\n").trim()
        : shipped;
      if (!text) throw new Error(`${one.name}@${one.version} ships no licence file`);
      return { name: one.name, version: one.version, license: one.license, text };
    });
}

/** The file's text. */
export function render(crates) {
  const rule = "=".repeat(78);
  const head = [
    "Pullcept: third-party notices (Rust crates)",
    "",
    "Pullcept's Windows app is built from the following Rust crates (#360). Each",
    "has the version shipped, its licence, and the licence text and copyright",
    "notice it is distributed under. The MPL-2.0 crates are used unmodified; their",
    "source is available from crates.io at the version listed. Generated by",
    "scripts/rust-notices.mjs from src-tauri/Cargo.lock and the crates' own",
    "licence files.",
    "",
    ...crates.map((one) => `- ${one.name} ${one.version} (${one.license})`),
  ];
  const bodies = crates.map((one) => [rule, `${one.name} ${one.version} (${one.license})`, rule, "", one.text].join("\n"));
  return [head.join("\n"), ...bodies].join("\n\n") + "\n";
}

/** The `name version` pairs the file lists, for the test without cargo. */
export function listed(text) {
  const head = text.replace(/\r\n?/g, "\n").split(`\n\n${"=".repeat(78)}`)[0];
  return [...head.matchAll(/^- (\S+) (\S+) \(/gm)].map(([, name, version]) => `${name} ${version}`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  writeFileSync(RUST_NOTICES, render(collect()));
}
