//! A seat's own usage-limit figures, as it reads them back through the room
//! (#364). See docs/1-room.md, "席が自分の利用上限を読む（#364）".
//!
//! The figures are the panel's: the 5-hour and weekly used percentage and
//! when each resets, from a Claude Code seat's status line (`/hooks/status`,
//! `SessionStats::read`) or a Codex seat's rollout (`codex::status`). Nothing
//! new is measured. What this adds is when the app received them, because
//! they arrive only while the seat's session runs, and a seat that has been
//! quiet is holding old ones.
//!
//! This is the book and the answer, free of tauri so that it is tested. The
//! lock, and who records into it, are the app's (`src-tauri/src/room.rs`,
//! `src-tauri/src/codex_status.rs`).
//!
//! **A seat reads its own, and only its own.** The book is keyed on the topic
//! and the account, which is the seat; the room asks it with the pair of the
//! connection asking (`room::serve_participant`), never with a pair the frame
//! names. Another seat's figures are asked of that seat in the room (Master
//! 判断 2026-10-10: every seat's at once grows with the room).
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// One seat's figures as last received. A window whose percentage was not
/// reported is absent, its reset with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub five_hour: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day: Option<f64>,
    pub seven_day_resets_at: Option<i64>,
    /// When the app received these, as Unix seconds.
    pub received_at: i64,
}

/// Every seat's figures, by topic and account.
///
/// Kept after the session ends, like the panel's last values: the seat's next
/// launch in the topic reads them, with how old they are, until its own
/// arrive.
#[derive(Debug, Default)]
pub struct Book {
    seats: HashMap<(String, String), Usage>,
}

impl Book {
    pub fn record(&mut self, topic: &str, account: &str, usage: Usage) {
        self.seats.insert((topic.into(), account.into()), usage);
    }

    pub fn get(&self, topic: &str, account: &str) -> Option<&Usage> {
        self.seats.get(&(topic.into(), account.into()))
    }

    /// The topic is deleted: its seats' figures go with it.
    pub fn forget_topic(&mut self, topic: &str) {
        self.seats.retain(|(t, _), _| t != topic);
    }
}

/// The room's answer to one `usage` frame.
///
/// `held` is the asking seat's figures, or `None` when none have arrived: then
/// the answer carries no `received_at`, which is how the sidecar tells "none
/// yet" from figures. A window not reported is left out of the frame, not sent
/// as zero.
pub fn answer(request_id: &str, held: Option<&Usage>) -> Value {
    let mut frame = Map::new();
    frame.insert("type".into(), json!("usage_result"));
    frame.insert("request_id".into(), json!(request_id));
    if let Some(usage) = held {
        let windows = [
            ("five_hour", usage.five_hour, usage.five_hour_resets_at),
            ("seven_day", usage.seven_day, usage.seven_day_resets_at),
        ];
        for (key, used, resets_at) in windows {
            let Some(used) = used.filter(|n| n.is_finite()) else {
                continue;
            };
            let mut window = Map::new();
            window.insert("used_percentage".into(), json!(used));
            if let Some(at) = resets_at {
                window.insert("resets_at".into(), json!(at));
            }
            frame.insert(key.into(), Value::Object(window));
        }
        frame.insert("received_at".into(), json!(usage.received_at));
    }
    Value::Object(frame)
}

/// The room's answer when it cannot say whose figures these would be: a
/// connection in no room, or one with no account behind it.
pub fn refusal(request_id: &str, error: &str) -> Value {
    json!({ "type": "usage_result", "request_id": request_id, "error": error })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(received_at: i64) -> Usage {
        Usage {
            five_hour: Some(85.0),
            five_hour_resets_at: Some(1791207909),
            seven_day: Some(93.0),
            seven_day_resets_at: Some(1791613577),
            received_at,
        }
    }

    #[test]
    fn a_seat_reads_its_own_figures_and_no_other_seats() {
        let mut book = Book::default();
        book.record("topic", "lin", usage(100));
        book.record("topic", "lay", Usage { five_hour: Some(1.0), ..usage(200) });
        book.record("other", "lin", Usage { five_hour: Some(2.0), ..usage(300) });
        assert_eq!(book.get("topic", "lin"), Some(&usage(100)));
        assert_eq!(book.get("topic", "nobody"), None);

        let frame = answer("r1", book.get("topic", "lin"));
        assert_eq!(frame["type"], "usage_result");
        assert_eq!(frame["request_id"], "r1");
        assert_eq!(frame["five_hour"]["used_percentage"], 85.0);
        assert_eq!(frame["five_hour"]["resets_at"], 1791207909);
        assert_eq!(frame["seven_day"]["used_percentage"], 93.0);
        assert_eq!(frame["seven_day"]["resets_at"], 1791613577);
        assert_eq!(frame["received_at"], 100);
    }

    #[test]
    fn a_newer_report_replaces_the_seats_figures_whole() {
        let mut book = Book::default();
        book.record("topic", "lin", usage(100));
        let later = Usage { five_hour: None, five_hour_resets_at: None, ..usage(160) };
        book.record("topic", "lin", later.clone());
        assert_eq!(book.get("topic", "lin"), Some(&later));
    }

    #[test]
    fn an_absent_window_is_left_out_not_zero() {
        let held = Usage {
            five_hour: None,
            five_hour_resets_at: Some(1791207909),
            seven_day: Some(40.0),
            seven_day_resets_at: None,
            received_at: 7,
        };
        let frame = answer("r", Some(&held));
        assert!(frame.get("five_hour").is_none(), "{frame}");
        assert_eq!(frame["seven_day"]["used_percentage"], 40.0);
        assert!(frame["seven_day"].get("resets_at").is_none(), "{frame}");
        assert_eq!(frame["received_at"], 7);
    }

    #[test]
    fn nothing_received_yet_carries_no_received_at() {
        let frame = answer("r", None);
        assert_eq!(frame, json!({ "type": "usage_result", "request_id": "r" }));
    }

    #[test]
    fn a_deleted_topic_takes_its_figures_with_it() {
        let mut book = Book::default();
        book.record("gone", "lin", usage(1));
        book.record("kept", "lin", usage(2));
        book.forget_topic("gone");
        assert_eq!(book.get("gone", "lin"), None);
        assert_eq!(book.get("kept", "lin"), Some(&usage(2)));
    }

    #[test]
    fn a_refusal_names_why() {
        let frame = refusal("r", "no account");
        assert_eq!(frame["error"], "no account");
        assert!(frame.get("received_at").is_none());
    }
}
