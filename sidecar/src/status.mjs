#!/usr/bin/env node
/**
 * Pullcept status-line reporter
 *
 * Claude Code runs this once per session start and again whenever an assistant
 * message arrives, hands it the session's own JSON on stdin, and draws whatever
 * it prints (Claude Code docs, `statusline`, read 2026-09-17; the trigger list
 * is not measured on a live CLI). What it does with that JSON is send it to the
 * seat's address on the room's port and print one line.
 *
 *   CLI -> this -> app : the JSON as it arrived, POSTed whole (#155, decision 3)
 *   CLI <- this        : one line for the bar at the bottom of the terminal
 *
 * Plain `.mjs` run by `node` directly, not TypeScript through the sidecar's
 * runner. Two words on the launch line instead of three, and every word on that
 * line is a word three shells have to leave alone (`mcp-config`,
 * `line_safe_word`).
 *
 * **The whole body goes over, and the app reads fields out of it.** That is a
 * departure from the usage-limit hook beside it, which reads nothing of what
 * the CLI sends (#149): there the signal was the arrival, and here the values
 * are the point. What it costs is what that decision named — the field names
 * are the CLI's, so a CLI that renames one takes a row of the panel with it.
 * Nothing of the terminal's own output is read either way (#82).
 *
 * **Nothing here may fail loudly.** A status-line script that throws leaves the
 * bar empty and the person looking at the terminal with no idea why, so every
 * step that can fail is answered by printing the line anyway. The app being
 * closed is the ordinary case of that, not an error.
 *
 * Run by hand:
 *   echo {} | node sidecar/src/status.mjs http://127.0.0.1:1234/hooks/status/r/a
 */

/**
 * How long the POST may take before the line is printed without it.
 *
 * The CLI cancels an in-flight script when the next update triggers, so a slow
 * send is a bar that stays blank. Loopback against a listener that is either
 * there or refusing at once does not need more than this.
 */
const SEND_TIMEOUT_MS = 2000;

/**
 * The most of the CLI's JSON this forwards.
 *
 * The session JSON is a few kilobytes. The cap is here because stdin is read
 * whole before anything is sent, and an unbounded read is a promise about a
 * stream this process does not own.
 */
const INPUT_MAX = 1024 * 1024;

/** Read stdin to the end, or to `INPUT_MAX`, whichever comes first. */
async function readInput() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > INPUT_MAX) break;
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

/** A percentage as the panel and the bar both show it: whole numbers. */
function percent(value) {
  return typeof value === "number" && Number.isFinite(value)
    ? `${Math.round(value)}%`
    : null;
}

/**
 * The one line this prints.
 *
 * The same five values the account panel shows, in the order the panel lists
 * them. A value the CLI did not send is left out rather than shown as a blank:
 * `rate_limits` is absent for an account that is not on a claude.ai plan, and
 * `effort` is absent on a model with no effort parameter (Claude Code docs,
 * `statusline`, read 2026-09-17), and a bar reading `5h —%` would be saying
 * something about a limit that does not apply.
 */
function statusLine(data) {
  const parts = [];
  const model = data?.model?.display_name ?? data?.model?.id;
  if (typeof model === "string" && model !== "") parts.push(model);
  const effort = data?.effort?.level;
  if (typeof effort === "string" && effort !== "") parts.push(effort);
  const five = percent(data?.rate_limits?.five_hour?.used_percentage);
  if (five) parts.push(`5h ${five}`);
  const seven = percent(data?.rate_limits?.seven_day?.used_percentage);
  if (seven) parts.push(`7d ${seven}`);
  const context = percent(data?.context_window?.used_percentage);
  if (context) parts.push(`ctx ${context}`);
  // Never empty. A bar with nothing in it reads as a status line that is
  // broken, and the name says which thing put it there.
  return parts.length > 0 ? parts.join(" · ") : "pullcept";
}

/**
 * Hand the JSON to the app.
 *
 * The token is read from the environment the launch set on the CLI, which this
 * process inherits — the launch line is drawn on screen and the token is not a
 * thing to draw (#149). A room that answers 401 gets no second try: the answer
 * is not read at all, because nothing here would do anything differently for
 * any of them.
 */
async function send(url, body) {
  await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${process.env.PULLCEPT_ROOM_TOKEN ?? ""}`,
    },
    body,
    signal: AbortSignal.timeout(SEND_TIMEOUT_MS),
  });
}

async function main() {
  const url = process.argv[2];
  const raw = await readInput();

  let data = null;
  try {
    data = JSON.parse(raw);
  } catch {
    // Forwarded anyway: what the app cannot read it drops, and this process
    // guessing at the shape would be a second reader of the CLI's format.
    data = null;
  }

  if (url) {
    try {
      await send(url, raw);
    } catch {
      // The app is closed, or the run it was listening for has ended. The bar
      // is still drawn — the person is looking at a terminal, not at Pullcept.
    }
  }

  process.stdout.write(`${statusLine(data)}\n`);
}

await main();
