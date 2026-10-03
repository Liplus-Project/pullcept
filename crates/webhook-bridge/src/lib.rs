//! The app's side of the `github-webhook-mcp` bridge it runs itself (#169).
//!
//! The app is the bridge's MCP client, on its stdio, in the place a CLI session
//! usually is. The bridge does not ask who connected: it declares
//! `claude/channel` and pushes `notifications/claude/channel` to whoever did.
//! So what the app needs is small, and it is all here — the two lines it
//! writes (`initialize`, `notifications/initialized`), the reading of each line
//! it gets back, and the rule for which rooms a notice goes into.
//!
//! It calls one tool, `get_event`, to read who signed a body (#269), and that
//! tool only reads. Marking an event processed is for the session that handled
//! it, not for the app that showed it (#180).
//!
//! The wire is MCP's stdio transport: one JSON-RPC message per line.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

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
    /// the second, not the first. `text` is the first text part of a tool
    /// result that is not a failure, and `None` otherwise.
    Answer {
        id: u64,
        failure: Option<String>,
        text: Option<String>,
    },
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
    let first_text = || {
        message["result"]["content"]
            .as_array()
            .and_then(|parts| parts.iter().find_map(|part| part["text"].as_str()))
    };
    let failure = if let Some(error) = message.get("error") {
        Some(
            error["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string()),
        )
    } else if message["result"]["isError"] == true {
        Some(
            first_text()
                .unwrap_or("the tool reported an error and said nothing more")
                .to_string(),
        )
    } else {
        None
    };
    let text = match failure {
        Some(_) => None,
        None => first_text().map(str::to_string),
    };
    Line::Answer { id, failure, text }
}

// ── Who wrote it (#269) ─────────────────────────────────────────────────────
//
// One GitHub account is shared by several sessions, so the sender of an event
// does not say which of them wrote it. What does is the last line of the body,
// `— <name>` (#270). The bridge's push carries no body — only its summary — so
// the app reads the event once more with `get_event`, which returns the full
// payload and does not mark it processed (`worker/src/store.ts` `/event`).

/// How long a notice waits for its body to be read before it goes out
/// unsigned.
///
/// The worker answers in well under this. Past it the notice is posted as it
/// was before #269: the mark is an addition to the notice, never a condition
/// on it.
pub const SIGNATURE_WAIT: Duration = Duration::from_secs(5);

/// The event kinds that carry a body a session writes, and the payload key
/// that body sits under.
const BODIES: &[(&str, &str)] = &[
    ("issue_comment", "comment"),
    ("pull_request_review_comment", "comment"),
    ("discussion_comment", "comment"),
    ("issues", "issue"),
    ("pull_request", "pull_request"),
    ("pull_request_review", "review"),
];

/// The actions on which the sender has just written the body.
///
/// Not `edited`: the editor may be someone other than the session that signed
/// the body, and the signature would then name the wrong writer. Not the rest
/// (`closed`, `labeled`, `deleted`, …): the body there was written earlier,
/// possibly by someone other than the one acting now.
const WRITTEN: &[&str] = &["created", "opened", "submitted"];

/// What a notice opens with when its body was signed.
///
/// `署名` rather than "written by": what the app read is the line the body
/// ends with, which is a statement in the body, not a checked identity.
pub const SIGNED_PREFIX: &str = "署名: ";

/// The `tools/call` request for one event's full payload, as one line.
pub fn get_event_request(id: u64, event_id: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "get_event", "arguments": { "event_id": event_id } },
    })
    .to_string()
}

/// Whether a pushed summary is of an event whose body may carry a signature.
///
/// Read off the bridge's summary — `[<kind>] <repo>` on the first line,
/// `action: <action>` on its own line — so an event that cannot carry one is
/// posted at once, without a round trip.
pub fn wants_signature(content: &str) -> bool {
    let mut lines = content.lines();
    let kind = lines
        .next()
        .and_then(|first| first.strip_prefix('['))
        .and_then(|rest| rest.split_once(']'))
        .map(|(kind, _)| kind);
    let Some(kind) = kind else {
        return false;
    };
    let action = lines.find_map(|line| line.strip_prefix("action: "));
    BODIES.iter().any(|(k, _)| *k == kind)
        && action.is_some_and(|action| WRITTEN.contains(&action.trim()))
}

/// The name signed on the body of the event `get_event` answered with, or
/// `None` when it carries none.
///
/// `answer` is the tool's text: the stored event as JSON, with its kind under
/// `type` and the webhook payload under `payload`.
pub fn signer(answer: &str) -> Option<String> {
    let event: Value = serde_json::from_str(answer).ok()?;
    let kind = event["type"].as_str()?;
    let payload = &event["payload"];
    let action = payload["action"].as_str()?;
    if !WRITTEN.contains(&action) {
        return None;
    }
    let (_, key) = BODIES.iter().find(|(k, _)| *k == kind)?;
    signature(payload[*key]["body"].as_str()?)
}

/// The name on a body's signature line, or `None` when its last line is not
/// one.
///
/// The last line is the last one with anything on it: GitHub keeps a body's
/// trailing newline, and a `\r\n` body reads the same as a `\n` one. The line
/// is `—` (U+2014), one space, and the name, exactly (#270) — a line with more
/// space before the name, or none, is not this form.
pub fn signature(body: &str) -> Option<String> {
    let last = body.lines().rev().find(|line| !line.trim().is_empty())?;
    let name = last.trim_end().strip_prefix("\u{2014} ")?;
    if name.is_empty() || name.starts_with(char::is_whitespace) {
        return None;
    }
    Some(name.to_string())
}

/// A notice, opened with the name its body was signed with.
pub fn signed(content: &str, name: &str) -> String {
    format!("{SIGNED_PREFIX}{name}\n{content}")
}

/// One event waiting to be posted.
struct Waiting {
    event: Event,
    /// The id of its `get_event` request, while that is unanswered.
    request: Option<u64>,
    /// The signed name, once read. `None` both before a read and after one
    /// that found none; `request` says which.
    name: Option<String>,
    deadline: Instant,
}

/// The events between the bridge's push and the room, in the order pushed.
///
/// A notice whose body is being read holds the ones behind it: the room's
/// order is the order things happened in, and a push posted ahead of the
/// comment that came before it would say otherwise. The hold is bounded by
/// [`SIGNATURE_WAIT`] — past it the notice goes out unsigned, and the ones
/// behind it go out with it.
pub struct Notices {
    waiting: VecDeque<Waiting>,
    next_id: u64,
}

impl Default for Notices {
    fn default() -> Self {
        Self::new()
    }
}

impl Notices {
    pub fn new() -> Self {
        Self {
            waiting: VecDeque::new(),
            next_id: INITIALIZE_ID + 1,
        }
    }

    /// Take one pushed event. Returns the request id and the line to send the
    /// bridge when its body is to be read; with `None` it waits only on those
    /// ahead of it.
    ///
    /// `ready` is whether the session with the bridge is open: a request
    /// before `initialized` would be refused, so such an event is not read.
    pub fn push(&mut self, event: Event, ready: bool, now: Instant) -> Option<(u64, String)> {
        let request = (ready && wants_signature(&event.content)).then(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        });
        let line = request.map(|id| (id, get_event_request(id, &event.event_id)));
        self.waiting.push_back(Waiting {
            event,
            request,
            name: None,
            deadline: now + SIGNATURE_WAIT,
        });
        line
    }

    /// An answer from the bridge. One to no request of ours is ignored; a
    /// failure settles the request unsigned.
    pub fn answer(&mut self, id: u64, failure: Option<&str>, text: Option<&str>) {
        let Some(one) = self.waiting.iter_mut().find(|w| w.request == Some(id)) else {
            return;
        };
        one.request = None;
        if failure.is_none() {
            one.name = text.and_then(signer);
        }
    }

    /// Give up on one request: it could not be sent.
    pub fn abandon(&mut self, id: u64) {
        self.answer(id, Some("not sent"), None);
    }

    /// The notices to post now, in order: from the front, every one that is
    /// settled or whose wait is over.
    pub fn due(&mut self, now: Instant) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(front) = self.waiting.front() {
            if front.request.is_some() && front.deadline > now {
                break;
            }
            if let Some(one) = self.waiting.pop_front() {
                out.push(Self::notice(one));
            }
        }
        out
    }

    /// When the front one stops waiting, if it is waiting on a read.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.waiting
            .front()
            .filter(|front| front.request.is_some())
            .map(|front| front.deadline)
    }

    /// Everything still held, unread ones unsigned: the bridge has ended, and
    /// no answer is coming.
    pub fn drain(&mut self) -> Vec<Event> {
        self.waiting.drain(..).map(Self::notice).collect()
    }

    fn notice(one: Waiting) -> Event {
        match one.name {
            Some(name) => Event {
                content: signed(&one.event.content, &name),
                event_id: one.event.event_id,
            },
            None => one.event,
        }
    }
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
                failure: None,
                text: Some("{\"success\":true}".to_string()),
            }
        );
        assert_eq!(
            read_line(tool_error),
            Line::Answer {
                id: 4,
                failure: Some("Failed to reach worker: boom".to_string()),
                text: None,
            }
        );
        assert_eq!(
            read_line(rpc_error),
            Line::Answer {
                id: 5,
                failure: Some("Method not found".to_string()),
                text: None,
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

    // ── signatures (#269) ──────────────────────────────────────────────────

    #[test]
    fn the_signature_is_the_last_line_with_anything_on_it() {
        assert_eq!(signature("本文\n\n— Lin"), Some("Lin".to_string()));
        // GitHub keeps the trailing newline, and Windows writes \r\n.
        assert_eq!(signature("本文\r\n— Lay\r\n\r\n"), Some("Lay".to_string()));
        // A name with a space in it is still one name.
        assert_eq!(signature("x\n— Claude Lin  "), Some("Claude Lin".to_string()));
        // A body that is only the signature is signed.
        assert_eq!(signature("— Lin"), Some("Lin".to_string()));
    }

    #[test]
    fn anything_but_the_one_form_on_the_last_line_is_unsigned() {
        // Not on the last line.
        assert_eq!(signature("— Lin\n本文"), None);
        // Another dash: ASCII hyphen, en dash (U+2013), a doubled em dash.
        assert_eq!(signature("本文\n- Lin"), None);
        assert_eq!(signature("本文\n\u{2013} Lin"), None);
        assert_eq!(signature("本文\n—— Lin"), None);
        // No space, two spaces, a full-width space, no name.
        assert_eq!(signature("本文\n—Lin"), None);
        assert_eq!(signature("本文\n—  Lin"), None);
        assert_eq!(signature("本文\n—\u{3000}Lin"), None);
        assert_eq!(signature("本文\n— "), None);
        // Indented: not the line's start.
        assert_eq!(signature("本文\n  — Lin"), None);
        assert_eq!(signature(""), None);
    }

    /// The bridge's summary, as `server/index.js` builds it from the worker's.
    fn summary(kind: &str, action: &str) -> String {
        format!("[{kind}] Liplus-Project/pullcept\naction: {action}\n#1 title\nby liplus-lin-lay\nhttps://github.com/x")
    }

    #[test]
    fn only_a_freshly_written_body_is_read() {
        for kind in [
            "issue_comment",
            "pull_request_review_comment",
            "discussion_comment",
            "issues",
            "pull_request",
            "pull_request_review",
        ] {
            for action in ["created", "opened", "submitted"] {
                assert!(wants_signature(&summary(kind, action)), "{kind} {action}");
            }
            // The body there was written before, maybe by someone else.
            for action in ["edited", "closed", "labeled", "deleted"] {
                assert!(!wants_signature(&summary(kind, action)), "{kind} {action}");
            }
        }
        // Kinds without a body a session writes are posted at once.
        for kind in ["push", "workflow_run", "check_run", "label"] {
            assert!(!wants_signature(&summary(kind, "created")), "{kind}");
        }
        assert!(!wants_signature("[issue_comment] repo"), "no action line");
        assert!(!wants_signature("issue_comment opened"), "no kind");
    }

    /// `get_event`'s text: the stored event, as `worker/src/store.ts` `/event`
    /// returns it.
    fn stored(kind: &str, payload: Value) -> String {
        json!({
            "id": "delivery-1",
            "type": kind,
            "received_at": "2026-10-03T00:00:00.000Z",
            "processed": false,
            "trigger_status": null,
            "last_triggered_at": null,
            "payload": payload,
        })
        .to_string()
    }

    #[test]
    fn the_signer_is_read_from_where_each_kind_keeps_its_body() {
        let body = "見ました\n— Lin";
        for (kind, key, action) in [
            ("issue_comment", "comment", "created"),
            ("pull_request_review_comment", "comment", "created"),
            ("discussion_comment", "comment", "created"),
            ("issues", "issue", "opened"),
            ("pull_request", "pull_request", "opened"),
            ("pull_request_review", "review", "submitted"),
        ] {
            let mut payload = json!({ "action": action });
            payload[key] = json!({ "body": body });
            assert_eq!(signer(&stored(kind, payload)), Some("Lin".to_string()), "{kind}");
        }
        // An issue_comment carries the issue too, whose body is not this one.
        let both = json!({
            "action": "created",
            "issue": { "body": "issue\n— Lay" },
            "comment": { "body": "comment, unsigned" },
        });
        assert_eq!(signer(&stored("issue_comment", both)), None);
    }

    #[test]
    fn no_signer_is_read_where_none_can_be() {
        // A review approved with no text has a null body.
        let empty = json!({ "action": "submitted", "review": { "body": null } });
        assert_eq!(signer(&stored("pull_request_review", empty)), None);
        // An edit: the editor need not be the signer.
        let edited = json!({ "action": "edited", "comment": { "body": "x\n— Lin" } });
        assert_eq!(signer(&stored("issue_comment", edited)), None);
        let push = json!({ "ref": "refs/heads/main" });
        assert_eq!(signer(&stored("push", push)), None);
        assert_eq!(signer("Event delivery-1 not found"), None);
    }

    #[test]
    fn a_signed_notice_opens_with_the_name() {
        assert_eq!(
            signed("[issue_comment] repo\naction: created", "Lin"),
            "署名: Lin\n[issue_comment] repo\naction: created"
        );
    }

    #[test]
    fn get_event_is_a_tool_call_on_one_line() {
        let line = get_event_request(7, "delivery-1");
        assert!(!line.contains('\n'));
        let request = parsed(&line);
        assert_eq!(request["id"], 7);
        assert_eq!(request["method"], "tools/call");
        assert_eq!(request["params"]["name"], "get_event");
        assert_eq!(request["params"]["arguments"]["event_id"], "delivery-1");
    }

    // ── the order notices go out in ───────────────────────────────────────

    fn event(id: &str, content: &str) -> Event {
        Event {
            event_id: id.to_string(),
            content: content.to_string(),
        }
    }

    fn ids(events: &[Event]) -> Vec<&str> {
        events.iter().map(|e| e.event_id.as_str()).collect()
    }

    #[test]
    fn a_notice_with_no_body_to_read_goes_out_at_once() {
        let mut notices = Notices::new();
        let now = Instant::now();
        assert_eq!(notices.push(event("p", "[push] repo"), true, now), None);
        assert_eq!(notices.next_deadline(), None);
        let due = notices.due(now);
        assert_eq!(due, vec![event("p", "[push] repo")]);
    }

    #[test]
    fn a_signed_body_marks_its_notice() {
        let mut notices = Notices::new();
        let now = Instant::now();
        let content = summary("issue_comment", "created");
        let (id, line) = notices.push(event("c", &content), true, now).expect("read");
        assert_ne!(id, INITIALIZE_ID, "a request id must not be taken for initialize's answer");
        assert_eq!(parsed(&line)["params"]["arguments"]["event_id"], "c");
        assert!(notices.due(now).is_empty(), "waits for its body");

        let answer = stored(
            "issue_comment",
            json!({ "action": "created", "comment": { "body": "x\n— Lay" } }),
        );
        notices.answer(id, None, Some(&answer));
        let due = notices.due(now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].content, format!("署名: Lay\n{content}"));
    }

    #[test]
    fn a_failed_slow_or_unsigned_read_posts_the_notice_as_it_was() {
        let now = Instant::now();
        let content = summary("issues", "opened");

        // Failure.
        let mut notices = Notices::new();
        let (id, _) = notices.push(event("a", &content), true, now).expect("read");
        notices.answer(id, Some("Failed to reach worker"), None);
        assert_eq!(notices.due(now), vec![event("a", &content)]);

        // Unsigned body.
        let (id, _) = notices.push(event("b", &content), true, now).expect("read");
        let answer = stored("issues", json!({ "action": "opened", "issue": { "body": "x" } }));
        notices.answer(id, None, Some(&answer));
        assert_eq!(notices.due(now), vec![event("b", &content)]);

        // No answer at all: out at the deadline, unsigned.
        notices.push(event("c", &content), true, now).expect("read");
        assert_eq!(notices.next_deadline(), Some(now + SIGNATURE_WAIT));
        assert!(notices.due(now + SIGNATURE_WAIT / 2).is_empty());
        assert_eq!(notices.due(now + SIGNATURE_WAIT), vec![event("c", &content)]);

        // Not sent.
        let (id, _) = notices.push(event("d", &content), true, now).expect("read");
        notices.abandon(id);
        assert_eq!(notices.due(now), vec![event("d", &content)]);

        // Before the session is open, nothing is asked.
        assert_eq!(notices.push(event("e", &content), false, now), None);
        assert_eq!(notices.due(now), vec![event("e", &content)]);
    }

    #[test]
    fn notices_go_out_in_the_order_they_were_pushed() {
        let mut notices = Notices::new();
        let now = Instant::now();
        let comment = summary("issue_comment", "created");
        let (first, _) = notices.push(event("1", &comment), true, now).expect("read");
        assert_eq!(notices.push(event("2", "[push] repo"), true, now), None);
        let (third, _) = notices.push(event("3", &comment), true, now).expect("read");
        // Nothing passes the one in front while it is read.
        assert!(notices.due(now).is_empty());
        // The third answered first still waits behind the first.
        notices.answer(third, Some("x"), None);
        assert!(notices.due(now).is_empty());
        notices.answer(first, Some("x"), None);
        assert_eq!(ids(&notices.due(now)), vec!["1", "2", "3"]);
    }

    #[test]
    fn an_answer_to_no_request_changes_nothing() {
        let mut notices = Notices::new();
        let now = Instant::now();
        notices
            .push(event("1", &summary("issue_comment", "created")), true, now)
            .expect("read");
        notices.answer(INITIALIZE_ID, None, Some("{}"));
        notices.answer(999, None, Some("{}"));
        assert!(notices.due(now).is_empty());
    }

    #[test]
    fn what_is_held_when_the_bridge_ends_goes_out_unsigned() {
        let mut notices = Notices::new();
        let now = Instant::now();
        let comment = summary("issue_comment", "created");
        notices.push(event("1", &comment), true, now).expect("read");
        notices.push(event("2", "[push] repo"), true, now);
        assert_eq!(ids(&notices.drain()), vec!["1", "2"]);
        assert!(notices.due(now + SIGNATURE_WAIT).is_empty());
    }
}
