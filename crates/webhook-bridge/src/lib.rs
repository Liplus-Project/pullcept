//! The app's side of the `github-webhook-mcp` bridge it runs itself (#169).
//!
//! The app is the bridge's MCP client, on its stdio, in the place a CLI session
//! usually is. The bridge does not ask who connected: it declares
//! `claude/channel` and pushes `notifications/claude/channel` to whoever did.
//! So what the app needs is small, and it is all here — the two lines it
//! writes (`initialize`, `notifications/initialized`), the reading of each line
//! it gets back, and the rule for which rooms a notice goes into.
//!
//! It calls no tool. Marking an event processed is for the session that
//! handled it, not for the app that showed it (#180).
//!
//! The wire is MCP's stdio transport: one JSON-RPC message per line.

use serde_json::{json, Value};

/// The name a webhook notice is posted under.
///
/// A name, like every speaker in the room, and not a participant class. The
/// screen folds the lines under it (#169), and it does so by this name; nothing
/// in the room branches on it.
pub const SPEAKER: &str = "webhook";

/// The id of the `initialize` request. The one request the app sends before
/// anything else, so its answer is the one that opens the session.
pub const INITIALIZE_ID: u64 = 1;

/// The protocol revision the app asks for. The bridge's stdio face is the MCP
/// SDK's v1 server, which answers with a revision of its own when it does not
/// hold this one; nothing here reads which one it chose.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The `initialize` request, as one line.
///
/// No capabilities: the app reads one notification, and that needs the client
/// to declare nothing.
pub fn initialize_request() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_ID,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "pullcept", "version": env!("CARGO_PKG_VERSION") },
        },
    })
    .to_string()
}

/// The notification that follows the answer to `initialize`.
pub fn initialized_notification() -> String {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()
}

/// One webhook event, as the bridge pushed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The worker's id for the event: what names it in the app's log, and what
    /// a session hands to `mark_processed` once it has handled it.
    pub event_id: String,
    /// The bridge's own summary of it, posted as it came.
    pub content: String,
}

/// What one line from the bridge is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A webhook event.
    Event(Event),
    /// The answer to a request the app sent. `failure` is `Some` for a JSON-RPC
    /// error and for a tool result flagged `isError` — the bridge answers a
    /// worker it could not reach, or an authorisation it does not have, with
    /// the second, not the first.
    Answer { id: u64, failure: Option<String> },
    /// Anything else: a line that is not JSON, another notification, a request.
    Other,
}

/// Read one line from the bridge.
///
/// **The event id is read off `meta.message_id`.** The published bridge puts it
/// there and nowhere else; the repository's `local-mcp` twin also carries
/// `meta.event_id`, which is read when `message_id` is absent. A push that
/// carries neither, or no content, is not an event this app posts, and is
/// `Other`.
pub fn read_line(line: &str) -> Line {
    let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
        return Line::Other;
    };

    if message["method"] == "notifications/claude/channel" {
        let params = &message["params"];
        let meta = &params["meta"];
        let event_id = meta["message_id"]
            .as_str()
            .or_else(|| meta["event_id"].as_str())
            .filter(|id| !id.is_empty());
        let content = params["content"]
            .as_str()
            .filter(|text| !text.trim().is_empty());
        return match (event_id, content) {
            (Some(event_id), Some(content)) => Line::Event(Event {
                event_id: event_id.to_string(),
                content: content.to_string(),
            }),
            _ => Line::Other,
        };
    }

    // An answer carries an id and no method; a request from the bridge carries
    // both, and is not one of ours.
    let Some(id) = message["id"].as_u64() else {
        return Line::Other;
    };
    if message.get("method").is_some() {
        return Line::Other;
    }
    let failure = if let Some(error) = message.get("error") {
        Some(
            error["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string()),
        )
    } else if message["result"]["isError"] == true {
        Some(
            message["result"]["content"]
                .as_array()
                .and_then(|parts| parts.iter().find_map(|part| part["text"].as_str()))
                .unwrap_or("the tool reported an error and said nothing more")
                .to_string(),
        )
    } else {
        None
    };
    Line::Answer { id, failure }
}

/// Whether a room holds an AI session: anyone seated in it but the screen's
/// own person.
///
/// A notice goes into a room only when this holds (#169, Master 判断
/// 2026-09-27). A session that is not running is not seated, so it is not
/// reached, and one that sits down later is not handed the notice after the
/// fact — the same line as the room not pushing its past to a participant.
///
/// `seats` are the room's connections and `screen` is the screen's own one.
/// Every other seat is a connection that said `hello` on the room socket,
/// which is a session: a person is at the screen, not on the socket.
pub fn holds_session<'a>(seats: impl IntoIterator<Item = &'a str>, screen: &str) -> bool {
    seats.into_iter().any(|seat| seat != screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Value {
        serde_json::from_str(line).expect("a line the app writes is JSON")
    }

    #[test]
    fn every_line_the_app_writes_is_one_line() {
        for line in [initialize_request(), initialized_notification()] {
            assert!(!line.contains('\n'), "{line}");
        }
    }

    #[test]
    fn initialize_is_the_request_its_answer_is_matched_against() {
        let request = parsed(&initialize_request());
        assert_eq!(request["method"], "initialize");
        assert_eq!(request["id"], INITIALIZE_ID);
        assert_eq!(request["params"]["clientInfo"]["name"], "pullcept");
    }

    /// The published bridge's push, as `sidecar/test/webhook-bridge.test.mjs`
    /// reads it off the installed package.
    #[test]
    fn a_published_bridge_push_is_an_event() {
        let line = r#"{"method":"notifications/claude/channel","params":{"content":"[issues] Liplus-Project/pullcept\naction: opened","meta":{"chat_id":"github","message_id":"delivery-1","user":"someone","ts":"2026-09-27T00:00:00.000Z"}},"jsonrpc":"2.0"}"#;
        assert_eq!(
            read_line(line),
            Line::Event(Event {
                event_id: "delivery-1".to_string(),
                content: "[issues] Liplus-Project/pullcept\naction: opened".to_string(),
            })
        );
    }

    #[test]
    fn a_push_carrying_only_event_id_is_still_an_event() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/claude/channel","params":{"content":"issues opened","meta":{"event_id":"delivery-2"}}}"#;
        assert!(
            matches!(read_line(line), Line::Event(Event { event_id, .. }) if event_id == "delivery-2")
        );
    }

    #[test]
    fn a_push_without_an_id_or_content_is_not_an_event() {
        // No id: nothing would name it, in the app's log or to the session
        // that would mark it, so the app does not post it.
        let no_id = r#"{"jsonrpc":"2.0","method":"notifications/claude/channel","params":{"content":"x","meta":{}}}"#;
        let no_content = r#"{"jsonrpc":"2.0","method":"notifications/claude/channel","params":{"content":"  ","meta":{"message_id":"d"}}}"#;
        assert_eq!(read_line(no_id), Line::Other);
        assert_eq!(read_line(no_content), Line::Other);
    }

    #[test]
    fn an_answer_is_read_as_success_or_failure() {
        let ok = r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"success\":true}"}]}}"#;
        let tool_error = r#"{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"Failed to reach worker: boom"}],"isError":true}}"#;
        let rpc_error =
            r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method not found"}}"#;
        assert_eq!(
            read_line(ok),
            Line::Answer {
                id: 3,
                failure: None
            }
        );
        assert_eq!(
            read_line(tool_error),
            Line::Answer {
                id: 4,
                failure: Some("Failed to reach worker: boom".to_string())
            }
        );
        assert_eq!(
            read_line(rpc_error),
            Line::Answer {
                id: 5,
                failure: Some("Method not found".to_string())
            }
        );
    }

    #[test]
    fn anything_else_is_other() {
        assert_eq!(read_line("not json"), Line::Other);
        assert_eq!(
            read_line(r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#),
            Line::Other
        );
        // A request from the bridge carries an id too, and is not an answer.
        assert_eq!(
            read_line(r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#),
            Line::Other
        );
    }

    #[test]
    fn a_room_with_only_the_screen_in_it_holds_no_session() {
        assert!(!holds_session(Vec::<&str>::new(), "screen"));
        assert!(!holds_session(["screen"], "screen"));
    }

    #[test]
    fn a_room_with_a_session_seated_holds_one() {
        assert!(holds_session(["screen", "session-a"], "screen"));
        // The person need not be seated for a session to be there.
        assert!(holds_session(["session-a"], "screen"));
    }
}
