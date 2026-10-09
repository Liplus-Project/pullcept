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
//!
//! **How each held request ended is written down** (#346): one line in
//! `logs/permission-requests.log` (`permission_prompt::log_line`) — the card,
//! the outcome, the decision and how the press reached the screen, which sign
//! let it go when it went elsewhere, and the times. A press is the only way an
//! answer goes out, and the line is what tells one from the rest on a real
//! device.
//!
//! **Room posts are not typed into a seat waiting on a prompt** (#346). The
//! submit key a post ends with would land on the prompt and take its default.
//! While any agent of the seat is waiting (`Activity::any_waiting`), what the
//! room would type there is kept in the seat's queue (`prompt_hold`), and
//! handed over once the wait has ended, whichever of its ends came:
//! `prompt_hold::RELEASE_GRACE` after it, if no wait has begun again. A seat
//! launched again carries its queue to the new launch. Each kept post and how
//! its hand-over went is a line in the same log.

use crate::session::RoomSeats;
use mcp_config::hook_activity::{Activity, Reporter};
use mcp_config::permission_prompt::{Decision, Press, Request};
use mcp_config::prompt_hold::{self, Delivered, Item, Queue};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

/// How a held request was let go, as its connection is told it.
pub enum Ending {
    /// The person pressed a button on its card, the press as the screen saw it.
    Pressed(Decision, Press),
    /// A sign it was settled elsewhere: `wait-ended`, `terminal-key` or
    /// `relaunch` (`permission_prompt::Ended::reason`).
    Settled(&'static str),
}

/// One permission request held open for the room's answer.
struct Held {
    id: String,
    /// The agent that asked: `None` for the main agent.
    agent_id: Option<String>,
    can_always: bool,
    /// What the screen draws, kept for a screen that asks again
    /// (`permission_requests`).
    card: Value,
    /// Answering or dropping it lets the request go; anything but a press
    /// answers `{}`.
    answer: oneshot::Sender<Ending>,
}

impl Held {
    /// Let it go on a sign it was settled elsewhere.
    fn settle(self, reason: &'static str) {
        let _ = self.answer.send(Ending::Settled(reason));
    }
}

/// One launch's state and what it last told the screen.
struct Seat {
    activity: Activity,
    reporter: Reporter,
    held: Vec<Held>,
    /// Room posts kept while the seat waits on a prompt (#346).
    posts: Queue,
}

impl Seat {
    fn new(topic_id: &str, account_id: &str, pty_id: &str) -> Self {
        Seat {
            activity: Activity::new(),
            reporter: Reporter::new(topic_id, account_id, pty_id),
            held: Vec::new(),
            posts: Queue::new(),
        }
    }

    /// Let go of every held request whose agent is no longer waiting.
    fn settle(&mut self) {
        let (still, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|held| self.activity.waiting(held.agent_id.as_deref()));
        self.held = still;
        for held in gone {
            held.settle("wait-ended");
        }
    }

    /// Whether the kept posts are to be handed over: something is kept and no
    /// agent is waiting.
    fn to_hand_over(&self) -> bool {
        !self.posts.is_empty() && !self.activity.any_waiting()
    }
}

/// A held request, as the hook's connection waits on it.
pub struct Hold {
    pub id: String,
    /// What the screen draws (`permission-request`).
    pub card: Value,
    pub answer: oneshot::Receiver<Ending>,
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
        let (told, hand_over) = {
            let mut seats = self.seats.lock();
            let key = (topic_id.to_string(), account_id.to_string());
            let seat = seats
                .entry(key)
                .or_insert_with(|| Seat::new(topic_id, account_id, &pty_id));
            if seat.reporter.pty_id() != pty_id {
                // Launched again: the last run's prompts are gone with it, and
                // what the room kept for it goes to this one.
                let old = std::mem::replace(seat, Seat::new(topic_id, account_id, &pty_id));
                for held in old.held {
                    held.settle("relaunch");
                }
                seat.posts = old.posts.carry();
            }
            seat.activity.hear(event, body);
            seat.settle();
            (seat.reporter.next(&seat.activity), seat.to_hand_over())
        };
        if let Some(event) = told {
            let _ = app.emit("seat-activity", event);
        }
        if hand_over {
            schedule_hand_over(app, topic_id, account_id);
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
        let (told, hand_over) = {
            let mut seats = self.seats.lock();
            let Some(((topic_id, account_id), seat)) =
                seats.iter_mut().find(|(_, seat)| seat.reporter.pty_id() == pty_id)
            else {
                return;
            };
            seat.activity.typed(data);
            // A prompt answered in the terminal, whichever of the seat's
            // prompts it was: none of them waits on the room any longer. A
            // subagent's prompt surfaces in the same terminal, so its request
            // goes too; its terminal prompt is untouched by that.
            for held in std::mem::take(&mut seat.held) {
                held.settle("terminal-key");
            }
            let hand_over = seat
                .to_hand_over()
                .then(|| (topic_id.clone(), account_id.clone()));
            (seat.reporter.next(&seat.activity), hand_over)
        };
        if let Some(event) = told {
            let _ = app.emit("seat-activity", event);
        }
        if let Some((topic_id, account_id)) = hand_over {
            schedule_hand_over(app, &topic_id, &account_id);
        }
    }

    /// What the room would type into the terminal `pty_id`, kept when that
    /// terminal is a Claude Code seat waiting on a prompt, or queued behind
    /// what is already kept for it (`prompt_hold::Queue::pass`). `Some` is the
    /// item back, to be typed now; `None` is the item kept.
    fn pass(&self, app: &AppHandle, pty_id: &str, item: Item) -> Option<Item> {
        let (kept, topic_id, account_id) = {
            let mut seats = self.seats.lock();
            let Some(((topic_id, account_id), seat)) =
                seats.iter_mut().find(|(_, seat)| seat.reporter.pty_id() == pty_id)
            else {
                return Some(item);
            };
            let id = item.message_id.clone();
            let waiting = seat.activity.any_waiting();
            match seat.posts.pass(waiting, item) {
                Some(item) => return Some(item),
                None => (id, topic_id.clone(), account_id.clone()),
            }
        };
        log(app, &prompt_hold::log_line("kept", &kept, &topic_id, &account_id, &crate::room::now_iso()));
        None
    }

    /// Hand the posts kept for one seat over, once its wait has ended
    /// (`prompt_hold`). Each item is taken under the lock and delivered
    /// outside it, so the limit holds' locks are never taken inside this one.
    fn hand_over(&self, app: &AppHandle, topic_id: &str, account_id: &str) {
        let key = (topic_id.to_string(), account_id.to_string());
        {
            let mut seats = self.seats.lock();
            let Some(seat) = seats.get_mut(&key) else {
                return;
            };
            let waiting = seat.activity.any_waiting();
            if !seat.posts.begin(waiting) {
                return;
            }
        }
        let ptys = app.state::<crate::pty::PtyState>();
        loop {
            let (item, pty_id) = {
                let mut seats = self.seats.lock();
                let Some(seat) = seats.get_mut(&key) else {
                    return;
                };
                let waiting = seat.activity.any_waiting();
                match seat.posts.next(waiting) {
                    Some(item) => (item, seat.reporter.pty_id().to_string()),
                    None => return,
                }
            };
            let id = item.message_id.clone();
            let at = crate::room::now_iso();
            let delivered = prompt_hold::deliver(
                item,
                |post| crate::room::limit_hold(app, &pty_id, post),
                |text| ptys.type_in(&pty_id, text.to_string()),
            );
            match delivered {
                Ok(how) => {
                    let action = match how {
                        Delivered::ToLimit => "to-limit",
                        Delivered::Typed => "typed",
                    };
                    log(app, &prompt_hold::log_line(action, &id, topic_id, account_id, &at));
                }
                Err(item) => {
                    log(app, &prompt_hold::log_line("refused", &id, topic_id, account_id, &at));
                    let mut seats = self.seats.lock();
                    if let Some(seat) = seats.get_mut(&key) {
                        if seat.reporter.pty_id() == pty_id {
                            seat.posts.put_back(item);
                        } else {
                            // Launched again meanwhile: the new launch's queue
                            // carried the rest, and this one goes before them.
                            seat.posts.put_first(item);
                        }
                    }
                    return;
                }
            }
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
    pub fn answer(&self, id: &str, decision: Decision, press: Press) -> bool {
        let mut seats = self.seats.lock();
        for seat in seats.values_mut() {
            let Some(at) = seat.held.iter().position(|held| held.id == id) else {
                continue;
            };
            if decision == Decision::AlwaysAllow && !seat.held[at].can_always {
                return false;
            }
            let held = seat.held.remove(at);
            return held.answer.send(Ending::Pressed(decision, press)).is_ok();
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
///
/// `press` is how the click reached the screen (`permission_prompt::Press`),
/// for the log only: whether a mouse pressed the button or a key activated it.
#[tauri::command]
pub fn permission_answer(
    app: AppHandle,
    id: String,
    decision: String,
    press: Option<Value>,
) -> Result<bool, String> {
    let decision =
        Decision::parse(&decision).ok_or_else(|| format!("unknown decision {decision}"))?;
    let press = press.as_ref().map(Press::read).unwrap_or_default();
    Ok(app.state::<HookSeats>().answer(&id, decision, press))
}

/// Type `item` into the terminal `pty_id`, or keep it for the seat's wait
/// (#346). True when it was typed or kept: either way it is on its way, and
/// the caller hands it nowhere else. False when the terminal is not running.
pub fn type_or_keep(app: &AppHandle, pty_id: &str, item: Item) -> bool {
    match app.state::<HookSeats>().pass(app, pty_id, item) {
        Some(item) => app.state::<crate::pty::PtyState>().type_in(pty_id, item.text),
        None => true,
    }
}

/// Begin a seat's hand-over `prompt_hold::RELEASE_GRACE` after its wait
/// ended, if no wait has begun by then (`HookSeats::hand_over` reads it again).
fn schedule_hand_over(app: &AppHandle, topic_id: &str, account_id: &str) {
    let app = app.clone();
    let (topic_id, account_id) = (topic_id.to_string(), account_id.to_string());
    std::thread::spawn(move || {
        std::thread::sleep(prompt_hold::RELEASE_GRACE);
        app.state::<HookSeats>().hand_over(&app, &topic_id, &account_id);
    });
}

/// One line of `logs/permission-requests.log` (#346), written whole in one
/// call. A failure to open or write is dropped, as the probe's is
/// (`room_log::open_probe`): the log observes.
pub fn log(app: &AppHandle, line: &str) {
    use std::io::Write;
    if let Some(mut file) = crate::room_log::open_probe(app, mcp_config::permission_prompt::LOG_FILE) {
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

/// The permission cards still pressable, for a screen loaded after they came.
#[tauri::command]
pub fn permission_requests(app: AppHandle) -> Vec<Value> {
    app.state::<HookSeats>().cards()
}
