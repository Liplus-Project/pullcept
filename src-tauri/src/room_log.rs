//! The room's log, kept one file per topic.
//!
//! What was said in the room, so an exchange can be read back after the window
//! that showed it has closed. The room itself keeps nothing: its lines live in
//! the DOM, and closing the app took them (#48).
//!
//! **A topic is the unit.** The log used to be one file per room and one flow
//! of posts, which is the shape #48 left behind and #115 replaces: a topic is
//! not a section of a transcript, it is the vessel a conversation happens in.
//! Opening one puts it back in the room and the talk continues in it. The
//! boundary between two topics is drawn by hand — 新規 — and has nothing to do
//! with when the app was started: one run may hold several topics, and one
//! topic may span several runs.
//!
//! The layout that follows from that:
//!
//! ```text
//! logs/{room}/index.json      one entry per topic: name, when it was made,
//!                             and the session each account was in
//! logs/{room}/{topic}.jsonl   one topic's posts, append-only
//! ```
//!
//! One file per topic rather than one file with a `topic_id` column, for two
//! reasons that both have to hold. Reading one topic reads one file, where a
//! column would make every read a read of everything ever said (#114). And a
//! topic has something to carry that a post does not: the session each account
//! was in while it was open. A per-post column has nowhere to put that; the
//! index does.
//!
//! **The directory is what exists; the index annotates it.** A topic's posts
//! are the file, and `read_index` adopts any `.jsonl` it finds without an
//! entry. That is what makes lazy creation safe: a launch mints a topic and
//! writes nothing, so a run where nothing was said leaves no row in the list
//! (#115, 決定5から導かれること) — and the one window that would otherwise
//! open, a crash between the post reaching the file and the entry reaching the
//! index, closes because the file alone is enough to rebuild the entry.
//!
//! **The store itself is the `topic-index` crate; this file is the app's door
//! to it.** What is here needs tauri — the path to the room's directory comes
//! out of `AppHandle`, the lock over read-modify-write of the index is held
//! here, and the screen is told from here that the list changed. What is not
//! here is everything that only ever needed the directory: the shapes, the
//! reconciliation, the title, the parse, and the delete. That split is the one
//! docs/0-requirements.md named under テストの配置 before it existed, and #119
//! is what made it load-bearing rather than tidy — deleting a topic has to take
//! the file, because an index with the entry taken out and the file left is a
//! topic the very next read adopts back.
//!
//! `logs/{room}.jsonl` from before this — the single flow — is carried in as
//! one topic the first time the index is built. It is moved, not copied and not
//! dropped (#115, decision 8), and the scan above is what gives it its entry.
//!
//! One line is one post, and it carries the five fields a post is:
//! `message_id` / `speaker` / `content` / `to` / `ts`.
//!
//! `hue` is not among them. It is a declaration the speaker made at the moment
//! of joining, and the seat that held it is gone by the time this file is read
//! back; what a later reader would draw is a colour nobody is declaring any
//! more. `own` is not among them either, and could not be: the type it comes
//! from says so itself — it is a property of whoever is looking, not of whoever
//! spoke (`room.rs`) — so a file that recorded it would be recording one
//! viewer's position as if it were part of the utterance.
//!
//! The room's name occupies a position in the path and is fixed at `main`:
//! several rooms are not implemented, and the path is shaped so that
//! implementing them adds a value here rather than a directory level (#48).
//!
//! No rotation and no ceiling, per topic or across them. A log that drops its
//! own oldest entries answers the question this exists for — what was actually
//! said — with silence at exactly the distance that makes the question worth
//! asking.

use parking_lot::Mutex;
use room_floor::Post;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};
use topic_index::TopicIndex;

/// The shapes and the store, from the crate that holds them.
///
/// Re-exported because they are what this module's commands hand back, and a
/// caller inside the app should not have to know which side of the tauri
/// boundary a topic's shape lives on.
pub use topic_index::{LoggedPost, Topic};

/// The room whose log this is, as it appears in the path.
///
/// One room, and this constant is the seam rather than the omission: what a
/// second room would need is another value here, not another shape of path.
const ROOM_NAME: &str = "main";

/// The event a failure on this surface reaches the screen on.
///
/// A post is never lost to a log failure — it is in the room before anything is
/// written, and the write cannot take it back out. What can be lost is the
/// record of it, and that is the thing this event exists to stop happening
/// quietly (#48).
const LOG_ERROR_EVENT: &str = "room-log-error";

/// The event the topic list reaches the screen on.
///
/// Emitted whenever the index changes under the screen rather than because of
/// it: a topic realised by its own first post, or a session id recorded by a
/// launch. What the screen did itself it already knows about, and is told again
/// here rather than being trusted to keep a second copy in step.
const TOPICS_EVENT: &str = "room-topics";

/// One acquisition for every read-modify-write of the index.
///
/// The index is a single small file read and rewritten whole. Two writers
/// interleaving would drop one of the two changes — a launch recording a
/// session id while a post realises the topic it landed in is the concrete
/// pair — and neither is a change anyone would notice losing until the topic
/// failed to resume.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// The five fields of a post, taken off the post itself.
///
/// A mapping rather than a `Serialize` on `Post`: `Post` carries `hue` as well,
/// and a derive would put it in the file. What is dropped here is dropped on
/// purpose, and this is where that is legible. A free function rather than an
/// inherent method, because the type it builds belongs to the `topic-index`
/// crate now — and this is the half that could not go with it, since `Post` is
/// the room's own shape.
fn logged(post: &Post) -> LoggedPost {
    LoggedPost {
        message_id: post.message_id.clone(),
        speaker: post.speaker.clone(),
        content: post.content.clone(),
        to: post.to.clone(),
        ts: post.ts.clone(),
    }
}

/// The topic the room is in, before anything has been said in it.
///
/// Held by the room rather than written down. A launch opens a new topic
/// (#115, Master 判断5) and most of what an app run does is not speaking, so a
/// row written at launch would be a row for every time the app was opened.
#[derive(Debug, Clone, Serialize)]
pub struct TopicRef {
    pub topic_id: String,
    pub created_at: String,
}

impl TopicRef {
    pub fn new(created_at: String) -> Self {
        TopicRef {
            topic_id: uuid::Uuid::new_v4().to_string(),
            created_at,
        }
    }
}

/// Where this room's topics live.
fn room_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("logs").join(ROOM_NAME))
}

/// Where one topic's posts live.
fn topic_path(app: &AppHandle, topic_id: &str) -> Result<PathBuf, String> {
    Ok(topic_index::topic_path(&room_dir(app)?, topic_id))
}

/// The single flow this log kept before topics existed.
fn legacy_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("logs").join(format!("{ROOM_NAME}.jsonl")))
}

/// Carry the pre-topic single flow in as one topic.
///
/// Moved rather than copied. Two files holding one conversation is a second
/// ordering of it, and the one thing the room is the authority on is that there
/// is one (`room.rs`). Nothing is dropped: the posts are the file, and the file
/// is what moves (#115, decision 8). The entry it gets is the scan's, like any
/// other file in the directory.
///
/// It stays on this side of the boundary rather than going into the crate with
/// the rest of the store: what it reads is `logs/{room}.jsonl`, one level above
/// the room's own directory, and the crate is given that directory and nothing
/// around it.
///
/// Call under `INDEX_LOCK`.
fn migrate_legacy(app: &AppHandle) -> Result<(), String> {
    let legacy = legacy_path(app)?;
    if !legacy.is_file() {
        return Ok(());
    }
    let dir = room_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create the room log dir: {e}"))?;
    let destination = dir.join(format!("{}.jsonl", uuid::Uuid::new_v4()));
    if std::fs::rename(&legacy, &destination).is_ok() {
        return Ok(());
    }
    // `rename` fails across volumes, which is not a case this app's own app
    // data directory produces. The fallback's result is taken from the remove:
    // a copy that succeeded and a remove that failed is the one way this ends
    // with the conversation in two places.
    std::fs::copy(&legacy, &destination)
        .map_err(|e| format!("Failed to carry the existing log into a topic: {e}"))?;
    std::fs::remove_file(&legacy)
        .map_err(|e| format!("Failed to remove the log after carrying it into a topic: {e}"))
}

/// Read the index off disk and reconcile it with the directory.
///
/// The reconciliation is the crate's (`topic_index::read`); what is added here
/// is the legacy carry-in ahead of it, and writing back whatever the read
/// changed. A read that adopts is a read that has to be written down: the entry
/// it built is what the next reader would otherwise build again, and the
/// session ids recorded against it in between would land on an entry no file
/// agrees with.
///
/// Call under `INDEX_LOCK`.
fn read_index(app: &AppHandle) -> Result<TopicIndex, String> {
    migrate_legacy(app)?;

    let dir = room_dir(app)?;
    let (index, needs_write) = topic_index::read(&dir, &crate::room::now_iso())?;
    if needs_write {
        topic_index::write(&dir, &index)?;
    }
    Ok(index)
}

/// Call under `INDEX_LOCK`.
fn write_index(app: &AppHandle, index: &TopicIndex) -> Result<(), String> {
    topic_index::write(&room_dir(app)?, index)
}

/// Say on the screen that the log failed.
///
/// The room went on without it, so nothing here stops anything. What it stops is
/// the failure being invisible: a log that had quietly stopped recording would
/// still look like a log, and would read as nothing having been said (#48).
pub fn report(app: &AppHandle, message: String) {
    eprintln!("[room] {message}");
    let _ = app.emit(LOG_ERROR_EVENT, message);
}

/// Hand the screen the topic list as it now stands.
///
/// Reachable from `room.rs` as well as from here: deleting a topic changes this
/// list from a command that is not on this surface, and the screen is told
/// about the list from one place regardless of who changed it.
pub(crate) fn announce(app: &AppHandle) {
    let read = {
        let _guard = INDEX_LOCK.lock();
        read_index(app)
    };
    match read {
        Ok(index) => {
            let _ = app.emit(TOPICS_EVENT, index.topics);
        }
        Err(err) => report(app, err),
    }
}

/// Put one post at the end of a topic.
///
/// Called from inside the room's own critical section, which is what makes the
/// file's order the floor's order. The alternative — appending once the lock is
/// dropped — leaves two speakers the floor has already ordered to reach the disk
/// in whichever order the scheduler hands them, and the room would then hold two
/// orderings of one conversation instead of one (`room.rs`).
///
/// Returns whether this was the first post of its topic, so the caller can
/// realise and name the topic once it is out of the lock. The index is a second
/// file read and rewritten whole, and it has no business happening with the room
/// stopped; nothing about a post waits on it.
///
/// Returns the failure rather than reporting it, for the same reason: the caller
/// is holding the room lock at this point, and emitting from under it would be
/// doing the screen's work with the room stopped; `room.rs` reports once it is
/// out.
///
/// The content may contain newlines. It stays one line per post regardless,
/// because what is written is JSON and JSON escapes them.
pub fn append(app: &AppHandle, topic_id: &str, post: &Post) -> Result<bool, String> {
    let path = topic_path(app, topic_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create the room log dir: {e}"))?;
    }
    // Read before the write, so "first" means the first in the topic and not
    // the first this process wrote. A missing file and an empty one are the
    // same state here.
    let first = std::fs::metadata(&path)
        .map(|meta| meta.len() == 0)
        .unwrap_or(true);

    let mut line = serde_json::to_string(&logged(post))
        .map_err(|e| format!("Failed to serialize the post: {e}"))?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Failed to open the room log: {e}"))?;
    // One write of the whole line, so no line is half put down by one call and
    // finished by another's.
    file.write_all(line.as_bytes())
        .map_err(|e| format!("Failed to append to the room log: {e}"))?;
    Ok(first)
}

/// Give a topic its entry and its name, from the first thing said in it.
///
/// Out of the room's lock. Silent about a title when the topic already has one:
/// a title the person edited is theirs, and a first post arriving in a topic
/// renamed before anyone spoke would otherwise take it back.
pub fn realize_from_first_post(app: &AppHandle, topic: &TopicRef, content: &str) {
    let written = {
        let _guard = INDEX_LOCK.lock();
        match read_index(app) {
            Err(err) => {
                report(app, err);
                return;
            }
            Ok(mut index) => {
                let entry = index.realize(&topic.topic_id, &topic.created_at);
                if entry.title.is_empty() {
                    entry.title = topic_index::title_from(content);
                }
                match write_index(app, &index) {
                    Ok(()) => true,
                    Err(err) => {
                        report(app, err);
                        false
                    }
                }
            }
        }
    };
    if written {
        announce(app);
    }
}

/// One topic's posts, for a caller inside this process.
///
/// The pull the sidecar's read tool reaches (`room.rs`), and the same read the
/// screen makes. One function, so the two surfaces cannot disagree about what a
/// topic contains.
pub fn topic_posts(app: &AppHandle, topic_id: &str) -> Result<Vec<LoggedPost>, String> {
    let path = topic_path(app, topic_id)?;
    let (posts, skipped) = topic_index::read_posts(&path);
    if skipped > 0 {
        report(app, format!("読めなかった記録が {skipped} 件あります"));
    }
    Ok(posts)
}

/// The session `account_id` was in while `topic_id` was open, if one is on
/// record.
pub fn session_of(app: &AppHandle, topic_id: &str, account_id: &str) -> Option<String> {
    let _guard = INDEX_LOCK.lock();
    read_index(app)
        .ok()?
        .find(topic_id)?
        .sessions
        .get(account_id)
        .cloned()
}

/// Whether a topic has an entry in the index.
///
/// What selecting one from the list is checked against. A topic the room is
/// currently in but nothing has realised yet answers false here, and the room
/// answers for that one itself (`room.rs`).
pub fn topic_exists(app: &AppHandle, topic_id: &str) -> bool {
    let _guard = INDEX_LOCK.lock();
    read_index(app)
        .map(|index| index.find(topic_id).is_some())
        .unwrap_or(false)
}

/// Record which session an account was launched into a topic under.
///
/// Written at launch rather than at exit, because the id is decided before the
/// CLI starts and a session that ends badly is exactly the one worth being able
/// to resume. It realises the topic: starting a session in a topic is a
/// deliberate act, and the id has to outlive the run to be worth anything.
pub fn record_session(
    app: &AppHandle,
    topic: &TopicRef,
    account_id: &str,
    session_id: &str,
) -> Result<(), String> {
    {
        let _guard = INDEX_LOCK.lock();
        let mut index = read_index(app)?;
        index
            .realize(&topic.topic_id, &topic.created_at)
            .sessions
            .insert(account_id.to_string(), session_id.to_string());
        write_index(app, &index)?;
    }
    announce(app);
    Ok(())
}

/// Delete one topic: its posts, and the entry that annotated them.
///
/// The store half of the delete. What surrounds it — stopping the sessions that
/// were running in the topic, and moving the room off it when it is the one the
/// room is in — is `room::room_delete_topic`, because neither of those is the
/// log's to do.
///
/// Both halves go, and the file goes first (`topic_index::delete`). The entry
/// is an annotation of the file, so an index with the entry taken out and the
/// file still there is a topic the very next read adopts back under a rebuilt
/// entry — a delete that quietly undoes itself, reporting nothing while it does
/// (#119, decision 1).
///
/// Refused when the index does not name the topic. Nothing the screen can press
/// reaches that: the current topic before anything has realised it is drawn from
/// the room rather than from the list, and it carries no delete (#119, decision
/// 5). What the refusal covers is the same thing `room_select_topic`'s does — a
/// caller naming a topic this room does not have — and it says so the same way.
///
/// The screen is told the list changed by the caller, once the room has been put
/// somewhere the deleted topic is not.
pub fn delete_topic(app: &AppHandle, topic_id: &str) -> Result<(), String> {
    let _guard = INDEX_LOCK.lock();
    let dir = room_dir(app)?;
    let mut index = read_index(app)?;
    if index.find(topic_id).is_none() {
        return Err(format!("トピック {topic_id} は見つかりません。"));
    }
    topic_index::delete(&dir, &mut index, topic_id)?;
    write_index(app, &index)
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// The topics, oldest first.
///
/// The topic the room is currently in is in this list only once something has
/// realised it. The screen holds its own current topic and draws it whether or
/// not the list names it (`room_current_topic`).
#[tauri::command]
pub fn room_topics(app: AppHandle) -> Result<Vec<Topic>, String> {
    let _guard = INDEX_LOCK.lock();
    Ok(read_index(&app)?.topics)
}

/// One topic's posts, oldest first.
///
/// For the screen. The topic is named rather than assumed, because the screen
/// reads a topic that is not the current one every time the list is used: that
/// is what selecting one is.
#[tauri::command]
pub fn room_topic_log(app: AppHandle, topic_id: String) -> Result<Vec<LoggedPost>, String> {
    topic_posts(&app, &topic_id)
}

/// Rename a topic.
///
/// The generated title is a starting point in an editable field, which is the
/// same thing an account's name is (#115, decision 9). Blank is refused rather
/// than stored: an empty title is the state a topic has before anything is said
/// in it, and a topic deliberately cleared would read as one nobody has spoken
/// in.
///
/// Realising is deliberate here too — naming a topic before speaking in it is
/// the person deciding it exists.
#[tauri::command]
pub fn room_rename_topic(
    app: AppHandle,
    state: tauri::State<crate::room::RoomState>,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err("トピック名を入力してください。".to_string());
    }
    {
        let _guard = INDEX_LOCK.lock();
        let mut index = read_index(&app)?;
        match index.find_mut(&topic_id) {
            Some(topic) => topic.title = title,
            None => {
                let current = state.topic();
                if current.topic_id != topic_id {
                    return Err(format!("トピック {topic_id} は見つかりません。"));
                }
                index
                    .realize(&current.topic_id, &current.created_at)
                    .title = title;
            }
        }
        write_index(&app, &index)?;
    }
    announce(&app);
    Ok(())
}
