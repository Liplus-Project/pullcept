// Native, model-free runtime config resolution, including profile-v2 and trust.
import {spawn, execFileSync} from "node:child_process";
import {resolve, isAbsolute} from "node:path";
import {randomUUID} from "node:crypto";
import {pathToFileURL} from "node:url";

const LIMIT = 8 * 1024 * 1024;
const FAILURE = "Codex の有効指示を確認できません。診断端末で CLI の設定・profile・信頼を確認してください。";
function stop(child) {
  if (process.platform === "win32" && child.pid) {
    try { execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {stdio:"ignore", windowsHide:true}); }
    catch { child.kill(); }
  } else child.kill();
}

// Keep only options affecting runtime configuration; positional resume IDs and
// interactive UI switches have no meaning for debug prompt-input. Their actual
// launch argv is untouched. Values are consumed before interpreting the next flag.
export function promptOptions(options) {
  const result = [];
  const valued = new Set(["-c", "--config", "-p", "--profile", "-C", "--cd", "--enable", "--disable",
    "-m", "--model", "-a", "--ask-for-approval", "-s", "--sandbox", "--add-dir", "-i", "--image"]);
  const kept = new Set(["-c", "--config", "-p", "--profile", "-C", "--cd", "--enable", "--disable"]);
  for (let i = 0; i < options.length; i++) {
    const arg = options[i], key = arg.split("=", 1)[0];
    if (/^-(?:p|C|c)./.test(arg)) throw Error(FAILURE);
    if (valued.has(key)) {
      if (arg.includes("=")) { if (kept.has(key)) result.push(arg); }
      else {
        const value = options[++i];
        if (value === undefined) throw Error(FAILURE);
        if (kept.has(key)) result.push(arg, value);
      }
    }
  }
  return result;
}

function readPrompt(rawBin, cwd, options) {
  const bin = /[\\/]/.test(rawBin) && !isAbsolute(rawBin) ? resolve(cwd, rawBin) : rawBin;
  const shell = process.platform === "win32" && !bin.toLowerCase().endsWith(".exe");
  const words = [...options, "debug", "prompt-input"];
  // Rust encodes TOML strings before this point. Quotes are only TOML syntax;
  // expansion/metacharacters and physical newlines cannot cross the cmd shim.
  if (shell && [bin, ...words].some(word => /[&|<>^()%\r\n]/.test(word))) return Promise.reject(Error(FAILURE));
  return new Promise((accept, reject) => {
    const child = spawn(shell ? `"${bin}"` : bin, shell ? words.map(w => `"${w.replaceAll('"', '\\"')}"`) : words,
      {cwd, env:process.env, stdio:["ignore", "pipe", "ignore"], windowsHide:true, shell});
    let chunks = [], size = 0, ended = false;
    const fail = () => { if (ended) return; ended = true; clearTimeout(timer); stop(child); reject(Error(FAILURE)); };
    const timer = setTimeout(fail, 15000);
    child.stdout.on("data", chunk => { size += chunk.length; if (size > LIMIT) fail(); else chunks.push(chunk); });
    child.once("error", fail);
    child.once("close", code => {
      if (ended) return;
      ended = true; clearTimeout(timer);
      if (code !== 0) return reject(Error(FAILURE));
      try { accept(JSON.parse(Buffer.concat(chunks).toString("utf8"))); } catch { reject(Error(FAILURE)); }
    });
  });
}

// IDs and message grouping may differ between invocations. All model-visible
// role/content pairs must match except one replacement (or absent instructions).
function parts(prompt) {
  if (!Array.isArray(prompt)) throw Error(FAILURE);
  return prompt.flatMap(message => {
    if (message.type !== "message" || !Array.isArray(message.content)) throw Error(FAILURE);
    return message.content.map(content => ({role:message.role, ...content}));
  });
}
export function extractInstructions(original, reference, sentinel) {
  const before = parts(original), after = parts(reference);
  const indices = after.flatMap((part, index) => part.role === "developer" && part.type === "input_text" && part.text === sentinel ? [index] : []);
  if (indices.length !== 1) throw Error(FAILURE);
  const index = indices[0], same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  if (before.length === after.length - 1 && before.every((part, i) => same(part, after[i < index ? i : i + 1]))) return null;
  if (before.length !== after.length || !before.every((part, i) => i === index || same(part, after[i])) ||
      before[index].role !== "developer" || before[index].type !== "input_text" || typeof before[index].text !== "string") throw Error(FAILURE);
  return before[index].text;
}
export async function discoverInstructions(bin, cwd, options) {
  const args = promptOptions(options), sentinel = `pullcept-instructions-${randomUUID()}`;
  const original = await readPrompt(bin, cwd, args);
  const reference = await readPrompt(bin, cwd, [...args, "-c", `developer_instructions='${sentinel}'`]);
  return extractInstructions(original, reference, sentinel);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [bin, cwd, options] = process.argv.slice(2);
    const instructions = await discoverInstructions(bin, cwd, JSON.parse(options));
    process.stdout.write(JSON.stringify({instructions}));
  } catch {
    process.stdout.write(JSON.stringify({error:FAILURE}));
    process.exitCode = 1;
  }
}
