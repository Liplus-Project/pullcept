//! A Codex CLI seat stopped on its usage limit, held, and handed back (#294).
//! See docs/3-accounts.md, "Codex の席が利用上限で止まったとき".
//!
//! Three readings and one state machine, all free of the app so that a test
//! can run them:
//!
//! - **the stop**, off the rollout: a turn that ends on the limit writes an
//!   `event_msg` / `task_complete` line whose `payload.error.codex_error_info`
//!   is `usage_limit_exceeded` ([`turn_end`]). Not the percentages: the rollout
//!   was observed holding 99.0 for a seat that had stopped (#290);
//! - **the reset**, off the same rollout's `token_count` while nothing better
//!   is at hand ([`rollout_reset`]), and off the app-server's answer when it
//!   is ([`read_answer`]);
//! - **the recovery**, off `account/rateLimits/read` and nothing else: the
//!   protocol says a client must not infer recovery from percentages or reset
//!   times (`app-server-protocol/src/protocol/v2/account.rs:333`). Only
//!   `ordinaryUsageAllowed: true` is [`Answer::Allowed`]; a `null`, an error
//!   and a broken answer are all [`Answer::Unavailable`], which keeps the seat
//!   limited.
//!
//! [`Limit`] is one seat: free, or stopped with the posts the room held for it
//! and when to ask next. Times are Unix seconds, passed in.
//!
//! A stopped seat has a mailbox ([`Mailboxes`], #312): the ids of the posts
//! held for it, kept in a small file beside its topic's record so that they
//! outlive the session and the app. What the seat is handed at the recovery is
//! those posts, read again from the room's record ([`mailbox`]). A seat
//! launched while its mailbox is still there starts stopped
//! ([`Limit::resumed`]).
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// How a turn ended, as its `task_complete` line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// No error on the line.
    Completed,
    /// `codex_error_info: "usage_limit_exceeded"`.
    UsageLimit,
    /// Some other error. Not the limit, and not a turn that worked either.
    Failed,
}

/// The value of `codex_error_info` a turn stopped on the limit carries
/// (observed, Codex CLI 0.160.0, 2026-10-05).
pub const USAGE_LIMIT: &str = "usage_limit_exceeded";

/// How one rollout line ended a turn, or `None` when it is not a turn's end.
pub fn turn_end(line: &Value) -> Option<TurnEnd> {
    if line["type"] != "event_msg" || line["payload"]["type"] != "task_complete" {
        return None;
    }
    let error = &line["payload"]["error"];
    Some(if error.is_null() {
        TurnEnd::Completed
    } else if error["codex_error_info"] == USAGE_LIMIT {
        TurnEnd::UsageLimit
    } else {
        TurnEnd::Failed
    })
}

/// The reset that lifts the stop, out of the windows a snapshot reports: the
/// latest reset among the fullest windows. A full 5-hour window beside a
/// weekly one at 96% resets with the 5-hour one; two full windows both have
/// to reset.
fn binding_reset(windows: &[(Option<f64>, Option<i64>)]) -> Option<i64> {
    let fullest = windows
        .iter()
        .filter_map(|(used, _)| *used)
        .fold(None, |max: Option<f64>, used| Some(max.map_or(used, |m| m.max(used))))?;
    windows
        .iter()
        .filter(|(used, _)| *used == Some(fullest))
        .filter_map(|(_, reset)| *reset)
        .max()
}

/// The reset a rollout `token_count` line reports, or `None` when the line is
/// not one or reports none. Only the `codex` bucket, for the reason the panel
/// reads only it (`status`).
pub fn rollout_reset(line: &Value) -> Option<i64> {
    if line["type"] != "event_msg" || line["payload"]["type"] != "token_count" {
        return None;
    }
    let limits = &line["payload"]["rate_limits"];
    if !limits.is_object() || !limits["limit_id"].as_str().is_none_or(|id| id == "codex") {
        return None;
    }
    let windows: Vec<_> = ["primary", "secondary"]
        .iter()
        .map(|key| {
            let w = &limits[key];
            (w["used_percent"].as_f64(), w["resets_at"].as_i64())
        })
        .collect();
    binding_reset(&windows)
}

/// What `account/rateLimits/read` said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// `ordinaryUsageAllowed: true`. The one answer that is a recovery.
    Allowed,
    /// `ordinaryUsageAllowed: false`, with the reset of the fullest window.
    Denied { resets_at: Option<i64> },
    /// No answer to read: the server could not be reached, answered with an
    /// error, or said `null`. Never read as allowed.
    Unavailable,
}

/// The id the rate-limit request is sent under ([`requests`]).
pub const READ_ID: u64 = 2;

/// The three lines the app-server is sent over stdio, one JSON object per line
/// with no `jsonrpc` key (as Codex's own clients send them): `initialize` (id
/// 1), `initialized`, then `account/rateLimits/read` ([`READ_ID`]). No thread
/// is started and no model is called.
pub fn requests(version: &str) -> [String; 3] {
    [
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "pullcept", "title": "Pullcept", "version": version}},
        })
        .to_string(),
        r#"{"method":"initialized"}"#.to_string(),
        format!(r#"{{"id":{READ_ID},"method":"account/rateLimits/read"}}"#),
    ]
}

/// Read the response to [`READ_ID`].
///
/// The `codex` bucket of `rateLimitsByLimitId` when it is there, the
/// single-bucket `rateLimits` otherwise, for the reset.
pub fn read_answer(response: &Value) -> Answer {
    let result = &response["result"];
    let allowed = match result["ordinaryUsageAllowed"].as_bool() {
        Some(allowed) => allowed,
        None => return Answer::Unavailable,
    };
    if allowed {
        return Answer::Allowed;
    }
    let bucket = match &result["rateLimitsByLimitId"]["codex"] {
        Value::Object(_) => &result["rateLimitsByLimitId"]["codex"],
        _ => &result["rateLimits"],
    };
    let windows: Vec<_> = ["primary", "secondary"]
        .iter()
        .map(|key| {
            let w = &bucket[key];
            (w["usedPercent"].as_f64(), w["resetsAt"].as_i64())
        })
        .collect();
    Answer::Denied {
        resets_at: binding_reset(&windows),
    }
}

/// How long after the reset the first question is put: "a little after".
pub const GRACE: i64 = 30;

/// The waits between questions once the reset has passed and the answer is
/// still no, or no answer came. The last one repeats.
pub const BACKOFF: [i64; 5] = [60, 120, 300, 600, 900];

/// The least time between two questions when a held post brings the next one
/// forward (#377): posts arriving together ask once.
pub const HELD_GAP: i64 = 60;

/// One room post the room did not type into a stopped seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub message_id: String,
    pub speaker: String,
    /// The label's `at`, when the post's time could be read.
    pub at: Option<String>,
    pub content: String,
    /// Its `to` names this seat.
    pub addressed: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Stopped {
    resets_at: Option<i64>,
    next_query: i64,
    misses: u32,
    /// When the last question was answered, if one has been.
    asked: Option<i64>,
}

/// One Codex seat: free, or stopped with what the room held for it.
#[derive(Debug, Default)]
pub struct Limit {
    stopped: Option<Stopped>,
    held: Vec<Held>,
}

fn first_query(now: i64, resets_at: Option<i64>) -> i64 {
    match resets_at {
        Some(reset) if reset + GRACE > now => reset + GRACE,
        _ => now + BACKOFF[0],
    }
}

impl Limit {
    /// A seat launched while its mailbox is still there (#312): stopped from
    /// the start, with its first question due now. Not at a reset: none is
    /// known, and the one the stop was told may long have passed.
    pub fn resumed(now: i64) -> Self {
        Limit {
            stopped: Some(Stopped {
                resets_at: None,
                next_query: now,
                misses: 0,
                asked: None,
            }),
            held: Vec::new(),
        }
    }

    pub fn is_limited(&self) -> bool {
        self.stopped.is_some()
    }

    pub fn resets_at(&self) -> Option<i64> {
        self.stopped.as_ref().and_then(|s| s.resets_at)
    }

    /// What has been held so far, oldest first.
    pub fn held(&self) -> &[Held] {
        &self.held
    }

    /// The rollout said a turn ended on the limit. Whether that stopped a seat
    /// that was free — the moment the room is told. A seat already stopped
    /// stays as it is: its schedule is the app-server's to move.
    pub fn stop(&mut self, now: i64, resets_at: Option<i64>) -> bool {
        if self.stopped.is_some() {
            return false;
        }
        self.stopped = Some(Stopped {
            resets_at,
            next_query: first_query(now, resets_at),
            misses: 0,
            asked: None,
        });
        true
    }

    /// The answer of the one question put at the stop, for the reset the room
    /// is told. Only a `Denied` carrying a reset moves anything: an `Allowed`
    /// seconds after the rollout said the opposite is not taken as a recovery
    /// at this point — the first question after the reset settles it.
    pub fn refine(&mut self, now: i64, answer: Answer) {
        if let (Some(stopped), Answer::Denied { resets_at: Some(reset) }) =
            (self.stopped.as_mut(), answer)
        {
            stopped.resets_at = Some(reset);
            stopped.next_query = first_query(now, Some(reset));
        }
    }

    /// Keep a post back from a stopped seat. `false`, and nothing kept, when
    /// the seat is free: the post is to be typed.
    pub fn hold(&mut self, post: impl FnOnce() -> Held) -> bool {
        if self.stopped.is_none() {
            return false;
        }
        self.held.push(post());
        true
    }

    /// Whether a question is due now.
    pub fn due(&self, now: i64) -> bool {
        self.stopped.as_ref().is_some_and(|s| now >= s.next_query)
    }

    /// A turn finished without error while stopped (a person typed into the
    /// terminal). Not a recovery by itself; it brings the next question to now.
    pub fn turn_completed(&mut self, now: i64) {
        if let Some(stopped) = self.stopped.as_mut() {
            stopped.next_query = stopped.next_query.min(now);
        }
    }

    /// A post has just been held (#377): the room has something for the seat,
    /// so whether it is free is worth asking now rather than at the reset —
    /// the limit may have been reset by hand. Not sooner than [`HELD_GAP`]
    /// after the last question.
    pub fn post_held(&mut self, now: i64) {
        if let Some(stopped) = self.stopped.as_mut() {
            let earliest = stopped.asked.map_or(now, |asked| (asked + HELD_GAP).max(now));
            stopped.next_query = stopped.next_query.min(earliest);
        }
    }

    /// The answer of a due question. Whether it confirmed the recovery; the
    /// seat stays stopped, still holding, until [`Limit::release`].
    pub fn answer(&mut self, now: i64, answer: Answer) -> bool {
        let Some(stopped) = self.stopped.as_mut() else {
            return false;
        };
        stopped.asked = Some(now);
        match answer {
            Answer::Allowed => return true,
            Answer::Denied { resets_at: Some(reset) } if reset + GRACE > now => {
                stopped.resets_at = Some(reset);
                stopped.next_query = reset + GRACE;
                stopped.misses = 0;
            }
            Answer::Denied { .. } | Answer::Unavailable => {
                stopped.misses = stopped.misses.saturating_add(1);
                let step = (stopped.misses as usize - 1).min(BACKOFF.len() - 1);
                stopped.next_query = now + BACKOFF[step];
            }
        }
        false
    }

    /// Free the seat and hand over what was held, oldest first.
    pub fn release(&mut self) -> Vec<Held> {
        self.stopped = None;
        std::mem::take(&mut self.held)
    }
}

/// The one line handed to a seat on its recovery, or `None` when nothing was
/// held: the count, the way to read them, and in full only the posts whose
/// `to` named the seat.
pub fn digest(held: &[Held]) -> Option<String> {
    if held.is_empty() {
        return None;
    }
    let mut text = format!(
        "制限中に部屋で {} 件の発言がありました。必要なら read_room_history で読んでください。",
        held.len()
    );
    let addressed: Vec<&Held> = held.iter().filter(|post| post.addressed).collect();
    if !addressed.is_empty() {
        text.push_str(&format!("\n\nそのうちあなた宛ての {} 件:", addressed.len()));
        for post in addressed {
            let at = post.at.as_deref().map(|at| format!("、{at}")).unwrap_or_default();
            text.push_str(&format!(
                "\n\n--- {}{}（message_id {}）\n{}",
                post.speaker, at, post.message_id, post.content
            ));
        }
    }
    Some(text)
}

/// One post of the topic's record, as [`mailbox`] reads it. `at` is the
/// post's time already read the way a label carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub message_id: String,
    pub speaker: String,
    pub account: Option<String>,
    pub content: String,
    pub to: Vec<String>,
    pub at: Option<String>,
}

/// The seat a mailbox is read for.
#[derive(Debug, Clone, Copy)]
pub struct Seat<'a> {
    /// Its name now.
    pub name: &'a str,
    pub account_id: &'a str,
}

/// The names the seat is known by in the record: its name now, and every name
/// its account has spoken under there — what a post addressed to the seat
/// before a rename carries. A name some other account has spoken under is not
/// taken from the record: it would read posts to that seat as to this one.
fn names_of(record: &[Recorded], seat: &Seat) -> Vec<String> {
    let mut names = vec![seat.name.to_string()];
    for post in record {
        if post.account.as_deref() == Some(seat.account_id) && !names.contains(&post.speaker) {
            names.push(post.speaker.clone());
        }
    }
    let others: Vec<&str> = record
        .iter()
        .filter(|post| post.account.as_deref().is_some_and(|id| id != seat.account_id))
        .map(|post| post.speaker.as_str())
        .collect();
    names.retain(|name| name == seat.name || !others.contains(&name.as_str()));
    names
}

/// What the seat is handed at its recovery, in the order held (#312): the
/// posts whose ids its mailbox holds, and nothing else. `ids` is the mailbox as
/// its file keeps it; `held` is what this launch's memory held, whose ids the
/// file normally holds too, and any it does not are kept after them.
///
/// Each post is read from the record, with `addressed` read against every name
/// the seat is known by there. A post the record does not show is taken as
/// memory held it, and one neither shows is left out: there is nothing of it
/// to hand over. Nothing else in the record is looked at — no notice, no range
/// between two ids: a post said while the seat was closed, or while the app was
/// down, was never held, and is not the seat's to be handed.
pub fn mailbox(record: &[Recorded], seat: &Seat, ids: &[String], held: Vec<Held>) -> Vec<Held> {
    let names = names_of(record, seat);
    let by_id: HashMap<&str, &Recorded> = record
        .iter()
        .map(|post| (post.message_id.as_str(), post))
        .collect();
    let mut order: Vec<String> = Vec::new();
    for id in ids.iter().chain(held.iter().map(|post| &post.message_id)) {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    order
        .into_iter()
        .filter_map(|id| {
            let memory = held.iter().find(|post| post.message_id == id);
            match by_id.get(id.as_str()) {
                Some(post) => Some(Held {
                    message_id: post.message_id.clone(),
                    speaker: post.speaker.clone(),
                    at: post.at.clone(),
                    content: post.content.clone(),
                    addressed: memory.is_some_and(|m| m.addressed)
                        || post.to.iter().any(|to| names.contains(to)),
                }),
                None => memory.cloned(),
            }
        })
        .collect()
}

/// The mailboxes of one topic's stopped seats, as the small file beside the
/// topic's record keeps them across a session's end and the app's (#312): by
/// account, the ids of the posts held for that seat, oldest first. Ids only —
/// what was said is the record's. A seat has a mailbox from its stop until its
/// recovery has been handed over, empty while nothing has been held. The file
/// goes with its topic (`topic_index::delete`).
///
/// A file that is missing, cannot be read, or is not what this writes is no
/// mailbox at all.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Mailboxes {
    seats: serde_json::Map<String, Value>,
}

impl Mailboxes {
    pub fn read(path: &Path) -> Self {
        let seats = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(seats) => Some(seats),
                _ => None,
            })
            .unwrap_or_default();
        Mailboxes { seats }
    }

    /// Written whole, through a file beside it renamed over it, so a write cut
    /// short leaves the last one standing. No mailbox left is no file.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if self.seats.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let next = path.with_extension("next");
        std::fs::write(&next, Value::Object(self.seats.clone()).to_string())?;
        std::fs::rename(&next, path)
    }

    /// The seat's mailbox, or `None` when it has none.
    pub fn get(&self, account_id: &str) -> Option<Vec<String>> {
        let ids = self.seats.get(account_id)?.as_array()?;
        Some(
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect(),
        )
    }

    fn ids_mut(&mut self, account_id: &str) -> &mut Vec<Value> {
        let ids = self
            .seats
            .entry(account_id.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !ids.is_array() {
            *ids = Value::Array(Vec::new());
        }
        match ids {
            Value::Array(ids) => ids,
            _ => unreachable!("made an array above"),
        }
    }

    /// The seat has stopped: it has a mailbox from now, empty if it had none.
    pub fn open(&mut self, account_id: &str) {
        self.ids_mut(account_id);
    }

    /// One more post held for the seat.
    pub fn push(&mut self, account_id: &str, message_id: &str) {
        let ids = self.ids_mut(account_id);
        if !ids.iter().any(|id| id == message_id) {
            ids.push(Value::String(message_id.to_string()));
        }
    }

    /// The seat's recovery is being handed over: its mailbox goes.
    pub fn close(&mut self, account_id: &str) {
        self.seats.remove(account_id);
    }
}

/// What the room is told when a seat stops. `reset` is the reset already
/// written as the person reads it, or `None` when none is known.
pub fn stop_notice(name: &str, reset: Option<&str>) -> String {
    let when = match reset {
        Some(reset) => format!("解除予定は {reset} です。"),
        None => "解除予定は分かっていません。".to_string(),
    };
    format!(
        "{name} は利用上限で止まりました。{when}解除を確かめるまで、{name} への部屋の発言は Pullcept が預かります。"
    )
}

/// What the room is told when the recovery is confirmed.
pub fn recovery_notice(name: &str, held: usize) -> String {
    if held == 0 {
        format!("{name} の利用上限の解除を確かめました。")
    } else {
        format!(
            "{name} の利用上限の解除を確かめました。預かっていた {held} 件の発言は、一通にまとめて {name} に渡します。"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    // The task_complete of a turn stopped on the limit, cut down from Luna's
    // rollout (2026-10-05, 0.160.0).
    const STOPPED: &str = r#"{"timestamp":"2026-10-05T12:58:43.771Z","ordinal":719,"type":"event_msg","payload":{"type":"task_complete","turn_id":"t","last_agent_message":null,"error":{"message":"Error running remote compact task: You’ve hit your usage limit. ... try again at 10:45 PM.","codex_error_info":"usage_limit_exceeded"},"started_at":1791205121,"completed_at":1791205123,"duration_ms":2344}}"#;

    // The answer measured on #290 / #294 (trimmed).
    const DENIED: &str = r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"limitId":"codex","primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":1791207908},"secondary":{"usedPercent":96,"windowDurationMins":10080,"resetsAt":1791613576}},"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":1791207908},"secondary":{"usedPercent":96,"windowDurationMins":10080,"resetsAt":1791613576}}}}}"#;

    #[test]
    fn a_turn_end_is_read_off_task_complete() {
        assert_eq!(turn_end(&json(STOPPED)), Some(TurnEnd::UsageLimit));
        assert_eq!(
            turn_end(&json(r#"{"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"ok"}}"#)),
            Some(TurnEnd::Completed)
        );
        assert_eq!(
            turn_end(&json(r#"{"type":"event_msg","payload":{"type":"task_complete","error":null}}"#)),
            Some(TurnEnd::Completed)
        );
        assert_eq!(
            turn_end(&json(r#"{"type":"event_msg","payload":{"type":"task_complete","error":{"codex_error_info":"server_overloaded"}}}"#)),
            Some(TurnEnd::Failed)
        );
        for other in [
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
            r#"{"type":"response_item","payload":{"type":"task_complete","error":{"codex_error_info":"usage_limit_exceeded"}}}"#,
            r#"{}"#,
            r#"[]"#,
        ] {
            assert_eq!(turn_end(&json(other)), None, "{other}");
        }
    }

    #[test]
    fn the_reset_is_the_fullest_windows() {
        // The rollout's stale 99 still points at the 5-hour window.
        let line = json(r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":99.0,"window_minutes":300,"resets_at":1791207909},"secondary":{"used_percent":96,"window_minutes":10080,"resets_at":1791613577}}}}"#);
        assert_eq!(rollout_reset(&line), Some(1791207909));
        // Two full windows: both have to reset.
        let both = json(r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":100,"resets_at":10},"secondary":{"used_percent":100,"resets_at":20}}}}"#);
        assert_eq!(rollout_reset(&both), Some(20));
        // Another bucket, no limits, not a token_count: nothing.
        for other in [
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"other","primary":{"used_percent":100,"resets_at":10}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":"x","resets_at":10}}}}"#,
            r#"{"type":"turn_context","payload":{}}"#,
        ] {
            assert_eq!(rollout_reset(&json(other)), None, "{other}");
        }
    }

    #[test]
    fn only_ordinary_usage_allowed_true_is_a_recovery() {
        assert_eq!(read_answer(&json(DENIED)), Answer::Denied { resets_at: Some(1791207908) });
        assert_eq!(
            read_answer(&json(r#"{"id":2,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"primary":{"usedPercent":100}}}}"#)),
            Answer::Allowed
        );
        // Single-bucket view only.
        assert_eq!(
            read_answer(&json(r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"primary":{"usedPercent":10,"resetsAt":5},"secondary":{"usedPercent":100,"resetsAt":9}}}}"#)),
            Answer::Denied { resets_at: Some(9) }
        );
        for broken in [
            r#"{"id":2,"result":{"ordinaryUsageAllowed":null,"rateLimits":{"primary":{"usedPercent":0}}}}"#,
            r#"{"id":2,"result":{"rateLimits":{}}}"#,
            r#"{"id":2,"error":{"code":-32600,"message":"no"}}"#,
            r#"{"id":2,"result":{"ordinaryUsageAllowed":"true"}}"#,
            r#"null"#,
        ] {
            assert_eq!(read_answer(&json(broken)), Answer::Unavailable, "{broken}");
        }
    }

    #[test]
    fn the_requests_are_one_json_object_per_line() {
        let lines = requests("0.1.0");
        for line in &lines {
            assert!(!line.contains('\n'));
            json(line);
        }
        assert_eq!(json(&lines[0])["method"], "initialize");
        assert_eq!(json(&lines[1])["method"], "initialized");
        assert!(json(&lines[1]).get("id").is_none());
        assert_eq!(json(&lines[2])["method"], "account/rateLimits/read");
        assert_eq!(json(&lines[2])["id"], READ_ID);
    }

    fn post(id: &str, addressed: bool) -> Held {
        Held {
            message_id: id.into(),
            speaker: "Master".into(),
            at: Some("2026-10-05T22:10+09:00".into()),
            content: format!("body {id}"),
            addressed,
        }
    }

    #[test]
    fn a_free_seat_holds_nothing_and_is_never_due() {
        let mut limit = Limit::default();
        assert!(!limit.hold(|| post("a", false)));
        assert!(!limit.due(i64::MAX));
        assert!(!limit.answer(0, Answer::Allowed));
        assert_eq!(limit.release(), Vec::new());
    }

    #[test]
    fn the_first_question_is_a_little_after_the_reset() {
        let mut limit = Limit::default();
        assert!(limit.stop(1000, Some(5000)));
        assert!(!limit.stop(1001, Some(9000)), "a second stop is not a second notice");
        assert_eq!(limit.resets_at(), Some(5000));
        assert!(!limit.due(5000 + GRACE - 1));
        assert!(limit.due(5000 + GRACE));
        // No reset known, or one already past: the first wait of the backoff.
        let mut limit = Limit::default();
        limit.stop(1000, None);
        assert!(limit.due(1000 + BACKOFF[0]));
        assert!(!limit.due(1000 + BACKOFF[0] - 1));
        let mut limit = Limit::default();
        limit.stop(1000, Some(10));
        assert!(!limit.due(1001));
        assert!(limit.due(1000 + BACKOFF[0]));
    }

    #[test]
    fn the_question_at_the_stop_only_refines_the_reset() {
        let mut limit = Limit::default();
        limit.stop(1000, Some(5000));
        limit.refine(1000, Answer::Allowed);
        limit.refine(1000, Answer::Unavailable);
        assert!(limit.is_limited());
        assert_eq!(limit.resets_at(), Some(5000));
        limit.refine(1000, Answer::Denied { resets_at: Some(7000) });
        assert_eq!(limit.resets_at(), Some(7000));
        assert!(!limit.due(5000 + GRACE));
        assert!(limit.due(7000 + GRACE));
    }

    #[test]
    fn no_or_no_answer_backs_off_and_stays_limited() {
        let mut limit = Limit::default();
        limit.stop(0, Some(100));
        let mut now = 100 + GRACE;
        for wait in [60, 120, 300, 600, 900, 900] {
            assert!(limit.due(now));
            assert!(!limit.answer(now, Answer::Unavailable));
            assert!(!limit.due(now + wait - 1));
            now += wait;
        }
        assert!(limit.is_limited());
        // A no that names a reset still ahead waits for that reset instead.
        assert!(!limit.answer(now, Answer::Denied { resets_at: Some(now + 5000) }));
        assert!(!limit.due(now + 5000));
        assert!(limit.due(now + 5000 + GRACE));
        // ...and a no whose reset has passed backs off from the start again.
        now += 5000 + GRACE;
        assert!(!limit.answer(now, Answer::Denied { resets_at: Some(now - GRACE) }));
        assert!(limit.due(now + BACKOFF[0]));
        assert!(!limit.due(now + BACKOFF[0] - 1));
    }

    #[test]
    fn a_turn_that_worked_brings_the_question_forward_but_is_not_a_recovery() {
        let mut limit = Limit::default();
        limit.stop(0, Some(10_000));
        limit.turn_completed(50);
        assert!(limit.is_limited());
        assert!(limit.due(50));
    }

    #[test]
    fn a_held_post_asks_before_the_reset_but_not_twice_a_minute() {
        let mut limit = Limit::default();
        limit.stop(0, Some(10_000));
        assert!(!limit.due(100));
        limit.post_held(100);
        assert!(limit.due(100));
        assert!(!limit.answer(100, Answer::Denied { resets_at: Some(10_000) }));
        assert!(!limit.due(130));
        limit.post_held(130);
        assert!(!limit.due(130));
        assert!(limit.due(160));
        assert!(limit.answer(160, Answer::Allowed));
    }

    #[test]
    fn the_recovery_hands_over_what_was_held_in_order() {
        let mut limit = Limit::default();
        limit.stop(0, Some(100));
        assert!(limit.hold(|| post("a", false)));
        assert!(limit.hold(|| post("b", true)));
        assert_eq!(limit.held().len(), 2);
        assert!(limit.answer(200, Answer::Allowed));
        // Confirmed, but still holding until released.
        assert!(limit.is_limited());
        assert!(limit.hold(|| post("c", false)));
        let held = limit.release();
        assert_eq!(
            held.iter().map(|p| p.message_id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert!(!limit.is_limited());
        assert!(!limit.hold(|| post("d", false)));
    }

    #[test]
    fn the_digest_counts_all_and_carries_only_the_addressed() {
        assert_eq!(digest(&[]), None);
        let text = digest(&[post("a", false), post("b", true), post("c", false)]).unwrap();
        assert!(text.starts_with(
            "制限中に部屋で 3 件の発言がありました。必要なら read_room_history で読んでください。"
        ));
        assert!(text.contains("あなた宛ての 1 件"));
        assert!(text.contains("body b"));
        assert!(text.contains("message_id b"));
        assert!(!text.contains("body a"));
        assert!(!text.contains("body c"));
        let none_addressed = digest(&[post("a", false)]).unwrap();
        assert!(!none_addressed.contains("あなた宛て"));
    }

    #[test]
    fn the_notices_name_the_seat_and_the_reset() {
        assert_eq!(
            stop_notice("Codex Luna", Some("10月5日 22:45")),
            "Codex Luna は利用上限で止まりました。解除予定は 10月5日 22:45 です。解除を確かめるまで、Codex Luna への部屋の発言は Pullcept が預かります。"
        );
        assert!(stop_notice("L", None).contains("解除予定は分かっていません。"));
        assert!(recovery_notice("L", 3).contains("3 件"));
        assert!(!recovery_notice("L", 0).contains("件"));
    }
    const SEAT: Seat = Seat {
        name: "Codex Luna",
        account_id: "luna",
    };

    fn said(id: &str, speaker: &str, account: Option<&str>, to: &[&str], content: &str) -> Recorded {
        Recorded {
            message_id: id.into(),
            speaker: speaker.into(),
            account: account.map(str::to_string),
            content: content.into(),
            to: to.iter().map(|t| t.to_string()).collect(),
            at: Some(format!("at {id}")),
        }
    }

    fn by_master(id: &str, to: &[&str]) -> Recorded {
        said(id, "Master", None, to, &format!("body {id}"))
    }

    fn ids(posts: &[Held]) -> Vec<&str> {
        posts.iter().map(|p| p.message_id.as_str()).collect()
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn a_resumed_seat_is_stopped_and_asks_at_once() {
        let mut limit = Limit::resumed(1000);
        assert!(limit.is_limited());
        assert_eq!(limit.resets_at(), None);
        assert!(limit.due(1000));
        assert!(limit.hold(|| post("a", false)));
        assert!(!limit.stop(1001, Some(9000)), "already stopped: no second notice");
        // Still no: held, and asked again on the backoff.
        assert!(!limit.answer(1000, Answer::Unavailable));
        assert!(limit.is_limited());
        assert!(!limit.due(1000 + BACKOFF[0] - 1));
        assert!(limit.due(1000 + BACKOFF[0]));
        assert!(limit.answer(1000 + BACKOFF[0], Answer::Allowed));
        assert_eq!(ids(&limit.release()), ["a"]);
    }

    #[test]
    fn the_mailbox_is_its_ids_and_nothing_between_them() {
        // The seat stopped and held m1; the app went down; m2 was said while
        // it was down; the relaunched seat, still stopped, held m3.
        let record = vec![
            by_master("before", &["Codex Luna"]),
            by_master("m1", &["Codex Luna"]),
            said("lin", "Claude Lin", Some("lin"), &[], "body lin"),
            by_master("m2", &["Codex Luna"]),
            by_master("m3", &["Codex Luna", "Claude Lin"]),
        ];
        let held = vec![Held {
            message_id: "m3".into(),
            speaker: "Master".into(),
            at: None,
            content: "body m3".into(),
            addressed: true,
        }];
        let posts = mailbox(&record, &SEAT, &strings(&["m1", "lin", "m3"]), held);
        assert_eq!(ids(&posts), ["m1", "lin", "m3"]);
        assert_eq!(
            posts.iter().filter(|p| p.addressed).map(|p| p.message_id.as_str()).collect::<Vec<_>>(),
            ["m1", "m3"]
        );
        // Read from the record: its time, not memory's missing one.
        assert_eq!(posts[2].at.as_deref(), Some("at m3"));
        let text = digest(&posts).unwrap();
        assert!(text.starts_with("制限中に部屋で 3 件の発言がありました。"));
        assert!(text.contains("body m1"));
        assert!(!text.contains("body m2"));
        assert!(!text.contains("body before"));
        // The restart's memory is empty: the file alone carries it.
        assert_eq!(ids(&mailbox(&record, &SEAT, &strings(&["m1", "lin"]), Vec::new())), ["m1", "lin"]);
    }

    #[test]
    fn what_memory_held_outside_the_file_or_the_record_is_kept() {
        let record = vec![by_master("m", &[]), by_master("n", &[])];
        // "n" was held but its id did not reach the file; "late" reached the
        // record after it was read; "gone" is in neither and is left out.
        let held = vec![post("m", false), post("n", false), post("late", true)];
        let posts = mailbox(&record, &SEAT, &strings(&["m", "gone"]), held);
        assert_eq!(ids(&posts), ["m", "n", "late"]);
        assert!(posts[2].addressed);
        assert_eq!(mailbox(&record, &SEAT, &[], Vec::new()), Vec::new());
    }

    #[test]
    fn a_renamed_seat_is_followed_by_its_account() {
        // Held as "Luna", handed over as "Codex Luna".
        let record = vec![
            said("hi", "Luna", Some("luna"), &[], "hello"),
            by_master("m1", &["Luna"]),
            by_master("m2", &["Codex Luna"]),
        ];
        let posts = mailbox(&record, &SEAT, &strings(&["m1", "m2"]), Vec::new());
        assert!(posts.iter().all(|p| p.addressed));
        // A name another account has spoken under is not taken as the seat's.
        let record = vec![
            said("hi", "Luna", Some("luna"), &[], "hello"),
            said("sol", "Luna", Some("sol"), &[], "I am Luna now"),
            by_master("m1", &["Luna"]),
        ];
        let posts = mailbox(&record, &SEAT, &strings(&["m1"]), Vec::new());
        assert!(!posts[0].addressed);
    }

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("pullcept-mailbox-test-{}", uuid::Uuid::new_v4()));
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn the_mailboxes_outlive_a_restart_and_go_at_the_recovery() {
        let scratch = Scratch::new();
        let path = scratch.0.join("mailboxes").join("topic.json");
        assert_eq!(Mailboxes::read(&path), Mailboxes::default());
        let mut boxes = Mailboxes::default();
        boxes.open("luna");
        boxes.write(&path).unwrap();
        // Stopped with nothing held is still a mailbox.
        assert_eq!(Mailboxes::read(&path).get("luna"), Some(Vec::new()));
        boxes.push("luna", "a");
        boxes.push("luna", "b");
        boxes.push("luna", "a");
        boxes.push("sol", "c");
        boxes.write(&path).unwrap();
        let mut read = Mailboxes::read(&path);
        assert_eq!(read.get("luna"), Some(strings(&["a", "b"])));
        assert_eq!(read.get("sol"), Some(strings(&["c"])));
        assert_eq!(read.get("other"), None);
        // Opening again keeps what is there.
        read.open("luna");
        assert_eq!(read.get("luna"), Some(strings(&["a", "b"])));
        read.close("luna");
        read.write(&path).unwrap();
        assert_eq!(Mailboxes::read(&path).get("luna"), None);
        read.close("sol");
        read.write(&path).unwrap();
        assert!(!path.exists(), "no mailbox left is no file");
        read.write(&path).unwrap();
        // A file that is not what this writes is no mailbox.
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(Mailboxes::read(&path), Mailboxes::default());
        std::fs::write(&path, r#"{"luna":"x"}"#).unwrap();
        let mut odd = Mailboxes::read(&path);
        assert_eq!(odd.get("luna"), None);
        odd.push("luna", "a");
        assert_eq!(odd.get("luna"), Some(strings(&["a"])));
    }
}
