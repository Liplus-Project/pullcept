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
//! emits (`SessionStats::emit`), carrying the five as read so far and 制限中,
//! sent every round so that a reloaded webview picks them up again (#374).
//!
//! **The same round drives the seat's usage limit** (#294, `codex_limit`):
//! the turn end and the reset each read leaves behind are handed to the
//! seat's `Limiter`, which stops the seat, asks the app-server when a question
//! is due, and hands the seat back. The question does not wait for the rollout:
//! a seat that starts stopped (#312) is asked on the first round, before its
//! CLI has written anything. The limit in memory is the launch's, like this
//! thread: when the thread ends, what was held in memory goes with it, and the
//! seat's mailbox keeps the ids of what was held (#312).

use crate::codex_limit::Limiter;
use crate::pty::PtyState;
use crate::session::RoomSeats;
use mcp_config::codex::status::{Status, Tail};
use std::path::PathBuf;
use std::time::Duration;
use tauri::Manager;

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
///
/// The limiter is already in the table (`Limiter::open`). A seat it started
/// stopped is shown 制限中 once, here, since nothing turns over to send it.
pub fn watch(limiter: Limiter, home: PathBuf, resumed: Option<(PathBuf, u64)>) {
    let (app, pty_id) = (limiter.app.clone(), limiter.pty_id.clone());
    let spawned = std::thread::Builder::new()
        .name("codex-status".into())
        .spawn(move || {
            if limiter.is_limited() {
                limiter.stats(&Status::default()).emit(&limiter.app);
            }
            follow(&limiter, &home, resumed);
            limiter.forget();
        });
    if let Err(err) = spawned {
        // The session runs without its five values, which read `—`, and
        // without its limit being watched: the room types into it as before,
        // whatever its mailbox says.
        app.state::<crate::codex_limit::CodexLimits>().forget(&pty_id);
        eprintln!("[codex-status] watcher could not start: {err}");
    }
}

/// The watcher's rounds, until the launch leaves the seat or its PTY ends.
fn follow(limiter: &Limiter, home: &std::path::Path, resumed: Option<(PathBuf, u64)>) {
    let (app, topic_id, account_id, pty_id) = (
        &limiter.app,
        &limiter.topic_id,
        &limiter.account_id,
        &limiter.pty_id,
    );
    let mut followed: Option<Followed> = None;
    loop {
        std::thread::sleep(POLL);
        if !app.state::<PtyState>().is_running(pty_id) {
            return;
        }
        let Some(native) = app
            .state::<RoomSeats>()
            .native_id_of(topic_id, account_id, pty_id)
        else {
            return;
        };
        if let Some(id) = native {
            read(&mut followed, id, home, &resumed);
        }
        let mut tail = followed.as_mut().and_then(|f| f.tail.as_mut());
        // Every round, read or not, and with no rollout yet: a stopped seat's
        // due question does not wait for the file to move, or to exist.
        let _ = match tail.as_deref_mut() {
            Some(tail) => limiter.round(tail.take_turn_end(), tail.resets_at),
            None => limiter.round(None, None),
        };
        // When the figures were received, changed or not (#364).
        if let Some(tail) = tail.as_deref_mut() {
            if tail.take_limits_read() {
                limiter.received_usage(&tail.status);
            }
        }
        // Every round, changed or not (#374): a reloaded webview has no stats
        // until it is sent them, and 制限中 is one of them. The Claude seat's
        // watcher replays the same way (`claude_limit::emit_current`).
        let none = Status::default();
        let status = tail.as_deref().map_or(&none, |tail| &tail.status);
        limiter.stats(status).emit(app);
    }
}

/// One round's reading of the rollout of `id`: found if it was not, then read
/// forward. Whether any of the five changed.
fn read(
    followed: &mut Option<Followed>,
    id: String,
    home: &std::path::Path,
    resumed: &Option<(PathBuf, u64)>,
) -> bool {
    if followed.as_ref().is_none_or(|f| f.id != id) {
        *followed = Some(Followed {
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
        return false;
    };
    if path.is_none() {
        *misses = misses.saturating_add(1);
        if *misses > SEARCH_EVERY_AFTER && *misses % SEARCH_SLOW != 0 {
            return false;
        }
        let Some(found) = mcp_config::codex::transcript(home, id) else {
            return false;
        };
        *misses = 0;
        // Placed once, at the first finding: a file found again
        // after a failed read keeps the offset it had reached.
        if tail.is_none() {
            let start = match resumed {
                Some((file, length)) if *file == found => *length,
                _ => 0,
            };
            *tail = Some(Tail::new(start));
        }
        *path = Some(found);
    }
    let Some(tail) = tail.as_mut() else {
        return false;
    };
    match path.as_deref().map(|file| tail.read(file)) {
        Some(Ok(changed)) => changed,
        // Gone or unreadable for now: find it again next round. The
        // reading so far is kept; a file replaced under the same
        // name is read from its start (`Tail::read`).
        Some(Err(_)) => {
            *path = None;
            false
        }
        None => false,
    }
}
