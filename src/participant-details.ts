import type { LoggedPost, TopicStatus, WorkRecord } from "./contracts";

type Options = {
  element: HTMLElement;
  planEnabled: (id: string | null) => boolean;
  topic: () => string;
  human: () => string;
  read: (topic: string, account?: string) => Promise<TopicStatus>;
  update: (topic: string, operation: Record<string, unknown>, answer?: string) => Promise<unknown>;
  history: (topic: string) => Promise<LoggedPost[]>;
  openUrl: (url: string) => void;
  changed: () => void;
};

/** Work selection and fetching have no terminal or launch capability. */
const SVG_NS = "http://www.w3.org/2000/svg";
const ICONS = {
  refresh: ["M20 11a8 8 0 0 0-14.9-4", "M4 3v4h4", "M4 13a8 8 0 0 0 14.9 4", "M20 21v-4h-4"],
  circle: ["M12 4a8 8 0 1 0 0 16a8 8 0 1 0 0-16"],
  check: ["M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18", "M8 12.5l2.7 2.7L16 9.5"],
  calendar: ["M5 6h14v14H5z", "M5 10h14", "M9 3v5", "M15 3v5"],
  external: ["M8 16L16 8", "M10 8h6v6"],
} as const;

export function createParticipantDetails(o: Options) {
  let selected: { id: string | null; name: string; server?: string; kind?: string } | null = null;
  let data: TopicStatus | null = null;
  let dataTopic = "";
  let epoch = 0;
  let pending: Promise<void> | null = null;
  let requestedTopic = "";
  let dirty = false;
  let error = "";
  // The fold states belong to the user: only their toggle changes them, so a redraw,
  // a participant switch or an MCP view (which draws no fold) does not reset them.
  let todoOpen = false;
  let historyOpen = false;
  const el = o.element;
  const text = (tag: string, value: string) => {
    const e = document.createElement(tag); e.textContent = value; return e;
  };
  const button = (label: string, action: () => void) => {
    const b = text("button", label) as HTMLButtonElement; b.type = "button";
    b.addEventListener("click", action); return b;
  };
  /** A line icon bundled with the app (#523, after the mockup): drawn in the text colour at the text size, hidden from reading. */
  const icon = (name: keyof typeof ICONS) => {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24"); svg.setAttribute("aria-hidden", "true");
    svg.classList.add("todo-icon", `todo-icon-${name}`);
    for (const d of ICONS[name]) { const path = document.createElementNS(SVG_NS, "path"); path.setAttribute("d", d); svg.append(path); }
    return svg;
  };
  const showDialog = (title: string, content: string) => {
    const dialog = document.createElement("dialog"); dialog.className = "work-dialog";
    dialog.append(text("h3", title), text("pre", content), button("閉じる", () => dialog.close()));
    dialog.addEventListener("close", () => dialog.remove(), { once: true });
    document.body.append(dialog); dialog.showModal();
  };
  async function source(id: string) {
    const topic = o.topic();
    try {
      const rows = await o.history(topic);
      if (topic !== o.topic()) return;
      const posts = rows.filter(p => p.message_id === id);
      showDialog("元の発言", posts.length === 1 ? `${posts[0].speaker} · ${posts[0].ts}\n\n${posts[0].content}` : `発言を一意に取得できません: ${id}`);
    } catch (e) { if (topic === o.topic()) showDialog("取得できません", String(e)); }
  }
  function edit(record: WorkRecord, action: "answer" | "put") {
    const topic = o.topic();
    const dialog = document.createElement("dialog"); dialog.className = "work-dialog";
    const input = document.createElement("textarea"); input.rows = 5;
    input.value = action === "put" ? record.text : "";
    input.maxLength = action === "put" ? 8000 : 16000;
    input.setAttribute("aria-label", action === "put" ? "質問" : "回答");
    const message = text("p", ""); message.setAttribute("role", "status");
    const operation = { operation_id: crypto.randomUUID(), id: record.id, action, expected_revision: record.revision,
      ...(action === "put" ? { kind: record.kind, progress: record.progress, references: record.references, source_message_id: record.source_message_id } : {}) };
    let sentText: string | null = null;
    const save = button("保存", () => void (async () => {
      if (!input.value.trim() || topic !== o.topic()) return;
      // Identical retry after an unconfirmed response keeps its operation ID and payload.
      sentText ??= input.value;
      input.disabled = true; save.disabled = true;
      try {
        await o.update(topic, { ...operation, ...(action === "put" ? {text:sentText} : {}) }, action === "answer" ? sentText : undefined);
        dialog.close(); await refresh();
      } catch (e) { message.textContent = String(e); save.disabled = false; }
    })());
    dialog.append(text("h3", action === "put" ? "判断依頼を編集" : "判断依頼への回答"), text("p", record.text), input, message,
      save, button("閉じる", () => dialog.close()));
    dialog.addEventListener("close", () => dialog.remove(), { once: true });
    document.body.append(dialog); dialog.showModal();
  }
  function recordView(record: WorkRecord) {
    const row = document.createElement("article"); row.className = "work-record";
    row.append(text("strong", `${record.kind === "decision" ? "判断依頼" : "作業"} · ${record.text}`));
    row.append(text("small", `${record.added_by} · ${record.updated_at} · ${{active:"登録中",completed:"完了",answered:"回答済み",cancelled:"取消済み"}[record.state] ?? record.state}${record.state === "active" && record.interrupted ? " · 中断中" : ""}${record.state === "active" && record.owner_limited ? " · 利用上限" : ""}`));
    if (record.progress) row.append(text("p", record.progress));
    for (const ref of record.references) {
      try {
        const url = new URL(ref);
        if (url.protocol === "https:" || url.protocol === "http:") row.append(button(ref, () => o.openUrl(url.href)));
        else row.append(text("span", ref));
      } catch { row.append(text("span", ref)); }
    }
    if (record.source_message_id) row.append(button("元の発言", () => void source(record.source_message_id!)));
    if (record.answer) {
      row.append(text("p", `${record.answer.name} · ${record.answer.answered_at}\n${record.answer.content}`));
      if (record.answer.message_id) row.append(button("回答の発言", () => void source(record.answer!.message_id!)));
    }
    if (record.kind === "decision" && record.state === "active" && record.target_account_id === o.human()) {
      row.append(button("回答", () => { row.closest("dialog")?.close(); edit(record,"answer"); }), button("編集", () => { row.closest("dialog")?.close(); edit(record,"put"); }));
      const op = {operation_id:crypto.randomUUID(), id:record.id, action:"cancel", expected_revision:record.revision};
      const cancel = button("取消", () => void (async () => {
        const topic = o.topic(); cancel.disabled = true;
        try { await o.update(topic,op); row.closest("dialog")?.close(); await refresh(); }
        catch (e) { if (topic === o.topic()) { error = String(e); draw(); } }
        finally { cancel.disabled = false; }
      })());
      row.append(cancel);
    }
    return row;
  }
  const short = (value: string) => { const chars = Array.from(value.replace(/\s+/g, " ").trim()); return chars.length > 32 ? `${chars.slice(0,32).join("")}…` : chars.join(""); };
  function openRecord(record: WorkRecord) {
    const dialog = document.createElement("dialog"); dialog.className = "work-dialog";
    dialog.append(text("h3", "項目の詳細"), recordView(record), button("閉じる", () => dialog.close()));
    dialog.addEventListener("close", () => dialog.remove(), {once:true});
    document.body.append(dialog); dialog.showModal();
  }
  const stateLabel = (record: WorkRecord) => record.state === "active"
    ? (record.owner_limited ? "利用上限" : record.interrupted ? "中断中" : record.kind === "decision" ? "判断待ち" : "作業")
    : ({completed:"完了",answered:"回答済み",cancelled:"取消済み"} as Record<string,string>)[record.state] ?? record.state;
  const span = (className: string, value: string) => { const e = text("span", value); e.className = className; return e; };
  function sectionHead(label: string, count: number) {
    const head = document.createElement("div"); head.className = "todo-head";
    head.append(text("h4", label), span("todo-count", String(count))); return head;
  }
  /** One record as a card: mark, title clamped to two lines (opens the full record), then state, who added it and the source post. */
  function recordCard(record: WorkRecord, done: boolean) {
    const card = document.createElement("article"); card.className = done ? "todo-card todo-done" : "todo-card";
    const state = stateLabel(record);
    const mark = done && record.state === "cancelled" ? span("todo-mark", "–") : span("todo-mark", "");
    if (!(done && record.state === "cancelled")) mark.append(icon(done ? "check" : "circle"));
    mark.setAttribute("aria-label", state); mark.title = state;
    const body = document.createElement("div"); body.className = "todo-body";
    const title = button(record.text, () => openRecord(record)); title.className = "todo-item"; title.title = record.text;
    body.append(title);
    // The meta row: state and who on the left, 参照：発言を開く ↗ on the right (#523). A done item keeps only the link.
    const meta = document.createElement("div"); meta.className = "todo-meta";
    if (!done) meta.append(span(`todo-badge${record.state === "active" && record.kind === "decision" ? " todo-badge-decision" : ""}`, state), span("todo-who", record.added_by));
    if (record.source_message_id) {
      const ref = document.createElement("span"); ref.className = "todo-ref";
      if (!done) ref.append(span("todo-ref-label", "参照："));
      const open = button("発言を開く", () => void source(record.source_message_id!)); open.className = "todo-source"; open.append(icon("external"));
      ref.append(open); meta.append(ref);
    }
    if (meta.childElementCount) body.append(meta);
    card.append(mark, body); return card;
  }
  function planBlock(title: string, badge: string, note: string) {
    const block = document.createElement("section"); block.className = "todo-plan";
    const head = document.createElement("div"); head.className = "todo-head";
    head.append(icon("calendar"), text("h4", title), span(`todo-badge todo-plan-state${badge === "未取得" ? " todo-plan-missing" : ""}`, badge));
    block.append(head, text("small", note)); return block;
  }
  function draw() {
    el.replaceChildren();
    if (selected?.kind === "mcp" || selected?.server) {
      if (error) el.append(text("p", `取得できません: ${error}`));
      if (!data || dataTopic !== o.topic()) { el.append(text("p", "未取得")); return; }
      const server = data.mcp_servers.find(s => s.server === selected?.server);
      const labels: Record<string,string> = {unsupported:"未対応",unobserved:"未取得",timeout:"タイムアウト",auth_required:"未認証",observed:"取得済み"};
      el.append(text("h4", "アプリの MCP"));
      el.append(text("p", server ? `接続: ${{starting:"接続中",running:"接続済み",ended:"切断",failed:"接続失敗",stopped:"停止"}[server.connection] ?? "未取得"}` : "接続: 未取得"));
      if (!server) el.append(text("p", "未処理: 未取得"));
      if (server) {
        const p = server.pending;
        el.append(text("p", `未処理: ${p.state === "observed" ? `${p.pending_count} 件` : (labels[p.state] ?? "未取得")}`));
        if (p.state === "observed") {
          el.append(text("p", Object.entries(p.types ?? {}).map(([k,v]) => `${k}: ${v}`).join(" / ")));
          el.append(text("small", `未処理内の最新受信: ${p.latest_received_at ?? "なし"}`));
        }
        if (p.observed_at) el.append(text("small", `観測: ${new Date(p.observed_at).toLocaleString()}`));
      }
      return;
    }
    const fold = document.createElement("details"); fold.className = "todo-fold facts-more"; fold.open = todoOpen;
    const summary = document.createElement("summary"); summary.append(span("todo-title", "TODO"));
    // Read the user's toggle at its click, before the toggle event: a redraw landing in between
    // would otherwise draw the old state and drop the toggle with the replaced element.
    summary.addEventListener("click", e => { if (!e.defaultPrevented) todoOpen = !fold.open; });
    fold.append(summary); el.append(fold);
    if (!selected) { fold.append(text("p", "参加者を選ぶと作業と判断待ちを表示します。")); return; }
    // The refresh control sits in the heading; pressing it does not toggle the fold.
    const update = button("更新", () => void refresh()); update.className = "todo-refresh"; update.append(icon("refresh")); update.setAttribute("aria-label", `${selected.name} の TODO を更新`);
    update.addEventListener("click", e => e.preventDefault()); summary.append(update);
    if (error) fold.append(text("p", `取得できません: ${error}`));
    if (!data || dataTopic !== o.topic()) { fold.append(text("p", "未取得")); return; }
    const include = (r: WorkRecord) => selected?.id != null && (r.owner === selected.id || (r.kind === "decision" && r.target_account_id === selected.id));
    const active = data.work.active.filter(include);
    // #539: the folded heading tells how many open items (判断待ち + 作業) wait inside, between
    // TODO and 更新; none -> no count. The open fold hides it (styles.css), its sections count.
    if (active.length) { const count = span("todo-count todo-fold-count", String(active.length)); count.title = `未完了 ${active.length} 件`; count.setAttribute("aria-label", count.title); summary.insertBefore(count, update); }
    const decisions = active.filter(r => r.kind === "decision"), works = active.filter(r => r.kind !== "decision");
    const decisionSection = document.createElement("section"); decisionSection.className = "todo-section todo-decisions";
    decisionSection.append(sectionHead("判断待ち", decisions.length));
    if (decisions.length) decisions.forEach(r => decisionSection.append(recordCard(r, false)));
    else decisionSection.append(text("p", "登録なし"));
    fold.append(decisionSection);
    if (works.length) {
      const workSection = document.createElement("section"); workSection.className = "todo-section todo-works";
      workSection.append(sectionHead("作業", works.length));
      works.forEach(r => workSection.append(recordCard(r, false)));
      fold.append(workSection);
    }
    const planState = (status: string) => ({pending:"未着手",inProgress:"進行中",completed:"完了"} as Record<string,string>)[status] ?? "未取得";
    const showPlan = o.planEnabled(selected.id);
    for (const plan of data.plans.filter(p => showPlan && p.account_id === selected?.id)) {
      const title = "計画（自己申告）";
      const done = plan.steps.filter(s => s.status === "completed").length;
      const sourceName = plan.source === "codex-app-server" ? "Codex" : "Claude";
      const block = planBlock(title, plan.current_launch ? `${done}/${plan.steps.length}` : "前の起動", `${sourceName} · ${new Date(plan.updated_at).toLocaleString()}`);
      const content = `${sourceName} · ${new Date(plan.updated_at).toLocaleString()}${plan.current_launch ? "" : " · 前の起動"}\n\n${plan.steps.map(s=>`${planState(s.status)} · ${s.step}`).join("\n")}`;
      const steps = document.createElement("ol"); steps.className = "todo-steps";
      for (const step of plan.steps) {
        const item = document.createElement("li");
        const open = button(short(step.step), () => showDialog(title, content)); open.className = "todo-item"; open.title = step.step;
        item.append(span("todo-step-state", planState(step.status)), open); steps.append(item);
      }
      block.append(steps); fold.append(block);
    }
    if (showPlan && !selected.server && selected.kind !== "admin" && selected.id !== o.human() && !data.plans.some(p => p.account_id === selected?.id)) {
      fold.append(planBlock("計画（自己申告）", "未取得", "計画データを取得していません"));
    }
    const history = data.work.history.filter(include);
    const historyEl = document.createElement("details"); historyEl.className = "todo-history"; historyEl.open = historyOpen;
    const historySummary = document.createElement("summary");
    historySummary.addEventListener("click", e => { if (!e.defaultPrevented) historyOpen = !historyEl.open; });
    historySummary.append(span("todo-history-title", "完了履歴"), span("todo-count", `${history.length}件`), span("todo-limit", `最大${data.work.history_limit}件`));
    historyEl.append(historySummary);
    if (history.length) history.forEach(r => historyEl.append(recordCard(r, true)));
    else historyEl.append(text("p", "履歴なし"));
    fold.append(historyEl);
  }
  function refresh(): Promise<void> {
    const topic = o.topic();
    if (pending && requestedTopic === topic) { dirty = true; return pending; }
    const generation = ++epoch; requestedTopic = topic;
    if (dataTopic !== topic) { data = null; error = ""; draw(); }
    const work = (async () => {
      try {
        const result = await o.read(topic);
        const account = selected?.id;
        if (account && !selected?.server && selected?.kind !== "admin" && selected?.kind !== "mcp" && account !== o.human()) {
          try {
            const detail = await o.read(topic,account);
            if (detail.topic_id === topic) for (const row of detail.participants) {
              const index = result.participants.findIndex(p => p.account_id === row.account_id);
              if (index >= 0) result.participants[index] = row;
            }
          } catch { /* A guest or departed account may have no session detail. */ }
        }
        if (generation !== epoch || topic !== o.topic() || result.topic_id !== topic) return;
        data = result; dataTopic = topic; error = ""; draw(); o.changed();
      } catch (e) { if (generation === epoch && topic === o.topic()) { data = null; dataTopic = ""; error = String(e); draw(); o.changed(); } }
      finally {
        if (generation === epoch) { pending = null; if (dirty) { dirty = false; void refresh(); } }
      }
    })();
    pending = work; return work;
  }
  return {
    select(id: string | null, name: string, server?: string, kind?: string) { selected = {id,name,server,kind}; draw(); void refresh(); },
    selected: () => selected?.id,
    selection: () => selected,
    session: () => dataTopic === o.topic() ? data?.participants.find(p => p.account_id === selected?.id)?.session : undefined,
    count(id: string, server?: string, kind?: string): number | undefined {
      if (!data || dataTopic !== o.topic()) return undefined;
      if (kind === "mcp" || server) {
        const pending = data.mcp_servers.find(row => row.server === server)?.pending;
        const count = pending?.pending_count;
        return pending?.state === "observed" && typeof count === "number" && Number.isSafeInteger(count) && count >= 0 ? count : undefined;
      }
      // topic_status is the full UI journal, not the bounded MCP status projection.
      return data.work.active.filter(r => r.state === "active" && (r.owner === id || (r.kind === "decision" && r.target_account_id === id))).length;
    },
    refresh,
    topicChanged() { document.querySelectorAll<HTMLDialogElement>("dialog.work-dialog").forEach(d => d.close()); ++epoch; pending = null; dirty = false; data = null; dataTopic = ""; selected = null; error = ""; el.replaceChildren(); draw(); o.changed(); },
  };
}
