//! The one line a busy seat is typed about what the other seats in its room
//! are doing (#412). See docs/1-room.md, "動いている席にほかの席の様子を一行で
//! 知らせる（#412）".
//!
//! The screen hands each room's seats' badges here whenever they change
//! (`seat_words`, `src/main.ts` `reportSeatWords`): the same word each row of
//! the participant panel says, which is the one place 考え中… and 出力中 are
//! read. Which seat is typed what, and when, is `mcp_config::seat_status`,
//! free of tauri so that it is tested; what stays here is the room's names,
//! the seats' terminals, and the typing.
//!
//! **Typed the way a post is, for every kind of seat**: into the seat's
//! terminal, through the one typing queue (`pty::PtyState::type_status`). A
//! Codex app-server seat's TUI is attached to its thread in that terminal and
//! reads the room's posts from it the same way. Nothing goes through the room:
//! no floor, no log, no `room-message`.
//!
//! **Dropped where a post would wait**: a Claude Code seat waiting on a
//! permission prompt or with posts kept for it (`hook_activity::type_status`),
//! a box the person is typing in. A line not typed is due again at the next
//! change the screen reports.

use crate::room::RoomState;
use crate::session::RoomSeats;
use mcp_config::seat_status::{line, Ledger, Seat, Word};
use parking_lot::Mutex;
use serde::Deserialize;
use tauri::{AppHandle, Manager};

/// What each seat was last typed (`mcp_config::seat_status::Ledger`).
#[derive(Default)]
pub struct SeatStatus {
    ledger: Mutex<Ledger>,
}

/// One seat's badge, as the screen's row says it.
#[derive(Deserialize)]
pub struct SeatWord {
    account_id: String,
    word: String,
}

/// The screen's reading of one room's seats (#412): each seat's account and
/// the badge its row says. Types the line into each seat it is due to.
#[tauri::command]
pub fn seat_words(app: AppHandle, topic_id: String, seats: Vec<SeatWord>) {
    let participants = app.state::<RoomState>().participants(&topic_id);
    let running = app.state::<RoomSeats>();
    let seats: Vec<Seat> = seats
        .iter()
        .filter_map(|seat| {
            let name = participants
                .iter()
                .find(|one| !one.own && one.account.as_deref() == Some(seat.account_id.as_str()))?
                .name
                .clone();
            let pty_id = running.running_pty(&topic_id, &seat.account_id)?;
            Some(Seat {
                pty_id,
                name,
                word: Word::from_badge(&seat.word),
            })
        })
        .collect();
    let state = app.state::<SeatStatus>();
    let due = state.ledger.lock().due(&topic_id, &seats);
    for (pty_id, others) in due {
        let text = line(terminal_input::HEADER_TAG, &others);
        if crate::hook_activity::type_status(&app, &pty_id, text) {
            state.ledger.lock().sent(&topic_id, &pty_id, others);
        }
    }
}
