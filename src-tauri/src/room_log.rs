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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

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

/// How long an auto-generated title is allowed to be, in characters.
///
/// Characters rather than bytes: the titles are Japanese more often than not,
/// and a byte cut would land inside one.
const TITLE_CHARS: usize = 40;

/// One acquisition for every read-modify-write of the index.
///
/// The index is a single small file read and rewritten whole. Two writers
/// interleaving would drop one of the two changes — a launch recording a
/// session id while a post realises the topic it landed in is the concrete
/// pair — and neither is a change anyone would notice losing until the topic
/// failed to resume.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// One post, as the log holds it.
///
/// The same five fields going in and coming out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedPost {
    pub message_id: String,
    pub speaker: String,
    pub content: String,
    /// The participant this was addressed to, or absent when it was said to the
    /// room. Omitted rather than written as null, so the two states are the
    /// field's presence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub ts: String,
}

impl LoggedPost {
    /// The five fields of a post, taken off the post itself.
    ///
    /// A mapping rather than a `Serialize` on `Post`: `Post` carries `hue` as
    /// well, and a derive would put it in the file. What is dropped here is
    /// dropped on purpose, and this is where that is legible.
    fn of(post: &Post) -> Self {
        LoggedPost {
            message_id: post.message_id.clone(),
            speaker: post.speaker.clone(),
            content: post.content.clone(),
            to: post.to.clone(),
            ts: post.ts.clone(),
        }
    }
}

/// One topic, as the index holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    /// Opaque, minted once, and the file name of this topic's posts. Never a
    /// title: a title is edited, and a file whose name moved with it would
    /// leave the posts behind.
    pub topic_id: String,
    /// What the list shows. Generated from the opening of the first post said
    /// in it and editable afterwards (#115, decision 9). Empty until that first
    /// post lands, which is a state the screen draws rather than a value
    /// missing.
    pub title: String,
    /// When it was made, RFC 3339. The room's clock (`room::now_iso`), or the
    /// first post's own stamp for a topic adopted from a file.
    pub created_at: String,
    /// The session each account was in while this topic was open, keyed by
    /// account id.
    ///
    /// This is what makes a topic a vessel rather than a transcript: reopening
    /// it hands these back to the launch, and the participant returns carrying
    /// its own context instead of being read a summary of it (#115, decisions 3
    /// and 4).
    ///
    /// Keyed on the account id, which is the identity (#53). Not on the name,
    /// which is editable, and not on the room's connection id, which is minted
    /// per connection and is gone by the time a topic is reopened.
    #[serde(default)]
    pub sessions: BTreeMap<String, String>,
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

/// The topics, as the file holds them.
///
/// Oldest first, by `created_at`. The screen reverses it — a list is read
/// newest first — and the file keeps the order the conversation happened in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TopicIndex {
    #[serde(default)]
    topics: Vec<Topic>,
}

impl TopicIndex {
    fn find(&self, topic_id: &str) -> Option<&Topic> {
        self.topics.iter().find(|one| one.topic_id == topic_id)
    }

    fn find_mut(&mut self, topic_id: &str) -> Option<&mut Topic> {
        self.topics.iter_mut().find(|one| one.topic_id == topic_id)
    }
}

fn room_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("logs").join(ROOM_NAME))
}

fn index_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(room_dir(app)?.join("index.json"))
}

/// Where one topic's posts live.
///
/// The id is a UUID minted by `TopicRef`, so nothing a person types reaches a
/// path. A title with a slash in it would otherwise be a directory.
fn topic_path(app: &AppHandle, topic_id: &str) -> Result<PathBuf, String> {
    Ok(room_dir(app)?.join(format!("{topic_id}.jsonl")))
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
/// The reconciliation is not a repair path bolted on: it is what lets a topic
/// exist before any entry is written for it. An entry with no file stays — a
/// topic whose session was launched but which nobody has spoken in yet is that
/// case, and its session id is the whole reason to keep it.
///
/// Call under `INDEX_LOCK`.
fn read_index(app: &AppHandle) -> Result<TopicIndex, String> {
    migrate_legacy(app)?;

    let path = index_path(app)?;
    let mut index = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read the topic index: {e}"))?;
        serde_json::from_str::<TopicIndex>(&content)
            .map_err(|e| format!("Failed to parse the topic index: {e}"))?
    } else {
        TopicIndex::default()
    };

    let adopted = adopt_orphans(app, &mut index)?;
    index
        .topics
        .sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.topic_id.cmp(&b.topic_id)));
    if adopted || !path.exists() {
        write_index(app, &index)?;
    }
    Ok(index)
}

/// Give an entry to every topic file the index does not name.
///
/// Answers true when it changed anything.
fn adopt_orphans(app: &AppHandle, index: &mut TopicIndex) -> Result<bool, String> {
    let dir = room_dir(app)?;
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // No directory is no topics, not a failure. It is the first run.
        return Ok(false);
    };
    let mut adopted = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(topic_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if index.find(topic_id).is_some() {
            continue;
        }
        let (posts, _) = read_posts(&path);
        let first = posts.first();
        index.topics.push(Topic {
            topic_id: topic_id.to_string(),
            title: first.map(|post| title_from(&post.content)).unwrap_or_default(),
            // The first thing said in it, which is the closest thing a file
            // carries to when it began. A topic made through the app has its
            // own stamp and never reaches here.
            created_at: first
                .map(|post| post.ts.clone())
                .unwrap_or_else(crate::room::now_iso),
            sessions: BTreeMap::new(),
        });
        adopted = true;
    }
    Ok(adopted)
}

/// Call under `INDEX_LOCK`.
fn write_index(app: &AppHandle, index: &TopicIndex) -> Result<(), String> {
    let path = index_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create the room log dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize the topic index: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write the topic index: {e}"))
}

/// Make sure `topic` has an entry, and hand back a mutable hold on it.
///
/// The one place a topic reaches the index. Every caller is a deliberate act
/// that has to survive the app closing — the first post, a session id, a
/// rename — and none of them is "the app was opened".
///
/// Call under `INDEX_LOCK`.
fn realize<'a>(index: &'a mut TopicIndex, topic: &TopicRef) -> &'a mut Topic {
    if index.find(&topic.topic_id).is_none() {
        index.topics.push(Topic {
            topic_id: topic.topic_id.clone(),
            title: String::new(),
            created_at: topic.created_at.clone(),
            sessions: BTreeMap::new(),
        });
    }
    index
        .find_mut(&topic.topic_id)
        .expect("just inserted when absent")
}

/// A title from the opening of the first thing said in the topic.
///
/// The first line, whitespace collapsed, cut at `TITLE_CHARS`. A list of
/// timestamps is a list nobody can read (#115, decision 9), and the opening of
/// the first post is what a person would have written there anyway.
fn title_from(content: &str) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return String::new();
    }
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(TITLE_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
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
fn announce(app: &AppHandle) {
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

    let mut line = serde_json::to_string(&LoggedPost::of(post))
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
                let entry = realize(&mut index, topic);
                if entry.title.is_empty() {
                    entry.title = title_from(content);
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

/// One topic's posts, oldest first.
///
/// The tuple's second half is how many lines did not parse. A line that does
/// not parse is skipped rather than failing the read: the case it covers is a
/// torn tail from a run that ended mid-write, and refusing the whole topic over
/// the last line of it would lose everything to protect nothing. It is not
/// skipped quietly — the count goes back to the screen on the same event a
/// failed append does.
fn read_posts(path: &Path) -> (Vec<LoggedPost>, usize) {
    // No file is no history, not a failure. It is what a topic nobody has
    // spoken in yet looks like.
    let Ok(content) = std::fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };

    let mut posts = Vec::new();
    let mut skipped = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LoggedPost>(line) {
            Ok(post) => posts.push(post),
            Err(_) => skipped += 1,
        }
    }
    (posts, skipped)
}

/// One topic's posts, for a caller inside this process.
///
/// The pull the sidecar's read tool reaches (`room.rs`), and the same read the
/// screen makes. One function, so the two surfaces cannot disagree about what a
/// topic contains.
pub fn topic_posts(app: &AppHandle, topic_id: &str) -> Result<Vec<LoggedPost>, String> {
    let path = topic_path(app, topic_id)?;
    let (posts, skipped) = read_posts(&path);
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
        realize(&mut index, topic)
            .sessions
            .insert(account_id.to_string(), session_id.to_string());
        write_index(app, &index)?;
    }
    announce(app);
    Ok(())
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
                realize(&mut index, &current).title = title;
            }
        }
        write_index(&app, &index)?;
    }
    announce(&app);
    Ok(())
}
