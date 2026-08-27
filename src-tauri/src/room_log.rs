//! The room's log.
//!
//! What was said in the room, kept as one append-only jsonl file so a past
//! exchange can be read back after the window that showed it has closed. The
//! room itself keeps nothing: its lines live in the DOM, and closing the app
//! took them (#48).
//!
//! The log is the room's, not a tab's. `config.rs` already holds a saving
//! apparatus — `SavedChatMessage` / `SessionData` / `TabSessions` — and none of
//! it is used here, deliberately. `SavedChatMessage` carries `role` and
//! `content_type`, which is a chat assistant's vocabulary: the room has no axis
//! `role` lands on, because a person and a session are one kind of participant
//! and what separates two lines is the name on them (#39). `TabSessions` is
//! keyed on a tab, and a tab is how something is launched, not the vessel a
//! conversation happens in. Wiring either through would put the asymmetry #39
//! removed back into the app from underneath.
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
//! The file name is `logs/{room}.jsonl`, which is where `design/Vision.dc.html`
//! put it. The room's name occupies a position in that path and is fixed at
//! `main`: several rooms are not implemented, and the path is shaped so that
//! implementing them adds a value here rather than a directory level (#48).
//!
//! No rotation and no ceiling. Vision holds one `main.jsonl` carrying no date,
//! and a log that drops its own oldest entries answers the question this exists
//! for — what was actually said — with silence at exactly the distance that
//! makes the question worth asking.

use room_floor::Post;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// The room whose log this is, as it appears in the file name.
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

fn log_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("logs").join(format!("{ROOM_NAME}.jsonl")))
}

/// Put one post at the end of the log.
///
/// Called from inside the room's own critical section, which is what makes the
/// file's order the floor's order. The alternative — appending once the lock is
/// dropped — leaves two speakers the floor has already ordered to reach the disk
/// in whichever order the scheduler hands them, and the room would then hold two
/// orderings of one conversation instead of one (`room.rs`).
///
/// Returns the failure rather than reporting it. The caller is holding the room
/// lock at this point, and emitting from under it would be doing the screen's
/// work with the room stopped; `room.rs` reports once it is out.
///
/// The content may contain newlines. It stays one line per post regardless,
/// because what is written is JSON and JSON escapes them.
pub fn append(app: &AppHandle, post: &Post) -> Result<(), String> {
    let path = log_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create the room log dir: {e}"))?;
    }

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
        .map_err(|e| format!("Failed to append to the room log: {e}"))
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

/// The log, oldest first.
///
/// For the screen, and only for the screen. Nothing here is re-delivered to
/// participants: a session that joined late missed what predates its seat, and
/// that is the room's existing answer (`room.rs`). Feeding the log back into a
/// channel would make the room start holding who has heard what, which is the
/// shape #31 and #39 turned down.
///
/// A line that does not parse is skipped rather than failing the read. The case
/// it covers is a torn tail from a run that ended mid-write, and refusing the
/// whole history over the last line of it would lose everything to protect
/// nothing. It is not skipped quietly — the count goes back to the screen on
/// the same event a failed append does.
#[tauri::command]
pub fn room_log(app: AppHandle) -> Result<Vec<LoggedPost>, String> {
    let path = log_path(&app)?;
    // No file is no history, not a failure. It is what the first run of the app
    // looks like.
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read the room log: {e}"))?;

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

    if skipped > 0 {
        report(&app, format!("読めなかった記録が {skipped} 件あります"));
    }

    Ok(posts)
}
