import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";

interface Pairing { pairingCode: string; manualPairingCode: string | null; expiresAt: number }
interface Reply { epoch: string; status: string | null; error: string | null; pairing: Pairing | null; reason?: string | null }

/** The relay refused this seat with 409 "already online" (#430). The 409 does
 * not say who holds it, so the desktop app is only an example of where to look. */
const RELAY_IN_USE = "別の接続が中継を使用中です。Codex デスクトップアプリなど、同じ中継を使っていそうなものを閉じて、少し待ってください。席は再接続を続けており、つながると表示が変わります。";

/** Private, transient screen. No storage, room event, URL navigation or logging. */
export function openCodexRemote(ptyId: string, name: string, isLive: () => boolean,
  deps = { invoke, renderQr: QRCode.toCanvas }): void {
  const dialog = document.createElement("dialog");
  dialog.className = "codex-remote-dialog";
  const heading = document.createElement("h2");
  heading.textContent = `${name} — スマホから接続`;
  const description = document.createElement("p");
  description.textContent = "公式アプリから、この席の会話を続けられます。ペアリングの QR やコードは他の人に渡さないでください。";
  const state = document.createElement("p");
  state.setAttribute("role", "status");
  const error = document.createElement("p");
  error.setAttribute("role", "alert");
  const privateArea = document.createElement("div");
  const buttons = document.createElement("div");
  buttons.className = "codex-remote-actions";
  const button = (text: string, act: () => void) => {
    const b = document.createElement("button"); b.type = "button"; b.textContent = text;
    b.addEventListener("click", act); buttons.append(b); return b;
  };
  let epoch: string | null = null;
  let alive = true;
  let busy = false;
  let generation = 0;
  let expiresAt = 0;
  let timer: ReturnType<typeof setInterval> | undefined;
  let pollAt = 0;
  let currentStatus: string | null = null;
  const clearPrivate = () => {
    generation += 1;
    expiresAt = 0;
    for (const canvas of privateArea.querySelectorAll("canvas")) { canvas.width = 0; canvas.height = 0; }
    privateArea.replaceChildren();
  };
  const refreshButtons = () => {
    enable.disabled = busy || !epoch;
    disable.disabled = busy || !epoch;
    pair.disabled = busy || !epoch || currentStatus !== "connected";
  };
  const call = async (action: "status" | "enable" | "disable" | "pair") => {
    if (!alive || busy) return;
    if (!isLive()) { ended(); return; }
    busy = true; refreshButtons();
    if (action !== "status") { clearPrivate(); error.textContent = ""; }
    const requestGeneration = generation;
    try {
      const reply = await deps.invoke<Reply>("codex_remote_control", { ptyId, epoch, action });
      if (!alive || !isLive()) { if (alive) ended(); return; }
      if (epoch && epoch !== reply.epoch) { ended(); return; }
      epoch = reply.epoch;
      error.textContent = reply.error ?? "";
      if (reply.status) {
        currentStatus = reply.status;
        state.textContent = reply.reason === "relay_in_use" && (reply.status === "errored" || reply.status === "connecting") ? RELAY_IN_USE : ({ disabled: "接続は停止中", connecting: "接続中…", connected: "接続済み", errored: "接続に失敗しました。アカウントの許可やネットワークを確認し、接続を開始し直してください。" } as Record<string,string>)[reply.status] ?? "状態を確認できません";
        if (reply.status === "disabled" || reply.status === "errored") clearPrivate();
      }
      if (reply.pairing && generation === requestGeneration && !document.hidden) {
        // Keep no copy of the codes after rendering. The QR canvas and text
        // are erased on every close, expiry, hidden screen and terminal exit.
        const pairData = reply.pairing;
        expiresAt = pairData.expiresAt;
        if (expiresAt * 1000 <= Date.now()) { clearPrivate(); error.textContent = "期限が切れました。ペアリングを更新してください。"; return; }
        const canvas = document.createElement("canvas");
        canvas.setAttribute("aria-label", "公式アプリで読み取るペアリング QR");
        const code = document.createElement("p");
        code.textContent = pairData.manualPairingCode ? `手入力コード: ${pairData.manualPairingCode}` : "手入力コードは発行されませんでした。QR を使用してください。";
        const expiry = document.createElement("p");
        expiry.textContent = `有効期限: ${new Date(expiresAt * 1000).toLocaleString()}`;
        privateArea.append(canvas, code, expiry);
        const url = new URL("https://chatgpt.com/codex/pair");
        url.searchParams.set("pairing_code", pairData.pairingCode);
        await deps.renderQr(canvas, url.toString(), { width: 280, margin: 4, errorCorrectionLevel: "M" });
        if (!alive || generation !== requestGeneration || document.hidden) { canvas.width=0; canvas.height=0; canvas.remove(); }
      }
    } catch (reason) {
      if (!alive) return;
      clearPrivate();
      error.textContent = typeof reason === "string" ? reason : "接続画面の操作を完了できませんでした。再試行できます。";
    } finally {
      busy = false;
      if (alive) refreshButtons();
    }
  };
  const enable = button("接続を開始", () => void call("enable"));
  const disable = button("接続を停止", () => void call("disable"));
  const pair = button("ペアリングを更新", () => void call("pair"));
  button("閉じる", () => dialog.close());
  const hidden = () => { if (document.hidden) clearPrivate(); };
  const dispose = () => {
    alive = false; clearPrivate();
    if (timer) clearInterval(timer);
    document.removeEventListener("visibilitychange", hidden);
    dialog.remove();
  };
  const ended = () => {
    clearPrivate(); epoch = null;
    state.textContent = "この席は終了しました。新しい席から開き直してください。";
    enable.disabled = disable.disabled = pair.disabled = true;
    if (timer) clearInterval(timer);
  };
  dialog.addEventListener("close", dispose, { once: true });
  document.addEventListener("visibilitychange", hidden);
  dialog.append(heading, description, state, error, privateArea, buttons);
  document.body.append(dialog);
  dialog.showModal();
  refreshButtons();
  timer = setInterval(() => {
    if (!isLive()) { ended(); return; }
    if (expiresAt && expiresAt * 1000 <= Date.now()) {
      clearPrivate(); error.textContent = "期限が切れました。ペアリングを更新してください。";
    }
    if (!document.hidden && Date.now() >= pollAt) { pollAt = Date.now() + 3000; void call("status"); }
  }, 250);
  void call("status");
}
