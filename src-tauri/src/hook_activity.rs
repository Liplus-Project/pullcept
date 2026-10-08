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
//!
//! **Permission prompts held for the room (#336).** A `PermissionRequest` the
//! room can show is held open (`room::serve_hook`) and registered here with the
//! seat, keyed by an id of the app's own: the request carries no
//! `tool_use_id`, so the id, the seat, the launch and the agent are what tell
//! two requests apart. The person's press reaches a held request only through
//! `permission_answer`, a command the webview alone can call; no frame on the
//! room socket, no post and no hook answers one. A held request is let go,
//! answering `{}`, the moment there is a sign it was settled elsewhere: its
//! agent's wait ends (`Activity::waiting`), an answer key is typed into the
//! seat's terminal, the seat is launched again, the CLI closes the connection,
//! or the hold runs out. What the CLI does with a hook still running once the
//! terminal has answered is not observed; letting go at once on those signs
//! keeps the seat from waiting on the room if it does wait, and a room answer
//! arriving after one of them finds nothing to answer.

use crate::session::RoomSeats;
use mcp_config::hook_activity::{Activity, Reporter};
use mcp_config::permission_prompt::{Decision, Request};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

/// One permission request held open for the room's answer.
struct Held {
    id: String,
    /// The agent that asked: `None` for the main agent.
    agent_id: Option<String>,
    can_always: bool,
    /// What the screen draws, kept for a screen that asks again
    /// (`permission_requests`).
    card: Value,
    /// Dropping it lets the request go with `{}`.
    answer: oneshot::Sender<Decision>,
}

/// One launch's state and what it last told the screen.
struct Seat {
    activity: Activity,
    reporter: Reporter,
    held: Vec<Held>,
}

impl Seat {
    fn new(topic_id: &str, account_id: &str, pty_id: &str) -> Self {
        Seat {
            activity: Activity::new(),
            reporter: Reporter::new(topic_id, account_id, pty_id),
            held: Vec::new(),
        }
    }

    /// Let go of every held request whose agent is no longer waiting.
    fn settle(&mut self) {
        let activity = &self.activity;
        self.held.retain(|held| activity.waiting(held.agent_id.as_deref()));
    }
}

/// A held request, as the hook's connection waits on it.
pub struct Hold {
    pub id: String,
    /// What the screen draws (`permission-request`).
    pub card: Value,
    pub answer: oneshot::Receiver<Decision>,
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
            let seat = seats
                .entry(key)
                .or_insert_with(|| Seat::new(topic_id, account_id, &pty_id));
            if seat.reporter.pty_id() != pty_id {
                *seat = Seat::new(topic_id, account_id, &pty_id);
            }
            seat.activity.hear(event, body);
            seat.settle();
            seat.reporter.next(&seat.activity)
        };
        if let Some(event) = told {
            let _ = app.emit("seat-activity", event);
        }
    }

    /// The person typed `data` into the terminal `pty_id` (`pty::write_pty`,
    /// the pane's keys and pastes — never a post the room types in). Handed to
    /// the state of the launch that terminal is, if it is a Claude Code seat
    /// whose hooks have been heard (`Activity::typed`).
    pub fn typed(&self, app: &AppHandle, pty_id: &str, data: &str) {
        if !mcp_config::hook_activity::confirms_prompt(data) {
            return;
        }
        let told = {
            let mut seats = self.seats.lock();
            let Some(seat) = seats.values_mut().find(|seat| seat.reporter.pty_id() == pty_id) else {
                return;
            };
            seat.activity.typed(data);
            // A prompt answered in the terminal, whichever of the seat's
            // prompts it was: none of them waits on the room any longer. A
            // subagent's prompt surfaces in the same terminal, so its request
            // goes too; its terminal prompt is untouched by that.
            seat.held.clear();
            seat.reporter.next(&seat.activity)
        };
        if let Some(event) = told {
            let _ = app.emit("seat-activity", event);
        }
    }

    /// Hold the permission request just heard (`hear`) for the room's answer,
    /// or `None` when it is not to be held: the seat has no state for this
    /// launch, or its agent is not waiting on a prompt by this app's reading
    /// (an agent whose start was not heard, or one whose wait already ended).
    pub fn hold(
        &self,
        topic_id: &str,
        account_id: &str,
        request: &Request,
        card: impl FnOnce(&str, &str) -> Value,
    ) -> Option<Hold> {
        let mut seats = self.seats.lock();
        let seat = seats.get_mut(&(topic_id.to_string(), account_id.to_string()))?;
        if !seat.activity.waiting(request.agent_id.as_deref()) {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, answer) = oneshot::channel();
        let card = card(&id, seat.reporter.pty_id());
        seat.held.push(Held {
            id: id.clone(),
            agent_id: request.agent_id.clone(),
            can_always: request.can_always(),
            card: card.clone(),
            answer: sender,
        });
        Some(Hold { id, card, answer })
    }

    /// Forget a held request whose connection is ending, however it ended.
    pub fn release(&self, id: &str) {
        for seat in self.seats.lock().values_mut() {
            seat.held.retain(|held| held.id != id);
        }
    }

    /// The person pressed `decision` on the card `id`. False when the request
    /// is no longer held — settled elsewhere, answered already — or when it
    /// offers no 常に許可 and that is what was pressed. A request is answered
    /// once: it leaves the seat as the answer goes.
    pub fn answer(&self, id: &str, decision: Decision) -> bool {
        let mut seats = self.seats.lock();
        for seat in seats.values_mut() {
            let Some(at) = seat.held.iter().position(|held| held.id == id) else {
                continue;
            };
            if decision == Decision::AlwaysAllow && !seat.held[at].can_always {
                return false;
            }
            let held = seat.held.remove(at);
            return held.answer.send(decision).is_ok();
        }
        false
    }

    /// Every card still pressable, for a screen drawing them afresh.
    pub fn cards(&self) -> Vec<Value> {
        self.seats
            .lock()
            .values()
            .flat_map(|seat| seat.held.iter().map(|held| held.card.clone()))
            .collect()
    }
}

/// The person's press on a permission card (#336): `deny`, `allow` or
/// `always`. Whether it reached the held hook; false when the request is no
/// longer waiting on the room.
///
/// **Only the person at the screen can answer.** This is a command of the
/// webview, which no session can call: a session speaks through the room
/// socket and its terminal, and neither carries a way to reach a held
/// request. Nothing a post says is read as an answer either.
#[tauri::command]
pub fn permission_answer(app: AppHandle, id: String, decision: String) -> Result<bool, String> {
    let decision =
        Decision::parse(&decision).ok_or_else(|| format!("unknown decision {decision}"))?;
    Ok(app.state::<HookSeats>().answer(&id, decision))
}

/// The permission cards still pressable, for a screen loaded after they came.
#[tauri::command]
pub fn permission_requests(app: AppHandle) -> Vec<Value> {
    app.state::<HookSeats>().cards()
}
