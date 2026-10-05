//! A Codex CLI seat's five panel values, read off its rollout (#283).
//!
//! Codex has no status-line command to put on the launch line (#155 is Claude
//! Code's), so the app reads the values out of the file the CLI writes them
//! into. What is read and how is `mcp_config::codex::status`, which holds no
//! tauri so that it can be tested; what stays here is the part that needs the
//! app — which seat, which file, and when to stop.
//!
//! **One thread per Codex launch, polling.** Every `POLL` it asks the seat for
//! the native id (`RoomSeats::native_id_of`); a fresh launch has none until the
//! first `SessionStart` callback (`/hooks/codex-session`), and the file may
//! appear after that, so both are looked for again on every round rather than
//! once. A missing id, a missing file and a read error are all the same answer:
//! nothing this round, look again next round. Nothing here stops the session or
//! the launch.
//!
//! The thread ends when its launch no longer holds the seat or its PTY has
//! exited. The panel then keeps the last values, as it does for a Claude Code
//! seat whose session has ended (docs/3-accounts.md).
//!
//! What reaches the screen is the same `session-stats` the status-line receiver
//! emits (`SessionStats::emit`), carrying the five as read so far, sent only
//! when one of them changed.

use crate::pty::PtyState;
use crate::room::SessionStats;
use crate::session::RoomSeats;
use mcp_config::codex::status::Tail;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// How often the rollout is looked at. The CLI writes a `token_count` per model
/// answer; a second behind it is well inside what a person reads the panel at.
const POLL: Duration = Duration::from_secs(1);

/// The native id a watcher is following, its rollout once found, and the
/// reading of it once placed.
struct Followed {
    id: String,
    path: Option<PathBuf>,
    tail: Option<Tail>,
    /// Rounds spent looking for the file (`SEARCH_EVERY_AFTER`).
    misses: u32,
}

/// After this many rounds without the file, it is looked for every
/// `SEARCH_SLOW` rounds instead of every round. Finding it walks the
/// `sessions/` tree, and a CLI configured not to write a rollout would
/// otherwise have that walk run every second for the life of the session.
const SEARCH_EVERY_AFTER: u32 = 60;
const SEARCH_SLOW: u32 = 10;

/// Start watching one launch's rollout.
///
/// `home` is the `CODEX_HOME` the seat runs under. `resumed` is the rollout a
/// resume went back into and its length at launch: reading starts there, so
/// the last run's values are not shown as this one's. Any other file this
/// launch ends up writing is read from its start.
pub fn watch(
    app: AppHandle,
    topic_id: String,
    account_id: String,
    pty_id: String,
    home: PathBuf,
    resumed: Option<(PathBuf, u64)>,
) {
    let spawned = std::thread::Builder::new()
        .name("codex-status".into())
        .spawn(move || {
            let mut followed: Option<Followed> = None;
            loop {
                std::thread::sleep(POLL);
                if !app.state::<PtyState>().is_running(&pty_id) {
                    return;
                }
                let Some(native) =
                    app.state::<RoomSeats>()
                        .native_id_of(&topic_id, &account_id, &pty_id)
                else {
                    return;
                };
                let Some(id) = native else {
                    continue;
                };
                if followed.as_ref().is_none_or(|f| f.id != id) {
                    followed = Some(Followed {
                        id,
                        path: None,
                        tail: None,
                        misses: 0,
                    });
                }
                let Some(Followed {
                    id,
                    path,
                    tail,
                    misses,
                }) = followed.as_mut()
                else {
                    continue;
                };
                if path.is_none() {
                    *misses = misses.saturating_add(1);
                    if *misses > SEARCH_EVERY_AFTER && *misses % SEARCH_SLOW != 0 {
                        continue;
                    }
                    let Some(found) = mcp_config::codex::transcript(&home, id) else {
                        continue;
                    };
                    *misses = 0;
                    // Placed once, at the first finding: a file found again
                    // after a failed read keeps the offset it had reached.
                    if tail.is_none() {
                        let start = match &resumed {
                            Some((file, length)) if *file == found => *length,
                            _ => 0,
                        };
                        *tail = Some(Tail::new(start));
                    }
                    *path = Some(found);
                }
                let (Some(file), Some(tail)) = (path.as_deref(), tail.as_mut()) else {
                    continue;
                };
                match tail.read(file) {
                    Ok(true) => {
                        SessionStats::from_codex(
                            topic_id.clone(),
                            account_id.clone(),
                            &tail.status,
                        )
                        .emit(&app);
                    }
                    Ok(false) => {}
                    // Gone or unreadable for now: find it again next round. The
                    // reading so far is kept; a file replaced under the same
                    // name is read from its start (`Tail::read`).
                    Err(_) => *path = None,
                }
            }
        });
    if let Err(err) = spawned {
        // The session runs without its five values, which read `—`.
        eprintln!("[codex-status] watcher could not start: {err}");
    }
}
