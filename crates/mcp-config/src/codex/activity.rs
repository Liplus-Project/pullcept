//! What a Codex app-server seat is doing, read off the notifications its
//! server sends the app (#326). See docs/2-screen.md, "走っているアカウントが何をしているか".
//!
//! The app is a client of the seat's thread for the life of the seat (#299,
//! `codex_app_server`). Its server tells every client of the thread what the
//! thread is doing, and this turns that into the one word the participant
//! panel's row has room for, and the longer form the line under the room says.
//! Nothing here reads the terminal, a command line, a tool's arguments or any
//! message body: only an item's kind and, for a tool, its server and name.
//!
//! **Method names and shapes are the generated schema's** (`codex app-server
//! generate-json-schema`, Codex CLI 0.160.1, with and without
//! `--experimental`), not the published docs', which differ in places (#324):
//!
//! | method | read |
//! |---|---|
//! | `thread/status/changed` | `threadId`, `status.type` (`active` / `idle` / `systemError` / `notLoaded`), `status.activeFlags` (`waitingOnApproval` / `waitingOnUserInput`) |
//! | `turn/started`, `turn/completed` | `threadId` |
//! | `item/started`, `item/completed` | `threadId`, `item.id`, `item.type`; `item.server` / `item.tool` (`mcpToolCall`), `item.namespace` / `item.tool` (`dynamicToolCall`) |
//! | `thread/closed` | `threadId` |
//!
//! The start and resume answers carry `thread.status` in the same shape
//! (`ThreadStartResponse` / `ThreadResumeResponse`), read once as a snapshot.
//!
//! **A wait outranks an item.** `item/started` for a command arrives before
//! its approval is asked for (#324), so while either flag is up the row says
//! 許可待ち / 答え待ち and never that the command is running.
//!
//! **Items are a set**, keyed by id, because tools run in parallel. The row
//! says the one started last; the line adds how many others are running.
//!
//! **Nothing outlives what it describes.** A turn's start and end, and a thread
//! that is no longer active, clear the items and the flags. A connection that
//! ends clears everything, and the screen is told so as its own state
//! (`connected: false`), not as nothing: an idle seat and one whose server can
//! no longer be heard are different, and only the first may fall back to 待機.
//!
//! **What reaches the screen is decided here too** (`Reporter`), so the event
//! itself is tested: one `seat-activity` per change of what is shown or of the
//! connection, the first one at once.
//!
//! **Nothing sent before the thread is known is lost.** A fresh thread's id is
//! in the answer to `thread/start`, and its first notifications may come
//! before that answer. They are held (up to `EARLY_MAX`) and read once the id
//! is set.
use serde_json::{json, Value};

/// Notifications held while the thread's id is not yet known. A start sends a
/// handful; the cap only keeps a server that floods before answering from
/// growing this without end.
const EARLY_MAX: usize = 256;

/// The most characters of a tool's name shown. Names are a server's and a
/// tool's identifiers, not text a model wrote, but they are still cut.
const NAME_MAX: usize = 48;

/// One running item: what kind of thing the seat is doing.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Command,
    FileChange,
    /// An MCP or dynamic tool, by `server/tool` (or `namespace/tool`, or
    /// `tool` alone) when the notification names it.
    Tool(Option<String>),
    WebSearch,
    Compaction,
    ImageGeneration,
    ImageView,
}

impl Kind {
    /// The kinds shown. Every other item — a message, reasoning, a plan, a
    /// sub-agent — is not a state this says: the first three are what 考え中…
    /// and 出力中 already cover, and a sub-agent's activity was not measured (#324).
    fn of(item: &Value) -> Option<Kind> {
        let name = |first: &str| {
            let tool = text(&item["tool"])?;
            Some(match text(&item[first]) {
                Some(owner) => format!("{owner}/{tool}"),
                None => tool,
            })
        };
        Some(match item["type"].as_str()? {
            "commandExecution" => Kind::Command,
            "fileChange" => Kind::FileChange,
            "mcpToolCall" => Kind::Tool(name("server")),
            "dynamicToolCall" => Kind::Tool(name("namespace")),
            "webSearch" => Kind::WebSearch,
            "contextCompaction" => Kind::Compaction,
            "imageGeneration" => Kind::ImageGeneration,
            "imageView" => Kind::ImageView,
            _ => return None,
        })
    }

    /// The row's word: four characters at most, the width 起動失敗 already
    /// takes beside the name (docs/2-screen.md).
    fn word(&self) -> &'static str {
        match self {
            Kind::Command => "実行中",
            Kind::FileChange => "編集中",
            Kind::Tool(_) => "ツール",
            Kind::WebSearch => "検索中",
            Kind::Compaction => "要約中",
            Kind::ImageGeneration => "画像生成",
            Kind::ImageView => "画像参照",
        }
    }

    /// The longer form, for the line under the room and the badge's title.
    fn line(&self) -> String {
        match self {
            Kind::Command => "コマンド実行中".into(),
            Kind::FileChange => "ファイル変更中".into(),
            Kind::Tool(Some(name)) => format!("ツール使用中（{name}）"),
            Kind::Tool(None) => "ツール使用中".into(),
            Kind::WebSearch => "Web 検索中".into(),
            Kind::Compaction => "要約中".into(),
            Kind::ImageGeneration => "画像生成中".into(),
            Kind::ImageView => "画像参照中".into(),
        }
    }
}

/// A nonempty string field, without control characters and cut to `NAME_MAX`.
fn text(value: &Value) -> Option<String> {
    let cleaned: String = value
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// What the screen is told.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// The row's badge.
    pub word: &'static str,
    /// The line under the room, after 「名前が」, and the badge's title.
    pub line: String,
    /// A wait on the person (許可待ち / 答え待ち), rather than work under way.
    pub waiting: bool,
}

/// One seat's state, fed every message its server sends the app.
#[derive(Debug, Default)]
pub struct Activity {
    thread: Option<String>,
    early: Vec<Value>,
    approval: bool,
    input: bool,
    /// Running items, oldest first.
    items: Vec<(String, Kind)>,
    disconnected: bool,
}

impl Activity {
    pub fn new() -> Self {
        Self::default()
    }

    /// The seat's thread is `id`: what was held for it is read now, and
    /// notifications for any other thread (a sub-agent's) are not this seat's.
    pub fn set_thread(&mut self, id: &str) {
        self.thread = Some(id.to_string());
        for message in std::mem::take(&mut self.early) {
            self.feed(&message);
        }
    }

    /// The thread's status as an answer to `thread/start` or `thread/resume`
    /// carried it (`result.thread.status`).
    pub fn snapshot(&mut self, status: &Value) {
        self.status(status);
    }

    /// One message from the server. Answers and requests (anything with an
    /// `id`) are not read: approvals are the terminal's to answer.
    pub fn feed(&mut self, message: &Value) {
        if self.disconnected || message.get("id").is_some() {
            return;
        }
        let Some(method) = message["method"].as_str() else {
            return;
        };
        let params = &message["params"];
        let Some(thread) = params["threadId"].as_str() else {
            return;
        };
        match &self.thread {
            None => {
                if self.early.len() < EARLY_MAX {
                    self.early.push(message.clone());
                }
                return;
            }
            Some(own) if own != thread => return,
            Some(_) => {}
        }
        match method {
            "thread/status/changed" => self.status(&params["status"]),
            "turn/started" | "turn/completed" | "thread/closed" => self.clear(),
            "item/started" => {
                let item = &params["item"];
                let (Some(id), Some(kind)) = (item["id"].as_str(), Kind::of(item)) else {
                    return;
                };
                self.items.retain(|(other, _)| other != id);
                self.items.push((id.to_string(), kind));
            }
            "item/completed" => {
                if let Some(id) = params["item"]["id"].as_str() {
                    self.items.retain(|(other, _)| other != id);
                }
            }
            _ => {}
        }
    }

    /// The connection to the server ended. Nothing more is known, and nothing
    /// is said: not 待機, which would be a claim.
    pub fn disconnect(&mut self) {
        self.clear();
        self.early.clear();
        self.disconnected = true;
    }

    fn status(&mut self, status: &Value) {
        if status["type"].as_str() == Some("active") {
            let flags = status["activeFlags"].as_array();
            let has = |flag: &str| flags.is_some_and(|f| f.iter().any(|v| v.as_str() == Some(flag)));
            self.approval = has("waitingOnApproval");
            self.input = has("waitingOnUserInput");
        } else if status["type"].is_string() {
            // idle, systemError, notLoaded: no turn is running.
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.approval = false;
        self.input = false;
        self.items.clear();
    }

    /// Whether the server can still be heard. False for good once the
    /// connection has ended.
    pub fn connected(&self) -> bool {
        !self.disconnected
    }

    /// What the seat is doing, or `None` when this says nothing: no work under
    /// way, or the connection gone (`connected` tells the two apart).
    pub fn display(&self) -> Option<Display> {
        if self.disconnected {
            return None;
        }
        if self.approval {
            return Some(Display { word: "許可待ち", line: "許可待ち".into(), waiting: true });
        }
        if self.input {
            return Some(Display { word: "答え待ち", line: "答え待ち".into(), waiting: true });
        }
        let (_, kind) = self.items.last()?;
        let mut line = kind.line();
        if self.items.len() > 1 {
            line.push_str(&format!("ほか {} 件", self.items.len() - 1));
        }
        Some(Display { word: kind.word(), line, waiting: false })
    }
}

/// The `seat-activity` events one launch sends the screen: its seat, and what
/// it last sent.
#[derive(Debug)]
pub struct Reporter {
    topic_id: String,
    account_id: String,
    pty_id: String,
    sent: Option<(bool, Option<Display>)>,
}

impl Reporter {
    /// `pty_id` is the launch's terminal, so a relaunch in the same seat is
    /// told apart from the run before it, whose server may still be going down.
    pub fn new(topic_id: &str, account_id: &str, pty_id: &str) -> Self {
        Reporter {
            topic_id: topic_id.into(),
            account_id: account_id.into(),
            pty_id: pty_id.into(),
            sent: None,
        }
    }

    /// The event to send now, or `None` when neither what is shown nor the
    /// connection changed since the last one. The first call always sends,
    /// so the screen learns the seat is connected before anything happens.
    pub fn next(&mut self, activity: &Activity) -> Option<Value> {
        let now = (activity.connected(), activity.display());
        if self.sent.as_ref() == Some(&now) {
            return None;
        }
        let (connected, display) = &now;
        let event = json!({
            "topic_id": self.topic_id,
            "account_id": self.account_id,
            "pty_id": self.pty_id,
            "connected": connected,
            "word": display.as_ref().map(|d| d.word),
            "line": display.as_ref().map(|d| d.line.clone()),
            "waiting": display.as_ref().is_some_and(|d| d.waiting),
        });
        self.sent = Some(now);
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T: &str = "thr-1";

    fn seat() -> Activity {
        let mut a = Activity::new();
        a.set_thread(T);
        a
    }

    fn note(method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    }

    fn started(id: &str, item: Value) -> Value {
        let mut item = item;
        item["id"] = json!(id);
        note("item/started", json!({"threadId": T, "turnId": "turn-1", "startedAtMs": 1, "item": item}))
    }

    fn completed(id: &str) -> Value {
        note(
            "item/completed",
            json!({"threadId": T, "turnId": "turn-1", "completedAtMs": 2, "item": {"id": id, "type": "commandExecution"}}),
        )
    }

    fn status(value: Value) -> Value {
        note("thread/status/changed", json!({"threadId": T, "status": value}))
    }

    fn command() -> Value {
        json!({"type": "commandExecution", "command": "rm -rf secret", "commandActions": [], "cwd": "C:/x", "status": "inProgress"})
    }

    fn words(a: &Activity) -> Option<(&'static str, String)> {
        a.display().map(|d| (d.word, d.line))
    }

    #[test]
    fn a_command_shows_its_kind_and_never_its_text() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        let d = a.display().unwrap();
        assert_eq!((d.word, d.line.as_str(), d.waiting), ("実行中", "コマンド実行中", false));
        assert!(!d.line.contains("rm"));
    }

    #[test]
    fn approval_flag_outranks_an_item_started_before_approval() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        a.feed(&status(json!({"type": "active", "activeFlags": ["waitingOnApproval"]})));
        let d = a.display().unwrap();
        assert_eq!((d.word, d.waiting), ("許可待ち", true));
        // Approved: the flag goes down and the command is what is running.
        a.feed(&status(json!({"type": "active", "activeFlags": []})));
        assert_eq!(words(&a), Some(("実行中", "コマンド実行中".into())));
    }

    #[test]
    fn user_input_flag_says_waiting_for_an_answer() {
        let mut a = seat();
        a.feed(&status(json!({"type": "active", "activeFlags": ["waitingOnUserInput"]})));
        assert_eq!(words(&a), Some(("答え待ち", "答え待ち".into())));
        assert!(a.display().unwrap().waiting);
    }

    #[test]
    fn parallel_items_are_a_set() {
        let mut a = seat();
        a.feed(&started("t1", json!({"type": "mcpToolCall", "server": "room", "tool": "say_to_room", "arguments": {"text": "hello"}, "status": "inProgress"})));
        a.feed(&started("f1", json!({"type": "fileChange", "changes": [], "status": "inProgress"})));
        assert_eq!(words(&a), Some(("編集中", "ファイル変更中ほか 1 件".into())));
        a.feed(&completed("f1"));
        assert_eq!(words(&a), Some(("ツール", "ツール使用中（room/say_to_room）".into())));
        assert!(!a.display().unwrap().line.contains("hello"));
        a.feed(&completed("t1"));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn the_same_item_started_twice_is_one() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        a.feed(&started("c1", command()));
        assert_eq!(words(&a), Some(("実行中", "コマンド実行中".into())));
    }

    #[test]
    fn dynamic_tools_and_other_kinds() {
        let mut a = seat();
        a.feed(&started("d1", json!({"type": "dynamicToolCall", "namespace": "ns", "tool": "go", "arguments": {}, "status": "inProgress"})));
        assert_eq!(words(&a).unwrap().1, "ツール使用中（ns/go）");
        a.feed(&started("d2", json!({"type": "dynamicToolCall", "tool": "solo", "arguments": {}, "status": "inProgress"})));
        assert_eq!(words(&a).unwrap().1, "ツール使用中（solo）ほか 1 件");
        a.feed(&started("k1", json!({"type": "contextCompaction"})));
        assert_eq!(words(&a).unwrap().0, "要約中");
        a.feed(&started("w1", json!({"type": "webSearch", "query": "private words"})));
        assert_eq!(words(&a).unwrap(), ("検索中", "Web 検索中ほか 3 件".into()));
    }

    #[test]
    fn messages_and_reasoning_are_not_shown() {
        let mut a = seat();
        a.feed(&started("m1", json!({"type": "agentMessage", "text": "hi"})));
        a.feed(&started("r1", json!({"type": "reasoning"})));
        a.feed(&started("s1", json!({"type": "subAgentActivity", "agentPath": "x", "agentThreadId": "y", "kind": "started"})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_tool_name_is_cleaned_and_cut() {
        let mut a = seat();
        let long = "x".repeat(200);
        a.feed(&started("t1", json!({"type": "mcpToolCall", "server": "a\u{1b}[31mb", "tool": long, "arguments": {}, "status": "inProgress"})));
        let line = a.display().unwrap().line;
        assert!(!line.contains('\u{1b}'));
        assert!(line.starts_with("ツール使用中（a[31mb/"));
        assert_eq!(line.matches('x').count(), NAME_MAX);
    }

    #[test]
    fn turn_completion_clears_items_and_flags() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        a.feed(&started("c2", command()));
        a.feed(&status(json!({"type": "active", "activeFlags": ["waitingOnApproval"]})));
        a.feed(&note("turn/completed", json!({"threadId": T, "turn": {"id": "turn-1", "items": [], "status": "interrupted"}})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_thread_gone_idle_leaves_nothing_behind() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        a.feed(&status(json!({"type": "idle"})));
        assert_eq!(a.display(), None);
        a.feed(&started("c2", command()));
        a.feed(&status(json!({"type": "systemError"})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn disconnect_is_unknown_and_stays_so() {
        let mut a = seat();
        a.feed(&status(json!({"type": "active", "activeFlags": ["waitingOnApproval"]})));
        a.feed(&started("c1", command()));
        a.disconnect();
        assert_eq!(a.display(), None);
        // Nothing read after the end is believed.
        a.feed(&started("c2", command()));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn notifications_before_the_thread_is_known_are_not_lost() {
        let mut a = Activity::new();
        a.feed(&status(json!({"type": "active", "activeFlags": []})));
        a.feed(&started("c1", command()));
        assert_eq!(a.display(), None);
        a.set_thread(T);
        assert_eq!(words(&a), Some(("実行中", "コマンド実行中".into())));
    }

    #[test]
    fn early_notifications_of_another_thread_are_dropped_on_replay() {
        let mut a = Activity::new();
        a.feed(&note("item/started", json!({"threadId": "other", "turnId": "x", "startedAtMs": 1, "item": {"id": "c9", "type": "commandExecution"}})));
        a.set_thread(T);
        assert_eq!(a.display(), None);
    }

    #[test]
    fn another_threads_notifications_are_not_this_seats() {
        let mut a = seat();
        a.feed(&note("thread/status/changed", json!({"threadId": "sub", "status": {"type": "active", "activeFlags": ["waitingOnApproval"]}})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn requests_and_answers_are_not_read() {
        let mut a = seat();
        a.feed(&json!({"id": 7, "method": "item/commandExecution/requestApproval", "params": {"threadId": T, "itemId": "c1"}}));
        a.feed(&json!({"id": 3, "result": {}}));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn the_start_answer_is_a_snapshot() {
        let mut a = seat();
        a.snapshot(&json!({"type": "active", "activeFlags": ["waitingOnUserInput"]}));
        assert_eq!(words(&a).unwrap().0, "答え待ち");
        a.snapshot(&json!({"type": "idle"}));
        assert_eq!(a.display(), None);
    }

    fn reporter() -> Reporter {
        Reporter::new("topic", "acct", "pty-1")
    }

    #[test]
    fn the_first_event_says_connected_and_quiet() {
        let a = seat();
        let mut r = reporter();
        assert_eq!(
            r.next(&a),
            Some(json!({"topic_id": "topic", "account_id": "acct", "pty_id": "pty-1", "connected": true, "word": null, "line": null, "waiting": false}))
        );
        assert_eq!(r.next(&a), None);
    }

    #[test]
    fn idle_then_disconnect_still_sends_the_disconnect() {
        let mut a = seat();
        let mut r = reporter();
        r.next(&a);
        a.feed(&status(json!({"type": "idle"})));
        assert_eq!(r.next(&a), None);
        a.disconnect();
        assert_eq!(
            r.next(&a),
            Some(json!({"topic_id": "topic", "account_id": "acct", "pty_id": "pty-1", "connected": false, "word": null, "line": null, "waiting": false}))
        );
        assert_eq!(r.next(&a), None);
    }

    #[test]
    fn running_then_disconnect_drops_the_word_and_says_disconnected() {
        let mut a = seat();
        let mut r = reporter();
        r.next(&a);
        a.feed(&started("c1", command()));
        assert_eq!(
            r.next(&a),
            Some(json!({"topic_id": "topic", "account_id": "acct", "pty_id": "pty-1", "connected": true, "word": "実行中", "line": "コマンド実行中", "waiting": false}))
        );
        a.disconnect();
        assert_eq!(
            r.next(&a),
            Some(json!({"topic_id": "topic", "account_id": "acct", "pty_id": "pty-1", "connected": false, "word": null, "line": null, "waiting": false}))
        );
    }

    #[test]
    fn an_unchanged_display_sends_nothing() {
        let mut a = seat();
        let mut r = reporter();
        r.next(&a);
        a.feed(&started("c1", command()));
        assert!(r.next(&a).is_some());
        a.feed(&note("item/commandExecution/outputDelta", json!({"threadId": T, "itemId": "c1", "delta": "x"})));
        assert_eq!(r.next(&a), None);
    }

    #[test]
    fn a_new_turn_drops_what_the_last_one_left() {
        let mut a = seat();
        a.feed(&started("c1", command()));
        a.feed(&note("turn/started", json!({"threadId": T, "turn": {"id": "turn-2", "items": [], "status": "inProgress"}})));
        assert_eq!(a.display(), None);
    }
}
