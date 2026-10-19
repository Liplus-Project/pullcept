import type { LoggedPost, TopicStatus, WorkRecord } from "./contracts";

type Options = {
  element: HTMLElement;
  topic: () => string;
  human: () => string;
  read: (topic: string, account?: string) => Promise<TopicStatus>;
  update: (topic: string, operation: Record<string, unknown>, answer?: string) => Promise<unknown>;
  history: (topic: string) => Promise<LoggedPost[]>;
  openUrl: (url: string) => void;
  changed: () => void;
};

/** Work selection and fetching have no terminal or launch capability. */
export function createParticipantDetails(o: Options) {
  let selected: { id: string | null; name: string; server?: string; kind?: string } | null = null;
  let data: TopicStatus | null = null;
  let dataTopic = "";
  let epoch = 0;
  let pending: Promise<void> | null = null;
  let requestedTopic = "";
  let dirty = false;
  let error = "";
  const el = o.element;
  const text = (tag: string, value: string) => {
    const e = document.createElement(tag); e.textContent = value; return e;
  };
  const button = (label: string, action: () => void) => {
    const b = text("button", label) as HTMLButtonElement; b.type = "button";
    b.addEventListener("click", action); return b;
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
  function table() {
    const table = document.createElement("table"); table.className = "todo-table";
    const head = document.createElement("thead"); const row = document.createElement("tr");
    for (const label of ["状態", "内容", "参照"]) { const cell = text("th",label); cell.setAttribute("scope","col"); row.append(cell); }
    head.append(row); const body = document.createElement("tbody"); table.append(head,body);
    return {element:table, body};
  }
  function tableRow(body: HTMLElement, state: string, label: string, detail: () => void, sourceId?: string | null) {
    const row = document.createElement("tr"); const content = document.createElement("td");
    const link = button(short(label), detail); link.className = "todo-item"; content.append(link);
    const reference = document.createElement("td");
    if (sourceId) reference.append(button("発言", () => void source(sourceId)));
    row.append(text("td",state), content,reference); body.append(row);
  }
  function recordsTable(records: WorkRecord[]) {
    const rows = table();
    for (const record of records) {
      const state = record.state === "active" ? (record.owner_limited ? "利用上限" : record.interrupted ? "中断中" : record.kind === "decision" ? "判断待ち" : "作業") : ({completed:"完了",answered:"回答済み",cancelled:"取消済み"} as Record<string,string>)[record.state] ?? record.state;
      tableRow(rows.body,state,record.text,() => openRecord(record),record.source_message_id);
    }
    return rows.element;
  }
  function draw() {
    const todoOpen = el.querySelector<HTMLDetailsElement>(".todo-fold")?.open ?? false;
    const historyOpen = el.querySelector<HTMLDetailsElement>(".todo-history")?.open ?? false;
    el.replaceChildren();
    const fold = document.createElement("details"); fold.className = "todo-fold facts-more"; fold.open = todoOpen;
    fold.append(text("summary", "todo")); el.append(fold);
    if (!selected) { fold.append(text("p", "参加者を選ぶと作業と判断待ちを表示します。")); return; }
    const tools = document.createElement("div"); tools.className = "todo-tools";
    tools.append(text("h3", selected.name), button("更新", () => void refresh())); fold.append(tools);
    if (error) fold.append(text("p", `取得できません: ${error}`));
    if (!data || dataTopic !== o.topic()) { fold.append(text("p", "未取得")); return; }
    const include = (r: WorkRecord) => selected?.id != null && (r.owner === selected.id || (r.kind === "decision" && r.target_account_id === selected.id));
    const active = data.work.active.filter(include);
    fold.append(text("p", `判断待ち ${active.filter(r => r.kind === "decision").length} 件`));
    fold.append(active.length ? recordsTable(active) : text("p", "登録なし"));
    for (const plan of data.plans.filter(p => p.account_id === selected?.id)) {
      const title = `計画（自己申告）${plan.current_launch ? "" : " · 前の起動"}`;
      fold.append(text("h4", title));
      const rows = table();
      const planState = (status: string) => ({pending:"未着手",inProgress:"進行中",completed:"完了"} as Record<string,string>)[status] ?? "未取得";
      const content = `${plan.source === "codex-app-server" ? "Codex" : "Claude"} · ${new Date(plan.updated_at).toLocaleString()}\n\n${plan.steps.map(s=>`${planState(s.status)} · ${s.step}`).join("\n")}`;
      plan.steps.forEach(s => tableRow(rows.body,planState(s.status),s.step,()=>showDialog(title,content)));
      fold.append(rows.element);
    }
    if (!selected.server && selected.kind !== "admin" && selected.id !== o.human() && !data.plans.some(p => p.account_id === selected?.id)) {
      fold.append(text("h4", "計画（自己申告）"),text("p", "未取得"));
    }
    const history = data.work.history.filter(include);
    const historyEl = document.createElement("details"); historyEl.className = "todo-history"; historyEl.open = historyOpen;
    historyEl.append(text("summary", `完了履歴 ${history.length} 件（最大 ${data.work.history_limit} 件）`));
    if (history.length) historyEl.append(recordsTable(history));
    fold.append(historyEl);
    if (selected.server) {
      const server = data.mcp_servers.find(s => s.server === selected?.server);
      const labels: Record<string,string> = {unsupported:"未対応",unobserved:"未取得",timeout:"タイムアウト",auth_required:"未認証",observed:"取得済み"};
      el.append(text("h4", "アプリの MCP"));
      el.append(text("p", server ? `接続: ${{starting:"接続中",running:"接続済み",ended:"切断",failed:"接続失敗",stopped:"停止"}[server.connection] ?? "未取得"}` : "接続: 未取得"));
      if (server) {
        const p = server.pending;
        el.append(text("p", `未処理: ${p.state === "observed" ? `${p.pending_count} 件` : (labels[p.state] ?? "未取得")}`));
        if (p.state === "observed") {
          el.append(text("p", Object.entries(p.types ?? {}).map(([k,v]) => `${k}: ${v}`).join(" / ")));
          el.append(text("small", `未処理内の最新受信: ${p.latest_received_at ?? "なし"}`));
        }
        if (p.observed_at) el.append(text("small", `観測: ${new Date(p.observed_at).toLocaleString()}`));
      }
    }
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
        if (account && !selected?.server && selected?.kind !== "admin" && account !== o.human()) {
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
      } catch (e) { if (generation === epoch && topic === o.topic()) { error = String(e); draw(); } }
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
    count(id: string) { return dataTopic === o.topic() ? data?.work.active.filter(r => r.kind === "decision" && (r.owner === id || r.target_account_id === id)).length ?? 0 : 0; },
    refresh,
    topicChanged() { document.querySelectorAll<HTMLDialogElement>("dialog.work-dialog").forEach(d => d.close()); ++epoch; pending = null; dirty = false; data = null; dataTopic = ""; selected = null; el.replaceChildren(); draw(); },
  };
}
