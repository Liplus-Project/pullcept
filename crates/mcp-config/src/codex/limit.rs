//! A Codex CLI seat stopped on its usage limit, and resumed (#294, #403).
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
//!   limited;
//! - **the usage**, off the same answer ([`read_usage`], #496): the two
//!   windows it reports, for a limited seat whose rollout no longer moves.
//!
//! [`Limit`] is one seat: free, or stopped with when to ask next. Times are
//! Unix seconds, passed in. Nothing is held back from a stopped seat (#403):
//! the room types into it as into any seat, and a post typed there while it is
//! stopped stays in its conversation as a turn the limit refused.
//!
//! A stopped seat has a mark ([`Mailboxes`], #312): a small file beside its
//! topic's record that outlives the session and the app. A seat launched while
//! its mark is still there starts stopped ([`Limit::resumed`]), so the resume
//! is typed by whichever launch holds the seat when the recovery is confirmed.
use serde_json::Value;
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

/// The id of `account/read` (#495), asked after [`READ_ID`] on the same
/// process: which account the seat's `CODEX_HOME` is logged in to.
pub const ACCOUNT_ID: u64 = 3;

/// `account/read` without a token refresh: the question only reads.
pub fn account_request() -> String {
    format!(r#"{{"id":{ACCOUNT_ID},"method":"account/read","params":{{"refreshToken":false}}}}"#)
}

/// The name the panel shows for the account the response to [`ACCOUNT_ID`]
/// names (#495): a ChatGPT login's email, else the kind of login. `None`
/// when no account is logged in or the response is not one.
pub fn read_login(response: &Value) -> Option<String> {
    let account = &response["result"]["account"];
    if let Some(email) = account["email"].as_str().filter(|s| !s.is_empty()) {
        return Some(email.into());
    }
    match account["type"].as_str()? {
        "chatgpt" => Some("ChatGPT".into()),
        "apiKey" => Some("API キー".into()),
        "amazonBedrock" => Some("Amazon Bedrock".into()),
        _ => None,
    }
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

/// The 5-hour and weekly windows an `account/rateLimits/read` answer reports
/// (#496): what the panel and `my_usage` show of a seat whose turns, and so
/// whose rollout, no longer move while it is limited.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Windows {
    pub five_hour: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day: Option<f64>,
    pub seven_day_resets_at: Option<i64>,
}

/// The two windows of the response to [`READ_ID`], or `None` when it carries
/// none to read.
///
/// Read as the rollout's are (`status`): by window length (300 / 10080
/// minutes), not by position, and only from the `codex` bucket — the `codex`
/// entry of `rateLimitsByLimitId` when it is there, otherwise `rateLimits`
/// when its `limitId` is `codex` or absent. Both windows are replaced
/// together, a missing one by `None`.
///
/// An answer [`read_answer`] reads as [`Answer::Unavailable`] (no boolean
/// `ordinaryUsageAllowed`) gives `None`, and so does one with neither window,
/// so the figures already held stand.
pub fn read_usage(response: &Value) -> Option<Windows> {
    let result = &response["result"];
    result["ordinaryUsageAllowed"].as_bool()?;
    let bucket = match &result["rateLimitsByLimitId"]["codex"] {
        Value::Object(_) => &result["rateLimitsByLimitId"]["codex"],
        _ => {
            let single = &result["rateLimits"];
            if !single.is_object() || !single["limitId"].as_str().is_none_or(|id| id == "codex") {
                return None;
            }
            single
        }
    };
    let window = |minutes: u64| {
        ["primary", "secondary"]
            .iter()
            .map(|key| &bucket[key])
            .find(|w| w["windowDurationMins"].as_u64() == Some(minutes))
    };
    let used = |w: Option<&Value>| w.and_then(|w| w["usedPercent"].as_f64().filter(|n| n.is_finite()));
    let reset = |w: Option<&Value>| w.and_then(|w| crate::epoch_seconds(&w["resetsAt"]));
    let (five, seven) = (window(300), window(10080));
    let windows = Windows {
        five_hour: used(five),
        five_hour_resets_at: reset(five),
        seven_day: used(seven),
        seven_day_resets_at: reset(seven),
    };
    (windows.five_hour.is_some() || windows.seven_day.is_some()).then_some(windows)
}

/// How long after the reset the first question is put: "a little after".
pub const GRACE: i64 = 30;

/// The waits between questions once the reset has passed and the answer is
/// still no, or no answer came. The last one repeats.
pub const BACKOFF: [i64; 5] = [60, 120, 300, 600, 900];

/// How long the resume input waits for the seat's answer (a turn starting or
/// ending) before the seat counts as stopped on something other than the
/// limit (#403, the same wait as a Claude seat's, #401).
pub const ANSWER_WAIT: i64 = 120;

/// What is typed into the seat once its recovery is confirmed (#389).
pub const CONTINUE: &str = "利用上限の解除を確認しました。中断していた作業を再開してください。中断していた作業がなければ、そのまま待機してください。";

#[derive(Debug, Clone, PartialEq)]
struct Stopped {
    resets_at: Option<i64>,
    next_query: i64,
    misses: u32,
}

/// One Codex seat: free or stopped, and whether the resume typed into it is
/// still waiting for its answer.
#[derive(Debug, Default)]
pub struct Limit {
    stopped: Option<Stopped>,
    /// When [`CONTINUE`] was typed, until a turn starts or ends.
    typed: Option<i64>,
}

fn first_query(now: i64, resets_at: Option<i64>) -> i64 {
    match resets_at {
        Some(reset) if reset + GRACE > now => reset + GRACE,
        _ => now + BACKOFF[0],
    }
}

impl Limit {
    /// A seat launched while its mark is still there (#312): stopped from the
    /// start, with its first question due now. Not at a reset: none is known,
    /// and the one the stop was told may long have passed.
    pub fn resumed(now: i64) -> Self {
        Limit {
            stopped: Some(Stopped {
                resets_at: None,
                next_query: now,
                misses: 0,
            }),
            typed: None,
        }
    }

    pub fn is_limited(&self) -> bool {
        self.stopped.is_some()
    }

    pub fn resets_at(&self) -> Option<i64> {
        self.stopped.as_ref().and_then(|s| s.resets_at)
    }

    /// The rollout said a turn ended on the limit. Whether that stopped a seat
    /// that was free — the moment the room is told. A seat already stopped
    /// stays as it is: its schedule is the app-server's to move. A resume
    /// answered by the limit is answered: the stop starts the cycle again, and
    /// the next confirmed recovery types it again.
    pub fn stop(&mut self, now: i64, resets_at: Option<i64>) -> bool {
        self.typed = None;
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

    /// Whether a question is due now.
    pub fn due(&self, now: i64) -> bool {
        self.stopped.as_ref().is_some_and(|s| now >= s.next_query)
    }

    /// A turn finished without error while stopped — a post typed in by the
    /// room or a person after the limit was reset by hand (#377). Not a
    /// recovery by itself; it brings the next question to now.
    pub fn turn_completed(&mut self, now: i64) {
        if let Some(stopped) = self.stopped.as_mut() {
            stopped.next_query = stopped.next_query.min(now);
        }
    }

    /// The answer of a due question. Whether it confirmed the recovery; the
    /// seat stays stopped until [`Limit::free`].
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

    /// The recovery is confirmed and the resume goes in: the seat is free.
    pub fn free(&mut self) {
        self.stopped = None;
    }

    /// [`CONTINUE`] was typed into the seat now: its answer is awaited.
    pub fn resume_typed(&mut self, now: i64) {
        self.typed = Some(now);
    }

    /// A turn of the seat started or ended: the resume has its answer.
    pub fn answered(&mut self) {
        self.typed = None;
    }

    /// Whether the resume drew no answer within [`ANSWER_WAIT`]: the seat is
    /// stopped on something else (a selection screen, an expired login, a
    /// trust prompt), where a submit key could pick a choice. `true` once;
    /// nothing is typed again for it.
    pub fn unanswered(&mut self, now: i64) -> bool {
        if self.typed.is_some_and(|typed| now >= typed + ANSWER_WAIT) {
            self.typed = None;
            return true;
        }
        false
    }
}

/// The marks of one topic's stopped seats, as the small file beside the
/// topic's record keeps them across a session's end and the app's (#312): by
/// account, from the seat's stop until its resume is typed. The file goes with
/// its topic (`topic_index::delete`). A seat's value is an empty list; a file
/// written before #403 carries the ids of posts held then, and is read as the
/// same mark.
///
/// A file that is missing, cannot be read, or is not what this writes is no
/// mark at all.
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
    /// short leaves the last one standing. No mark left is no file.
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

    /// Whether the seat is marked stopped.
    pub fn has(&self, account_id: &str) -> bool {
        self.seats.contains_key(account_id)
    }

    /// The seat has stopped: it is marked from now. A mark already there is
    /// kept as it is.
    pub fn open(&mut self, account_id: &str) {
        self.seats
            .entry(account_id.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    /// The seat's resume is being typed: its mark goes.
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
    format!("{name} は利用上限で止まりました。{when}")
}

/// What the room is told when the recovery is confirmed and the resume is
/// about to be typed into the seat. `resume` is the account's switch for it
/// (#475): off, the recovery is still told and nothing is typed.
pub fn recovery_notice(name: &str, resume: bool) -> String {
    if resume {
        format!("{name} の利用上限の解除を確かめました。{name} の端末へ再開の指示を打ちます。")
    } else {
        format!("{name} の利用上限の解除を確かめました。このアカウントは再開の指示を打たない設定のため、端末へは打ちません。")
    }
}

/// The resume drew no answer within [`ANSWER_WAIT`] (#403).
pub fn other_cause(name: &str) -> String {
    format!("{name} は、再開の指示を打ってから応答がありません。利用上限以外の理由（起動時の選択画面、ログイン切れ、信頼の確認など）で止まっているようです。指示は打ち直しません。端末を見てください。")
}

/// A seat the room was told of with [`other_cause`] finished a turn again.
pub fn back_after_other_cause(name: &str) -> String {
    format!("{name} は再開しました（正常な応答を確認）。先の「端末を見てください」は解消しています。")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn login_is_the_email_else_the_kind_of_login() {
        let answer = |account: Value| serde_json::json!({"id": ACCOUNT_ID, "result": {"account": account, "requiresOpenaiAuth": true}});
        assert_eq!(read_login(&answer(serde_json::json!({"type": "chatgpt", "email": "l@example.com", "planType": "pro"}))),
            Some("l@example.com".into()));
        assert_eq!(read_login(&answer(serde_json::json!({"type": "chatgpt", "email": null, "planType": "pro"}))),
            Some("ChatGPT".into()));
        assert_eq!(read_login(&answer(serde_json::json!({"type": "apiKey"}))), Some("API キー".into()));
        assert_eq!(read_login(&answer(Value::Null)), None);
        assert_eq!(read_login(&serde_json::json!({"id": ACCOUNT_ID, "error": {"message": "x"}})), None);
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
    fn the_answer_carries_the_two_windows() {
        // The measured answer: 5 hours full, weekly at 96.
        assert_eq!(
            read_usage(&json(DENIED)),
            Some(Windows {
                five_hour: Some(100.0),
                five_hour_resets_at: Some(1791207908),
                seven_day: Some(96.0),
                seven_day_resets_at: Some(1791613576),
            })
        );
        // An allowed answer carries them too, told apart by length, not position.
        assert_eq!(
            read_usage(&json(r#"{"id":2,"result":{"ordinaryUsageAllowed":true,"rateLimits":{"primary":{"usedPercent":12.5,"windowDurationMins":10080,"resetsAt":20},"secondary":{"usedPercent":3,"windowDurationMins":300,"resetsAt":10}}}}"#)),
            Some(Windows {
                five_hour: Some(3.0),
                five_hour_resets_at: Some(10),
                seven_day: Some(12.5),
                seven_day_resets_at: Some(20),
            })
        );
        // The codex bucket wins over the single-bucket view.
        assert_eq!(
            read_usage(&json(r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"limitId":"other","primary":{"usedPercent":1,"windowDurationMins":300}},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":77,"windowDurationMins":300}}}}}"#)),
            Some(Windows { five_hour: Some(77.0), ..Windows::default() })
        );
        // Nothing to read: unavailable answers, another bucket, no window of
        // either length. The figures already held stand.
        for none in [
            r#"{"id":2,"result":{"ordinaryUsageAllowed":null,"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300}}}}"#,
            r#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":50,"windowDurationMins":300}}}}"#,
            r#"{"id":2,"error":{"code":-32600,"message":"no"}}"#,
            r#"null"#,
            r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"limitId":"other","primary":{"usedPercent":50,"windowDurationMins":300}}}}"#,
            r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"primary":{"usedPercent":50,"windowDurationMins":60}}}}"#,
            r#"{"id":2,"result":{"ordinaryUsageAllowed":false,"rateLimits":{"primary":{"usedPercent":"x","windowDurationMins":300}}}}"#,
            r#"{"id":2,"result":{"ordinaryUsageAllowed":false}}"#,
        ] {
            assert_eq!(read_usage(&json(none)), None, "{none}");
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
        assert_eq!(json(&account_request())["id"], ACCOUNT_ID);
        assert_eq!(json(&account_request())["method"], "account/read");
        assert_eq!(json(&account_request())["params"]["refreshToken"], false);
    }

    #[test]
    fn a_free_seat_is_never_due() {
        let mut limit = Limit::default();
        assert!(!limit.is_limited());
        assert!(!limit.due(i64::MAX));
        assert!(!limit.answer(0, Answer::Allowed));
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
        // A room post typed into a seat whose limit was reset by hand (#377).
        let mut limit = Limit::default();
        limit.stop(0, Some(10_000));
        limit.turn_completed(50);
        assert!(limit.is_limited());
        assert!(limit.due(50));
    }

    #[test]
    fn a_confirmed_recovery_frees_the_seat_once() {
        let mut limit = Limit::default();
        limit.stop(100, Some(200));
        assert!(!limit.answer(201, Answer::Unavailable));
        assert!(!limit.answer(202, Answer::Denied { resets_at: Some(200) }));
        assert!(limit.answer(204, Answer::Allowed));
        // Confirmed, but stopped until freed.
        assert!(limit.is_limited());
        limit.free();
        assert!(!limit.is_limited());
        assert!(!limit.answer(205, Answer::Allowed));
    }

    #[test]
    fn the_resume_is_answered_by_a_turn_or_by_the_limit_or_counts_as_another_cause() {
        // A turn starts or ends: answered, nothing more.
        let mut limit = Limit::default();
        limit.resume_typed(1000);
        limit.answered();
        assert!(!limit.unanswered(1000 + ANSWER_WAIT));
        // The limit again: a new stop, and no other cause.
        let mut limit = Limit::default();
        limit.resume_typed(1000);
        assert!(limit.stop(1010, Some(5000)));
        assert!(!limit.unanswered(1000 + ANSWER_WAIT));
        // Nothing: once, at the wait's end, and not again.
        let mut limit = Limit::default();
        limit.resume_typed(1000);
        assert!(!limit.unanswered(1000 + ANSWER_WAIT - 1));
        assert!(limit.unanswered(1000 + ANSWER_WAIT));
        assert!(!limit.unanswered(1000 + 2 * ANSWER_WAIT));
        assert!(!limit.is_limited());
    }

    #[test]
    fn the_notices_name_the_seat_and_the_reset_and_hold_nothing() {
        assert_eq!(
            stop_notice("Codex Luna", Some("10月5日 22:45")),
            "Codex Luna は利用上限で止まりました。解除予定は 10月5日 22:45 です。"
        );
        assert!(stop_notice("L", None).contains("解除予定は分かっていません。"));
        for text in [stop_notice("L", None), recovery_notice("L", true), recovery_notice("L", false)] {
            assert!(!text.contains("預か"), "{text}");
        }
        assert!(recovery_notice("L", true).contains("再開の指示を打ちます"));
        // Switched off (#475): the recovery is told, the prompt is not promised.
        let off = recovery_notice("L", false);
        assert!(off.contains("解除を確かめました") && off.contains("打たない設定") && !off.contains("打ちます。"), "{off}");
        assert!(other_cause("L").contains("端末を見てください"));
        assert!(back_after_other_cause("L").contains("解消"));
        assert!(CONTINUE.contains("中断していた作業を再開してください"));
        assert!(CONTINUE.contains("作業がなければ、そのまま待機"));
        assert!(!CONTINUE.contains("read_room_history"));
    }

    #[test]
    fn a_resumed_seat_is_stopped_and_asks_at_once() {
        let mut limit = Limit::resumed(1000);
        assert!(limit.is_limited());
        assert_eq!(limit.resets_at(), None);
        assert!(limit.due(1000));
        assert!(!limit.stop(1001, Some(9000)), "already stopped: no second notice");
        // Still no: asked again on the backoff.
        assert!(!limit.answer(1000, Answer::Unavailable));
        assert!(limit.is_limited());
        assert!(!limit.due(1000 + BACKOFF[0] - 1));
        assert!(limit.due(1000 + BACKOFF[0]));
        assert!(limit.answer(1000 + BACKOFF[0], Answer::Allowed));
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
    fn the_marks_outlive_a_restart_and_go_at_the_resume() {
        let scratch = Scratch::new();
        let path = scratch.0.join("mailboxes").join("topic.json");
        assert_eq!(Mailboxes::read(&path), Mailboxes::default());
        let mut boxes = Mailboxes::default();
        boxes.open("luna");
        boxes.open("sol");
        boxes.write(&path).unwrap();
        let mut read = Mailboxes::read(&path);
        assert!(read.has("luna") && read.has("sol"));
        assert!(!read.has("other"));
        read.close("luna");
        read.write(&path).unwrap();
        assert!(!Mailboxes::read(&path).has("luna"));
        read.close("sol");
        read.write(&path).unwrap();
        assert!(!path.exists(), "no mark left is no file");
        read.write(&path).unwrap();
        // A file written before #403, with held ids, is the same mark.
        std::fs::write(&path, r#"{"luna":["a","b"]}"#).unwrap();
        let mut old = Mailboxes::read(&path);
        assert!(old.has("luna"));
        old.open("luna");
        assert!(old.has("luna"));
        // A file that is not what this writes is no mark.
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(Mailboxes::read(&path), Mailboxes::default());
    }
}
