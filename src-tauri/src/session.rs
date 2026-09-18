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
use crate::room_log::{self, TopicRef};
use mcp_config::{
    carried_launch_options, console_safe, declared_character, declares_session_id,
    declares_settings, launch_args, limited_hook_url, other_room_servers, register_sidecar,
    reject_incompatible_flags, server_name_for, session_id_launch_args, split_launch_options,
    status_hook_url, status_line_command, substitute_session_id, Cli, RoomRegistration,
    ROOM_TOKEN_ENV,
};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

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

/// The status-line script, which ships beside the sidecar entry point (#155).
///
/// Derived from the one walk above rather than looked for by a second one, and
/// with no override of its own: an override that pointed the sidecar somewhere
/// else and left this behind would be two halves of one distribution in two
/// places.
///
/// `None` when the file is not there. A launch is not refused for it — the
/// statusLine simply does not go on the line, and the panel's five values read
/// `—` for that seat.
fn status_script(entry: &Path) -> Option<PathBuf> {
    let script = entry.parent()?.join("status.mjs");
    script.is_file().then_some(script)
}

/// This seat's status-line command, given the sidecar entry it ships beside.
///
/// Two ways to have nothing: the script is not where the distribution puts it,
/// or its own path holds a character a shell on the way acts on
/// (`mcp_config::line_safe_word`). Both are the same answer here, because both
/// leave the same rows reading `—`.
fn status_command_beside(
    entry: &Path,
    port: u16,
    topic_id: &str,
    account_id: &str,
) -> Option<String> {
    status_line_command(
        &status_script(entry)?,
        &status_hook_url(port, topic_id, account_id),
    )
}

/// The same command for a caller that is not holding a resolved sidecar — the
/// preview, which answers for a launch that has not happened.
///
/// A third way to have nothing joins the two above: a room with no port to
/// address, and a tree the sidecar cannot be found in at all. The launch
/// resolves the sidecar for itself and passes it in, so the line it spawns and
/// the line the form shows cannot come from two different walks.
fn status_command(port: Option<u16>, topic_id: &str, account_id: &str) -> Option<String> {
    let (entry, _) = resolve_sidecar_paths().ok()?;
    status_command_beside(&entry, port?, topic_id, account_id)
}

/// Which seat each account is holding, in which room.
///
/// **One account, one seat per room.** An account is who someone is, and the
/// same someone cannot be in one room twice: two connections under one account
/// would put one identity in the roster twice, and a post addressed to that
/// name would have two places to land.
///
/// **The rule is scoped to the room, not to the account, and rooms are plural
/// now (#141).** A room is a topic, and the key here is the pair — the topic a
/// launch goes into, and the account launched. One account holding a seat in
/// each of two topics is the intended shape and not a violation: a session left
/// running in the topic the screen moved away from, and the same account
/// started again in the topic the screen moved to (decisions 2 and 3). What is
/// still refused is the same account twice in the same topic.
///
/// That scope was written down here while there was one room, precisely so a
/// rule remembered as "an account runs once" would not outlive its reason and
/// block this case. It did not: the key gained its room and the refusal kept
/// its meaning.
#[derive(Clone)]
pub struct RoomSeats {
    seats: Arc<Mutex<BTreeMap<SeatKey, Seat>>>,
}

/// Which seat: the topic it is in, then the account holding it.
///
/// Topic first, so every seat of one topic sits together in the map — which is
/// the order a topic delete reads them in (`running_in_topic`).
type SeatKey = (String, String);

fn seat_key(topic_id: &str, account_id: &str) -> SeatKey {
    (topic_id.to_string(), account_id.to_string())
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
    /// The topic this session was started into, which is the room it is in.
    ///
    /// Which topic a session belongs to is a fact about the launch, not about
    /// the account and not about the screen now: the screen moves between
    /// topics while a session keeps running in its own (#141, decision 2).
    /// Half of the seat's key, and carried here as well so the screen can put
    /// the session's terminal under its topic. Deleting a topic ends the
    /// sessions that were in it and nothing else (#119, decision 4) — read off
    /// the topic's `sessions` map instead, a delete would reach an account that
    /// ran in this topic once and is now running in another one.
    ///
    /// A seat still being claimed (`Seat::Starting`) has no entry here at all,
    /// which is the one session a delete cannot see; see the accepted tradeoff
    /// in docs/0-requirements.md.
    pub topic_id: String,
    /// The session id this launch went back into, or `None` when it started
    /// fresh.
    ///
    /// Which of the two lines ran is a fact about the launch, like the three
    /// above, and it is kept here for the reason the pty id is: the screen
    /// loses it on a reload and the seat does not. What reads it is the exit —
    /// a resume that ends without the room ever having seen it leaves a record
    /// with no conversation behind it, and dropping that record is what puts
    /// the next launch back on the normal line (#127).
    pub resumed_from: Option<String>,
}

/// One account's seat, as the screen reads it.
#[derive(Clone, serde::Serialize)]
pub struct SeatedAccount {
    pub account_id: String,
    /// The topic the seat is in.
    ///
    /// On the seat rather than only on the session under it: a seat whose
    /// launch is still in flight has no session yet, and it is still a seat in
    /// one topic and not in another (#141).
    pub topic_id: String,
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
            .map(|((topic_id, account_id), seat)| SeatedAccount {
                account_id: account_id.clone(),
                topic_id: topic_id.clone(),
                session: match seat {
                    Seat::Starting => None,
                    Seat::Running(session) => Some(session.clone()),
                },
            })
            .collect()
    }

    /// Claim the seat for `account_id` in `topic_id`, or fail because it is
    /// taken.
    ///
    /// The sweep and the claim are one acquisition of the lock: checking first
    /// and claiming after would let two launches pass the same empty seat.
    fn claim(&self, topic_id: &str, account_id: &str, ptys: &PtyState) -> Result<(), ()> {
        let key = seat_key(topic_id, account_id);
        let mut seats = self.seats.lock();
        let taken = match seats.get(&key) {
            Some(Seat::Starting) => true,
            Some(Seat::Running(session)) => ptys.is_running(&session.pty_id),
            None => false,
        };
        if taken {
            return Err(());
        }
        seats.insert(key, Seat::Starting);
        Ok(())
    }

    /// The PTY of every seat running in one topic.
    ///
    /// The sweep and the read are one acquisition, like `seated`: a seat whose
    /// session has already exited is not a session to end, and the sweep is
    /// what says so.
    ///
    /// A seat still starting is not here and cannot be: it holds no PTY yet, so
    /// there is nothing to end. That is the window the delete cannot close, and
    /// it is written down rather than papered over (docs/0-requirements.md,
    /// 受容したトレードオフ).
    pub fn running_in_topic(&self, topic_id: &str, ptys: &PtyState) -> Vec<String> {
        let mut seats = self.seats.lock();
        seats.retain(|_, seat| match seat {
            Seat::Starting => true,
            Seat::Running(session) => ptys.is_running(&session.pty_id),
        });
        seats
            .iter()
            .filter(|((seat_topic, _), _)| seat_topic == topic_id)
            .filter_map(|(_, seat)| match seat {
                Seat::Starting => None,
                Seat::Running(session) => Some(session.pty_id.clone()),
            })
            .collect()
    }

    /// The launch got a session up; the seat is now held by that session.
    fn hold(&self, account_id: &str, session: RunningSession) {
        let key = seat_key(&session.topic_id, account_id);
        self.seats.lock().insert(key, Seat::Running(session));
    }

    /// The launch failed. Nothing is running, so nothing holds the seat.
    fn release(&self, topic_id: &str, account_id: &str) {
        self.seats.lock().remove(&seat_key(topic_id, account_id));
    }
}

/// The seats held in every room, for the screen to draw against its own list of
/// accounts.
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

/// What a launch would do with each value the form holds (#154, 決定4).
///
/// Four answers rather than one, because what is at stake differs per field:
/// three of them cost what they name and the fourth costs the launch. The form
/// says which before 決定, and saving is still allowed — the person is the one
/// who can tell whether a name they meant is worth rewriting.
#[derive(serde::Serialize)]
pub struct LaunchFieldReport {
    /// The character will be selected, or the session speaks as the working
    /// directory's own default.
    pub character: bool,
    /// The launch options are on the line, or none of them are.
    pub options: bool,
    /// The account's command can be written onto the line at all.
    pub command: bool,
    /// The resume line can be, or there is none.
    pub resume: bool,
}

/// Answer for the four, through the same functions the launch goes through.
///
/// The judgment stays in Rust for the reason the splitter does
/// (`parse_launch_options`): a second copy in TypeScript is one that drifts
/// from the tested one, and this one is the character set two of these fields
/// are refused by. The sentences are the screen's — they are read in Japanese,
/// and which of them to show is what these four booleans decide.
#[tauri::command]
pub fn launch_field_report(
    character: Option<String>,
    options: String,
    command: String,
    resume: Option<String>,
) -> LaunchFieldReport {
    let character = character.unwrap_or_default();
    let character = character.trim();
    let options = split_launch_options(&options);
    let resume = split_launch_options(resume.as_deref().unwrap_or_default());
    LaunchFieldReport {
        // Blank declares no character, and declaring none is not a refusal —
        // the working directory's own default is an answer (#99).
        character: character.is_empty() || declared_character(Some(character)).is_some(),
        // As many as were written, since the set is carried whole or not at all.
        options: carried_launch_options(&options).len() == options.len(),
        command: console_safe(&command),
        // The whole line, because the whole line is what is refused: its
        // arguments are how it goes back, and a resume that lost them is a
        // fresh session (`start_session`).
        resume: resume.iter().all(|word| console_safe(word)),
    }
}

/// The arguments a launch would actually use, for display.
///
/// The app merges its own channel entry into what the person wrote and selects
/// the account's character on top of it, so the line they typed is not the line
/// that runs. This returns the line that runs, through the same function the
/// launch itself goes through (`launch_args`).
///
/// The entry names this account's own server in the topic the launch would go
/// into, which is a function of the account id and the topic id, so the preview
/// changes when a different account is selected and holds still while that
/// account's name is edited. Holding still is the point: the identity being
/// launched is the account, and renaming it does not make it something else
/// (#53). The topic is the one the screen names, or the one it has open when it
/// names none (#141).
///
/// The working directory is read for the same reason the character is: the
/// sibling registrations sitting in it are on the line too (#103), and a
/// directory shared with another account is the case where the person most
/// needs to see that before starting. A directory that does not exist yet, or
/// a room not listening, names nothing — the preview answers for what it can
/// see, and the launch reads the directory again for itself.
///
/// The usage-limit hook is on the line too (#149), addressed to this seat on the
/// room's port. A room not listening names no port, so the preview shows the
/// line without it — which is also the line a launch could not run at all, since
/// a launch with no port is refused below.
///
/// The kind is read for the same reason all of those are: what its CLI's
/// conventions put on the line is on the line (#156). It comes from the form
/// rather than from the saved account, because the kind is one of the things
/// being edited — and it is the field the person is most likely to be looking
/// at while they wonder what changed.
#[tauri::command]
pub fn preview_launch_args(
    room: tauri::State<RoomState>,
    args: Vec<String>,
    account_id: String,
    kind: AccountKind,
    topic_id: Option<String>,
    character: Option<String>,
    cwd: Option<String>,
) -> Vec<String> {
    let topic_id = topic_id.unwrap_or_else(|| room.topic().topic_id);
    let server_name = server_name_for(account_id.trim(), &topic_id);
    let others = room
        .port()
        .zip(cwd.as_deref().map(str::trim).filter(|dir| !dir.is_empty()))
        .and_then(|(port, dir)| {
            other_room_servers(
                Path::new(dir),
                &format!("ws://127.0.0.1:{port}"),
                &server_name,
            )
            .ok()
        })
        .unwrap_or_default();
    let hook = room
        .port()
        .map(|port| limited_hook_url(port, &topic_id, account_id.trim()));
    let status = status_command(room.port(), &topic_id, account_id.trim());
    launch_args(
        &args,
        kind.cli(),
        &server_name,
        character.as_deref(),
        &others,
        hook.as_deref(),
        status.as_deref(),
    )
}

/// The line one launch runs, resolved against the topic it is being started
/// into.
///
/// Two lines exist for one account and this picks between them. The account's
/// own command starts a session; its resume line puts it back into one it was
/// already in, and which applies is not a property of the account — it is
/// whether *this topic* holds a session for it (#115, decisions 3 and 4B).
struct LaunchLine {
    command: String,
    /// The arguments of that line, with `{session_id}` still standing where an
    /// id goes.
    ///
    /// Unfilled on purpose. What the CLI's conventions add goes on further
    /// down, inside `launch_args`, and it carries the placeholder too (#156) —
    /// so the substitution runs once over the whole composed line rather than
    /// here over half of it, and neither half can be the one that ships the
    /// placeholder to the CLI as a literal.
    args: Vec<String>,
    /// The session id this launch is handing the CLI, when it is handing one.
    ///
    /// `Some` only on a fresh launch that had somewhere to put it: the id is
    /// decided here and recorded on the topic, so the next opening of that
    /// topic has something to resume. A resume passes an id it was given and
    /// mints nothing, so it is `None` — there is nothing new to record.
    session_id: Option<String>,
    /// The session id this launch is going back into, when it is a resume.
    ///
    /// `Some` is the resume line and `None` is the launch line, so this is
    /// also the answer to which of the two ran. One field rather than a flag
    /// beside an id: the two would have to agree, and the id is what the undo
    /// needs — a resume that could not go back has to name the record it is
    /// dropping (#127).
    resumed_from: Option<String>,
    /// The session id this launch took off the topic on its way past, because
    /// the conversation it named is not on disk (#131, decision 2).
    ///
    /// A resolved line and a dropped record are not alternatives: the drop is
    /// why this line is the fresh one. Carried out so the screen can say it —
    /// the record went without the person asking, and a repair nobody is told
    /// about is a history that quietly stopped being continuous (#127).
    dropped_resume: Option<String>,
}

/// Whether the conversation this id names can be found on disk.
///
/// The check decision 1 adds to the two the resume already had (#131). Both of
/// those are records this app wrote; this one asks the file system, which is
/// the only party that knows whether the conversation was ever made.
///
/// Answers true when the file is there — and also when this app cannot tell,
/// which is a kind that names no CLI, a home directory it could not resolve, or
/// a working directory the CLI spells with a hash
/// (`mcp_config::Cli::transcript_path`). Unknown falls on the side of keeping
/// the record: the launch then goes in on the resume line exactly as it did
/// before this check existed, and #127 still takes the record off if the CLI
/// turns it away. Guessing the other way would drop a record that was fine on
/// the strength of not having looked.
///
/// The kind is the first of those three and the one #156 adds. Where a CLI
/// keeps its conversations is that CLI's own layout, so an account whose kind
/// names none is an account this app cannot look for a transcript of — not one
/// whose transcript is missing.
///
/// Nothing is read out of the file. Whether a transcript is intact is a
/// question about its contents, and this one is about whether there is anything
/// there at all (#131, 制約).
fn transcript_found(app: &AppHandle, cli: Option<Cli>, cwd: &Path, session_id: &str) -> bool {
    let Some(cli) = cli else {
        return true;
    };
    let Ok(home) = app.path().home_dir() else {
        return true;
    };
    match cli.transcript_path(&home, cwd, session_id) {
        Some(path) => path.is_file(),
        None => true,
    }
}

/// Which of the account's two lines this launch is, and with which id.
///
/// Resume needs three things, and the third is the one #131 adds: a session
/// recorded for this account in this topic, a line that knows how to go back
/// into one, and the conversation that id names still being on disk. Missing
/// any of them, the launch is a fresh one — the account then reads back what it
/// needs through the room's own pull instead, which is the second tier of the
/// two-tier answer and the reason a missing resume line is a degraded state
/// rather than a failure (#115, decision 4).
///
/// The third is not the same kind of condition as the other two. They are read
/// off records this app keeps and cost nothing to be wrong about; this one is a
/// record the CLI keeps, and being wrong about it is what #127 had to repair
/// after the fact. Checking it here is what makes the state unbuildable rather
/// than recoverable: a line back into a conversation that is not there is never
/// chosen, so there is nothing to fall back from.
///
/// **A record with no conversation behind it is dropped where it is found**
/// (#131, decision 2), rather than left for the failure to take off. Waiting
/// would mean the next press goes in on the same line again, which is the loop
/// #127 exists at the far end of. What is dropped is named on the way out, so
/// the screen can say it happened.
///
/// A fresh launch mints an id whenever the composed line will have somewhere to
/// put one — the CLI's conventions, or a placeholder in the account's own
/// options — including when the topic already holds one. The old id is replaced
/// rather than kept: without a resume line it can never be used again, and what
/// a topic should hold is the session that is actually in it.
///
/// **Which line resumes is the kind's answer before it is the account's**
/// (#156, 決定6). A kind naming a CLI holds the way back into one of its
/// sessions, so that is the line; a kind naming none has only the field, which
/// is the one the person writes and the one the form shows for that kind.
fn resolve_launch(
    app: &AppHandle,
    account: &Account,
    topic: &TopicRef,
    cwd: &Path,
) -> Result<LaunchLine, String> {
    let cli = account.kind.cli();
    let recorded = room_log::session_of(app, &topic.topic_id, &account.id);
    let resume = match cli {
        Some(cli) => cli.resume_command(),
        None => account
            .resume_command
            .as_deref()
            .map(str::trim)
            .filter(|line| !line.is_empty()),
    };

    let mut dropped_resume = None;
    if let (Some(session_id), Some(line)) = (recorded.as_deref(), resume) {
        if transcript_found(app, cli, cwd, session_id) {
            let mut parts = split_launch_options(line);
            // The first token is the command, and a resume line whose first
            // token is empty would spawn nothing under a name the person never
            // chose. The reachable way in is a line of nothing but quotes,
            // which splits into one empty token rather than into none.
            let command = if parts.is_empty() {
                String::new()
            } else {
                parts.remove(0)
            };
            if command.is_empty() {
                return Err(format!(
                    "Account \"{}\" has a resume command with no command in it. Write the whole line, command first.",
                    account.name.trim()
                ));
            }
            return Ok(LaunchLine {
                command,
                // Still holding the placeholder: the id this line goes back
                // into is `resumed_from`, and one pass fills the composed line
                // with it (`launch`). A line filled here would also be a line
                // that no longer says where its id went, and the CLI's own
                // convention would then be added on top of it (#156).
                args: parts,
                session_id: None,
                resumed_from: Some(session_id.to_string()),
                dropped_resume: None,
            });
        }

        // The id is named, so what goes is this record and not one a relaunch
        // wrote in between (`room_log::forget_session`). Answering false is
        // that check having held, not a failure, and nothing is claimed in that
        // case. A failure to write does not fail the launch either: the record
        // stands and the next press comes back here, which is the same state
        // this arrived in — said on the log's own surface, because a record
        // that quietly stopped being kept still looks like one.
        match room_log::forget_session(app, &topic.topic_id, &account.id, session_id) {
            Ok(true) => dropped_resume = Some(session_id.to_string()),
            Ok(false) => {}
            Err(err) => room_log::report(app, err),
        }
    }

    // Asked of the line as the launch will carry it, not of the options as the
    // person wrote them: on a kind whose CLI hands the id over itself, an
    // account that writes nothing at all still has somewhere to put one (#156).
    // Options the line cannot carry are left off whole (#154), and a placeholder
    // that went with them is not somewhere to put an id — minting one anyway
    // would record on the topic a session the CLI was never handed.
    if declares_session_id(&session_id_launch_args(
        carried_launch_options(&account.args),
        cli,
    )) {
        let session_id = uuid::Uuid::new_v4().to_string();
        return Ok(LaunchLine {
            command: account.command.clone(),
            args: account.args.clone(),
            session_id: Some(session_id),
            resumed_from: None,
            dropped_resume,
        });
    }

    Ok(LaunchLine {
        command: account.command.clone(),
        args: account.args.clone(),
        session_id: None,
        resumed_from: None,
        dropped_resume,
    })
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
    /// The topic this launch went into, and the topic the record it may have to
    /// drop is on.
    ///
    /// The launch's own, named by the screen, and not whichever topic the
    /// screen has open later: the screen moves between topics while a session
    /// runs in its own (#141), and a session that ends badly has to reach the
    /// topic it started in (#127). Same fact the seat holds, sent
    /// here as well because the screen acts on the exit and the seat is gone by
    /// then.
    pub topic_id: String,
    /// The session id this went back into, or `null` when it started fresh.
    ///
    /// Handed back so the screen can say which of the two lines ran. Under
    /// decision 6 a topic opens whether or not a seat could be resumed, and a
    /// seat that came back fresh is not a failure — but it is a different thing
    /// from one that came back carrying its own context, and the person is the
    /// one who can tell whether that matters.
    ///
    /// The id itself rides rather than a flag, because the screen has one more
    /// thing to do with it: a resume that ends without the room ever seeing it
    /// is a record with nothing behind it, and dropping that record names it
    /// (#127).
    pub resumed_from: Option<String>,
    /// The session id this launch took off the topic before starting, because
    /// the conversation it named is not on disk, or `null` when it took none.
    ///
    /// Never set together with `resumed_from`: the drop is what made this the
    /// fresh line. The screen says it for the same reason it says the one #127
    /// drops — the way back into a conversation went, and nobody asked for that
    /// (#131, decision 2).
    pub dropped_resume: Option<String>,
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
///
/// Sharing the directory also means sharing its `.mcp.json`, which holds one
/// room registration per account. The CLI starts every enabled server it finds
/// there, so this launch has to name the siblings it is not — otherwise it
/// spawns their sidecars too and the room lists them twice (#103).
#[tauri::command]
pub fn start_session(
    app: AppHandle,
    room: tauri::State<RoomState>,
    pty_state: tauri::State<PtyState>,
    seats: tauri::State<RoomSeats>,
    account: Account,
    topic_id: String,
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

    // Which topic this seat is being started into: the one the screen pressed
    // ▶ in, named by it. Not the topic the app has open by the time this runs
    // — the screen made this session's terminal under the topic it had on the
    // glass, and a launch that went wherever the app had moved to since would
    // put a session in one topic and its terminal in another (#141). Resolved
    // against the rooms this run holds, so a topic deleted in between is a
    // refusal rather than a session in nothing.
    let topic = room.topic_of(&topic_id).ok_or_else(|| {
        format!("トピック {topic_id} はこのアプリで開かれていません。削除されたトピックかもしれません。")
    })?;

    // No fallback to the app's own process directory. Under `tauri dev` that
    // is `src-tauri`, and a session silently launched there is a session the
    // person never chose and cannot see they got (#20).
    //
    // Resolved ahead of the line rather than beside the rest of the launch's
    // materials, because the line is now resolved against it: the transcript of
    // a session is filed under the directory it ran in, so there is no reading
    // the topic's record without one. A directory that is missing or unset ends
    // the launch here, which is also what keeps a record from being dropped on
    // the strength of a directory nobody could have launched in (#131).
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

    // Resume or fresh, decided here so every check below runs against the line
    // that will actually be spawned. Checking the account's own options and
    // then spawning the resume line would be checking the wrong line.
    let launch_line = resolve_launch(&app, &account, &topic, &cwd)?;

    // Whether this seat is being taken in front of posts it does not have.
    //
    // Read off the line that resolved, which is why it is here and not beside
    // the topic: the question is not whether a record was dropped but whether
    // this launch went in on the resume line at all. A seat entering a topic
    // that has been spoken in for the first time is as blind as one whose way
    // back went, and `dropped_resume` is one case of the state rather than the
    // state (#133).
    //
    // Whether, not how much. A count would mean reading the topic on every
    // launch, and the session does not need one to decide whether to look —
    // which is the decision, and stays the session's (#115, decision 4C).
    // Nothing about the posts crosses here: the room pushes no past, and what
    // is handed over is that there is some.
    let unseen_history = launch_line.resumed_from.is_none()
        && room_log::topic_has_posts(&app, &topic.topic_id);

    // The one value on this line with no way of being left off (#154, 決定3).
    // A character the line cannot carry costs the character and launch options
    // it cannot carry cost the options, because in both cases there is a line
    // left to run; a command that cannot be written onto the line leaves none.
    // Said in the language it is read in: this reaches the screen whole, inside
    // a Japanese sentence (`main.ts`, `startSession`).
    if !console_safe(&launch_line.command) {
        return Err(format!(
            "「{name}」の起動コマンド `{}` には、Windows の起動の行に載せられない文字があります（`& | < > ^ ( ) \"`）。この行は `cmd.exe` を通るため、そこでコマンドが途中で切れます。",
            launch_line.command
        ));
    }
    // A resume line is one line the person wrote, and its arguments are how it
    // goes back: dropping them the way launch options are dropped would leave
    // the resume command starting a fresh session instead, which is the shape
    // this app refuses everywhere — a launch that quietly did half of what was
    // asked looks like it worked.
    if launch_line.resumed_from.is_some() && !launch_line.args.iter().all(|arg| console_safe(arg)) {
        return Err(format!(
            "「{name}」の再開コマンドには、Windows の起動の行に載せられない文字があります（`& | < > ^ ( ) \"`）。載せずに起動すれば戻る先へ戻らないため、この起動は行いません。"
        ));
    }

    if let Err(flag) = reject_incompatible_flags(&launch_line.args) {
        return Err(format!(
            "Account \"{name}\" passes {flag}, which stops channel pushes from arriving. \
             Remove it from the launch options."
        ));
    }

    let character = declared_character(account.character.as_deref());

    let port = room
        .port()
        .ok_or_else(|| "The room socket is not listening yet.".to_string())?;
    let room_url = format!("ws://127.0.0.1:{port}");

    let server_name = server_name_for(&account.id, &topic.topic_id);
    // Read before anything is written, and answering for the file as the
    // registration below will leave it. The CLI starts every enabled server in
    // the working directory's `.mcp.json`, and a directory shared with another
    // account holds that account's entry by design (#40) — so without this the
    // session would spawn the sibling's sidecar too, and the room would list
    // that sibling twice (#103).
    let others = other_room_servers(&cwd, &room_url, &server_name)?;

    // The character and the sibling registrations both ride in `--settings`,
    // so launch options carrying their own `--settings` are on the same axis as
    // either. Refused rather than resolved: which of two copies of one flag a
    // CLI reads is not something this app has established, and picking one here
    // would be a guess that surfaces as the wrong character speaking (#99) or
    // as the sibling's sidecar started anyway (#103). Refused rather than
    // stripped, for the reason the flag guard above is: a launch that quietly
    // dropped half of what was asked for looks like it worked.
    //
    // The approval of this launch's own server rides in `--settings` too
    // (#143), and does not widen this condition: a line with its own
    // `--settings` and nothing else to declare is left untouched by
    // `launch_args` rather than refused, so a line that ran before still runs.
    if (character.is_some() || !others.is_empty()) && declares_settings(&launch_line.args) {
        // Two refusals rather than one sentence with a hole in it: what the
        // person can do about it differs. A character is theirs to clear; a
        // sibling registration is another account's, and the way out of that
        // one is a directory of this account's own.
        return Err(if character.is_some() {
            format!(
                "Account \"{name}\" declares a character and also passes --settings in its \
                 launch options. Both name the same setting. Clear one of them."
            )
        } else {
            format!(
                "Account \"{name}\" passes --settings in its launch options, and its working \
                 directory holds another account's room registration that this session has to \
                 keep from starting. Both name the same setting. Remove the --settings, or give \
                 this account a working directory of its own."
            )
        });
    }

    // One account, one seat per room (`RoomSeats`). Claimed before anything is
    // written or spawned, so a refusal costs nothing and leaves nothing behind.
    seats.claim(&topic.topic_id, &account.id, &pty_state).map_err(|()| {
        format!(
            "Account \"{name}\" already holds a seat in this topic. One account holds one seat \
             per topic: stop its running session here before starting it again."
        )
    })?;

    match launch(
        app.clone(),
        &room,
        pty_state,
        &account,
        &launch_line,
        &name,
        character,
        &others,
        &server_name,
        &room_url,
        port,
        &cwd,
        &topic.topic_id,
        unseen_history,
        cols,
        rows,
    ) {
        Ok(started) => {
            // On the topic, so the next opening of it can resume this session.
            // Recorded after the spawn rather than before: an id written for a
            // launch that failed would be resumed into a session that was never
            // started.
            //
            // A failure here does not fail the launch. The session is running —
            // what is lost is the ability to resume it later, and the room's own
            // pull still stands for that topic (#115, decision 4C). It is said
            // on the same surface a failed append is said on, for the same
            // reason: a record that quietly stopped being kept still looks like
            // one.
            //
            // A launch with no id to record still puts its topic in the index.
            // The session stays in this topic whatever the screen opens next
            // (#141, decision 2), and a topic the list does not carry is a
            // running session nothing on the screen leads back to.
            let recorded = match &launch_line.session_id {
                Some(session_id) => {
                    room_log::record_session(&app, &topic, &account.id, session_id)
                }
                None => room_log::realize_topic(&app, &topic),
            };
            if let Err(err) = recorded {
                room_log::report(&app, err);
            }
            // The launch's own values, not the account's. The account may be
            // edited while this runs, and what is running would then be
            // reported as whatever was typed into the form afterwards.
            seats.hold(
                &account.id,
                RunningSession {
                    pty_id: started.pty_id.clone(),
                    started_at: started.started_at.clone(),
                    // The line that ran, which on a resume is not the account's
                    // launch command at all.
                    command: launch_line.command.clone(),
                    cwd: cwd.to_string_lossy().to_string(),
                    // The topic read at the top of this launch, not the room's
                    // current one: the two are the same here, and reading the
                    // room again would make them the same only by luck.
                    topic_id: topic.topic_id.clone(),
                    // The line that ran, for the same reason the command is:
                    // this is the launch's own fact, and the seat is where it
                    // survives a reload of the screen (#127).
                    resumed_from: launch_line.resumed_from.clone(),
                },
            );
            Ok(started)
        }
        Err(err) => {
            // Nothing is running, so nothing holds the seat. Without this the
            // account would stay locked out by a launch that never happened.
            seats.release(&topic.topic_id, &account.id);
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
    // Command and arguments as resolved against the topic, so what is spawned
    // is what was checked.
    line: &LaunchLine,
    name: &str,
    character: Option<&str>,
    // The sibling registrations this session must not start, read from the
    // working directory before this function writes into it.
    others: &[String],
    // Keyed on the account id, so two accounts launched into one working
    // directory write two entries instead of overwriting each other's identity
    // (#40), and renaming an account does not move the key out from under the
    // session running on it (#53).
    server_name: &str,
    room_url: &str,
    // The port `room_url` names, as the caller read it. Read again here it
    // could be a different run's, and the hook on the line would then be
    // addressed somewhere other than the room this session was registered into.
    room_port: u16,
    cwd: &Path,
    // The topic this launch is going into, carried through so the answer names
    // the topic a failed resume would have to be undone on (#127).
    topic_id: &str,
    // Whether the topic already holds posts this session was not seated with,
    // decided by the caller against the line that resolved (#133).
    unseen_history: bool,
    cols: u16,
    rows: u16,
) -> Result<StartedSession, String> {
    let (sidecar_entry, sidecar_runner) = resolve_sidecar_paths()?;
    let mcp_config = register_sidecar(
        cwd,
        &RoomRegistration {
            room_url,
            token: &room.token(),
            account_id: &account.id,
            room_id: topic_id,
            agent_name: name,
            agent_hue: account.hue,
            unseen_history,
            sidecar_entry: &sidecar_entry,
            sidecar_runner: &sidecar_runner,
        },
    )?;

    let started_at = crate::room::now_iso();
    // Where this seat's usage limit is reported, and it is this seat's own: the
    // address carries the topic and the account, because the CLI's hook input
    // carries neither (#149, decision 3).
    let hook = limited_hook_url(room_port, topic_id, &account.id);
    // What this session reports about itself while it runs, addressed to the
    // same seat on the same port (#155, decision 2). Absent leaves the panel's
    // five values at `—` and stops nothing else. Beside the sidecar this launch
    // resolved, not beside one a second walk found.
    let status = status_command_beside(&sidecar_entry, room_port, topic_id, &account.id);
    // The same function the preview goes through, so what the form showed is
    // what spawns. Nothing is written for the settings: `--settings` takes the
    // JSON inline, and a file per account would grow the very directory this
    // account is sharing (#99).
    let composed = launch_args(
        &line.args,
        account.kind.cli(),
        server_name,
        character,
        others,
        Some(&hook),
        status.as_deref(),
    );
    // The id goes in last, over the whole line. The account's own options may
    // name where it goes and the CLI's conventions may have put it there too
    // (#156), and one pass over the composed line fills both — filling either
    // half earlier leaves the other half handing the CLI the placeholder as a
    // literal. Exactly one of the two ids is set: a fresh launch mints one, a
    // resume carries the one it is going back into.
    let composed = match line.session_id.as_deref().or(line.resumed_from.as_deref()) {
        Some(session_id) => substitute_session_id(&composed, session_id),
        None => composed,
    };
    let pty_id = pty::spawn_pty_with_env(
        app,
        pty_state,
        line.command.clone(),
        composed,
        // The token the hook presents, in the environment rather than in the
        // header on the line: the line is drawn on screen, and the token is
        // what makes the room this room (#149).
        &[(ROOM_TOKEN_ENV, room.token())],
        cols,
        rows,
        Some(cwd.to_string_lossy().to_string()),
    )?;

    Ok(StartedSession {
        pty_id,
        mcp_config: mcp_config.to_string_lossy().to_string(),
        started_at,
        topic_id: topic_id.to_string(),
        resumed_from: line.resumed_from.clone(),
        dropped_resume: line.dropped_resume.clone(),
    })
}
