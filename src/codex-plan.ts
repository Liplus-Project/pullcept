/** The account launch override, shared by its editor and the TODO projection. */
const KEY = "tools.update_plan.enabled";
const assignment = /^tools\.update_plan\.enabled\s*=\s*(.*)$/;
function overrides(args: string[]) {
  const found: { start: number; end: number; value: string }[] = [];
  for (let i = 0; i < args.length && args[i] !== "--"; i++) {
    const start = i;
    let value: string | undefined;
    if (args[i] === "-c" || args[i] === "--config") value = args[++i];
    else if (args[i].startsWith("--config=")) value = args[i].slice(9);
    else if (args[i].startsWith("-c")) value = args[i].slice(2).replace(/^=/, "");
    const match = value?.trim().match(assignment);
    if (match) found.push({ start, end: i + 1, value: match[1].trim() });
  }
  return found;
}
export function codexPlanEnabled(args: string[]): boolean {
  const found = overrides(args);
  return found[found.length - 1]?.value === "true";
}
export function withCodexPlan(args: string[], enabled: boolean): string[] {
  const removed = new Set(overrides(args).flatMap(o => Array.from({length: o.end - o.start}, (_, i) => o.start + i)));
  const kept = args.filter((_, i) => !removed.has(i));
  const end = kept.indexOf("--");
  kept.splice(end < 0 ? kept.length : end, 0, "-c", `${KEY}=${enabled}`);
  return kept;
}

/** Claude Code's task tools, switched on by one variable of the account (#551). */
export const CLAUDE_PLAN_ENV = "CLAUDE_CODE_ENABLE_TODO_TOOLS";
const claudeLine = new RegExp(String.raw`^\s*${CLAUDE_PLAN_ENV}\s*=`);
export function claudePlanInEnvText(text: string): boolean {
  return text.split(/\r?\n/).some(line => claudeLine.test(line));
}
export function withClaudePlanEnvText(text: string, enabled: boolean): string {
  const lines = text === "" ? [] : text.split(/\r?\n/).filter(line => !claudeLine.test(line));
  while (lines.length && lines[lines.length - 1].trim() === "") lines.pop();
  if (enabled) lines.push(`${CLAUDE_PLAN_ENV}=1`);
  return lines.join("\n");
}
