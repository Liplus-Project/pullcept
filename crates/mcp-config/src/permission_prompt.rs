//! A Claude Code seat's permission prompt, answered from the room (#336). See
//! docs/2-screen.md, "許可の確認を部屋から答える".
//!
//! The seat's `PermissionRequest` hook (`activity_hook_settings`) is held open
//! by the app for up to `HOLD_SECS`, while the room shows a card saying what is
//! asked and the person at the screen may press 拒否 / 一度だけ許可 / 常に許可.
//! The terminal's own prompt is shown all the while (real device, 2026-10-08:
//! the prompt appeared about 3 seconds into a 30-second hook), so a card nobody
//! presses costs nothing: the hold ends with `{}`, which decides nothing.
//!
//! This module is what a request says and what an answer is, free of tauri so
//! that it is tested. Holding the connection, and who may answer, are the
//! app's (`src-tauri/src/hook_activity.rs`, `room::serve_hook`).
//!
//! **What is read** (Claude Code docs, `hooks`, "PermissionRequest input",
//! read 2026-10-08): `tool_name`, `tool_input`, `mcp_server.name`, `agent_id`,
//! `agent_type` and `permission_suggestions`. The input is shown to the person
//! on the card and goes nowhere else: not into the room's log, not typed into
//! any terminal. That revises #332's rule of never showing a tool's input for
//! this one surface, because judging a request needs its content.
//!
//! **常に許可 is an echo of the CLI's own suggestion** (same docs, "Permission
//! update entries": "A hook can echo one of the `permission_suggestions` it
//! received as its own `updatedPermissions` output"). Only `addRules` entries
//! whose `behavior` is `allow` are echoed — a suggestion to change the
//! permission mode is a different thing from always allowing this tool, and is
//! not offered. A request carrying no such entry has no 常に許可.
use serde_json::{json, Value};

/// How long the app holds a permission hook open for the room's answer.
///
/// Well under the 600-second default (same docs, "timeout"), so the hook's
/// timeout can sit just above it, and short enough that a hook the CLI keeps
/// waiting on after the terminal answered (not observed; see the module docs
/// of `src-tauri/src/hook_activity.rs`) costs at most this.
pub const HOLD_SECS: u64 = 180;

/// The `timeout` the `PermissionRequest` hook is declared with: just above
/// `HOLD_SECS`, so the app's own `{}` arrives before the CLI gives up on it.
/// Every other activity hook keeps `ACTIVITY_HOOK_TIMEOUT_SECS`.
pub const HOOK_TIMEOUT_SECS: u64 = HOLD_SECS + 10;

/// The event this module reads.
pub const EVENT: &str = "PermissionRequest";

/// The most characters of one input field shown on the card.
const VALUE_MAX: usize = 2000;

/// The most input fields shown on the card.
const FIELDS_MAX: usize = 24;

/// The most characters of a name (tool, server, agent type) shown.
const NAME_MAX: usize = 120;

/// One field of the tool's input, as the card shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub value: String,
    /// The value was longer than the card shows.
    pub cut: bool,
}

/// One permission request, as far as the card needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The tool's name as sent, for the card's title.
    pub tool_name: String,
    /// The MCP server, for a tool of one; `None` otherwise.
    pub server: Option<String>,
    /// The tool's own name: the part after the server for an MCP tool, the
    /// whole name otherwise.
    pub tool: String,
    /// The subagent asking, or `None` for the main agent.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub fields: Vec<Field>,
    /// The input fields left off the card (`FIELDS_MAX`).
    pub more_fields: usize,
    /// The suggestions 常に許可 echoes, as sent.
    pub always: Vec<Value>,
}

/// What the person pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Deny,
    AllowOnce,
    AlwaysAllow,
}

impl Decision {
    /// The screen's word for a button: `deny`, `allow`, `always`.
    pub fn parse(word: &str) -> Option<Decision> {
        match word {
            "deny" => Some(Decision::Deny),
            "allow" => Some(Decision::AllowOnce),
            "always" => Some(Decision::AlwaysAllow),
            _ => None,
        }
    }

    /// The word the screen is told back.
    pub fn word(self) -> &'static str {
        match self {
            Decision::Deny => "deny",
            Decision::AllowOnce => "allow",
            Decision::AlwaysAllow => "always",
        }
    }
}

/// Text without control characters other than a line break or a tab — the
/// card draws it as text, and an escape sequence has nothing to say there.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

/// A nonempty one-line name, cut to `NAME_MAX`.
fn name(value: &Value) -> Option<String> {
    let cleaned: String = value
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// `text` cut to `VALUE_MAX` characters, and whether it was.
fn cut(text: &str) -> (String, bool) {
    let cleaned = clean(text);
    match cleaned.char_indices().nth(VALUE_MAX) {
        Some((at, _)) => (cleaned[..at].to_string(), true),
        None => (cleaned, false),
    }
}

fn field(name: &str, value: &Value) -> Field {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        other => other.to_string(),
    };
    let (value, was_cut) = cut(&text);
    Field {
        name: clean(name),
        value,
        cut: was_cut,
    }
}

/// `(server, tool)` for an MCP tool's name, `mcp__<server>__<tool>`. The
/// server's own name, when the request carries it (`mcp_server.name`), settles
/// where the server ends; otherwise the first `__` after the prefix does.
pub fn split_mcp(tool_name: &str, server_name: Option<&str>) -> Option<(String, String)> {
    let rest = tool_name.strip_prefix("mcp__")?;
    if let Some(server) = server_name.filter(|s| !s.is_empty()) {
        if let Some(tool) = rest
            .strip_prefix(server)
            .and_then(|after| after.strip_prefix("__"))
            .filter(|tool| !tool.is_empty())
        {
            return Some((server.to_string(), tool.to_string()));
        }
    }
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then(|| (server.to_string(), tool.to_string()))
}

/// Whether a suggestion is one 常に許可 echoes: rules added to allow.
fn always_allows(entry: &Value) -> bool {
    entry["type"].as_str() == Some("addRules")
        && entry["behavior"].as_str() == Some("allow")
        && entry["rules"]
            .as_array()
            .is_some_and(|rules| !rules.is_empty())
}

/// One rule as the permission syntax writes it: `Tool` or `Tool(content)`.
fn rule_text(rule: &Value) -> Option<String> {
    let tool = rule["toolName"].as_str().filter(|t| !t.is_empty())?;
    Some(match rule["ruleContent"].as_str() {
        Some(content) if !content.is_empty() => clean(&format!("{tool}({content})")),
        _ => clean(tool),
    })
}

/// Where a suggestion would be written, as the person knows the place.
fn destination_text(destination: Option<&str>) -> &'static str {
    match destination {
        Some("session") => "このセッションの間だけ",
        Some("localSettings") => ".claude/settings.local.json",
        Some("projectSettings") => ".claude/settings.json",
        Some("userSettings") => "~/.claude/settings.json",
        _ => "",
    }
}

impl Request {
    /// The request a hook's body is, or `None` when it is not a readable
    /// `PermissionRequest` naming a tool: cut at the app's limit, not JSON,
    /// another event's body, or no `tool_name`. A request that cannot be read
    /// gets no card and is answered at once, as before #336.
    pub fn read(body: &[u8]) -> Option<Request> {
        let data = serde_json::from_slice::<Value>(body).ok()?;
        if data["hook_event_name"].as_str() != Some(EVENT) {
            return None;
        }
        let tool_name = name(&data["tool_name"])?;
        let (server, tool) = match split_mcp(&tool_name, data["mcp_server"]["name"].as_str()) {
            Some((server, tool)) => (Some(server), tool),
            None => (None, tool_name.clone()),
        };
        let mut fields = Vec::new();
        let mut more_fields = 0;
        match &data["tool_input"] {
            Value::Null => {}
            Value::Object(input) => {
                for (key, value) in input {
                    if fields.len() < FIELDS_MAX {
                        fields.push(field(key, value));
                    } else {
                        more_fields += 1;
                    }
                }
            }
            other => fields.push(field("input", other)),
        }
        let always = data["permission_suggestions"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|e| always_allows(e))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Some(Request {
            tool_name,
            server,
            tool,
            agent_id: data["agent_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            agent_type: name(&data["agent_type"]),
            fields,
            more_fields,
            always,
        })
    }

    /// Whether 常に許可 can be offered.
    pub fn can_always(&self) -> bool {
        !self.always.is_empty()
    }

    /// The rules 常に許可 would add, one line each, for the card to show
    /// before the person presses it.
    pub fn always_rules(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for entry in &self.always {
            let rules: Vec<String> = entry["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(rule_text)
                .collect();
            if rules.is_empty() {
                continue;
            }
            let place = destination_text(entry["destination"].as_str());
            if place.is_empty() {
                lines.push(rules.join("、"));
            } else {
                lines.push(format!("{}（{place}）", rules.join("、")));
            }
        }
        lines
    }

    /// What the screen draws the card from. The ids of the seat and of the
    /// request are the app's to add.
    pub fn card(&self) -> Value {
        json!({
            "tool_name": self.tool_name,
            "server": self.server,
            "tool": self.tool,
            "agent_type": self.agent_type,
            "fields": self.fields.iter().map(|f| json!({
                "name": f.name,
                "value": f.value,
                "cut": f.cut,
            })).collect::<Vec<_>>(),
            "more_fields": self.more_fields,
            "always_rules": self.always_rules(),
            "can_always": self.can_always(),
            "hold_secs": HOLD_SECS,
        })
    }

    /// The hook's answer for what the person pressed, or `None` for 常に許可
    /// on a request that offers none.
    ///
    /// The shape is the docs' "PermissionRequest decision control": `behavior`
    /// under `hookSpecificOutput.decision`, `updatedPermissions` for 常に許可,
    /// and a `message` telling the agent a denial came from the person.
    pub fn answer(&self, decision: Decision) -> Option<Value> {
        let decision = match decision {
            Decision::Deny => json!({
                "behavior": "deny",
                "message": "The user denied this permission request from the Pullcept room.",
            }),
            Decision::AllowOnce => json!({ "behavior": "allow" }),
            Decision::AlwaysAllow => {
                if !self.can_always() {
                    return None;
                }
                json!({ "behavior": "allow", "updatedPermissions": self.always })
            }
        };
        Some(json!({
            "hookSpecificOutput": {
                "hookEventName": EVENT,
                "decision": decision,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(value: Value) -> Vec<u8> {
        serde_json::to_vec(&value).unwrap()
    }

    fn bash() -> Value {
        json!({
            "session_id": "abc123",
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": {
                "command": "rm -rf node_modules",
                "description": "Remove node_modules directory"
            },
            "permission_suggestions": [
                {
                    "type": "addRules",
                    "rules": [{ "toolName": "Bash", "ruleContent": "rm -rf node_modules" }],
                    "behavior": "allow",
                    "destination": "localSettings"
                }
            ]
        })
    }

    #[test]
    fn a_bash_request_shows_its_command_and_offers_always() {
        let request = Request::read(&body(bash())).expect("readable");
        assert_eq!(request.tool_name, "Bash");
        assert_eq!(request.server, None);
        assert_eq!(request.tool, "Bash");
        assert_eq!(request.agent_id, None);
        assert_eq!(
            request.fields,
            vec![
                Field {
                    name: "command".into(),
                    value: "rm -rf node_modules".into(),
                    cut: false
                },
                Field {
                    name: "description".into(),
                    value: "Remove node_modules directory".into(),
                    cut: false
                },
            ]
        );
        assert!(request.can_always());
        assert_eq!(
            request.always_rules(),
            vec!["Bash(rm -rf node_modules)（.claude/settings.local.json）".to_string()]
        );
    }

    #[test]
    fn an_mcp_tool_is_split_into_server_and_tool() {
        let mut value = bash();
        value["tool_name"] = json!("mcp__pullcept-room-e31e7f__say_to_room");
        value["mcp_server"] = json!({ "name": "pullcept-room-e31e7f", "source": "project" });
        value["tool_input"] = json!({ "content": "こんにちは", "last_seen": "m1" });
        let request = Request::read(&body(value)).expect("readable");
        assert_eq!(request.server.as_deref(), Some("pullcept-room-e31e7f"));
        assert_eq!(request.tool, "say_to_room");
        assert_eq!(request.tool_name, "mcp__pullcept-room-e31e7f__say_to_room");
    }

    #[test]
    fn the_server_name_settles_a_server_with_underscores() {
        assert_eq!(
            split_mcp("mcp__my__server__do_it", Some("my__server")),
            Some(("my__server".into(), "do_it".into()))
        );
        // Without it, the first separator is taken.
        assert_eq!(
            split_mcp("mcp__github__create_issue", None),
            Some(("github".into(), "create_issue".into()))
        );
        // A server name that does not fit the tool's name is not trusted.
        assert_eq!(
            split_mcp("mcp__github__create_issue", Some("other")),
            Some(("github".into(), "create_issue".into()))
        );
        assert_eq!(split_mcp("Bash", None), None);
        assert_eq!(split_mcp("mcp__only", None), None);
    }

    #[test]
    fn a_subagent_request_names_the_agent() {
        let mut value = bash();
        value["agent_id"] = json!("ag1");
        value["agent_type"] = json!("Explore");
        let request = Request::read(&body(value)).expect("readable");
        assert_eq!(request.agent_id.as_deref(), Some("ag1"));
        assert_eq!(request.agent_type.as_deref(), Some("Explore"));
    }

    #[test]
    fn an_unreadable_or_foreign_body_gets_no_card() {
        assert_eq!(Request::read(b""), None);
        assert_eq!(Request::read(b"{\"hook_event_name\":\"PermissionReq"), None);
        let mut other = bash();
        other["hook_event_name"] = json!("PreToolUse");
        assert_eq!(Request::read(&body(other)), None);
        let mut nameless = bash();
        nameless["tool_name"] = json!("");
        assert_eq!(Request::read(&body(nameless)), None);
    }

    #[test]
    fn long_values_and_many_fields_are_cut() {
        let mut value = bash();
        let mut input = serde_json::Map::new();
        input.insert("a_long".into(), json!("x".repeat(VALUE_MAX + 5)));
        for i in 0..FIELDS_MAX + 3 {
            input.insert(format!("f{i:02}"), json!(i));
        }
        value["tool_input"] = Value::Object(input);
        let request = Request::read(&body(value)).expect("readable");
        assert_eq!(request.fields.len(), FIELDS_MAX);
        assert_eq!(request.more_fields, 4);
        let long = &request.fields[0];
        assert_eq!(long.name, "a_long");
        assert!(long.cut);
        assert_eq!(long.value.chars().count(), VALUE_MAX);
    }

    #[test]
    fn control_characters_are_dropped_but_lines_kept() {
        let mut value = bash();
        value["tool_input"] =
            json!({ "command": "echo a\u{1b}[31m\nb\tc", "nested": { "k": [1, 2] } });
        let request = Request::read(&body(value)).expect("readable");
        assert_eq!(request.fields[0].value, "echo a[31m\nb\tc");
        assert!(request.fields[1].value.contains("\"k\""));
    }

    #[test]
    fn only_allow_rule_suggestions_are_offered_as_always() {
        let mut value = bash();
        value["permission_suggestions"] = json!([
            { "type": "setMode", "mode": "acceptEdits", "destination": "session" },
            { "type": "addRules", "rules": [{ "toolName": "Bash" }], "behavior": "deny", "destination": "session" },
            { "type": "addRules", "rules": [], "behavior": "allow", "destination": "session" },
        ]);
        let request = Request::read(&body(value)).expect("readable");
        assert!(!request.can_always());
        assert_eq!(request.answer(Decision::AlwaysAllow), None);
        let mut none = bash();
        none.as_object_mut()
            .unwrap()
            .remove("permission_suggestions");
        assert!(!Request::read(&body(none)).unwrap().can_always());
    }

    #[test]
    fn the_answers_follow_the_decision_control_shape() {
        let request = Request::read(&body(bash())).expect("readable");
        assert_eq!(
            request.answer(Decision::AllowOnce),
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": { "behavior": "allow" }
                }
            }))
        );
        let deny = request.answer(Decision::Deny).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(deny["hookSpecificOutput"]["decision"]["message"].is_string());
        assert!(deny["hookSpecificOutput"]["decision"]
            .get("interrupt")
            .is_none());
        let always = request.answer(Decision::AlwaysAllow).unwrap();
        assert_eq!(
            always["hookSpecificOutput"]["decision"],
            json!({ "behavior": "allow", "updatedPermissions": bash()["permission_suggestions"] })
        );
    }

    #[test]
    fn the_buttons_words_round_trip() {
        for decision in [Decision::Deny, Decision::AllowOnce, Decision::AlwaysAllow] {
            assert_eq!(Decision::parse(decision.word()), Some(decision));
        }
        assert_eq!(Decision::parse("yes"), None);
    }

    #[test]
    fn the_hold_ends_before_the_hook_times_out() {
        assert!(HOLD_SECS < HOOK_TIMEOUT_SECS);
        assert!(HOOK_TIMEOUT_SECS < 600);
    }

    #[test]
    fn the_card_carries_what_the_screen_draws() {
        let card = Request::read(&body(bash())).unwrap().card();
        assert_eq!(card["tool"], "Bash");
        assert_eq!(card["can_always"], true);
        assert_eq!(card["fields"][0]["name"], "command");
        assert_eq!(card["hold_secs"], HOLD_SECS);
    }
}
