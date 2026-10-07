//! A Codex CLI seat stopped on its usage limit (#294). See docs/3-accounts.md,
//! "Codex の席が利用上限で止まったとき".
//!
//! What is read and decided is `mcp_config::codex::limit`, free of tauri so a
//! test can run it. What stays here is what needs the app: the table of
//! seats the room consults before typing (`CodexLimits`), the question put to
//! Codex's app-server, and the notices and the one line handed back.
//!
//! **Driven from the rollout watcher** (`codex_status`), one per Codex launch,
//! on its one-second round. A turn the rollout says ended on the limit stops
//! the seat; while stopped, the room keeps its posts back from that terminal
//! (`room::type_into_sessions`), and the watcher asks the app-server when a
//! question is due. Claude Code seats never enter the table and are never
//! asked about.
//!
//! **The app-server is asked over stdio, one short-lived process per
//! question** (`codex app-server`, the default `stdio://` transport: one JSON
//! object per line). Chosen over the shared daemon's AF_UNIX socket because
//! it needs nothing Windows' std lacks, starts no daemon, and runs under the
//! seat's own `CODEX_HOME` and environment, so a seat on another account is
//! asked about that account. Measured 2026-10-05 with Codex CLI 0.160.0:
//! `initialize` then `account/rateLimits/read` answered in under half a
//! second, no thread started, and the process ends when its stdin closes.
//! A process that does not answer within `ASK_TIMEOUT` is killed and the
//! answer is `Unavailable`, which keeps the seat stopped.
//!
//! **A stopped seat's mailbox outlives the session and the app** (#312): the
//! id of every post held for it is written to a small file beside its topic's
//! record (`limit::Mailboxes`) as it is held. A seat launched while its mailbox
//! is there starts stopped and asks at once (`Limiter::open`); the recovery
//! hands over exactly the posts the mailbox names, read from the record, and
//! the mailbox goes.

use crate::room::{RoomState, SessionStats};
use mcp_config::codex::limit::{self, Answer, Held, Limit, Mailboxes, TurnEnd};
use mcp_config::codex::status::Status;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Every Codex seat a watcher is following, by its terminal.
#[derive(Default)]
pub struct CodexLimits {
    seats: Mutex<HashMap<String, Entry>>,
}

/// One seat in the table: its limit, and where its mailbox is kept.
struct Entry {
    limit: Limit,
    account_id: String,
    /// The topic's mailbox file, or `None` when it could not be placed: the
    /// seat is then held in memory only, as before #312.
    mailboxes: Option<PathBuf>,
}

impl Entry {
    /// The seat's mailbox as its file has it, empty when it has none.
    fn stored(&self) -> Vec<String> {
        self.mailboxes
            .as_deref()
            .and_then(|path| Mailboxes::read(path).get(&self.account_id))
            .unwrap_or_default()
    }

    /// Change the seat's mailbox in its file. Every change goes through the
    /// table's lock, so two seats of one topic do not write over each other.
    /// A failure is said and goes no further: memory still holds the post.
    fn persist(&self, change: impl FnOnce(&mut Mailboxes, &str)) {
        let Some(path) = self.mailboxes.as_deref() else {
            return;
        };
        let mut boxes = Mailboxes::read(path);
        change(&mut boxes, &self.account_id);
        if let Err(err) = boxes.write(path) {
            eprintln!("[codex-limit] the mailbox could not be written: {err}");
        }
    }
}

impl CodexLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep a post back from the seat on `pty_id` if it is stopped, its id
    /// written to the seat's mailbox. `false` for a free seat and for a
    /// terminal that is not a Codex seat's.
    pub fn hold(&self, pty_id: &str, post: impl FnOnce() -> Held) -> bool {
        let mut seats = self.seats.lock();
        let Some(entry) = seats.get_mut(pty_id) else {
            return false;
        };
        let mut id = None;
        let held = entry.limit.hold(|| {
            let post = post();
            id = Some(post.message_id.clone());
            post
        });
        if let Some(id) = id {
            entry.persist(|boxes, account| boxes.push(account, &id));
        }
        held
    }

    /// The seat on `pty_id` has left the table. Its mailbox stays.
    pub fn forget(&self, pty_id: &str) {
        self.seats.lock().remove(pty_id);
    }
}

/// How to ask the app-server about one seat's account.
pub struct Asker {
    pub command: String,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub home: PathBuf,
}

/// One Codex seat's limit, as its watcher drives it.
pub struct Limiter {
    pub app: AppHandle,
    pub topic_id: String,
    pub account_id: String,
    pub pty_id: String,
    /// The seat's name in the room: what the notices call it.
    pub name: String,
    pub asker: Asker,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl Limiter {
    fn limits(&self) -> tauri::State<'_, CodexLimits> {
        self.app.state::<CodexLimits>()
    }

    fn mailboxes(&self) -> Option<PathBuf> {
        crate::room_log::mailboxes_path(&self.app, &self.topic_id)
            .map_err(|err| eprintln!("[codex-limit] the mailbox has no place: {err}"))
            .ok()
    }

    fn entry(&self, limit: Limit) -> Entry {
        Entry {
            limit,
            account_id: self.account_id.clone(),
            mailboxes: self.mailboxes(),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Entry) -> R) -> R {
        let limits = self.limits();
        let mut seats = limits.seats.lock();
        let entry = match seats.entry(self.pty_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(self.entry(Limit::default()))
            }
        };
        f(entry)
    }

    /// Put the seat in the table, before the room can type into it (#312).
    /// A mailbox left from an earlier launch — the seat was still stopped
    /// when its session or the app ended — starts it stopped, its first
    /// question due at once (`Limit::resumed`). Nothing is said to the room:
    /// the stop was told when it happened.
    pub fn open(&self) {
        let entry = self.entry(Limit::default());
        let left = entry
            .mailboxes
            .as_deref()
            .and_then(|path| Mailboxes::read(path).get(&self.account_id))
            .is_some();
        let entry = Entry {
            limit: if left { Limit::resumed(now()) } else { Limit::default() },
            ..entry
        };
        self.limits().seats.lock().insert(self.pty_id.clone(), entry);
    }

    pub fn is_limited(&self) -> bool {
        self.with(|entry| entry.limit.is_limited())
    }

    /// One round: the turn end the rollout last reported, if any, then a due
    /// question. `rollout_reset` is the reset the rollout last reported.
    /// Whether 制限中 turned over, for the caller to send to the screen.
    pub fn round(&self, end: Option<TurnEnd>, rollout_reset: Option<i64>) -> bool {
        let was = self.is_limited();
        match end {
            Some(TurnEnd::UsageLimit) => {
                // The mailbox from the stop on, empty until a post is held,
                // so a seat stopped when the app goes down starts stopped.
                let stopped = self.with(|entry| {
                    let stopped = entry.limit.stop(now(), rollout_reset);
                    if stopped {
                        entry.persist(|boxes, account| boxes.open(account));
                    }
                    stopped
                });
                if stopped {
                    self.announce_stop();
                }
            }
            Some(TurnEnd::Completed) => {
                self.with(|entry| entry.limit.turn_completed(now()));
            }
            Some(TurnEnd::Failed) | None => {}
        }
        if self.with(|entry| entry.limit.due(now())) {
            let answer = ask(&self.asker);
            let recovered = self.with(|entry| entry.limit.answer(now(), answer));
            if recovered {
                self.recover();
            }
        }
        self.is_limited() != was
    }

    /// The seat has just stopped. One question for the reset the room is
    /// told, then the notice. The room's posts are already being held.
    fn announce_stop(&self) {
        let answer = ask(&self.asker);
        let reset = self.with(|entry| {
            entry.limit.refine(now(), answer);
            entry.limit.resets_at()
        });
        let clock = reset.and_then(terminal_input::clock);
        let content = limit::stop_notice(&self.name, clock.as_deref());
        let room = self.app.state::<RoomState>();
        crate::room::post_app_notice(
            &self.app,
            &room,
            &self.topic_id,
            &content,
            Some(&self.pty_id),
        );
    }

    /// The app-server said the seat may run again. The room is told first,
    /// with the seat still holding, so the notice is not typed into it; then
    /// the seat is freed and handed its one line under the same lock, so a
    /// post arriving after is typed after it.
    ///
    /// What the notice counts and the line carries are the posts the seat's
    /// mailbox names (`limit::mailbox`, #312), read from the topic's record:
    /// a seat restarted while stopped holds in memory only what came after its
    /// restart, and its mailbox file holds the rest. The record is read once,
    /// before the notice goes in.
    fn recover(&self) {
        let record = self.record();
        let seat = limit::Seat {
            name: &self.name,
            account_id: &self.account_id,
        };
        let (ids, held) = self.with(|entry| (entry.stored(), entry.limit.held().to_vec()));
        let count = limit::mailbox(&record, &seat, &ids, held).len();
        let room = self.app.state::<RoomState>();
        let notice = crate::room::post_app_notice(
            &self.app,
            &room,
            &self.topic_id,
            &limit::recovery_notice(&self.name, count),
            Some(&self.pty_id),
        );
        let ptys = self.app.state::<crate::pty::PtyState>();
        self.with(|entry| {
            // Read again with what was held up to the release, so a post held
            // after the record was read is not lost.
            let ids = entry.stored();
            let held = limit::mailbox(&record, &seat, &ids, entry.limit.release());
            // The mailbox goes before the line is typed: cut short between the
            // two, the seat misses its line rather than being handed it twice.
            entry.persist(|boxes, account| boxes.close(account));
            let Some(text) = limit::digest(&held) else {
                return;
            };
            // The label carries the id of the newest post this line accounts
            // for — the recovery notice, or the newest held post if the notice
            // did not go in — so the seat's next `last_seen` stands past
            // everything it was told of here (#47).
            let watermark = notice.or_else(|| held.last().map(|p| p.message_id.clone()));
            let at = terminal_input::at(&crate::room::now_iso());
            let typed = terminal_input::compose(
                watermark.as_deref().unwrap_or_default(),
                crate::room::APP_SPEAKER,
                terminal_input::ROLE_APP,
                at.as_deref(),
                &[],
                &text,
            );
            ptys.type_in(&self.pty_id, typed);
        });
    }

    /// The topic's record, as `limit::mailbox` reads it. Unreadable is no
    /// record, which leaves the line to what memory held.
    fn record(&self) -> Vec<limit::Recorded> {
        crate::room_log::topic_posts(&self.app, &self.topic_id)
            .unwrap_or_else(|err| {
                eprintln!("[codex-limit] the record could not be read: {err}");
                Vec::new()
            })
            .into_iter()
            .map(|post| limit::Recorded {
                at: terminal_input::at(&post.ts),
                message_id: post.message_id,
                speaker: post.speaker,
                account: post.account,
                content: post.content,
                to: post.to,
            })
            .collect()
    }

    /// The session has ended: whatever was held in memory goes with it. The
    /// mailbox stays, and the seat's next launch starts from it (`open`).
    pub fn forget(&self) {
        self.limits().forget(&self.pty_id);
    }

    /// The seat's values as the screen gets them, 制限中 included.
    pub fn stats(&self, status: &Status) -> SessionStats {
        SessionStats::from_codex(
            self.topic_id.clone(),
            self.account_id.clone(),
            status,
            self.is_limited(),
        )
    }
}

/// How long one question may take, start to answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// Ask the app-server whether the seat's account may run again.
fn ask(asker: &Asker) -> Answer {
    match ask_once(asker) {
        Ok(response) => limit::read_answer(&response),
        Err(err) => {
            eprintln!("[codex-limit] the app-server could not be asked: {err}");
            Answer::Unavailable
        }
    }
}

fn ask_once(asker: &Asker) -> Result<Value, String> {
    let command = &asker.command;
    let mut cmd = if cfg!(windows) && !command.to_ascii_lowercase().ends_with(".exe") {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", command]);
        cmd
    } else {
        Command::new(command)
    };
    cmd.arg("app-server")
        .current_dir(&asker.cwd)
        .envs(asker.env.iter().cloned())
        .env("CODEX_HOME", &asker.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let result = converse(&mut child);
    end(child);
    result
}

/// The three requests, each answer waited for by id, all inside `ASK_TIMEOUT`.
fn converse(child: &mut Child) -> Result<Value, String> {
    let deadline = Instant::now() + ASK_TIMEOUT;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        if value.get("id").is_some() && tx.send(value).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    let wait = |id: u64| -> Result<Value, String> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let value = rx
                .recv_timeout(left)
                .map_err(|_| "no answer in time".to_string())?;
            if value["id"].as_u64() == Some(id) {
                return Ok(value);
            }
        }
    };
    let [initialize, initialized, read] = limit::requests(env!("CARGO_PKG_VERSION"));
    let mut send = |line: &str| writeln!(stdin, "{line}").and_then(|_| stdin.flush());
    send(&initialize).map_err(|e| e.to_string())?;
    let init = wait(1)?;
    if init.get("error").is_some() {
        return Err("initialize was refused".into());
    }
    send(&initialized).map_err(|e| e.to_string())?;
    send(&read).map_err(|e| e.to_string())?;
    let answer = wait(limit::READ_ID)?;
    // Closing stdin is how the server is told to go.
    drop(stdin);
    Ok(answer)
}

/// Let the server go on its own for a moment, then take its tree down.
fn end(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
