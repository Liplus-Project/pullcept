// SessionStart is authoritative; inherited CODEX_THREAD_ID is deliberately unused.
const env = process.env;
const url = env.PULLCEPT_NATIVE_URL;
if (url && env.PULLCEPT_ROOM_TOKEN && env.PULLCEPT_LAUNCH_ID && env.PULLCEPT_LAUNCHED_AS && env.PULLCEPT_LAUNCHED_ROOM) {
  let target;
  try { target = new URL(url); } catch { process.exit(0); }
  if (target.protocol === "http:" && target.hostname === "127.0.0.1" && target.pathname === "/hooks/codex-session") {
    let raw = "";
    const timer = setTimeout(() => process.exit(0), 8000);
    try {
      for await (const chunk of process.stdin) {
        raw += chunk;
        if (raw.length > 65536) throw new Error("oversize input");
      }
      const event = JSON.parse(raw);
      if (event.hook_event_name === "SessionStart" && /^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(event.session_id ?? "")) {
        const body = JSON.stringify({
          session_id: event.session_id, hook_event_name: event.hook_event_name,
          launch_id: env.PULLCEPT_LAUNCH_ID, account_id: env.PULLCEPT_LAUNCHED_AS,
          room_id: env.PULLCEPT_LAUNCHED_ROOM,
        });
        for (let retry = 0; retry < 4; retry++) {
          const result = await fetch(url, { method: "POST", headers: {
            "Content-Type": "application/json", Authorization: `Bearer ${env.PULLCEPT_ROOM_TOKEN}`,
          }, body, signal: AbortSignal.timeout(1500) });
          if (result.status !== 409) break;
          await new Promise(resolve => setTimeout(resolve, 200));
        }
      }
    } catch { /* Offline, malformed or stale: no model turn and no credential logging. */ }
    finally { clearTimeout(timer); }
  }
}
