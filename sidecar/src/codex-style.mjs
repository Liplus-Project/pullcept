// Model-free metadata only. The installed Li+ loader owns style parsing/delivery.
import {execFile} from "node:child_process";
import {readFile, realpath, lstat} from "node:fs/promises";
import {resolve, dirname, basename, relative, isAbsolute} from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const FAILURE = "Codex のキャラクターを確認できません。Python 3.11 以上、project の output style と専用 SessionStart hook の登録・信頼を確認してください。";
const REASONS = {
  invalid_selection:"キャラクター名は拡張子なしの ASCII 英数字・_・- で指定してください。",
  outside_project:"style 又は helper の参照先が project の境界外にあります。",
  invalid_project:"作業フォルダーが選択 project に属していません。",
  helper_root_mismatch:"project の installed Li+ helper が一致しません。",
  unclosed_frontmatter:"キャラファイルの frontmatter が閉じていません。",
  ambiguous_frontmatter_name:"キャラファイルの frontmatter name を一つにしてください。",
  frontmatter_name_mismatch:"キャラファイルの frontmatter name がファイル名と一致しません。",
  empty_style:"キャラファイルの本文が空です。",
  style_oversize:"キャラファイルが128 KiB上限を超えています。",
  handler_oversize:"キャラ本文の配送が128 KiB上限を超えています。",
  invalid_style_or_project:"キャラファイルの欠落・UTF-8・project 設定を確認してください。",
};
class StyleFailure extends Error {}
const fail = () => { throw Error(FAILURE); };
const within = (root, cwd) => { const part = relative(root, cwd); return !isAbsolute(part) && part !== ".." && !part.startsWith("../") && !part.startsWith("..\\"); };
function run(bin, args, cwd, env) {
  return new Promise((accept, reject) => execFile(bin, args, {cwd, env, windowsHide:true, timeout:15000, maxBuffer:256 * 1024},
    (error, stdout) => {
      let value; try { value = JSON.parse(stdout); } catch { return reject(Error(FAILURE)); }
      if (error) {
        const detail = value?.protocol_version === 1 && value.status === "blocked" && Object.hasOwn(REASONS, value.reason) ? REASONS[value.reason] : "";
        return reject(detail ? new StyleFailure(`${detail} ${FAILURE}`) : Error(FAILURE));
      }
      accept(value);
    }));
}
// The installed loader's own SessionStart reply, produced the way Codex would run it.
function runHook(bin, args, cwd, env, input) {
  return new Promise((accept, reject) => {
    const child = execFile(bin, args, {cwd, env, windowsHide:true, timeout:15000, maxBuffer:512 * 1024}, (error, stdout) => {
      if (error) return reject(Error(FAILURE));
      try { accept(JSON.parse(stdout)); } catch { reject(Error(FAILURE)); }
    });
    child.stdin.end(input);
  });
}

export async function fileModeRoot(projects, cwd) {
  if (!Array.isArray(projects)) fail();
  const effective = await realpath(cwd);
  // config/read lists highest-precedence project layers first, including nested ones.
  for (const layer of projects) {
    if (layer.disabled != null) continue;
    if (typeof layer.folder !== "string" || basename(layer.folder) !== ".codex") fail();
    const root = await realpath(dirname(layer.folder));
    if (!within(root, effective)) fail();
    if (Object.hasOwn(layer, "outputStyle")) return root;
    const styles = await lstat(resolve(layer.folder, "output-styles")).catch(error => {
      if (error.code === "ENOENT") return null;
      fail();
    });
    if (styles) return root;
  }
  return null;
}

// `withBody` (#299): also the character text the loader would deliver, for a seat
// that hands it to app-server as developerInstructions with that hook turned off.
export async function resolveOutputStyle(discovery, cwd, selected, env = process.env, withBody = false) {
  const root = await fileModeRoot(discovery.projects, cwd);
  if (root === null) return {mode:"legacy", name:selected || null};
  const helper = resolve(root, ".codex/hooks/codex-output-style.py");
  if (!within(root, await realpath(helper))) fail();
  const expectedHooks = await realpath(resolve(root, ".codex/hooks.json"));
  if (!within(root, expectedHooks)) fail();
  const commands = [root, root.replaceAll("\\", "/")].map(path =>
    `${process.platform === "win32" ? "python" : "python3"} "${path}/.codex/hooks/codex-output-style.py" hook --root "${path}"`);
  const hooks = [];
  for (const hook of discovery.styleHooks ?? []) {
    if (hook.enabled !== true) continue;
    if (hook.source !== "project" || hook.async !== false || hook.additionalContextLimit !== 0 || hook.trust !== "trusted" ||
        await realpath(hook.path).catch(() => null) !== expectedHooks || !commands.includes(hook.command)) fail();
    hooks.push(hook);
  }
  const matchers = ["startup", "resume", "clear", "compact"];
  for (const matcher of matchers) {
    const matches = hooks.filter(hook => hook.matcher === "startup|resume|clear|compact" || hook.matcher === matcher);
    if (matches.length !== 1) fail();
  }
  // No shell, no body/config on argv, no inherited account override.
  const childEnv = {...env, PYTHONUTF8:"1", PYTHONIOENCODING:"utf-8"};
  for (const key of Object.keys(childEnv)) if (key.toUpperCase() === "LI_PLUS_OUTPUT_STYLE") delete childEnv[key];
  const args = [helper, "resolve", "--cwd", cwd, "--root", root];
  if (selected) args.push("--style", selected);
  const metadata = await run(process.platform === "win32" ? "python" : "python3", args, cwd, childEnv);
  if (metadata.protocol_version !== 1 || !["file", "disabled"].includes(metadata.mode) || metadata.root !== root ||
      !Number.isInteger(metadata.byte_count) || metadata.byte_count < 0 || metadata.byte_count > 128 * 1024 ||
      (metadata.mode === "file" && (typeof metadata.name !== "string" || !/^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(metadata.name) ||
        !/^[a-f0-9]{64}$/.test(metadata.sha256) || metadata.byte_count === 0 || (selected && metadata.name !== selected))) ||
      (metadata.mode === "disabled" && (selected || metadata.name !== null || metadata.sha256 !== null || metadata.byte_count !== 0))) fail();
  const handler = metadata.handler;
  if (!handler || handler.additional_context_limit !== 0 || handler.path !== await realpath(helper) ||
      JSON.stringify(handler.matchers) !== JSON.stringify(["startup", "resume", "clear", "compact"]) ||
      !within(root, await realpath(helper)) ||
      createHash("sha256").update(await readFile(helper)).digest("hex") !== handler.sha256) fail();
  const result = {protocol_version:1, mode:metadata.mode, name:metadata.name, root:metadata.root,
    sha256:metadata.sha256, byte_count:metadata.byte_count};
  if (!withBody) return result;
  result.hook_keys = hooks.map(hook => hook.key);
  if (result.hook_keys.some(key => typeof key !== "string" || key === "")) fail();
  result.body = null;
  if (metadata.mode === "file") {
    // The hook path itself: same helper, same selection, same bytes.
    const reply = await runHook(process.platform === "win32" ? "python" : "python3", [helper, "hook", "--root", root], cwd,
      {...childEnv, LI_PLUS_OUTPUT_STYLE:metadata.name},
      JSON.stringify({hook_event_name:"SessionStart", source:"startup", cwd}));
    const body = reply?.hookSpecificOutput?.additionalContext;
    if (typeof body !== "string" || reply.hookSpecificOutput.hookEventName !== "SessionStart" ||
        Buffer.byteLength(body, "utf8") !== metadata.byte_count ||
        createHash("sha256").update(body, "utf8").digest("hex") !== metadata.sha256) fail();
    result.body = body;
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [bin, cwd, options, effective, selected = "", body = ""] = process.argv.slice(2);
    const env = {...process.env};
    for (const key of Object.keys(env)) if (key.toUpperCase() === "LI_PLUS_OUTPUT_STYLE") delete env[key];
    const discovery = await run(process.execPath, [resolve(dirname(process.argv[1]), "codex-project.mjs"), bin, cwd, options], cwd, env);
    if (discovery.error) fail();
    process.stdout.write(JSON.stringify(await resolveOutputStyle(discovery, effective, selected, env, body === "body")));
  } catch (error) {
    process.stdout.write(JSON.stringify({error:error instanceof StyleFailure ? error.message : FAILURE}));
    process.exitCode = 1;
  }
}
