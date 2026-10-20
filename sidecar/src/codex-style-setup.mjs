// Explicit account on-save only. No thread/model request or CLI-wide hook install.
import {spawn, execFileSync} from "node:child_process";
import {createInterface} from "node:readline";
import {readFile, realpath, lstat} from "node:fs/promises";
import {resolve, join, isAbsolute} from "node:path";
import {createHash} from "node:crypto";
import {pathToFileURL} from "node:url";

const digest = bytes => createHash("sha256").update(bytes).digest("hex");
const spelling = path => process.platform === "win32" ? path.replaceAll("/", "\\").replace(/^\\\\\?\\/, "").replace(/\\+$/, "").toLowerCase() : path;
async function verifyBundled(root, expectedHash) {
  const helper = join(root, ".codex/hooks/codex-output-style.py");
  const stat = await lstat(helper);
  if (!stat.isFile() || stat.isSymbolicLink() || spelling(await realpath(helper)) !== spelling(helper) || digest(await readFile(helper)) !== expectedHash)
    throw Error("同梱版と異なる読み込みスクリプトです。承認を中止しました。");
}
export function projectTrustEdits(config, root) {
  const matches = Object.keys(config.projects ?? {}).filter(key => spelling(key) === spelling(root));
  const edits = [];
  const nativeKey = key => process.platform === "win32" ? key.toLowerCase() : key;
  const compatible = matches.filter(key => nativeKey(key) === nativeKey(root));
  const moves = matches.filter(key => nativeKey(key) !== nativeKey(root));
  if (moves.length) {
    const target = compatible[0] ?? root;
    const value = {...config.projects?.[target], trust_level:"trusted"};
    for (const key of moves) {
      for (const [field, data] of Object.entries(config.projects[key])) {
        if (field === "trust_level") continue;
        if (Object.hasOwn(value, field) && JSON.stringify(value[field]) !== JSON.stringify(data))
          throw Error("同じフォルダーの projects 設定が競合しています。既存設定を保つため中止しました。");
        value[field] = data;
      }
      edits.push({keyPath:`projects.${JSON.stringify(key)}`, value:null, mergeStrategy:"replace"});
    }
    edits.push({keyPath:`projects.${JSON.stringify(target)}`, value, mergeStrategy:"upsert"});
  }
  for (const key of (compatible.length ? compatible : moves.length ? [] : [root])) {
    if (config.projects?.[key]?.trust_level !== "trusted")
      edits.push({keyPath:`projects.${JSON.stringify(key)}.trust_level`, value:"trusted", mergeStrategy:"upsert"});
  }
  return edits;
}

export async function verifyOwnHook(hooks, root, homeScope, expectedHash) {
  await verifyBundled(root, expectedHash);
  const path = root.replaceAll("\\", "/");
  const command = `${process.platform === "win32" ? "python" : "python3"} "${path}/.codex/hooks/codex-output-style.py" hook --root "${path}"`;
  const matches = hooks.filter(h => h.command?.includes("codex-output-style.py"));
  if (matches.length !== 1) throw Error("専用 hook が不足または競合しています。承認を中止しました。");
  const hook = matches[0];
  const correctPath = homeScope
    ? (process.platform === "win32" ? /^[A-Za-z]:\\<session-flags>\\config\.toml$/.test(hook.sourcePath) : hook.sourcePath === "/<session-flags>/config.toml")
    : spelling(hook.sourcePath ?? "") === spelling(await realpath(join(root, ".codex/hooks.json")));
  if (!correctPath || hook.source !== (homeScope ? "sessionFlags" : "project") || hook.command !== command ||
      hook.matcher !== "startup|resume|clear|compact" || hook.async !== false || hook.additionalContextLimit !== 0 || hook.enabled !== true ||
      typeof hook.currentHash !== "string" || !/^sha256:[a-f0-9]{64}$/.test(hook.currentHash) ||
      !hook.key?.startsWith(`${hook.sourcePath}:session_start:`) ||
      (homeScope && hook.key !== `${hook.sourcePath}:session_start:0:0`))
    throw Error("専用 hook の登録・command・native hash が一致しません。承認を中止しました。");
  return hook;
}

export async function approveStyle(bin, cwd, options, root, expectedHash, homeScope) {
  if (/[\\/]/.test(bin) && !isAbsolute(bin)) bin = resolve(cwd, bin);
  const shell = process.platform === "win32" && !bin.toLowerCase().endsWith(".exe");
  const configOptions = [];
  for (let i = 0; i < options.length; i++) {
    const arg = options[i];
    if (["-c", "--config", "--enable", "--disable"].includes(arg)) configOptions.push(arg, options[++i]);
    else if (["--config=", "--enable=", "--disable="].some(prefix => arg.startsWith(prefix))) configOptions.push(arg);
    else if (["-C", "--cd", "-m", "--model", "-a", "--ask-for-approval", "-s", "--sandbox", "--add-dir", "-i", "--image"].includes(arg)) i++;
  }
  const words = [...configOptions, "app-server", "--listen", "stdio://"];
  if (shell && [bin, ...words].some(word => /[&|<>^()%\r\n]/.test(word))) throw Error("Unsupported Windows command characters");
  const child = spawn(shell ? `"${bin}"` : bin, shell ? words.map(word => `"${word.replaceAll('"','\\"')}"`) : words,
    {cwd, env:process.env, windowsHide:true, shell, stdio:["pipe", "pipe", "ignore"]});
  const closed = new Promise(accept => child.once("close", accept));
  const lines = createInterface({input:child.stdout});
  let id = 0;
  const pending = new Map();
  const rejectAll = error => { for (const {reject} of pending.values()) reject(error); pending.clear(); };
  child.on("error", () => rejectAll(Error("Codex の設定 API を起動できません。")));
  child.on("exit", () => rejectAll(Error("Codex の設定 API が終了しました。")));
  lines.on("line", line => {
    let reply; try { reply = JSON.parse(line); } catch { return; }
    const request = pending.get(reply.id); if (!request) return;
    pending.delete(reply.id);
    if (reply.error) request.reject(Error(`Codex ${request.method}: ${reply.error.message}`)); else request.accept(reply.result);
  });
  const rpc = (method, params) => new Promise((accept, reject) => {
    const key = ++id; pending.set(key, {accept, reject, method});
    child.stdin.write(JSON.stringify({id:key, method, params}) + "\n");
  });
  const stop = () => {
    if (process.platform === "win32" && child.pid) { try { execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {stdio:"ignore", windowsHide:true}); } catch { child.kill(); } }
    else child.kill();
  };
  const timeout = setTimeout(() => { rejectAll(Error("Codex の設定 API が時間内に応答しませんでした。")); stop(); }, 15000);
  child.stdin.on("error", () => rejectAll(Error("Codex の設定 API に接続できません。")));
  const read = () => rpc("config/read", {cwd, includeLayers:true});
  // The typed effective config omits unknown project fields. The native user
  // layer retains them, and is the only file this operation is authorized to edit.
  const userLayer = snapshot => snapshot.layers?.find(layer => (layer.name ?? layer.source)?.type === "user");
  const write = async (snapshot, edits) => {
    if (!edits.length) return;
    const user = userLayer(snapshot);
    if (!user?.version) throw Error("Codex の user 設定 version を確認できません。");
    await rpc("config/batchWrite", {edits, filePath:null, expectedVersion:user.version, reloadUserConfig:true});
  };
  const list = async () => {
    const result = await rpc("hooks/list", {cwds:[cwd]});
    if (result.data?.some(row => row.errors?.length)) throw Error("Codex の hook 読み込みが失敗しました。");
    return (result.data ?? []).flatMap(row => row.hooks ?? []).filter(hook => hook.eventName === "sessionStart");
  };
  try {
    const init = await rpc("initialize", {clientInfo:{name:"pullcept_style_setup", version:"1"}, capabilities:{experimentalApi:true}});
    if (spelling(await realpath(init.codexHome)) !== spelling(await realpath(process.env.CODEX_HOME))) throw Error("Codex home が起動設定と一致しません。");
    child.stdin.write(JSON.stringify({method:"initialized"}) + "\n");
    let config = await read();
    await verifyBundled(root, expectedHash);
    await write(config, projectTrustEdits(userLayer(config)?.config ?? {}, root));
    config = await read();
    const hook = await verifyOwnHook(await list(), root, homeScope, expectedHash);
    // Re-read both filesystem bytes and native metadata immediately before the
    // versioned trust edit; never approve a changed script with a previous hash.
    const current = await verifyOwnHook(await list(), root, homeScope, expectedHash);
    if (current.key !== hook.key || current.currentHash !== hook.currentHash) throw Error("承認中に専用 hook が変更されました。");
    const prior = userLayer(config)?.config?.hooks?.state?.[current.key];
    if (homeScope && prior?.trusted_hash && prior.trusted_hash !== current.currentHash)
      throw Error("process 限定 hook のキーに別の承認が存在します。既存の承認を保つため中止しました。");
    if (current.trustStatus !== "trusted") {
      await write(config, [{keyPath:`hooks.state.${JSON.stringify(current.key)}.trusted_hash`, value:current.currentHash, mergeStrategy:"upsert"}]);
    }
    const approved = await verifyOwnHook(await list(), root, homeScope, expectedHash);
    if (approved.currentHash !== current.currentHash || approved.trustStatus !== "trusted") throw Error("専用 hook の承認を確認できません。");
    const after = await read();
    if (projectTrustEdits(userLayer(after)?.config ?? {}, root).length) throw Error("作業フォルダーの信頼を確認できません。");
    return {approved:true, source:approved.source, key:approved.key};
  } finally {
    clearTimeout(timeout); lines.close(); child.stdin.end();
    const kill = setTimeout(stop, 1000);
    await closed;
    clearTimeout(kill);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [bin, cwd, rawOptions, root, expectedHash, scope] = process.argv.slice(2);
    process.stdout.write(JSON.stringify(await approveStyle(bin, cwd, JSON.parse(rawOptions), root, expectedHash, scope === "home")));
  } catch (error) { process.stdout.write(JSON.stringify({error:error.message})); process.exitCode = 1; }
}
