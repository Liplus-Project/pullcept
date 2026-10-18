//! Claude parent rejection and response recovery (#342), with the resume
//! prompt typed at the known reset (#357, simplified by #401).
//! Each launch is registered before spawn, then bound to its own PTY. Room
//! posts are not held for a limited Claude seat (#401): they reach its
//! terminal as for any seat. One lock covers every Claude seat.
use crate::room::{RoomState, SessionStats};
use mcp_config::{claude_limit as policy, claude_notice as notice, claude_resume as resume};
use parking_lot::Mutex;
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tauri::{AppHandle, Manager};

type Key = (String, String);
/// What the watcher's round does about the resume prompt.
enum Prompt {
    Typed(resume::Action),
    /// The account's switch is off (#475).
    Skipped,
}
use Prompt::{Skipped, Typed};
struct Entry {
    nonce: String,
    parent: String,
    root: PathBuf,
    pty: Option<String>,
    name: String,
    path: Option<PathBuf>,
    tail: Option<policy::Tail>,
    baseline: Option<(PathBuf, u64)>,
    state: policy::State,
    notices: mcp_config::claude_notice::Notices,
    pending_notices: Vec<(mcp_config::claude_notice::Stop, bool)>,
    /// The resume prompt at the parent's reset (#357, #401).
    resume: resume::Resume,
    floor: i64,
    stats: SessionStats,
    /// Where this seat's login is kept, and its last reading keyed on the
    /// file's modified time, so a status line does not reread it (#495).
    login_file: Option<PathBuf>,
    login: Option<(std::time::SystemTime, Option<(String, String)>)>,
}
impl Entry {
    fn read_login(&mut self) -> Option<String> {
        let file = self.login_file.as_ref()?;
        let modified = std::fs::metadata(file).and_then(|m| m.modified()).ok()?;
        if self.login.as_ref().is_none_or(|(at, _)| *at != modified) {
            let read = std::fs::read_to_string(file).ok().and_then(|text| policy::login(&text));
            self.login = Some((modified, read));
        }
        self.login.as_ref()?.1.as_ref().map(|(_, name)| name.clone())
    }
    fn remember_notice(&mut self, stop: mcp_config::claude_notice::Stop) {
        let before = self.notices.known_reset(&stop.subject);
        if self.notices.stop(&stop) {
            self.pending_notices.push((stop, false));
        } else if stop.reset.is_some() && before != stop.reset {
            if let Some((pending, _)) = self
                .pending_notices
                .iter_mut()
                .find(|(n, _)| n.subject == stop.subject)
            {
                pending.reset = stop.reset;
            } else {
                self.pending_notices.push((stop, true));
            }
        }
    }
    fn current(&self, app: &AppHandle, topic: &str, account: &str) -> bool {
        let Some(pty) = &self.pty else {
            return true;
        }; // Own configured Starting launch.
        policy::running_matches(
            &self.parent,
            app.state::<crate::session::RoomSeats>()
                .native_id_of(topic, account, pty),
            app.state::<crate::pty::PtyState>().is_running(pty),
        )
    }
    fn limited(&self) -> Option<bool> {
        (!self.parent.is_empty()).then_some(self.state.limited)
    }
}
#[derive(Default)]
pub struct ClaudeLimits {
    seats: Mutex<HashMap<Key, Entry>>,
    /// Seats told of another cause, across their launches (#401).
    stalled: Mutex<resume::Stalled>,
}
impl ClaudeLimits {
    /// Returns the nonce carried only by this Claude launch and its reporters.
    pub fn configure(
        &self,
        app: &AppHandle,
        topic: &str,
        account: &str,
        parent: &str,
        name: &str,
        root: PathBuf,
        login_file: Option<PathBuf>,
        floor: i64,
    ) -> String {
        let nonce = uuid::Uuid::new_v4().to_string();
        let baseline = policy::existing(&root, parent)
            .and_then(|p| Some((p.clone(), std::fs::metadata(p).ok()?.len())));
        // The pending resume of a seat that had no terminal at its reset
        // (#366, #401): its parent's last event before this launch is an
        // unanswered rejection. Re-read from this parent's own transcript,
        // never from another seat's. The rejection lies before the baseline,
        // so the tail never applies it a second time. Read before the lock.
        let last = baseline
            .as_ref()
            .and_then(|(p, end)| policy::last_rejection(p, parent, *end));
        let limited = !parent.is_empty() && last.is_some();
        let resume = match last.filter(|_| limited) {
            Some((_, reset)) => {
                let restored = resume::Resume::restored(reset, unix_now());
                let detail = match restored.next() {
                    Some(next) => format!("restored=launch type_at={}", clock_or_unknown(Some(next))),
                    None => "restored=launch reset=unknown not_typed".into(),
                };
                log(app, "pending", topic, account, &detail);
                restored
            }
            None => Default::default(),
        };
        let stats = SessionStats {
            model_reported: false,
            topic_id: topic.into(),
            account_id: account.into(),
            model: None,
            effort: None,
            five_hour: None,
            seven_day: None,
            context: None,
            limited: (!parent.is_empty()).then_some(limited),
            limited_source: (!parent.is_empty()).then(|| "claude-parent".into()),
            pty_id: None,
            five_hour_resets_at: None,
            seven_day_resets_at: None,
            login: None,
        };
        let (path, tail) = match &baseline {
            Some((p, offset)) => (Some(p.clone()), Some(policy::Tail::new(p, *offset))),
            None => (None, None),
        };
        self.seats.lock().insert(
            (topic.into(), account.into()),
            Entry {
                nonce: nonce.clone(),
                parent: parent.into(),
                root,
                name: name.into(),
                pty: None,
                path,
                tail,
                baseline,
                state: policy::State::new(floor, limited),
                notices: Default::default(),
                pending_notices: Vec::new(),
                resume,
                floor,
                stats,
                login_file,
                login: None,
            },
        );
        nonce
    }
    /// The launch is gone. A resume still to be typed stays pending in the
    /// parent's transcript, for the seat's next launch (#401).
    pub fn forget(&self, app: &AppHandle, topic: &str, account: &str, nonce: &str) {
        let mut seats = self.seats.lock();
        let key = (topic.into(), account.into());
        if seats.get(&key).is_some_and(|e| e.nonce == nonce) {
            if let Some(e) = seats.remove(&key) {
                if e.state.limited && e.resume.pending() {
                    log(app, "pending", topic, account, "terminal_left=launch typed_at_next_launch");
                }
            }
        }
    }
    /// Reporter arrival binds a parent transcript, never stop/recovery itself.
    /// An old same-parent callback is rejected by the nonce before file access.
    pub fn report(
        &self,
        app: &AppHandle,
        topic: &str,
        account: &str,
        nonce: Option<&str>,
        body: &[u8],
        stats: Option<SessionStats>,
    ) -> bool {
        let mut seats = self.seats.lock();
        let Some(e) = seats.get_mut(&(topic.into(), account.into())) else {
            return false;
        };
        if !policy::reporter_matches(&e.parent, &e.nonce, nonce, body) {
            return false;
        }
        if !e.current(app, topic, account) {
            return false;
        }
        if let Some(path) =
            policy::report_path(&e.root, &e.parent, &e.nonce, nonce, body, e.path.as_deref())
        {
            if e.path.is_none() {
                let offset = e
                    .baseline
                    .as_ref()
                    .filter(|(p, _)| p == &path)
                    .map(|(_, n)| *n)
                    .unwrap_or(0);
                e.tail = Some(policy::Tail::new(&path, offset));
                e.path = Some(path);
            }
        }
        if let Some(mut stats) = stats {
            stats.limited = e.limited();
            stats.limited_source = (!e.parent.is_empty()).then(|| "claude-parent".into());
            stats.pty_id = e.pty.clone();
            stats.login = e.read_login();
            e.stats = stats;
        }
        if e.pty.is_some() {
            e.stats.clone().emit(app);
        }
        true
    }
    /// Replayed by the watcher so a reloaded webview can adopt the same state.
    fn emit_current(
        &self,
        app: &AppHandle,
        topic: &str,
        account: &str,
        nonce: &str,
        pty: &str,
    ) -> bool {
        let seats = self.seats.lock();
        let Some(e) = seats.get(&(topic.into(), account.into())).filter(|e| {
            e.nonce == nonce && e.pty.as_deref() == Some(pty) && e.current(app, topic, account)
        }) else {
            return false;
        };
        let mut stats = e.stats.clone();
        stats.limited = e.limited();
        stats.emit(app);
        true
    }
    /// Authenticated StopFailure intake is notification-only. Parent hooks
    /// bind the path but never announce: the structured watcher owns that notice.
    pub fn notification(
        &self,
        app: &AppHandle,
        topic: &str,
        account: &str,
        nonce: Option<&str>,
        body: &[u8],
    ) -> bool {
        if !self.report(app, topic, account, nonce, body, None) {
            return false;
        }
        let mut seats = self.seats.lock();
        let Some(e) = seats
            .get_mut(&(topic.into(), account.into()))
            .filter(|e| nonce == Some(e.nonce.as_str()) && e.current(app, topic, account))
        else {
            return false;
        };
        if let Some(mut stop) = mcp_config::claude_notice::child(body) {
            mcp_config::claude_notice::enrich(&e.root, &e.parent, &mut stop, body, e.floor);
            e.remember_notice(stop);
        }
        true
    }
    /// The watcher's notices, and the resume prompt typed into this launch's
    /// own terminal — the one occupying the seat, since the watcher runs only
    /// while it does (#401). No other launch's identity is compared.
    fn flush_notices(&self, app: &AppHandle, topic: &str, account: &str, nonce: &str, pty: &str) {
        // The account's switch for the resume prompt (#475), read from the
        // config only when a parent notice or a prompt is due, and outside
        // the seats' lock.
        let now = unix_now();
        let wanted = {
            let seats = self.seats.lock();
            seats.get(&(topic.into(), account.into())).is_some_and(|e| {
                (e.state.limited && e.resume.due(now))
                    || e.pending_notices.iter().any(|(n, _)| n.subject == "parent")
            })
        };
        let enabled = !wanted || crate::config::resume_notice_enabled(app, account);
        let (messages, action, name) = {
            let mut seats = self.seats.lock();
            let Some(e) = seats.get_mut(&(topic.into(), account.into())).filter(|e| {
                e.nonce == nonce && e.pty.as_deref() == Some(pty) && e.current(app, topic, account)
            }) else {
                return;
            };
            let mut messages: Vec<_> = std::mem::take(&mut e.pending_notices)
                .into_iter()
                .map(|(stop, update)| {
                    let clock = stop.reset.and_then(terminal_input::clock);
                    if update {
                        mcp_config::claude_notice::reset_known(
                            &e.name,
                            &stop,
                            clock.as_deref().unwrap_or("不明"),
                            enabled,
                        )
                    } else {
                        mcp_config::claude_notice::stopped(&e.name, &stop, clock.as_deref(), enabled)
                    }
                })
                .collect();
            // The parent's reset is the resume prompt's to act on (#357).
            messages.extend(
                e.notices
                    .due(now)
                    .into_iter()
                    .filter(|subject| subject != "parent")
                    .map(|subject| mcp_config::claude_notice::reached(&e.name, &subject)),
            );
            let action = if !e.state.limited {
                None
            } else if !enabled && e.resume.due(now) {
                // Switched off (#475): dropped, not typed; nothing is said to
                // the room and no answer is awaited.
                e.resume.skip();
                Some(Skipped)
            } else {
                e.resume.tick(now).map(Typed)
            };
            (messages, action, e.name.clone())
        };
        let room = app.state::<RoomState>();
        for message in messages {
            crate::room::post_app_notice(app, &room, topic, &message, Some(pty));
        }
        match action {
            Some(Skipped) => {
                log(app, "skipped", topic, account, "resume_notice=off");
            }
            Some(Typed(resume::Action::Type { retry })) => {
                // Said to the room first, its id the label's watermark; not
                // typed into this seat, which gets the prompt instead.
                let notice = crate::room::post_app_notice(
                    app,
                    &room,
                    topic,
                    &notice::typed(&name, retry),
                    Some(pty),
                );
                let at = terminal_input::at(&crate::room::now_iso());
                let text = terminal_input::compose(
                    notice.as_deref().unwrap_or_default(),
                    crate::room::APP_SPEAKER,
                    terminal_input::ROLE_APP,
                    at.as_deref(),
                    &[],
                    resume::PROMPT,
                );
                // A seat waiting on a permission prompt keeps it until the
                // wait ends (#346). Bound to this launch: not carried to the
                // next, which reads its own pending resume back (#366).
                let item = mcp_config::prompt_hold::Item {
                    message_id: notice.unwrap_or_default(),
                    text,
                    launch_bound: true,
                };
                let typed = crate::hook_activity::type_or_keep(app, pty, item);
                log(
                    app,
                    if typed { "typed" } else { "refused" },
                    topic,
                    account,
                    &format!("retry={retry} pty={pty}"),
                );
            }
            Some(Typed(resume::Action::OtherCause)) => {
                crate::room::post_app_notice(app, &room, topic, &notice::other_cause(&name), Some(pty));
                self.stalled.lock().told(topic, account);
                log(app, "other-cause", topic, account, &format!("no_answer_within={}s", resume::ANSWER_WAIT));
            }
            None => {}
        }
    }
    pub fn bind(&self, app: &AppHandle, topic: &str, account: &str, nonce: &str, pty: &str) {
        let mut seats = self.seats.lock();
        if let Some(e) = seats
            .get_mut(&(topic.into(), account.into()))
            .filter(|e| e.nonce == nonce)
        {
            e.pty = Some(pty.into());
            e.stats.pty_id = Some(pty.into());
            e.stats.clone().emit(app);
        }
    }
}

pub fn watch(app: AppHandle, topic: String, account: String, nonce: String, pty: String) {
    std::thread::spawn(move || {
        loop {
            if app
                .state::<crate::session::RoomSeats>()
                .running_pty(&topic, &account)
                .as_deref()
                != Some(&pty)
                || !app.state::<crate::pty::PtyState>().is_running(&pty)
            {
                break;
            }
            round(&app, &topic, &account, &nonce, &pty);
            app.state::<ClaudeLimits>()
                .flush_notices(&app, &topic, &account, &nonce, &pty);
            if !app
                .state::<ClaudeLimits>()
                .emit_current(&app, &topic, &account, &nonce, &pty)
            {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        app.state::<ClaudeLimits>().forget(&app, &topic, &account, &nonce);
    });
}

/// Read what the parent appended, and act on its stop or recovery. A seat
/// the room was told is stopped on another cause is announced back once its
/// parent answers normally (#401).
fn round(app: &AppHandle, topic: &str, account: &str, nonce: &str, pty: &str) {
    let limits = app.state::<ClaudeLimits>();
    let Some(name) = read(app, &limits, topic, account, nonce, pty) else {
        return;
    };
    if limits.stalled.lock().answered(topic, account) {
        let room = app.state::<RoomState>();
        crate::room::post_app_notice(app, &room, topic, &notice::back_after_other_cause(&name), Some(pty));
        log(app, "recovered", topic, account, "told=back_after_other_cause");
    }
}

/// The seat's name when this read applied a normal parent response and left
/// the parent free.
fn read(app: &AppHandle, limits: &ClaudeLimits, topic: &str, account: &str, nonce: &str, pty: &str) -> Option<String> {
    let mut seats = limits.seats.lock();
    let e = seats.get_mut(&(topic.into(), account.into())).filter(|e| {
        e.nonce == nonce && e.pty.as_deref() == Some(pty) && e.current(app, topic, account)
    })?;
    let was = e.state.limited;
    let mut answered = false;
    if let (Some(path), Some(tail)) = (&e.path, &mut e.tail) {
        // Revalidate canonical containment on every read (file replacement).
        if policy::checked(&e.root, &e.parent, path).as_ref() != Some(path) {
            return None;
        }
        if let Ok(events) = tail.read(path, &e.parent) {
            let now = unix_now();
            for event in events {
                let rejected = match &event {
                    policy::Event::Rejected { reset, at, .. } => Some((*reset, *at)),
                    _ => None,
                };
                if !e.state.apply(event) {
                    continue;
                }
                let Some((reset, at)) = rejected else {
                    answered = true;
                    continue;
                };
                let stop = mcp_config::claude_notice::Stop {
                    subject: "parent".into(),
                    reset,
                    episode: None,
                };
                // A rejection after the typed prompt answers it (#357): still
                // limited, typed again later. The room hears of it only when
                // the next prompt is typed (#401).
                if let Some(still) = e.resume.rejected(at, reset, now) {
                    e.notices.stop(&stop);
                    log(
                        app,
                        "still-limited",
                        topic,
                        account,
                        &format!("next={}", clock_or_unknown(Some(still.next))),
                    );
                } else {
                    e.remember_notice(stop);
                }
            }
        }
    }
    if !e.state.limited {
        if was {
            let detail = if e.resume.pending() { "after=prompt" } else { "after=other_input" };
            log(app, "recovered", topic, account, detail);
        }
        e.notices.recovered_parent();
        e.pending_notices.retain(|(n, _)| n.subject != "parent");
        e.resume.recovered();
    }
    (answered && !e.state.limited).then(|| e.name.clone())
}

fn clock_or_unknown(t: Option<i64>) -> String {
    t.and_then(terminal_input::clock).unwrap_or_else(|| "unknown".into())
}

/// One line of `logs/claude-resume.log` (#401), written whole in one call. A
/// failure to open or write is dropped, as the probe's is
/// (`room_log::open_probe`): the log observes.
fn log(app: &AppHandle, action: &str, topic: &str, account: &str, detail: &str) {
    use std::io::Write;
    if let Some(mut file) = crate::room_log::open_probe(app, resume::LOG_FILE) {
        let line = resume::log_line(action, topic, account, &crate::room::now_iso(), detail);
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
