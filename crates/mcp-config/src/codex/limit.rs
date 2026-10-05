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
use serde_json::Value;

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
    pub fn is_limited(&self) -> bool {
        self.stopped.is_some()
    }

    pub fn resets_at(&self) -> Option<i64> {
        self.stopped.as_ref().and_then(|s| s.resets_at)
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
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

    /// The answer of a due question. Whether it confirmed the recovery; the
    /// seat stays stopped, still holding, until [`Limit::release`].
    pub fn answer(&mut self, now: i64, answer: Answer) -> bool {
        let Some(stopped) = self.stopped.as_mut() else {
            return false;
        };
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
    fn the_recovery_hands_over_what_was_held_in_order() {
        let mut limit = Limit::default();
        limit.stop(0, Some(100));
        assert!(limit.hold(|| post("a", false)));
        assert!(limit.hold(|| post("b", true)));
        assert_eq!(limit.held_count(), 2);
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
}
