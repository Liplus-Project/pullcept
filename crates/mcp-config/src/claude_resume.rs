//! The resume attempt of a limited Claude parent at its reset (#357).
//!
//! The parent gate of #342 is untouched: only a new normal parent response
//! read from the transcript releases it. What this adds is the start of that
//! confirmation. A little after the known reset, the app types one short
//! nudge into the seat's terminal; the seat's own answer is the confirmation.
//! A rejection read after the nudge is "still limited" and schedules the next
//! attempt. No answer at all stops the automatic attempts, so that unsent
//! nudges never pile up in an input box. Child limits are not scheduled here.
//!
//! Times are Unix seconds, passed in; a rejection's `at` is the transcript's
//! milliseconds.
use super::codex::limit::{BACKOFF, GRACE};

/// How long a nudge waits for the seat's answer (a rejection or a normal
/// response) before the attempt counts as unanswered.
pub const ANSWER_WAIT: i64 = 120;

/// The one line typed into the terminal. It asks for no work and no room post:
/// any answer at all writes the parent response the gate is waiting for.
pub const NUDGE: &str = "利用上限の解除を確かめるための合図です（Pullcept）。作業は始めず、部屋にも返信せず、一言だけ応答してください。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Type [`NUDGE`] now. The attempt counts as sent from this moment.
    Nudge,
    /// The last nudge drew no answer within [`ANSWER_WAIT`]. Automatic
    /// attempts stop until the next rejection arms them again.
    NoAnswer,
}

/// What a rejection meant for an outstanding nudge.
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
}

impl Resume {
    /// A parent rejection the gate has just applied. Returns `Some` when it
    /// answers an outstanding nudge (the seat is still limited), with the
    /// next attempt. Otherwise it only arms or moves the schedule, and an
    /// unknown reset arms nothing: there is no reset time to act at.
    pub fn rejected(&mut self, at_ms: i64, reset: Option<i64>, now: i64) -> Option<StillLimited> {
        if let Some(sent) = self.sent {
            if at_ms < sent.saturating_mul(1000) {
                return None; // Written before the nudge: not its answer.
            }
            self.sent = None;
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

    /// The schedule of a seat restarted while still limited (#366), from the
    /// reset its last parent rejection named. Before the reset, one nudge a
    /// little after it; with the reset already passed, one nudge right away.
    /// An unknown reset arms nothing, as in [`Resume::rejected`].
    pub fn restored(reset: Option<i64>, now: i64) -> Self {
        Self {
            next: reset.map(|r| (r + GRACE).max(now)),
            ..Self::default()
        }
    }

    /// Called once a second while the parent is limited by a rejection.
    pub fn tick(&mut self, now: i64) -> Option<Action> {
        if let Some(sent) = self.sent {
            if now >= sent + ANSWER_WAIT {
                self.sent = None;
                self.next = None;
                self.misses = 0;
                return Some(Action::NoAnswer);
            }
            return None;
        }
        if self.next.is_some_and(|n| now >= n) {
            self.next = None;
            self.sent = Some(now);
            return Some(Action::Nudge);
        }
        None
    }

    /// The parent is no longer limited: nothing stays scheduled.
    pub fn recovered(&mut self) {
        *self = Self::default();
    }

    pub fn next(&self) -> Option<i64> {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_reset_arms_one_nudge_a_little_after_it() {
        let mut r = Resume::default();
        assert_eq!(r.rejected(1_000_000, Some(5000), 1000), None);
        assert_eq!(r.next(), Some(5000 + GRACE));
        assert_eq!(r.tick(5000), None); // The reset alone is not yet the moment.
        assert_eq!(r.tick(5000 + GRACE), Some(Action::Nudge));
        assert_eq!(r.tick(5000 + GRACE + 1), None); // One nudge, then wait.
        assert_eq!(r.next(), None);
    }

    #[test]
    fn unknown_reset_never_types_into_the_seat() {
        let mut r = Resume::default();
        r.rejected(1_000_000, None, 1000);
        for now in [1000, 100_000, i64::MAX / 2] {
            assert_eq!(r.tick(now), None);
        }
    }

    #[test]
    fn rejection_after_the_nudge_is_still_limited_and_retries_later() {
        let mut r = Resume::default();
        r.rejected(1_000_000, Some(5000), 1000);
        assert_eq!(r.tick(5030), Some(Action::Nudge));
        // A rejection written before the nudge is not its answer.
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
        assert_eq!(r.tick(5032 + BACKOFF[0]), Some(Action::Nudge));
        let second = r.rejected((5032 + BACKOFF[0] + 2) * 1000, None, 5032 + BACKOFF[0] + 2);
        assert_eq!(second.unwrap().next, 5032 + BACKOFF[0] + 2 + BACKOFF[1]);
        // A later reset (weekly after 5-hour) waits for that reset instead.
        let now = 5032 + BACKOFF[0] + 2 + BACKOFF[1];
        assert_eq!(r.tick(now), Some(Action::Nudge));
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
            assert_eq!(r.tick(now), Some(Action::Nudge));
            let still = r.rejected(now * 1000, Some(10), now).unwrap();
            waits.push(still.next - now);
            now = still.next;
        }
        assert_eq!(&waits[..5], &BACKOFF);
        assert!(waits[5..].iter().all(|w| *w == BACKOFF[4]));
    }

    #[test]
    fn no_answer_stops_until_a_rejection_arms_again() {
        let mut r = Resume::default();
        r.rejected(0, Some(10), 0);
        assert_eq!(r.tick(40), Some(Action::Nudge));
        assert_eq!(r.tick(40 + ANSWER_WAIT - 1), None);
        assert_eq!(r.tick(40 + ANSWER_WAIT), Some(Action::NoAnswer));
        assert_eq!(r.tick(100_000), None); // No second nudge on its own.
                                           // A person's manual retry that is rejected arms the schedule again.
        assert_eq!(r.rejected(200_000_000, Some(300_000), 200_000), None);
        assert_eq!(r.tick(300_000 + GRACE), Some(Action::Nudge));
    }

    #[test]
    fn recovery_clears_the_schedule_and_the_outstanding_nudge() {
        let mut r = Resume::default();
        r.rejected(0, Some(10), 0);
        assert_eq!(r.tick(40), Some(Action::Nudge));
        r.recovered();
        assert_eq!(r.tick(40 + ANSWER_WAIT), None);
        assert_eq!(r.next(), None);
        // The next stop starts from a clean first step.
        r.rejected(1_000_000, Some(2000), 1000);
        assert_eq!(r.tick(2000 + GRACE), Some(Action::Nudge));
        assert_eq!(
            r.rejected(2_031_000, None, 2031).unwrap().next,
            2031 + BACKOFF[0]
        );
    }

    /// The transcript lines the seat writes, read by the #342 gate.
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
    fn only_the_seats_answer_clears_the_hold_never_the_nudge() {
        use super::super::claude_limit::{event, Event, Gate};
        let mut gate = Gate::new(0, false);
        let mut resume = Resume::default();
        let read = |gate: &mut Gate, resume: &mut Resume, v: serde_json::Value, now: i64| {
            let e = event("p", &v).unwrap();
            let rejected = match &e {
                Event::Rejected { at, reset, .. } => Some((*at, *reset)),
                _ => None,
            };
            assert!(gate.state.apply(e));
            rejected.and_then(|(at, reset)| resume.rejected(at, reset, now))
        };
        assert_eq!(
            read(&mut gate, &mut resume, line("stop", 100, Some(5000)), 100),
            None
        );
        assert_eq!(resume.tick(5030), Some(Action::Nudge));
        // Typing the nudge and the reset passing change nothing on the gate.
        assert!(gate.is_limited());
        let still = read(
            &mut gate,
            &mut resume,
            line("again", 5031, Some(5000)),
            5031,
        );
        assert_eq!(still.map(|s| s.next), Some(5031 + BACKOFF[0]));
        assert!(gate.is_limited());
        assert_eq!(resume.tick(5031 + BACKOFF[0]), Some(Action::Nudge));
        // The seat's normal answer is what clears it.
        assert_eq!(
            read(&mut gate, &mut resume, line("answer", 5092, None), 5092),
            None
        );
        assert!(!gate.is_limited());
        resume.recovered();
        assert_eq!(resume.tick(5092 + ANSWER_WAIT), None);
    }

    #[test]
    fn restart_before_the_reset_nudges_once_at_the_reset() {
        let mut r = Resume::restored(Some(5000), 3000);
        assert_eq!(r.next(), Some(5000 + GRACE));
        assert_eq!(r.tick(3001), None);
        assert_eq!(r.tick(5000), None);
        assert_eq!(r.tick(5000 + GRACE), Some(Action::Nudge));
        for now in [5000 + GRACE + 1, 5000 + GRACE + ANSWER_WAIT - 1] {
            assert_eq!(r.tick(now), None); // No double nudge.
        }
    }

    #[test]
    fn restart_after_the_reset_nudges_once_right_away() {
        let mut r = Resume::restored(Some(5000), 9000);
        assert_eq!(r.tick(9000), Some(Action::Nudge));
        assert_eq!(r.tick(9001), None);
        assert_eq!(r.tick(9000 + ANSWER_WAIT), Some(Action::NoAnswer));
        assert_eq!(r.tick(100_000), None); // Still one nudge in all.
                                           // A rejection that answers it schedules as before.
        let mut r = Resume::restored(Some(5000), 9000);
        assert_eq!(r.tick(9000), Some(Action::Nudge));
        assert_eq!(
            r.rejected(9_001_000, Some(5000), 9001).unwrap().next,
            9001 + BACKOFF[0]
        );
    }

    #[test]
    fn restart_with_an_unknown_reset_never_types() {
        let mut r = Resume::restored(None, 1000);
        for now in [1000, 100_000, i64::MAX / 2] {
            assert_eq!(r.tick(now), None);
        }
    }

    #[test]
    fn nudge_is_one_line_and_asks_for_no_work() {
        assert!(!NUDGE.contains('\n'));
        assert!(NUDGE.contains("作業は始めず"));
        assert!(NUDGE.chars().count() < 80);
    }
}
