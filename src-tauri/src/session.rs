//! Putting one CLI session into the room.
//!
//! The conditions the round trip does not survive without — registration by
//! name in `.mcp.json`, and the launch flag carried alone — live in the
//! `mcp-config` crate, which holds no tauri so that they can be tested. What
//! stays here is the part that needs the app: the room's port and token, the
//! working directory, and the PTY the session is held open on.
//!
//! The session must be interactive: `--print` never receives a push.

use crate::config::{Account, AccountKind};
use crate::pty::{self, PtyState};
use crate::room::RoomState;
use mcp_config::{
    declared_character, declares_settings, launch_args, register_sidecar,
    reject_incompatible_flags, server_name_for, RoomRegistration,
};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;

/// The sidecar entry point and the runner that executes it.
///
/// Both come from one walk, and both are absolute. The CLI spawns the sidecar
/// from the user's own project directory, so a path resolved by name there
/// resolves against their tree, not ours (#22).
///
/// Stage-one distribution runs from the repository, so the walk up from the
/// working directory is the normal path; the env overrides exist for a layout
/// this does not predict.
fn resolve_sidecar_paths() -> Result<(PathBuf, PathBuf), String> {
    fn from_env(key: &str) -> Result<Option<PathBuf>, String> {
        match std::env::var(key) {
            Err(_) => Ok(None),
            Ok(value) => {
                let path = PathBuf::from(&value);
                if path.is_file() {
                    Ok(Some(path))
                } else {
                    Err(format!("{key} points at a missing file: {value}"))
                }
            }
        }
    }

    let entry_override = from_env("PULLCEPT_SIDECAR_ENTRY")?;
    let runner_override = from_env("PULLCEPT_SIDECAR_RUNNER")?;
    if let (Some(entry), Some(runner)) = (&entry_override, &runner_override) {
        return Ok((entry.clone(), runner.clone()));
    }

    let mut dir = std::env::current_dir()
        .map_err(|e| format!("Failed to resolve the working directory: {e}"))?;
    for _ in 0..4 {
        let entry = dir.join("sidecar").join("src").join("index.ts");
        let runner = dir
            .join("node_modules")
            .join("tsx")
            .join("dist")
            .join("cli.mjs");
        if entry.is_file() && runner.is_file() {
            return Ok((
                entry_override.unwrap_or(entry),
                runner_override.unwrap_or(runner),
            ));
        }
        if !dir.pop() {
            break;
        }
    }
    Err("Could not find sidecar/src/index.ts next to node_modules/tsx. \
         Run npm install, or set PULLCEPT_SIDECAR_ENTRY and PULLCEPT_SIDECAR_RUNNER."
        .to_string())
}

/// Which seat in the room each account is holding.
///
/// **One account, one seat per room.** An account is who someone is, and the
/// same someone cannot be in one room twice: two connections under one account
/// would put one identity in the roster twice, and a post addressed to that
/// name would have two places to land.
///
/// The rule is scoped to the room, not to the account. There is one room today
/// — the sidecar's `PULLCEPT_ROOM_ID` is fixed to `pullcept` — so this map
/// needs no room in its key yet, and with one room the refusal is
/// indistinguishable from "an account runs once". They are not the same rule.
/// Rooms are meant to become plural (`design/Vision.dc.html`), and one account
/// holding a seat in each of two rooms is the intended shape rather than a
/// violation of this one. The scope is written down here because the mechanism
/// cannot show it: a rule remembered as "an account runs once" would outlive
/// the reason for it and block that case later, when nobody remembers why the
/// line was drawn.
#[derive(Clone)]
pub struct RoomSeats {
    seats: Arc<Mutex<BTreeMap<String, Seat>>>,
}

/// One account's seat, from the launch being decided to the session ending.
enum Seat {
    /// A launch is in flight: the registration is being written, or the CLI is
    /// being spawned. Held so that two launches racing for one account cannot
    /// both find the seat empty — the loser is refused before a second process
    /// exists, rather than after.
    Starting,
    /// A session is running. The PTY id is what says so, and asking the PTY is
    /// the only liveness question anyone asks here.
    Running(RunningSession),
}

/// What is running under a held seat.
///
/// The screen holds these same facts while it is up, in the terminal it opened
/// for the session, and loses every one of them when the webview reloads. This
/// does not: it lives in the app, which a reload does not touch.
///
/// The pty id is the load-bearing one. It is made at spawn and handed out once,
/// so a screen that has forgotten it has no way back to the session at all —
/// the account cannot be started (this seat refuses it) and cannot be ended
/// (there is no id to kill). Keeping it here is what lets the screen ask (#84).
///
/// The other three are what the session panel says about a session. They are
/// kept beside the id rather than read off the account, because the account is
/// editable while its session runs: its command and directory are what a launch
/// would use now, not what this one used.
#[derive(Clone, serde::Serialize)]
pub struct RunningSession {
    pub pty_id: String,
    /// When the PTY was spawned, RFC 3339. The stamp `StartedSession` carries.
    pub started_at: String,
    /// The command this session was launched from.
    pub command: String,
    /// The working directory it was launched in.
    pub cwd: String,
}

/// One account's seat, as the screen reads it.
#[derive(Clone, serde::Serialize)]
pub struct SeatedAccount {
    pub account_id: String,
    /// What is running under the seat, or `null` while a launch is in flight.
    ///
    /// Null is a state, not a missing value: the seat is claimed before
    /// anything is spawned (`Seat::Starting`), so there is a window in which an
    /// account holds a seat and no session exists to reach. In that window the
    /// screen may neither start the account nor end it, and it can only say so
    /// if the case is distinguishable from a running one.
    pub session: Option<RunningSession>,
}

impl RoomSeats {
    pub fn new() -> Self {
        RoomSeats {
            seats: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The accounts holding a seat right now, with what runs under each.
    ///
    /// Liveness is read off the PTY rather than from a release call anyone has
    /// to remember to make. A seat whose session has exited is a free seat, and
    /// a release that never arrived would otherwise lock an account out of the
    /// room for the rest of the run with no way back short of restarting.
    pub fn seated(&self, ptys: &PtyState) -> Vec<SeatedAccount> {
        let mut seats = self.seats.lock();
        seats.retain(|_, seat| match seat {
            Seat::Starting => true,
            Seat::Running(session) => ptys.is_running(&session.pty_id),
        });
        seats
            .iter()
            .map(|(account_id, seat)| SeatedAccount {
                account_id: account_id.clone(),
                session: match seat {
                    Seat::Starting => None,
                    Seat::Running(session) => Some(session.clone()),
                },
            })
            .collect()
    }

    /// Claim the seat for `account_id`, or fail because it is taken.
    ///
    /// The sweep and the claim are one acquisition of the lock: checking first
    /// and claiming after would let two launches pass the same empty seat.
    fn claim(&self, account_id: &str, ptys: &PtyState) -> Result<(), ()> {
        let mut seats = self.seats.lock();
        let taken = match seats.get(account_id) {
            Some(Seat::Starting) => true,
            Some(Seat::Running(session)) => ptys.is_running(&session.pty_id),
            None => false,
        };
        if taken {
            return Err(());
        }
        seats.insert(account_id.to_string(), Seat::Starting);
        Ok(())
    }

    /// The launch got a session up; the seat is now held by that session.
    fn hold(&self, account_id: &str, session: RunningSession) {
        self.seats
            .lock()
            .insert(account_id.to_string(), Seat::Running(session));
    }

    /// The launch failed. Nothing is running, so nothing holds the seat.
    fn release(&self, account_id: &str) {
        self.seats.lock().remove(account_id);
    }
}

/// The accounts with a session in the room, for the screen to draw against its
/// own list of accounts.
///
/// Account ids, never names. The screen matches these against its accounts by
/// id, so an account renamed while its session runs is still the same account
/// on both sides, and two accounts sharing a name are still two.
///
/// What each one is running rides along (`RunningSession`), because the screen
/// cannot reconstruct it. This map from account to PTY was here from the start
/// and only the keys were ever handed over; a screen that lost its own copy of
/// a pty id was then told an account holds a seat and given no way to reach the
/// session holding it (#84).
#[tauri::command]
pub fn seated_accounts(
    pty_state: tauri::State<PtyState>,
    seats: tauri::State<RoomSeats>,
) -> Vec<SeatedAccount> {
    seats.seated(&pty_state)
}

/// Split a launch-options string into arguments.
///
/// The splitter lives in `mcp-config` so it is covered by tests; this is the
/// door the frontend reaches it through, rather than a second implementation
/// in TypeScript that would drift from the tested one.
#[tauri::command]
pub fn parse_launch_options(text: String) -> Vec<String> {
    mcp_config::split_launch_options(&text)
}

/// The arguments a launch would actually use, for display.
///
/// The app merges its own channel entry into what the person wrote and selects
/// the account's character on top of it, so the line they typed is not the line
/// that runs. This returns the line that runs, through the same function the
/// launch itself goes through (`launch_args`).
///
/// The entry names this account's own server, which is a function of the
/// account id, so the preview changes when a different account is selected and
/// holds still while that account's name is edited. Holding still is the point:
/// the identity being launched is the account, and renaming it does not make it
/// something else (#53).
#[tauri::command]
pub fn preview_launch_args(
    args: Vec<String>,
    account_id: String,
    character: Option<String>,
) -> Vec<String> {
    launch_args(
        &args,
        &server_name_for(account_id.trim()),
        character.as_deref(),
    )
}

/// What the caller gets back after a session joins.
#[derive(Debug, serde::Serialize)]
pub struct StartedSession {
    pub pty_id: String,
    /// Absolute path of the `.mcp.json` this touched, so the UI can say where.
    pub mcp_config: String,
    /// When the PTY was spawned, RFC 3339.
    ///
    /// Stamped here rather than on the screen because this is the moment the
    /// session began: the screen learns of it after the launch has returned,
    /// and a launch that takes a while would be recorded as having started
    /// late. Same clock as a post's `ts`, so the panel's start time and the
    /// first line of the conversation can be read against each other.
    pub started_at: String,
}

/// Put one account into the room.
///
/// The account carries who this is: its id is the identity, and its name and
/// hue are what the room lists it under. Both used to be declared per launch,
/// beside a tab that said only which CLI to run — the way to name a session at
/// all before there was anything durable to hang a name on (#40). The account
/// is that durable thing, so the launch no longer declares anything; it starts
/// someone who already exists.
///
/// Its character is on that same list since #99, and is why two accounts can
/// now be started in one working directory and still be two: the one file that
/// had kept them in separate directories is selected per launch instead.
#[tauri::command]
pub fn start_session(
    app: AppHandle,
    room: tauri::State<RoomState>,
    pty_state: tauri::State<PtyState>,
    seats: tauri::State<RoomSeats>,
    account: Account,
    cols: u16,
    rows: u16,
) -> Result<StartedSession, String> {
    let name = account.name.trim().to_string();
    if name.is_empty() {
        return Err(
            "This account has no name. Give it one before starting: it is what the room \
             lists it under and what a post is addressed to."
                .to_string(),
        );
    }

    // A person is not launched. Their account exists for the same reasons every
    // account does — a name, a colour, a row in the list whether or not they
    // are in the room — and there is no CLI under it to spawn (#59). Refused
    // here rather than only hidden from the launcher, so the screen is not the
    // only thing standing between a `user` account and a spawned `claude`.
    if account.kind == AccountKind::User {
        return Err(format!(
            "Account \"{name}\" is a person, not a session. There is nothing to launch: a \
             person joins by being at the screen."
        ));
    }

    if let Err(flag) = reject_incompatible_flags(&account.args) {
        return Err(format!(
            "Account \"{name}\" passes {flag}, which stops channel pushes from arriving. \
             Remove it from the launch options."
        ));
    }

    // The character rides in `--settings`, so launch options carrying their own
    // `--settings` are on the same axis as the character field. Refused rather
    // than resolved: which of two copies of one flag a CLI reads is not
    // something this app has established, and picking one here would be a
    // guess that surfaces as the wrong character speaking (#99). Refused rather
    // than stripped, for the reason the flag guard above is: a launch that
    // quietly dropped half of what was asked for looks like it worked.
    let character = declared_character(account.character.as_deref());
    if character.is_some() && declares_settings(&account.args) {
        return Err(format!(
            "Account \"{name}\" declares a character and also passes --settings in its launch \
             options. Both name the same setting. Clear one of them."
        ));
    }

    let port = room
        .port()
        .ok_or_else(|| "The room socket is not listening yet.".to_string())?;
    let room_url = format!("ws://127.0.0.1:{port}");

    // No fallback to the app's own process directory. Under `tauri dev` that
    // is `src-tauri`, and a session silently launched there is a session the
    // person never chose and cannot see they got (#20).
    let cwd = match account.cwd.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            return Err(format!(
                "Account \"{name}\" has no working directory. Set one before starting a session."
            ))
        }
    };
    if !cwd.is_dir() {
        return Err(format!(
            "Account \"{name}\" points at a missing directory: {}",
            cwd.display()
        ));
    }

    // One account, one seat per room (`RoomSeats`). Claimed before anything is
    // written or spawned, so a refusal costs nothing and leaves nothing behind.
    seats.claim(&account.id, &pty_state).map_err(|()| {
        format!(
            "Account \"{name}\" already holds a seat in this room. One account holds one seat \
             per room: stop its running session before starting it again."
        )
    })?;

    match launch(
        app, &room, pty_state, &account, &name, character, &room_url, &cwd, cols, rows,
    ) {
        Ok(started) => {
            // The launch's own values, not the account's. The account may be
            // edited while this runs, and what is running would then be
            // reported as whatever was typed into the form afterwards.
            seats.hold(
                &account.id,
                RunningSession {
                    pty_id: started.pty_id.clone(),
                    started_at: started.started_at.clone(),
                    command: account.command.clone(),
                    cwd: cwd.to_string_lossy().to_string(),
                },
            );
            Ok(started)
        }
        Err(err) => {
            // Nothing is running, so nothing holds the seat. Without this the
            // account would stay locked out by a launch that never happened.
            seats.release(&account.id);
            Err(err)
        }
    }
}

/// Write the registration and spawn the CLI, with the seat already claimed.
///
/// Split out so the seat has exactly one release point: every failure from here
/// down leaves the account seatless, and the caller does not have to remember
/// that at each `?`.
#[allow(clippy::too_many_arguments)]
fn launch(
    app: AppHandle,
    room: &RoomState,
    pty_state: tauri::State<PtyState>,
    account: &Account,
    name: &str,
    character: Option<&str>,
    room_url: &str,
    cwd: &Path,
    cols: u16,
    rows: u16,
) -> Result<StartedSession, String> {
    let (sidecar_entry, sidecar_runner) = resolve_sidecar_paths()?;
    // Keyed on the account id, so two accounts launched into one working
    // directory write two entries instead of overwriting each other's identity
    // (#40), and renaming an account does not move the key out from under the
    // session running on it (#53).
    let server_name = server_name_for(&account.id);
    let mcp_config = register_sidecar(
        cwd,
        &RoomRegistration {
            room_url,
            token: &room.token(),
            account_id: &account.id,
            agent_name: name,
            agent_hue: account.hue,
            sidecar_entry: &sidecar_entry,
            sidecar_runner: &sidecar_runner,
        },
    )?;

    let started_at = crate::room::now_iso();
    let pty_id = pty::spawn_pty(
        app,
        pty_state,
        account.command.clone(),
        // The same function the preview goes through, so what the form showed
        // is what spawns. Nothing is written for the character: `--settings`
        // takes the JSON inline, and a file per account would grow the very
        // directory this account is sharing (#99).
        launch_args(&account.args, &server_name, character),
        cols,
        rows,
        Some(cwd.to_string_lossy().to_string()),
    )?;

    Ok(StartedSession {
        pty_id,
        mcp_config: mcp_config.to_string_lossy().to_string(),
        started_at,
    })
}
