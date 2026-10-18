//! The resume prompt a limited Claude parent is typed at its reset (#357,
//! simplified by #401).
//!
//! The parent's stop and recovery are read from its transcript (#342): a
//! rejection stops it, a later normal parent response recovers it. What this
//! adds is the one input that lets a seat left alone come back by itself. A
//! little after the known reset, the app types [`PROMPT`] once into the
//! terminal that occupies the seat — whichever launch it is. No terminal, no
//! typing: the parent's unanswered rejection stays in its transcript, and the
//! seat's next launch reads it back ([`Resume::restored`]) and types then.
//!
//! After typing, three outcomes. A rejection read after it: still limited, so
//! the next attempt is scheduled and typed again. A normal response: the seat
//! is back, and nothing more is typed. Neither within [`ANSWER_WAIT`]: the
//! seat stopped on something other than the limit (a selection screen, an
//! expired login, a trust prompt), where a typed submit key could pick a
//! choice — so nothing is typed again, the room is told once, and the attempts
//! stop until a new rejection arms them. Child limits are not scheduled here.
//!
//! Times are Unix seconds, passed in; a rejection's `at` is the transcript's
//! milliseconds.
use super::codex::limit::{BACKOFF, GRACE};

/// How long a typed prompt waits for the seat's answer (a rejection or a
/// normal response) before the seat counts as stopped on something else.
pub const ANSWER_WAIT: i64 = 120;

/// What is typed into the seat's terminal at its reset.
pub const PROMPT: &str = "利用上限の解除予定時刻を過ぎました。中断していた作業を再開してください。中断していた作業がなければ、そのまま待機してください。";

/// One line per decision, beside the app's other logs (`logs/<LOG_FILE>`).
pub const LOG_FILE: &str = "claude-resume.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Type [`PROMPT`] now. `retry` when the last one was answered by the
    /// limit. The attempt counts as typed from this moment.
    Type { retry: bool },
    /// The last prompt drew no answer within [`ANSWER_WAIT`]. Nothing is typed
    /// again until the next rejection arms the schedule.
    OtherCause,
}

/// What a rejection meant for an outstanding prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StillLimited {
    pub reset: Option<i64>,
    pub next: i64,
}

#[derive(Debug, Default)]
pub struct Resume {
    next: Option<i64>,
    sent: Option<i64>,
    misses: u32,
    retry: bool,
}

impl Resume {
    /// A parent rejection just applied. Returns `Some` when it answers an
    /// outstanding prompt (the seat is still limited), with the next attempt.
    /// Otherwise it only arms or moves the schedule, and an unknown reset arms
    /// nothing: there is no reset time to act at.
    pub fn rejected(&mut self, at_ms: i64, reset: Option<i64>, now: i64) -> Option<StillLimited> {
        if let Some(sent) = self.sent {
            if at_ms < sent.saturating_mul(1000) {
                return None; // Written before the prompt: not its answer.
            }
            self.sent = None;
            self.retry = true;
            let next = match reset {
                Some(r) if r + GRACE > now + BACKOFF[0] => {
                    self.misses = 0;
                    r + GRACE
                }
                _ => {
                    self.misses = self.misses.saturating_add(1);
                    let step = (self.misses as usize - 1).min(BACKOFF.len() - 1);
                    now + BACKOFF[step]
                }
            };
            self.next = Some(next);
            return Some(StillLimited { reset, next });
        }
        if let Some(r) = reset {
            self.next = Some(if r + GRACE > now {
                r + GRACE
            } else {
                now + BACKOFF[0]
            });
        }
        None
    }

    /// The schedule of a launch whose parent's last event before it is an
    /// unanswered rejection (#366): the pending resume of a seat that had no
    /// terminal at its reset. Before the reset, one prompt a little after it;
    /// with the reset already passed, one right away. An unknown reset arms
    /// nothing, as in [`Resume::rejected`].
    pub fn restored(reset: Option<i64>, now: i64) -> Self {
        Self {
            next: reset.map(|r| (r + GRACE).max(now)),
            ..Self::default()
        }
    }

    /// Called once a second while the parent is limited.
    pub fn tick(&mut self, now: i64) -> Option<Action> {
        if let Some(sent) = self.sent {
            if now >= sent + ANSWER_WAIT {
                *self = Self::default();
                return Some(Action::OtherCause);
            }
            return None;
        }
        if self.next.is_some_and(|n| now >= n) {
            self.next = None;
            self.sent = Some(now);
            return Some(Action::Type { retry: self.retry });
        }
        None
    }

    /// Whether [`Resume::tick`] would type a prompt now. Asked first so the
    /// account's switch (#475) is read only at that moment.
    pub fn due(&self, now: i64) -> bool {
        self.sent.is_none() && self.next.is_some_and(|n| now >= n)
    }

    /// The account's switch is off (#475): the prompt due now is dropped, not
    /// typed. Nothing awaits an answer, so no [`Action::OtherCause`] follows,
    /// and nothing more is scheduled until the next rejection arms it.
    pub fn skip(&mut self) {
        *self = Self::default();
    }

    /// The parent is no longer limited: nothing stays scheduled.
    pub fn recovered(&mut self) {
        *self = Self::default();
    }

    /// Whether a prompt is still to be typed or answered.
    pub fn pending(&self) -> bool {
        self.next.is_some() || self.sent.is_some()
    }

    pub fn next(&self) -> Option<i64> {
        self.next
    }
}

/// The seats the room was told are stopped on another cause ([`Action::OtherCause`]),
/// kept per seat (topic, account) across its launches. A normal response of
/// that seat's parent later answers the notice once, so "look at the terminal"
/// is not left standing.
#[derive(Debug, Default)]
pub struct Stalled(std::collections::HashSet<(String, String)>);

impl Stalled {
    pub fn told(&mut self, topic: &str, account: &str) {
        self.0.insert((topic.into(), account.into()));
    }

    /// The seat's parent answered normally. `true` once when the room was
    /// told of another cause and is now to hear that the seat is back.
    pub fn answered(&mut self, topic: &str, account: &str) -> bool {
        self.0.remove(&(topic.to_string(), account.to_string()))
    }
}

/// One line of [`LOG_FILE`]: `[claude-resume] <action> room=<topic id>
/// account=<account id> at=<time> <detail>`. Actions: `typed`, `pending`,
/// `refused`, `still-limited`, `recovered`, `other-cause`, `skipped` (the
/// account's switch is off, #475). No conversational
/// text is written.
pub fn log_line(action: &str, topic_id: &str, account_id: &str, at: &str, detail: &str) -> String {
    let token = |text: &str| -> String {
        let cleaned: String = text
            .chars()
            .map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c })
            .take(120)
            .collect();
        if cleaned.is_empty() { "-".to_string() } else { cleaned }
    };
    let line = format!(
        "[claude-resume] {} room={} account={} at={}",
        token(action),
        token(topic_id),
        token(account_id),
        token(at)
    );
    if detail.is_empty() {
        line
    } else {
        format!("{line} {}", detail.replace(['\r', '\n'], " "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_reset_arms_one_prompt_a_little_after_it() {
        let mut r = Resume::default();
        assert_eq!(r.rejected(1_000_000, Some(5000), 1000), None);
        assert_eq!(r.next(), Some(5000 + GRACE));
        assert!(r.pending());
        assert_eq!(r.tick(5000), None); // The reset alone is not yet the moment.
        assert_eq!(r.tick(5000 + GRACE), Some(Action::Type { retry: false }));
        assert_eq!(r.tick(5000 + GRACE + 1), None); // One prompt, then wait.
        assert_eq!(r.next(), None);
        assert!(r.pending());
    }

    #[test]
    fn unknown_reset_never_types_into_the_seat() {
        let mut r = Resume::default();
        r.rejected(1_000_000, None, 1000);
        assert!(!r.pending());
        for now in [1000, 100_000, i64::MAX / 2] {
            assert_eq!(r.tick(now), None);
        }
    }

    #[test]
    fn rejection_after_the_prompt_is_still_limited_and_retypes_later() {
        let mut r = Resume::default();
        r.rejected(1_000_000, Some(5000), 1000);
        assert_eq!(r.tick(5030), Some(Action::Type { retry: false }));
        // A rejection written before the prompt is not its answer.
        assert_eq!(r.rejected(5_029_000, Some(5000), 5031), None);
        // Same, passed reset: backoff, never faster than the first step.
        let first = r.rejected(5_032_000, Some(5000), 5032).unwrap();
        assert_eq!(
            first,
            StillLimited {
                reset: Some(5000),
                next: 5032 + BACKOFF[0]
            }
        );
        assert_eq!(r.tick(5032 + BACKOFF[0]), Some(Action::Type { retry: true }));
        let second = r.rejected((5032 + BACKOFF[0] + 2) * 1000, None, 5032 + BACKOFF[0] + 2);
        assert_eq!(second.unwrap().next, 5032 + BACKOFF[0] + 2 + BACKOFF[1]);
        // A later reset (weekly after 5-hour) waits for that reset instead.
        let now = 5032 + BACKOFF[0] + 2 + BACKOFF[1];
        assert_eq!(r.tick(now), Some(Action::Type { retry: true }));
        let weekly = r.rejected(now * 1000, Some(now + 86_400), now).unwrap();
        assert_eq!(weekly.next, now + 86_400 + GRACE);
    }

    #[test]
    fn backoff_is_capped_at_its_last_step() {
        let mut r = Resume::default();
        r.rejected(0, Some(10), 0);
        let mut now = 10 + GRACE;
        let mut waits = Vec::new();
        for _ in 0..8 {
            assert!(matches!(r.tick(now), Some(Action::Type { .. })));
            let still = r.rejected(now * 1000, Some(10), now).unwrap();
            waits.push(still.next - now);
            now = still.next;
        }
        assert_eq!(&waits[..5], &BACKOFF);
        assert!(waits[5..].iter().all(|w| *w == BACKOFF[4]));
    }

    #[test]
    fn no_answer_is_another_cause_and_nothing_is_retyped_until_a_rejection() {
        let mut r = Resume::default();
        r.rejected(0, Some(10), 0);
        assert_eq!(r.tick(40), Some(Action::Type { retry: false }));
        assert_eq!(r.tick(40 + ANSWER_WAIT - 1), None);
        assert_eq!(r.tick(40 + ANSWER_WAIT), Some(Action::OtherCause));
        assert!(!r.pending());
        for now in [40 + ANSWER_WAIT + 1, 100_000, i64::MAX / 2] {
            assert_eq!(r.tick(now), None); // Never a second prompt on its own.
        }
        // A person's manual retry that is rejected arms the schedule again.
        assert_eq!(r.rejected(200_000_000, Some(300_000), 200_000), None);
        assert_eq!(r.tick(300_000 + GRACE), Some(Action::Type { retry: false }));
    }

    #[test]
    fn recovery_clears_the_schedule_and_the_outstanding_prompt() {
        let mut r = Resume::default();
        r.rejected(0, Some(10), 0);
        assert_eq!(r.tick(40), Some(Action::Type { retry: false }));
        r.recovered();
        assert!(!r.pending());
        assert_eq!(r.tick(40 + ANSWER_WAIT), None);
        assert_eq!(r.next(), None);
        // The next stop starts from a clean first step.
        r.rejected(1_000_000, Some(2000), 1000);
        assert_eq!(r.tick(2000 + GRACE), Some(Action::Type { retry: false }));
        assert_eq!(
            r.rejected(2_031_000, None, 2031).unwrap().next,
            2031 + BACKOFF[0]
        );
    }

    /// The transcript lines the seat writes, read by the #342 state.
    fn line(uuid: &str, secs: i64, reset: Option<i64>) -> serde_json::Value {
        let ts = chrono::DateTime::from_timestamp(secs, 0)
            .unwrap()
            .to_rfc3339();
        match reset {
            Some(reset) => {
                serde_json::json!({"type":"assistant","isSidechain":false,"sessionId":"p",
                "timestamp":ts,"uuid":uuid,"isApiErrorMessage":true,"apiErrorStatus":429,"error":"rate_limit",
                "quotaLimits":{"status":"rejected","resetsAt":reset},"message":{"model":"<synthetic>"}})
            }
            None => serde_json::json!({"type":"assistant","isSidechain":false,"sessionId":"p",
                "timestamp":ts,"uuid":uuid,"requestId":"req","message":{"model":"claude-test"}}),
        }
    }

    #[test]
    fn only_the_seats_answer_recovers_it_never_the_prompt() {
        use super::super::claude_limit::{event, Event, State};
        let mut state = State::new(0, false);
        let mut resume = Resume::default();
        let read = |state: &mut State, resume: &mut Resume, v: serde_json::Value, now: i64| {
            let e = event("p", &v).unwrap();
            let rejected = match &e {
                Event::Rejected { at, reset, .. } => Some((*at, *reset)),
                _ => None,
            };
            assert!(state.apply(e));
            rejected.and_then(|(at, reset)| resume.rejected(at, reset, now))
        };
        assert_eq!(
            read(&mut state, &mut resume, line("stop", 100, Some(5000)), 100),
            None
        );
        assert_eq!(resume.tick(5030), Some(Action::Type { retry: false }));
        // Typing the prompt and the reset passing change nothing on the state.
        assert!(state.limited);
        let still = read(
            &mut state,
            &mut resume,
            line("again", 5031, Some(5000)),
            5031,
        );
        assert_eq!(still.map(|s| s.next), Some(5031 + BACKOFF[0]));
        assert!(state.limited);
        assert_eq!(resume.tick(5031 + BACKOFF[0]), Some(Action::Type { retry: true }));
        // The seat's normal answer is what recovers it.
        assert_eq!(
            read(&mut state, &mut resume, line("answer", 5092, None), 5092),
            None
        );
        assert!(!state.limited);
        resume.recovered();
        assert_eq!(resume.tick(5092 + ANSWER_WAIT), None);
    }

    #[test]
    fn a_launch_after_the_reset_types_the_pending_resume_once_right_away() {
        let mut r = Resume::restored(Some(5000), 9000);
        assert!(r.pending());
        assert_eq!(r.tick(9000), Some(Action::Type { retry: false }));
        assert_eq!(r.tick(9001), None);
        assert_eq!(r.tick(9000 + ANSWER_WAIT), Some(Action::OtherCause));
        assert_eq!(r.tick(100_000), None); // Still one prompt in all.
        // A rejection that answers it schedules as before.
        let mut r = Resume::restored(Some(5000), 9000);
        assert_eq!(r.tick(9000), Some(Action::Type { retry: false }));
        assert_eq!(
            r.rejected(9_001_000, Some(5000), 9001).unwrap().next,
            9001 + BACKOFF[0]
        );
    }

    #[test]
    fn a_launch_before_the_reset_types_once_at_the_reset() {
        let mut r = Resume::restored(Some(5000), 3000);
        assert_eq!(r.next(), Some(5000 + GRACE));
        assert_eq!(r.tick(3001), None);
        assert_eq!(r.tick(5000), None);
        assert_eq!(r.tick(5000 + GRACE), Some(Action::Type { retry: false }));
        for now in [5000 + GRACE + 1, 5000 + GRACE + ANSWER_WAIT - 1] {
            assert_eq!(r.tick(now), None); // No double prompt.
        }
    }

    #[test]
    fn a_launch_with_an_unknown_reset_never_types() {
        let mut r = Resume::restored(None, 1000);
        assert!(!r.pending());
        for now in [1000, 100_000, i64::MAX / 2] {
            assert_eq!(r.tick(now), None);
        }
    }

    #[test]
    fn the_prompt_asks_to_resume_without_claiming_recovery() {
        assert!(PROMPT.contains("中断していた作業を再開してください"));
        assert!(PROMPT.contains("そのまま待機"));
        assert!(!PROMPT.contains("確認しました"));
        assert!(!PROMPT.contains('\n'));
    }

    #[test]
    fn a_seat_told_of_another_cause_hears_once_that_it_is_back() {
        let mut stalled = Stalled::default();
        // A normal response with no other-cause notice before it says nothing.
        assert!(!stalled.answered("t", "a"));
        stalled.told("t", "a");
        // Another seat's answer does not settle this one.
        assert!(!stalled.answered("t", "b"));
        assert!(!stalled.answered("other-topic", "a"));
        // This seat's answer does, once.
        assert!(stalled.answered("t", "a"));
        assert!(!stalled.answered("t", "a"));
        // Told again on a later stall, answered again.
        stalled.told("t", "a");
        stalled.told("t", "a");
        assert!(stalled.answered("t", "a"));
        assert!(!stalled.answered("t", "a"));
    }

    #[test]
    fn the_log_line_is_one_line_of_key_values() {
        assert_eq!(
            log_line("typed", "topic", "acct", "2026-10-12T10:00:00.000+09:00", "retry=false"),
            "[claude-resume] typed room=topic account=acct at=2026-10-12T10:00:00.000+09:00 retry=false"
        );
        assert_eq!(
            log_line("pending", "a b", "", "t", "x\ny"),
            "[claude-resume] pending room=a_b account=- at=t x y"
        );
        assert_eq!(log_line("recovered", "t", "a", "now", ""), "[claude-resume] recovered room=t account=a at=now");
    }

    #[test]
    fn switched_off_drops_the_due_prompt_without_other_cause() {
        let mut r = Resume::default();
        r.rejected(1_000_000, Some(5000), 1000);
        assert!(!r.due(5000));
        assert!(r.due(5000 + GRACE));
        r.skip();
        assert!(!r.pending());
        assert!(!r.due(5000 + GRACE));
        for now in [5000 + GRACE, 5000 + GRACE + ANSWER_WAIT, 100_000] {
            assert_eq!(r.tick(now), None); // Never typed, never "other cause".
        }
        // The next rejection arms it again, for a switch turned back on.
        r.rejected(200_000_000, Some(300_000), 200_000);
        assert!(r.due(300_000 + GRACE));
        assert_eq!(r.tick(300_000 + GRACE), Some(Action::Type { retry: false }));
        assert!(!r.due(300_000 + GRACE + 1)); // Typed: awaiting its answer.
    }

    #[test]
    fn switched_off_restored_launch_types_nothing() {
        let mut r = Resume::restored(Some(5000), 9000);
        assert!(r.due(9000));
        r.skip();
        assert_eq!(r.tick(9000), None);
        assert_eq!(r.tick(9000 + ANSWER_WAIT), None);
    }
}
