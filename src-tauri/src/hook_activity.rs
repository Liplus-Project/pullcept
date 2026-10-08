//! What each Claude Code seat is doing, as its hooks tell it (#331). See
//! docs/3-accounts.md, "Claude Code の席は自分の様子を hook で知らせる".
//!
//! The room's listener hands every authorized activity hook here
//! (`room::read_hook`). What a hook means is `mcp_config::hook_activity`, free
//! of tauri so that it is tested; what stays here is which seat's state a hook
//! goes to, and carrying the result to the screen as the same `seat-activity`
//! event a Codex app-server seat sends.
//!
//! **One state per launch.** A seat's state is keyed on the seat and kept with
//! the terminal it was made for. A hook arriving for a seat whose terminal has
//! changed — the account was launched again — starts a fresh state for the
//! new terminal, so nothing the last run left is said about this one. A hook
//! for a seat that has no running terminal is not kept at all.

use crate::session::RoomSeats;
use mcp_config::hook_activity::{Activity, Reporter};
use parking_lot::Mutex;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};

/// One launch's state and what it last told the screen.
struct Seat {
    activity: Activity,
    reporter: Reporter,
}

/// Every Claude Code seat's state, by `(topic id, account id)`.
#[derive(Default)]
pub struct HookSeats {
    seats: Mutex<HashMap<(String, String), Seat>>,
}

impl HookSeats {
    pub fn new() -> Self {
        Self::default()
    }

    /// One hook of `event` for the seat of `account_id` in `topic_id`, its body
    /// as far as the app read it. The state is changed before this returns, so
    /// a hook the CLI sends after the answer finds it already changed.
    pub fn hear(&self, app: &AppHandle, event: &str, topic_id: &str, account_id: &str, body: &[u8]) {
        let Some(pty_id) = app.state::<RoomSeats>().running_pty(topic_id, account_id) else {
            return;
        };
        let told = {
            let mut seats = self.seats.lock();
            let key = (topic_id.to_string(), account_id.to_string());
            let seat = seats.entry(key).or_insert_with(|| Seat {
                activity: Activity::new(),
                reporter: Reporter::new(topic_id, account_id, &pty_id),
            });
            if seat.reporter.pty_id() != pty_id {
                *seat = Seat {
                    activity: Activity::new(),
                    reporter: Reporter::new(topic_id, account_id, &pty_id),
                };
            }
            seat.activity.hear(event, body);
            seat.reporter.next(&seat.activity)
        };
        if let Some(event) = told {
            let _ = app.emit("seat-activity", event);
        }
    }
}
