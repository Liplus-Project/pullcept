//! A Codex CLI seat stopped on its usage limit (#294, #403). See
//! docs/3-accounts.md, "Codex の席が利用上限で止まったとき".
//!
//! What is read and decided is `mcp_config::codex::limit`, free of tauri so a
//! test can run it. What stays here is what needs the app: the table of
//! Codex seats a watcher is following (`CodexLimits`), the question put to
//! Codex's app-server, the notices, and the resume typed into the seat.
//!
//! **Driven from the rollout watcher** (`codex_status`), one per Codex launch,
//! on its one-second round. A turn the rollout says ended on the limit stops
//! the seat and the room is told; the watcher asks the app-server when a
//! question is due. Nothing is held back from a stopped seat (#403): the room
//! types into it as into any seat (`room::type_into_sessions`), and what is
//! typed stays in its conversation as a turn the limit refused. Claude Code
//! seats never enter the table and are never asked about.
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
//! **A stopped seat's mark outlives the session and the app** (#312): it is
//! written to a small file beside its topic's record (`limit::Mailboxes`) at
//! the stop. A seat launched while its mark is there starts stopped and asks
//! at once (`Limiter::open`), so the resume is typed by whichever launch holds
//! the seat when the recovery is confirmed. The mark goes as the resume is
//! typed.
//!
//! **After the resume is typed** (#403, as #401 for a Claude seat): a turn
//! that starts or ends answers it. A turn that ends on the limit is a new
//! stop, and the next confirmed recovery types it again. Neither within
//! `limit::ANSWER_WAIT`: the room is told once that the seat seems stopped on
//! something else, and nothing is typed again; a turn the seat completes
//! later is told back once.
//!
//! **An account can switch the resume off** (#475, `Account::resume_notice`):
//! the recovery is still confirmed, told and freed, and nothing is typed.
//!
//! **Every answer the app-server gives refreshes the seat's usage** (#496):
//! a limited seat's turns do not move, so its rollout carries no new rate
//! limits. The two windows an answer reports (`limit::read_usage`) stand over
//! the rollout's on the screen (`Limiter::stats`) and are recorded for
//! `my_usage`, until the rollout carries rate limits again. An answer with
//! none (`Unavailable` included) changes nothing. No question is added for
//! this: only the stop's and the due ones carry it.

use crate::room::{RoomState, SessionStats};
use mcp_config::codex::limit::{self, Answer, Limit, Mailboxes, TurnEnd, Windows};
use mcp_config::codex::status::Status;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
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
    /// The seats (topic, account) the room was told seem stopped on another
    /// cause, kept across their launches until a turn completes.
    stalled: Mutex<HashSet<(String, String)>>,
}

/// One seat in the table: its limit, and where its mark is kept.
struct Entry {
    limit: Limit,
    account_id: String,
    /// The topic's mark file, or `None` when it could not be placed: the stop
    /// is then known in memory only, as before #312.
    mailboxes: Option<PathBuf>,
    /// The windows the app-server last reported (#496), newer than the
    /// rollout's until the rollout carries rate limits again.
    answered: Option<Windows>,
}

impl Entry {
    /// Change the seat's mark in its file. Every change goes through the
    /// table's lock, so two seats of one topic do not write over each other.
    /// A failure is said and goes no further.
    fn persist(&self, change: impl FnOnce(&mut Mailboxes, &str)) {
        let Some(path) = self.mailboxes.as_deref() else {
            return;
        };
        let mut boxes = Mailboxes::read(path);
        change(&mut boxes, &self.account_id);
        if let Err(err) = boxes.write(path) {
            eprintln!("[codex-limit] the mark could not be written: {err}");
        }
    }
}

impl CodexLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// The seat on `pty_id` has left the table. Its mark stays.
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
            .map_err(|err| eprintln!("[codex-limit] the mark has no place: {err}"))
            .ok()
    }

    fn entry(&self, limit: Limit) -> Entry {
        Entry {
            limit,
            account_id: self.account_id.clone(),
            mailboxes: self.mailboxes(),
            answered: None,
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

    /// Put the seat in the table (#312). A mark left from an earlier launch —
    /// the seat was still stopped when its session or the app ended — starts
    /// it stopped, its first question due at once (`Limit::resumed`). Nothing
    /// is said to the room: the stop was told when it happened.
    pub fn open(&self) {
        let entry = self.entry(Limit::default());
        let left = entry
            .mailboxes
            .as_deref()
            .is_some_and(|path| Mailboxes::read(path).has(&self.account_id));
        let entry = Entry {
            limit: if left { Limit::resumed(now()) } else { Limit::default() },
            ..entry
        };
        self.limits().seats.lock().insert(self.pty_id.clone(), entry);
    }

    pub fn is_limited(&self) -> bool {
        self.with(|entry| entry.limit.is_limited())
    }

    /// One round: whether a turn started and the turn end the rollout last
    /// reported, if any, then a due question, then the resume's answer wait.
    /// `rollout_reset` is the reset the rollout last reported. Whether 制限中
    /// turned over, for the caller to send to the screen.
    pub fn round(&self, started: bool, end: Option<TurnEnd>, rollout_reset: Option<i64>) -> bool {
        let was = self.is_limited();
        if started {
            self.with(|entry| entry.limit.answered());
        }
        match end {
            Some(TurnEnd::UsageLimit) => {
                // The mark from the stop on, so a seat stopped when the app
                // goes down starts stopped.
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
                self.with(|entry| {
                    entry.limit.answered();
                    entry.limit.turn_completed(now());
                });
                self.announce_back();
            }
            Some(TurnEnd::Failed) => {
                self.with(|entry| entry.limit.answered());
            }
            None => {}
        }
        if self.with(|entry| entry.limit.due(now())) {
            let answer = self.ask();
            let recovered = self.with(|entry| entry.limit.answer(now(), answer));
            if recovered {
                self.recover();
            }
        }
        if self.with(|entry| entry.limit.unanswered(now())) {
            self.announce_other_cause();
        }
        self.is_limited() != was
    }

    /// The seat has just stopped. One question for the reset the room is
    /// told, then the notice.
    fn announce_stop(&self) {
        let answer = self.ask();
        let reset = self.with(|entry| {
            entry.limit.refine(now(), answer);
            entry.limit.resets_at()
        });
        let clock = reset.and_then(terminal_input::clock);
        self.notice(&limit::stop_notice(&self.name, clock.as_deref()));
    }

    /// The resume drew no answer: told once, and the seat is remembered so a
    /// turn it completes later is told back (`announce_back`).
    fn announce_other_cause(&self) {
        self.limits()
            .stalled
            .lock()
            .insert((self.topic_id.clone(), self.account_id.clone()));
        self.notice(&limit::other_cause(&self.name));
    }

    /// A turn completed: if the room was told the seat seemed stopped on
    /// another cause, it is told once that the seat is back.
    fn announce_back(&self) {
        let told = self
            .limits()
            .stalled
            .lock()
            .remove(&(self.topic_id.clone(), self.account_id.clone()));
        if told {
            self.notice(&limit::back_after_other_cause(&self.name));
        }
    }

    /// Say `content` to the room, not typed into this seat. The id it was
    /// filed under, if it went in.
    fn notice(&self, content: &str) -> Option<String> {
        let room = self.app.state::<RoomState>();
        crate::room::post_app_notice(&self.app, &room, &self.topic_id, content, Some(&self.pty_id))
    }

    /// The app-server said the seat may run again. The room is told first, so
    /// the notice is not typed into the seat; then, under one lock, the seat is
    /// freed, its mark goes, and the resume (`limit::CONTINUE`, #389) is typed
    /// once into its terminal, so a post arriving after is typed after it.
    ///
    /// Typed by the launch that holds the seat now. A launch that no longer
    /// does types nothing and leaves the mark: the launch after it started
    /// stopped from that mark, asks for itself, and types its own resume.
    ///
    /// The account's switch (#475), read here as the recovery is confirmed:
    /// off, the seat is freed and its mark goes all the same, the room hears
    /// the recovery, and nothing is typed, so no answer is awaited.
    fn recover(&self) {
        if !self.current() {
            return;
        }
        let resume = crate::config::resume_notice_enabled(&self.app, &self.account_id);
        let notice = self.notice(&limit::recovery_notice(&self.name, resume));
        self.with(|entry| {
            // The app-server query may have outlived this launch.
            if !self.current() {
                return;
            }
            entry.limit.free();
            // The mark goes before the resume is typed: cut short between the
            // two, the seat misses its resume rather than being typed it twice.
            entry.persist(|boxes, account| boxes.close(account));
            if !resume {
                return;
            }
            // The label carries the recovery notice's id, so the seat's next
            // `last_seen` stands past it (#47).
            let watermark = notice.unwrap_or_default();
            let at = terminal_input::at(&crate::room::now_iso());
            let text = terminal_input::compose(
                &watermark,
                crate::room::APP_SPEAKER,
                terminal_input::ROLE_APP,
                at.as_deref(),
                &[],
                limit::CONTINUE,
            );
            // Bound to this launch: not carried to the next (#389).
            let typed = crate::hook_activity::type_or_keep(
                &self.app,
                &self.pty_id,
                mcp_config::prompt_hold::Item {
                    message_id: watermark,
                    text,
                    launch_bound: true,
                },
            );
            if typed {
                entry.limit.resume_typed(now());
            }
        });
    }

    fn current(&self) -> bool {
        self.app
            .state::<crate::session::RoomSeats>()
            .running_pty(&self.topic_id, &self.account_id)
            .as_deref()
            == Some(&self.pty_id)
            && self.app.state::<crate::pty::PtyState>().is_running(&self.pty_id)
    }

    /// The session has ended. The mark stays, and the seat's next launch
    /// starts from it (`open`).
    pub fn forget(&self) {
        self.limits().forget(&self.pty_id);
    }

    /// The seat's values as the screen gets them, 制限中 included. The
    /// windows the app-server last reported, when newer than the rollout's
    /// (#496), stand in for the rollout's two windows.
    pub fn stats(&self, status: &Status) -> SessionStats {
        let answered = self.with(|entry| entry.answered);
        let mut stats = SessionStats::from_codex(
            self.topic_id.clone(),
            self.account_id.clone(),
            status,
            self.is_limited(),
        );
        if let Some(w) = answered {
            stats.five_hour = w.five_hour;
            stats.five_hour_resets_at = w.five_hour_resets_at;
            stats.seven_day = w.seven_day;
            stats.seven_day_resets_at = w.seven_day_resets_at;
        }
        stats.pty_id = Some(self.pty_id.clone());
        stats
    }

    /// The rollout has just carried the seat's rate limits (#364): they are
    /// now the newest, so the app-server's stand down (#496), and the two
    /// windows are recorded as received now, for the seat to read back
    /// (`my_usage`).
    pub fn received_usage(&self, status: &Status) {
        self.with(|entry| entry.answered = None);
        self.app.state::<crate::room::UsageBook>().record(
            &self.topic_id,
            &self.account_id,
            self.stats(status).usage(),
        );
    }

    /// Ask the app-server about the seat's account. The windows its answer
    /// reports, if any, become the seat's usage (#496): shown over the
    /// rollout's and recorded as received now. An answer without them leaves
    /// the figures held as they were.
    fn ask(&self) -> Answer {
        let (answer, windows) = ask(&self.asker);
        if let Some(windows) = windows {
            self.with(|entry| entry.answered = Some(windows));
            self.app.state::<crate::room::UsageBook>().record(
                &self.topic_id,
                &self.account_id,
                self.stats(&Status::default()).usage(),
            );
        }
        answer
    }
}

/// How long one question may take, start to answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// Ask the app-server whether the seat's account may run again, and with
/// what usage (#496).
fn ask(asker: &Asker) -> (Answer, Option<Windows>) {
    match ask_once(asker) {
        Ok(response) => (limit::read_answer(&response), limit::read_usage(&response)),
        Err(err) => {
            eprintln!("[codex-limit] the app-server could not be asked: {err}");
            (Answer::Unavailable, None)
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
