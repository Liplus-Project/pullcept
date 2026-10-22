// Pinned, model-free native fixture. It never reads a real Codex HOME.
import {existsSync} from "node:fs";
import {resolve,join} from "node:path";
import {spawnSync} from "node:child_process";
const root=resolve(".artifacts/native-cli/node_modules");
const suffix="vendor/x86_64-pc-windows-msvc/bin/codex.exe";
const native=process.env.CODEX_TEST_NATIVE ?? [join(root,"@openai/codex/node_modules/@openai/codex-win32-x64",suffix),join(root,"@openai/codex-win32-x64",suffix)].find(existsSync);
if(!native) throw new Error("Install the pinned @openai/codex@0.162.1 fixture, or set CODEX_TEST_NATIVE.");
const run=spawnSync("cargo",["test","--manifest-path","crates/codex-remote/Cargo.toml","--test","native_shared","--","--nocapture"],{stdio:"inherit",windowsHide:true,env:{...process.env,CODEX_TEST_NATIVE:native,CARGO_TERM_PROGRESS_WHEN:"never"}});
process.exitCode=run.status??1;
