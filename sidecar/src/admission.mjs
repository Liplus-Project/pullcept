// Pre-initialize launcher admission (#315); no credentials enter diagnostics.
export const ADMISSION_WAIT_MS = 3_000;
export const ADMISSION_INTERVAL_MS = 50;
export const ADMISSION_PATH = "/room/sidecar-admission";

export async function admitSidecar({ roomUrl, token, claim }, {
  request = fetch, now = Date.now, sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
} = {}) {
  if (!token || !claim.launch_id || !claim.instance_id || !claim.account_id || !claim.room_id) return false;
  let url;
  try {
    url = new URL(roomUrl);
    if (url.protocol !== "ws:" && url.protocol !== "wss:") return false;
    url.protocol = url.protocol === "ws:" ? "http:" : "https:";
    url.pathname = ADMISSION_PATH; url.search = ""; url.hash = "";
  } catch { return false; }
  const deadline = now() + ADMISSION_WAIT_MS;
  while (now() < deadline) {
    try {
      const response = await request(url, {
        method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
        body: JSON.stringify(claim), signal: AbortSignal.timeout(Math.max(1, deadline - now())),
      });
      // Drain before retrying so the listener can close the finished request.
      await response.text();
      if (now() > deadline) return false;
      if (response.status === 200) return true;
      if (response.status !== 425) return false;
    } catch { return false; }
    const remaining = deadline - now();
    if (remaining <= 0) return false;
    await sleep(Math.min(ADMISSION_INTERVAL_MS, remaining));
  }
  return false;
}
