//! Claude parent rejection and manual-response recovery (#342).
//! Each launch is registered before spawn, then bound to its own PTY. The
//! Claude mailbox is separate from Codex's, with one lock for every Claude seat.
use crate::room::{RoomState, SessionStats};
use mcp_config::{
    claude_limit as policy,
    codex::limit::{self, Held, Mailboxes},
};
use parking_lot::Mutex;
use std::{collections::HashMap, path::PathBuf, time::Duration};
use tauri::{AppHandle, Manager};

type Key = (String, String);
struct Entry {
    nonce: String,
    parent: String,
    root: PathBuf,
    pty: Option<String>,
    name: String,
    path: Option<PathBuf>,
    tail: Option<policy::Tail>,
    baseline: Option<(PathBuf, u64)>,
    gate: policy::Gate,
    mailbox: Option<PathBuf>,
    stats: SessionStats,
}
impl Entry {
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
    fn persist(&self, account: &str, change: impl FnOnce(&mut Mailboxes, &str)) {
        if let Some(path) = &self.mailbox {
            let mut boxes = Mailboxes::read(path);
            change(&mut boxes, account);
            if boxes.write(path).is_err() {
                eprintln!("[claude-limit] mailbox write failed");
            }
        }
    }
    fn stored(&self, account: &str) -> Vec<String> {
        self.mailbox
            .as_deref()
            .and_then(|p| Mailboxes::read(p).get(account))
            .unwrap_or_default()
    }
}
#[derive(Default)]
pub struct ClaudeLimits {
    seats: Mutex<HashMap<Key, Entry>>,
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
        floor: i64,
    ) -> String {
        let nonce = uuid::Uuid::new_v4().to_string();
        let baseline = policy::existing(&root, parent)
            .and_then(|p| Some((p.clone(), std::fs::metadata(p).ok()?.len())));
        let mailbox = crate::room_log::mailboxes_path(app, topic)
            .ok()
            .map(|p| p.with_extension("claude-mailboxes.json"));
        let mut seats = self.seats.lock();
        let persisted = !parent.is_empty()
            && mailbox
                .as_deref()
                .is_some_and(|p| Mailboxes::read(p).get(account).is_some());
        let stats = SessionStats {
            topic_id: topic.into(),
            account_id: account.into(),
            model: None,
            effort: None,
            five_hour: None,
            seven_day: None,
            context: None,
            limited: if parent.is_empty() {
                None
            } else {
                Some(persisted)
            },
            limited_source: if parent.is_empty() {
                None
            } else {
                Some("claude-parent".into())
            },
            five_hour_resets_at: None,
            seven_day_resets_at: None,
        };
        let (path, tail) = match &baseline {
            Some((p, offset)) => (Some(p.clone()), Some(policy::Tail::new(p, *offset))),
            None => (None, None),
        };
        seats.insert(
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
                gate: policy::Gate::new(floor, persisted),
                mailbox,
                stats,
            },
        );
        nonce
    }
    pub fn forget(&self, topic: &str, account: &str, nonce: &str) {
        let mut seats = self.seats.lock();
        let key = (topic.into(), account.into());
        if seats.get(&key).is_some_and(|e| e.nonce == nonce) {
            seats.remove(&key);
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
            stats.limited = if e.parent.is_empty() {
                None
            } else {
                Some(e.gate.is_limited())
            };
            stats.limited_source = if e.parent.is_empty() {
                None
            } else {
                Some("claude-parent".into())
            };
            e.stats = stats;
        }
        if e.pty.is_some() {
            e.stats.clone().emit(app);
        }
        true
    }
    /// Replayed by the watcher so a reloaded webview can adopt the same gate.
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
        stats.limited = if e.parent.is_empty() {
            None
        } else {
            Some(e.gate.is_limited())
        };
        stats.emit(app);
        true
    }
    pub fn hold(&self, pty: &str, post: impl FnOnce() -> Held) -> bool {
        let mut seats = self.seats.lock();
        let Some(((_, account), e)) = seats
            .iter_mut()
            .find(|(_, e)| e.pty.as_deref() == Some(pty))
        else {
            return false;
        };
        let Some(id) = e.gate.hold(post) else {
            return false;
        };
        e.persist(account, |boxes, account| boxes.push(account, &id));
        true
    }
    pub fn bind(&self, app: &AppHandle, topic: &str, account: &str, nonce: &str, pty: &str) {
        let mut seats = self.seats.lock();
        if let Some(e) = seats
            .get_mut(&(topic.into(), account.into()))
            .filter(|e| e.nonce == nonce)
        {
            e.pty = Some(pty.into());
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
            if !app
                .state::<ClaudeLimits>()
                .emit_current(&app, &topic, &account, &nonce, &pty)
            {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        app.state::<ClaudeLimits>().forget(&topic, &account, &nonce);
    });
}

fn round(app: &AppHandle, topic: &str, account: &str, nonce: &str, pty: &str) {
    let limits = app.state::<ClaudeLimits>();
    let key = (topic.into(), account.into());
    // Finish the entire appended batch before deciding to release: a new
    // rejection after a successful response in this same batch still holds.
    let change = {
        let mut seats = limits.seats.lock();
        let Some(e) = seats.get_mut(&key).filter(|e| {
            e.nonce == nonce && e.pty.as_deref() == Some(pty) && e.current(app, topic, account)
        }) else {
            return;
        };
        let was = e.gate.is_limited();
        if let (Some(path), Some(tail)) = (&e.path, &mut e.tail) {
            // Revalidate canonical containment on every read (file replacement).
            if policy::checked(&e.root, &e.parent, path).as_ref() != Some(path) {
                return;
            }
            if let Ok(events) = tail.read(path, &e.parent) {
                for event in events {
                    e.gate.state.apply(event);
                }
            }
        }
        if !e.gate.state.limited && was && !e.tail.as_ref().is_some_and(|tail| tail.caught_up()) {
            e.gate.keep_holding();
            return;
        }
        if was == e.gate.state.limited {
            return;
        }
        let stopped = e.gate.state.limited;
        if stopped {
            e.persist(account, |boxes, id| boxes.open(id));
        }
        // Until the digest is enqueued, posts continue to be held.
        if !stopped {
            e.gate.keep_holding();
        }
        Some((stopped, e.name.clone()))
    };
    let Some((stopped, name)) = change else {
        return;
    };
    let room = app.state::<RoomState>();
    if stopped {
        crate::room::post_app_notice(app, &room, topic,
            &format!("{name} は Claude 親の利用上限で停止しました。部屋の投稿を保留します。端末で手動再開し、正常な親応答が確認された後にまとめを渡します。"), Some(pty));
    } else {
        let record: Vec<_> = crate::room_log::topic_posts(app, topic)
            .unwrap_or_default()
            .into_iter()
            .map(|p| limit::Recorded {
                at: terminal_input::at(&p.ts),
                message_id: p.message_id,
                speaker: p.speaker,
                account: p.account,
                content: p.content,
                to: p.to,
            })
            .collect();
        let seat = limit::Seat {
            name: &name,
            account_id: account,
        };
        let count = {
            let seats = limits.seats.lock();
            let Some(e) = seats.get(&key).filter(|e| e.nonce == nonce) else {
                return;
            };
            limit::mailbox(&record, &seat, &e.stored(account), e.gate.held()).len()
        };
        let notice = crate::room::post_app_notice(
            app,
            &room,
            topic,
            &limit::recovery_notice(&name, count),
            Some(pty),
        );
        let mut seats = limits.seats.lock();
        let Some(e) = seats.get_mut(&key).filter(|e| {
            e.nonce == nonce && e.pty.as_deref() == Some(pty) && e.current(app, topic, account)
        }) else {
            return;
        };
        let held = limit::mailbox(&record, &seat, &e.stored(account), e.gate.release());
        e.persist(account, |boxes, id| boxes.close(id));
        if let Some(text) = limit::digest(&held) {
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
            if !app.state::<crate::pty::PtyState>().type_in(pty, typed) {
                // No input was enqueued: retain the mailbox for the next launch.
                e.gate.keep_holding();
                e.persist(account, |boxes, id| {
                    boxes.open(id);
                    for post in &held {
                        boxes.push(id, &post.message_id);
                    }
                });
                for post in held {
                    e.gate.hold(|| post);
                }
            }
        }
    }
    let mut seats = limits.seats.lock();
    if let Some(e) = seats.get_mut(&key).filter(|e| e.nonce == nonce) {
        e.stats.limited = Some(e.gate.is_limited());
        e.stats.clone().emit(app);
    }
}
