//! What a Claude Code seat is doing, read off the hooks its launch declares
//! (#331). See docs/2-screen.md, "走っているアカウントが何をしているか".
//!
//! The seat's `--settings` carries HTTP hooks (`activity_hook_settings`) that
//! post each event below to the room's port, the event named by the path. This
//! turns them into the same `seat-activity` event a Codex app-server seat sends
//! (`codex::activity`), so the screen reads both seats one way: the one word
//! the participant panel's row has room for, and the longer form the line
//! under the room says.
//!
//! **Five fields of the body are read, and nothing else** (Claude Code docs,
//! `hooks`, read 2026-10-08): `hook_event_name`, `tool_use_id`, `tool_name`,
//! `agent_id`, `agent_type` — plus, on `Stop` only, the `type` of each entry of
//! `background_tasks`. No tool input, tool response, prompt or message is read.
//!
//! | event | read |
//! |---|---|
//! | `PreToolUse`, `PostToolUse`, `PostToolUseFailure` | `tool_use_id`, `tool_name`, `agent_id` |
//! | `PermissionRequest` | `tool_name`, `agent_id` (it carries no `tool_use_id`) |
//! | `SubagentStart`, `SubagentStop` | `agent_id`, `agent_type` |
//! | `Stop` | `background_tasks[].type` |
//! | `UserPromptSubmit` | nothing but the event |
//!
//! `agent_id` is present only when the hook fires inside a subagent, and tool
//! events inside one carry it (same docs, "Common input fields").
//!
//! **A body that cannot be read is not guessed at**, with two exceptions where
//! the path alone says enough. The app stops reading a body at 4 MiB, and a
//! tool's input or response can be longer. A `PostToolUse` or
//! `PostToolUseFailure` that cannot be read says a tool ended without saying
//! which, so every running tool and every permission wait is dropped — a
//! parallel tool may leave the badge early, which is better than a tool shown
//! running for good. `Stop` and `UserPromptSubmit` clear the main agent's turn
//! whatever their body says. Every other unreadable event is not counted. A
//! body that is JSON but names another event (`hook_event_name`) is not read
//! at all, whatever the path says.
//!
//! **Tools are a set**, keyed by `tool_use_id`, because they run in parallel.
//! A subagent's tools are counted under that subagent and never as the main
//! agent's. The `Agent` tool (and `Task`, its older name) is not counted as a
//! tool: the subagent it starts is what is shown, by `SubagentStart`.
//!
//! **Subagents are a set**, keyed by `agent_id`, from `SubagentStart` to
//! `SubagentStop`. One whose `agent_type` is empty at the start is not counted:
//! the CLI's own internal agents report it so (same docs, "SubagentStop").
//!
//! **A permission wait** starts at `PermissionRequest` and ends at a key typed
//! to answer it (the main agent's wait only, next paragraph), at that
//! agent's next `PreToolUse`, or at a `PostToolUse` / `PostToolUseFailure` of
//! the same tool name (the request carries no `tool_use_id`). A wait that ends
//! at the next `PreToolUse` was denied — a denial fires neither post event
//! (same docs, "PostToolUseFailure") — so the calls of that tool name that were
//! running when the wait began are dropped with it. A subagent's wait is a wait
//! too: in an interactive session a background subagent's prompt surfaces in
//! the main session and waits there (Claude Code docs, `sub-agents`, "Run
//! subagents in foreground or background", read 2026-10-08).
//!
//! **An answer typed into the seat's terminal closes the main agent's wait**
//! (`Agent::answered`, PR #332 review). `PreToolUse` comes before the prompt,
//! so a tool the person allows sends nothing more until it ends: without this,
//! an allowed `cargo build` would read 許可待ち for its whole run. Only a key
//! that can answer the prompt counts (`confirms_prompt`): Enter, Esc, a digit
//! (the prompt's numbered choices), `y` or `n`. Moving the cursor answers
//! nothing. A key is not evidence of approval, so the close claims nothing
//! about the tool: the wait is dropped together with the call it asked about,
//! and the badge falls back to the screen's own words — 出力中 while the allowed
//! tool prints, 待機 when the terminal is silent. Saying ツール here would be a
//! state not observed (#82). Only the person's own keys reach this; what the
//! room types into the terminal goes another way. A subagent's wait is left to
//! its own ends.
//!
//! **Nothing outlives what it describes.** `Stop` and `UserPromptSubmit` clear
//! the main agent's tools and wait: an interrupted turn sends no end of its
//! own (`Stop` does not run on a user interrupt, same docs, "Stop"). They do
//! not clear subagents, which run in the background past the turn that started
//! them by default; a `Stop` whose `background_tasks` lists no subagent clears
//! them, and their tools and waits with them.
//!
//! **What reaches the screen is decided here too** (`Reporter`): one
//! `seat-activity` per change of what is shown. A hook seat has no connection
//! to lose and no thread status to tell, so `connected` is always true and
//! `thread_status` always null — with no word the screen keeps its own four.
use serde_json::{json, Value};

/// The most characters of a tool's or an agent type's name shown, as for a
/// Codex seat's tool (`codex::activity`).
const NAME_MAX: usize = 48;

/// The most tools one agent, and the most subagents one seat, is held to.
/// Every entry has an end; the cap only keeps ends that never came from
/// growing this without bound. The oldest goes first.
const HELD_MAX: usize = 64;

/// The tool names that start a subagent, which is shown as the subagent.
const AGENT_TOOLS: &[&str] = &["Agent", "Task"];

/// A nonempty name, without control characters and cut to `NAME_MAX`.
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

/// A nonempty identifier, kept as sent: it is a key, never shown.
fn id(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The fields one hook's body is read for.
struct Heard {
    tool_use_id: Option<String>,
    tool_name: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    /// `Stop` only: whether `background_tasks` lists a subagent, or `None`
    /// when the body carries no such list.
    subagent_in_flight: Option<bool>,
}

/// One hook's body, as far as it could be read.
enum Body {
    /// A JSON object naming the event its path names.
    Heard(Heard),
    /// Not JSON: cut at the app's limit, or not sent whole.
    Cut,
    /// JSON, but not a body of this event. Not read at all.
    Other,
}

impl Body {
    fn read(event: &str, body: &[u8]) -> Body {
        let Ok(data) = serde_json::from_slice::<Value>(body) else {
            return Body::Cut;
        };
        if data["hook_event_name"].as_str() != Some(event) {
            return Body::Other;
        }
        let subagent_in_flight = data["background_tasks"].as_array().map(|tasks| {
            tasks.iter().any(|task| task["type"].as_str() == Some("subagent"))
        });
        Body::Heard(Heard {
            tool_use_id: id(&data["tool_use_id"]),
            tool_name: text(&data["tool_name"]),
            agent_id: id(&data["agent_id"]),
            agent_type: text(&data["agent_type"]),
            subagent_in_flight,
        })
    }
}

/// A permission prompt one agent is stopped on.
#[derive(Debug, Default)]
struct Wait {
    /// The tool asked about, or empty when the request did not name one.
    tool: String,
    /// The calls of that tool that were running when the prompt came: the
    /// one asked about is among them, and goes if the prompt was denied.
    asked: Vec<String>,
}

/// What one agent — the main one, or a subagent — is doing.
#[derive(Debug, Default)]
struct Agent {
    /// Running tools, oldest first, as `(tool_use_id, name)`.
    tools: Vec<(String, String)>,
    wait: Option<Wait>,
}

impl Agent {
    fn pre(&mut self, use_id: Option<String>, name: Option<String>) {
        // The next call means the prompt was answered: allowed and the tool
        // already ran (its post would have ended the wait), or denied.
        if let Some(wait) = self.wait.take() {
            self.tools.retain(|(id, _)| !wait.asked.contains(id));
        }
        let (Some(use_id), Some(name)) = (use_id, name) else {
            return;
        };
        if AGENT_TOOLS.contains(&name.as_str()) {
            return;
        }
        self.tools.retain(|(id, _)| *id != use_id);
        self.tools.push((use_id, name));
        if self.tools.len() > HELD_MAX {
            self.tools.remove(0);
        }
    }

    fn post(&mut self, use_id: Option<&str>, name: Option<&str>) {
        if let Some(use_id) = use_id {
            self.tools.retain(|(id, _)| id != use_id);
        }
        if self.wait.as_ref().is_some_and(|wait| Some(wait.tool.as_str()) == name) {
            self.wait = None;
        }
    }

    fn ask(&mut self, name: Option<String>) {
        let tool = name.unwrap_or_default();
        let asked = self
            .tools
            .iter()
            .filter(|(_, running)| *running == tool)
            .map(|(id, _)| id.clone())
            .collect();
        self.wait = Some(Wait { tool, asked });
    }

    /// The person answered the prompt with a key: the wait goes, and the call
    /// it asked about with it — whether it now runs is not known.
    fn answered(&mut self) {
        if let Some(wait) = self.wait.take() {
            self.tools.retain(|(id, _)| !wait.asked.contains(id));
        }
    }

    fn clear(&mut self) {
        self.tools.clear();
        self.wait = None;
    }
}

/// Whether one write of the person's keys into a terminal is a key that can
/// answer a permission prompt: Enter, Esc, a digit, `y` or `n` (either case).
/// A write is one key as the terminal sends it; an arrow key, any other escape
/// sequence (focus and cursor reports among them), a paste or other text is
/// not an answer.
pub fn confirms_prompt(data: &str) -> bool {
    match data {
        "\r" | "\n" | "\r\n" | "\u{1b}" => true,
        _ => {
            let mut chars = data.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some(c), None) if c.is_ascii_digit() || matches!(c, 'y' | 'Y' | 'n' | 'N')
            )
        }
    }
}

/// One subagent, from its start to its stop.
#[derive(Debug)]
struct Subagent {
    id: String,
    kind: String,
    agent: Agent,
}

/// What the screen is told.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// The row's badge.
    pub word: &'static str,
    /// The line under the room, after 「名前が」, and the badge's title.
    pub line: String,
    /// A wait on the person (許可待ち), rather than work under way.
    pub waiting: bool,
}

/// The badge words, four characters at most (docs/2-screen.md).
pub const WAIT_WORD: &str = "許可待ち";
pub const TOOL_WORD: &str = "ツール";
pub const SUBAGENT_WORD: &str = "委任中";

/// One Claude Code seat's state, fed every hook its launch posts.
#[derive(Debug, Default)]
pub struct Activity {
    main: Agent,
    /// Running subagents, oldest first.
    subagents: Vec<Subagent>,
}

impl Activity {
    pub fn new() -> Self {
        Self::default()
    }

    /// One hook: `event` is the path's (`parse_activity_hook_target`), `body`
    /// what the CLI posted, as far as the app read it.
    pub fn hear(&mut self, event: &str, body: &[u8]) {
        let heard = match Body::read(event, body) {
            Body::Other => return,
            Body::Cut => None,
            Body::Heard(heard) => Some(heard),
        };
        match (event, heard) {
            // The path is enough for these: see "A body that cannot be read".
            ("UserPromptSubmit", _) => self.main.clear(),
            ("Stop", heard) => {
                self.main.clear();
                if heard.and_then(|h| h.subagent_in_flight) == Some(false) {
                    self.subagents.clear();
                }
            }
            ("PostToolUse" | "PostToolUseFailure", None) => {
                self.main.clear();
                for sub in &mut self.subagents {
                    sub.agent.clear();
                }
            }
            (_, None) => {}
            ("PreToolUse", Some(h)) => {
                if let Some(agent) = self.agent(h.agent_id.as_deref()) {
                    agent.pre(h.tool_use_id, h.tool_name);
                }
            }
            ("PostToolUse" | "PostToolUseFailure", Some(h)) => {
                if let Some(agent) = self.agent(h.agent_id.as_deref()) {
                    agent.post(h.tool_use_id.as_deref(), h.tool_name.as_deref());
                }
            }
            ("PermissionRequest", Some(h)) => {
                if let Some(agent) = self.agent(h.agent_id.as_deref()) {
                    agent.ask(h.tool_name);
                }
            }
            ("SubagentStart", Some(h)) => {
                let (Some(id), Some(kind)) = (h.agent_id, h.agent_type) else {
                    return;
                };
                // A resumed subagent starts again under its own id: one entry,
                // now the latest.
                let agent = match self.subagents.iter().position(|sub| sub.id == id) {
                    Some(at) => self.subagents.remove(at).agent,
                    None => Agent::default(),
                };
                self.subagents.push(Subagent { id, kind, agent });
                if self.subagents.len() > HELD_MAX {
                    self.subagents.remove(0);
                }
            }
            ("SubagentStop", Some(h)) => {
                if let Some(id) = h.agent_id {
                    self.subagents.retain(|sub| sub.id != id);
                }
            }
            _ => {}
        }
    }

    /// The person typed `data` into this seat's terminal. A key that can answer
    /// a prompt (`confirms_prompt`) closes the main agent's wait.
    pub fn typed(&mut self, data: &str) {
        if confirms_prompt(data) {
            self.main.answered();
        }
    }

    /// The agent a hook is about: the main one when it carries no `agent_id`,
    /// a counted subagent when it does, and none for any other — an internal
    /// agent's, or one whose start was not heard.
    fn agent(&mut self, agent_id: Option<&str>) -> Option<&mut Agent> {
        match agent_id {
            None => Some(&mut self.main),
            Some(id) => self
                .subagents
                .iter_mut()
                .find(|sub| sub.id == id)
                .map(|sub| &mut sub.agent),
        }
    }

    /// What the seat is doing, or `None` when this says nothing and the
    /// screen's own words stand. A wait outranks a tool, and a tool of the
    /// main agent outranks a subagent.
    pub fn display(&self) -> Option<Display> {
        let waited = |kind: Option<&str>, wait: &Wait| {
            let line = match (kind, wait.tool.as_str()) {
                (None, "") => "許可待ち".to_string(),
                (None, tool) => format!("許可待ち（{tool}）"),
                (Some(kind), "") => format!("許可待ち（{kind}）"),
                (Some(kind), tool) => format!("許可待ち（{kind}：{tool}）"),
            };
            Display { word: WAIT_WORD, line, waiting: true }
        };
        if let Some(wait) = &self.main.wait {
            return Some(waited(None, wait));
        }
        if let Some(sub) = self.subagents.iter().rev().find(|sub| sub.agent.wait.is_some()) {
            return Some(waited(Some(&sub.kind), sub.agent.wait.as_ref()?));
        }
        let others = |count: usize| {
            if count > 1 {
                format!("ほか {} 件", count - 1)
            } else {
                String::new()
            }
        };
        if let Some((_, name)) = self.main.tools.last() {
            let line = format!("ツール使用中（{name}）{}", others(self.main.tools.len()));
            return Some(Display { word: TOOL_WORD, line, waiting: false });
        }
        let sub = self.subagents.last()?;
        let what = match sub.agent.tools.last() {
            Some((_, tool)) => format!("{}：{tool}", sub.kind),
            None => sub.kind.clone(),
        };
        let line = format!("サブエージェント実行中（{what}）{}", others(self.subagents.len()));
        Some(Display { word: SUBAGENT_WORD, line, waiting: false })
    }
}

/// The `seat-activity` events one launch's hooks send the screen: its seat,
/// and what it last sent.
#[derive(Debug)]
pub struct Reporter {
    topic_id: String,
    account_id: String,
    pty_id: String,
    sent: Option<Option<Display>>,
}

impl Reporter {
    /// `pty_id` is the launch's terminal, so a relaunch in the same seat is
    /// told apart from the run before it.
    pub fn new(topic_id: &str, account_id: &str, pty_id: &str) -> Self {
        Reporter {
            topic_id: topic_id.into(),
            account_id: account_id.into(),
            pty_id: pty_id.into(),
            sent: None,
        }
    }

    /// The launch this reports for.
    pub fn pty_id(&self) -> &str {
        &self.pty_id
    }

    /// The event to send now, or `None` when what is shown has not changed
    /// since the last one. The first call always sends.
    pub fn next(&mut self, activity: &Activity) -> Option<Value> {
        let now = activity.display();
        if self.sent.as_ref() == Some(&now) {
            return None;
        }
        let event = json!({
            "topic_id": self.topic_id,
            "account_id": self.account_id,
            "pty_id": self.pty_id,
            "connected": true,
            "thread_status": null,
            "word": now.as_ref().map(|d| d.word),
            "line": now.as_ref().map(|d| d.line.clone()),
            "waiting": now.as_ref().is_some_and(|d| d.waiting),
        });
        self.sent = Some(now);
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(event: &str, fields: Value) -> Vec<u8> {
        let mut data = json!({
            "session_id": "s", "transcript_path": "/t.jsonl", "cwd": "/w",
            "permission_mode": "default", "hook_event_name": event,
        });
        for (key, value) in fields.as_object().unwrap() {
            data[key] = value.clone();
        }
        serde_json::to_vec(&data).unwrap()
    }

    fn pre(a: &mut Activity, id: &str, tool: &str) {
        a.hear("PreToolUse", &body("PreToolUse", json!({"tool_name": tool, "tool_input": {"command": "rm -rf secret"}, "tool_use_id": id})));
    }

    fn post(a: &mut Activity, id: &str, tool: &str) {
        a.hear("PostToolUse", &body("PostToolUse", json!({"tool_name": tool, "tool_input": {}, "tool_response": {"output": "secret"}, "tool_use_id": id})));
    }

    fn sub_pre(a: &mut Activity, agent: &str, id: &str, tool: &str) {
        a.hear("PreToolUse", &body("PreToolUse", json!({"tool_name": tool, "tool_input": {}, "tool_use_id": id, "agent_id": agent, "agent_type": "Explore"})));
    }

    fn start(a: &mut Activity, agent: &str, kind: &str) {
        a.hear("SubagentStart", &body("SubagentStart", json!({"agent_id": agent, "agent_type": kind})));
    }

    fn stop_sub(a: &mut Activity, agent: &str, kind: &str) {
        a.hear("SubagentStop", &body("SubagentStop", json!({"agent_id": agent, "agent_type": kind, "stop_hook_active": false, "last_assistant_message": "done"})));
    }

    fn ask(a: &mut Activity, tool: &str) {
        a.hear("PermissionRequest", &body("PermissionRequest", json!({"tool_name": tool, "tool_input": {"command": "rm -rf secret"}})));
    }

    fn words(a: &Activity) -> Option<(&'static str, String, bool)> {
        a.display().map(|d| (d.word, d.line, d.waiting))
    }

    #[test]
    fn a_tool_shows_its_name_and_never_its_input() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Bash");
        assert_eq!(words(&a), Some(("ツール", "ツール使用中（Bash）".into(), false)));
        assert!(!a.display().unwrap().line.contains("rm"));
        post(&mut a, "t1", "Bash");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn parallel_tools_are_a_set() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Read");
        pre(&mut a, "t2", "Grep");
        assert_eq!(words(&a).unwrap().1, "ツール使用中（Grep）ほか 1 件");
        a.hear("PostToolUseFailure", &body("PostToolUseFailure", json!({"tool_name": "Grep", "tool_input": {}, "tool_use_id": "t2", "error": "x"})));
        assert_eq!(words(&a).unwrap().1, "ツール使用中（Read）");
        // The same call heard twice is one.
        pre(&mut a, "t1", "Read");
        assert_eq!(words(&a).unwrap().1, "ツール使用中（Read）");
        post(&mut a, "t1", "Read");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_permission_wait_outranks_tools_and_its_tools_end_also_ends_it() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Read");
        pre(&mut a, "t2", "Bash");
        ask(&mut a, "Bash");
        assert_eq!(words(&a), Some(("許可待ち", "許可待ち（Bash）".into(), true)));
        assert!(!a.display().unwrap().line.contains("rm"));
        // Another tool ending does not end the wait.
        post(&mut a, "t1", "Read");
        assert_eq!(words(&a).unwrap().0, "許可待ち");
        // Answered where no key of the pane was seen (another hook decided,
        // say): the tool runs, and its end ends the wait.
        post(&mut a, "t2", "Bash");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn an_answer_typed_into_the_terminal_drops_the_wait_and_claims_nothing() {
        for key in ["\r", "\n", "\r\n", "\u{1b}", "1", "2", "y", "Y", "n", "N"] {
            let mut a = Activity::new();
            pre(&mut a, "t0", "Read");
            pre(&mut a, "t1", "Bash");
            ask(&mut a, "Bash");
            a.typed(key);
            // The wait and the call it asked about go; the other tool stays.
            assert_eq!(words(&a), Some(("ツール", "ツール使用中（Read）".into(), false)), "{key:?}");
            post(&mut a, "t0", "Read");
            // Nothing is claimed about the allowed call: the screen's words stand.
            assert_eq!(a.display(), None, "{key:?}");
            // Its end, when it comes, changes nothing.
            post(&mut a, "t1", "Bash");
            assert_eq!(a.display(), None, "{key:?}");
        }
    }

    #[test]
    fn moving_in_the_prompt_answers_nothing() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Bash");
        ask(&mut a, "Bash");
        for key in [
            "\u{1b}[A", "\u{1b}[B", "\u{1b}OB", "\u{1b}[C", "\t", "\u{1b}[I", "\u{1b}[O",
            "\u{1b}[12;3R", "a", "q", " ", "yes", "12", "\u{1b}[200~y\u{1b}[201~", "",
        ] {
            a.typed(key);
            assert_eq!(words(&a).unwrap().0, "許可待ち", "{key:?}");
        }
    }

    #[test]
    fn a_typed_answer_leaves_a_subagents_wait_alone() {
        let mut a = Activity::new();
        start(&mut a, "ag1", "Explore");
        sub_pre(&mut a, "ag1", "s1", "Bash");
        a.hear("PermissionRequest", &body("PermissionRequest", json!({"tool_name": "Bash", "tool_input": {}, "agent_id": "ag1", "agent_type": "Explore"})));
        a.typed("\r");
        assert_eq!(words(&a).unwrap().0, "許可待ち");
        // And with no wait at all, a key does nothing.
        let mut b = Activity::new();
        pre(&mut b, "t1", "Bash");
        b.typed("\r");
        assert_eq!(words(&b).unwrap().0, "ツール");
    }

    #[test]
    fn a_denied_tool_goes_with_its_wait_at_the_next_call() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Bash");
        ask(&mut a, "Bash");
        // Denied: no post comes, and the model goes on to another call.
        pre(&mut a, "t2", "Read");
        assert_eq!(words(&a), Some(("ツール", "ツール使用中（Read）".into(), false)));
        post(&mut a, "t2", "Read");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn stop_and_a_new_prompt_clear_the_turn() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Bash");
        ask(&mut a, "Bash");
        a.hear("Stop", &body("Stop", json!({"stop_hook_active": false, "last_assistant_message": "x", "background_tasks": [], "session_crons": []})));
        assert_eq!(a.display(), None);
        // Interrupted: no Stop, and the next prompt is the backstop.
        pre(&mut a, "t2", "Bash");
        a.hear("UserPromptSubmit", &body("UserPromptSubmit", json!({"prompt": "secret"})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn stop_and_a_new_prompt_clear_even_when_their_body_is_unreadable() {
        let mut a = Activity::new();
        pre(&mut a, "t1", "Bash");
        a.hear("UserPromptSubmit", b"{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"cut");
        assert_eq!(a.display(), None);
        pre(&mut a, "t2", "Bash");
        a.hear("Stop", b"");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_truncated_post_drops_every_running_tool_and_wait() {
        let mut a = Activity::new();
        start(&mut a, "ag1", "Explore");
        sub_pre(&mut a, "ag1", "s1", "Grep");
        pre(&mut a, "t1", "Read");
        pre(&mut a, "t2", "Bash");
        ask(&mut a, "Bash");
        // A body cut at the app's limit: the event is the path's, the tool is
        // not known, so nothing running is kept.
        let mut cut = body("PostToolUse", json!({"tool_name": "Read", "tool_use_id": "t1", "tool_response": {"content": "x".repeat(64)}}));
        cut.truncate(cut.len() / 2);
        a.hear("PostToolUse", &cut);
        // The subagent itself is still running, without its tool.
        assert_eq!(words(&a), Some(("委任中", "サブエージェント実行中（Explore）".into(), false)));
        pre(&mut a, "t3", "Read");
        a.hear("PostToolUseFailure", b"not json");
        assert_eq!(words(&a).unwrap().0, "委任中");
    }

    #[test]
    fn other_unreadable_events_are_not_counted() {
        let mut a = Activity::new();
        let mut cut = body("PreToolUse", json!({"tool_name": "Write", "tool_use_id": "t1", "tool_input": {"content": "x".repeat(64)}}));
        cut.truncate(cut.len() - 10);
        a.hear("PreToolUse", &cut);
        a.hear("SubagentStart", b"{");
        a.hear("PermissionRequest", b"");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_body_naming_another_event_is_not_read() {
        let mut a = Activity::new();
        a.hear("PreToolUse", &body("PostToolUse", json!({"tool_name": "Bash", "tool_use_id": "t1"})));
        assert_eq!(a.display(), None);
        pre(&mut a, "t1", "Bash");
        a.hear("PostToolUse", &body("PreToolUse", json!({"tool_name": "Bash", "tool_use_id": "t1"})));
        assert_eq!(words(&a).unwrap().0, "ツール");
    }

    #[test]
    fn a_subagent_shows_until_it_stops_and_its_tools_stay_its_own() {
        let mut a = Activity::new();
        // The Agent call itself is the subagent, not a tool.
        pre(&mut a, "t0", "Agent");
        start(&mut a, "ag1", "Explore");
        assert_eq!(words(&a), Some(("委任中", "サブエージェント実行中（Explore）".into(), false)));
        sub_pre(&mut a, "ag1", "s1", "Grep");
        assert_eq!(words(&a), Some(("委任中", "サブエージェント実行中（Explore：Grep）".into(), false)));
        // The main agent's tool outranks the subagent.
        pre(&mut a, "t1", "Read");
        assert_eq!(words(&a).unwrap().1, "ツール使用中（Read）");
        post(&mut a, "t1", "Read");
        start(&mut a, "ag2", "Plan");
        assert_eq!(words(&a).unwrap().1, "サブエージェント実行中（Plan）ほか 1 件");
        stop_sub(&mut a, "ag2", "Plan");
        stop_sub(&mut a, "ag1", "Explore");
        post(&mut a, "t0", "Agent");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn internal_agents_are_not_counted() {
        let mut a = Activity::new();
        start(&mut a, "int", "");
        assert_eq!(a.display(), None);
        // Nor are tools of an agent whose start was not counted.
        sub_pre(&mut a, "int", "s1", "Read");
        assert_eq!(a.display(), None);
        stop_sub(&mut a, "int", "");
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_subagents_permission_prompt_is_a_wait() {
        let mut a = Activity::new();
        start(&mut a, "ag1", "general-purpose");
        sub_pre(&mut a, "ag1", "s1", "Bash");
        a.hear("PermissionRequest", &body("PermissionRequest", json!({"tool_name": "Bash", "tool_input": {}, "agent_id": "ag1", "agent_type": "general-purpose"})));
        assert_eq!(words(&a), Some(("許可待ち", "許可待ち（general-purpose：Bash）".into(), true)));
        // A main-agent call does not answer the subagent's prompt.
        pre(&mut a, "t1", "Read");
        assert_eq!(words(&a).unwrap().0, "許可待ち");
        // The subagent going on to its next call does.
        sub_pre(&mut a, "ag1", "s2", "Read");
        assert_eq!(words(&a).unwrap().0, "ツール");
        post(&mut a, "t1", "Read");
        assert_eq!(words(&a).unwrap().1, "サブエージェント実行中（general-purpose：Read）");
    }

    #[test]
    fn a_turns_end_keeps_background_subagents() {
        let mut a = Activity::new();
        start(&mut a, "ag1", "Explore");
        pre(&mut a, "t1", "Bash");
        a.hear("UserPromptSubmit", &body("UserPromptSubmit", json!({"prompt": "x"})));
        assert_eq!(words(&a).unwrap().0, "委任中");
        a.hear("Stop", &body("Stop", json!({"background_tasks": [{"id": "b1", "type": "subagent", "status": "running", "agent_type": "Explore"}]})));
        assert_eq!(words(&a).unwrap().0, "委任中");
        // Not known either way: kept.
        a.hear("Stop", &body("Stop", json!({})));
        a.hear("Stop", b"cut");
        assert_eq!(words(&a).unwrap().0, "委任中");
        // The CLI says no subagent is in flight: whatever is held is stale.
        a.hear("Stop", &body("Stop", json!({"background_tasks": [{"id": "b2", "type": "shell", "status": "running"}]})));
        assert_eq!(a.display(), None);
    }

    #[test]
    fn a_resumed_subagent_is_one_entry() {
        let mut a = Activity::new();
        start(&mut a, "ag1", "Explore");
        start(&mut a, "ag2", "Plan");
        start(&mut a, "ag1", "Explore");
        assert_eq!(words(&a).unwrap().1, "サブエージェント実行中（Explore）ほか 1 件");
    }

    #[test]
    fn names_are_cleaned_and_cut() {
        let mut a = Activity::new();
        let long = format!("mcp__a\u{1b}[31m__{}", "x".repeat(200));
        pre(&mut a, "t1", &long);
        let line = a.display().unwrap().line;
        assert!(!line.contains('\u{1b}'));
        assert_eq!(line.chars().filter(|c| *c == 'x').count(), NAME_MAX - "mcp__a[31m__".len());
    }

    #[test]
    fn what_is_held_is_bounded() {
        let mut a = Activity::new();
        for n in 0..(HELD_MAX + 10) {
            pre(&mut a, &format!("t{n}"), "Read");
            start(&mut a, &format!("ag{n}"), "Explore");
        }
        assert_eq!(a.main.tools.len(), HELD_MAX);
        assert_eq!(a.subagents.len(), HELD_MAX);
    }

    fn event(word: Option<&str>, line: Option<&str>, waiting: bool) -> Option<Value> {
        Some(json!({
            "topic_id": "topic", "account_id": "acct", "pty_id": "pty-1",
            "connected": true, "thread_status": null,
            "word": word, "line": line, "waiting": waiting,
        }))
    }

    #[test]
    fn the_reporter_sends_each_change_once_in_the_codex_seats_shape() {
        let mut a = Activity::new();
        let mut r = Reporter::new("topic", "acct", "pty-1");
        assert_eq!(r.pty_id(), "pty-1");
        assert_eq!(r.next(&a), event(None, None, false));
        assert_eq!(r.next(&a), None);
        pre(&mut a, "t1", "Bash");
        assert_eq!(r.next(&a), event(Some("ツール"), Some("ツール使用中（Bash）"), false));
        ask(&mut a, "Bash");
        assert_eq!(r.next(&a), event(Some("許可待ち"), Some("許可待ち（Bash）"), true));
        post(&mut a, "t1", "Bash");
        assert_eq!(r.next(&a), event(None, None, false));
        assert_eq!(r.next(&a), None);
    }
}
