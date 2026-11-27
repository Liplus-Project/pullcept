//! The MCP servers the app runs itself, and the window each one is edited in
//! (#172; the window of the server's `mcp` account since #193).
//!
//! The servers are listed in `mcp-servers.json` in the app's data directory
//! (`mcp_servers::FILE_NAME`), in Claude Desktop's `mcpServers` shape. When
//! there is no file, one is written holding the bundled webhook bridge (#169)
//! the way the app ran it before there was a file, so a first start behaves as
//! the last one did.
//!
//! Each listed server is started when the app starts and run by the receiver in
//! `webhook.rs`, which is the one client the app has: it reads a server's
//! channel pushes into the rooms, as posts of the server's account (#193). It
//! marks nothing processed (#180). This file holds what is around that — which
//! run is current, what state it is in, and its log — and the commands the
//! account window, and the participant list's row for the account, reach it
//! through.
//!
//! **Each server is an account of kind `mcp`** (#193). The account is in
//! `config.json` and names its entry here as `server`; the entry stays in this
//! file, which is still where the server is described. An entry with no
//! account is given one as the config is read (`mcp_servers::migrate_accounts`,
//! from `config::load_config`), which is the migration from before there were
//! such accounts, and a server added to the file by hand later on. The account
//! form makes one too (#200): `create_mcp_server` writes the entry the new
//! account answers to, and `delete_mcp_server` takes it out again and ends the
//! run when that account is deleted.
//!
//! **An edit takes effect when that server is restarted, not when it is
//! saved** (#172, AI 判断). The window puts 再起動 beside 保存 and says when the
//! running server was started from something other than what the file now
//! holds. Nothing restarts a server on its own: a server that ended stays ended
//! until someone presses the button, a room seat restarts it
//! (`restart_mcp_server` in the sidecar, #620: `room_restart` below), or the app
//! is started again, for the reason `webhook.rs` gives for not restarting the
//! bridge.
//!
//! What the file holds, how the window's text is read, and which accounts the
//! entries are given are in the `mcp-servers` crate, where they are tested.

use crate::room::RoomState;
use mcp_servers::{Log, LogLine, Server, FILE_NAME};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

/// Emitted with the server's name whenever its state or its log changes. The
/// screen reads the whole view again on it: the account's row says the state,
/// and an open window draws the log.
const CHANGED_EVENT: &str = "mcp-servers-changed";

/// Where one run of a server is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    /// Spawned; `initialize` not answered yet.
    Starting,
    /// `initialize` answered.
    Running,
    /// It was running, and the process or its pipe ended.
    Ended { detail: String },
    /// It never got to running: it would not start, or refused `initialize`.
    Failed { detail: String },
    /// Stopped by 再起動 with no entry left in the file to start again.
    Stopped,
}

#[derive(Default)]
struct Run {
    /// Which start this is. A line or a state from an earlier one — a process
    /// that was replaced still has a pipe draining — carries an older number and
    /// is dropped.
    generation: u64,
    /// What the current run was started from, to tell the window when the file
    /// has moved on from it.
    started_with: Option<Server>,
    state: Option<RunState>,
    /// Kept across runs, so what the last run said before it ended is still
    /// there once the next one has started.
    log: Log,
    task: Option<JoinHandle<()>>,
    observer: Option<Value>,
}

#[derive(Default)]
struct Inner {
    runs: BTreeMap<String, Run>,
    next_generation: u64,
}

/// Every server the app has started, by name.
#[derive(Clone, Default)]
pub struct McpServers {
    inner: Arc<Mutex<Inner>>,
}

/// What one run reports back through. Bound to that run's generation, so a run
/// that has been replaced can no longer write over the one that replaced it.
#[derive(Clone)]
pub struct Reporter {
    app: AppHandle,
    servers: McpServers,
    name: String,
    generation: u64,
}

impl Reporter {
    /// The name of the entry this run was started from — what the server's
    /// account names as its `server` (#193).
    pub fn server(&self) -> &str {
        &self.name
    }

    /// Add a line to this server's log. Also said on the app's own stderr, which
    /// is where these lines went before there was a panel.
    pub fn log(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        eprintln!("[{}] {text}", self.name);
        let current = self.with_current(|run| run.log.push(now_ms(), text));
        if current {
            let _ = self.app.emit(CHANGED_EVENT, &self.name);
        }
    }

    /// `initialize` was answered.
    pub fn observer(&self, value: Value) {
        if self.with_current(|run| run.observer = Some(value)) { let _ = self.app.emit(CHANGED_EVENT, &self.name); }
    }
    pub fn running(&self) {
        if self.with_current(|run| run.state = Some(RunState::Running)) {
            let _ = self.app.emit(CHANGED_EVENT, &self.name);
        }
    }

    fn finish(&self, detail: String) {
        self.log(&detail);
        let current = self.with_current(|run| {
            run.state = Some(match run.state {
                Some(RunState::Running) => RunState::Ended { detail },
                _ => RunState::Failed { detail },
            });
            run.task = None;
        });
        if current {
            let _ = self.app.emit(CHANGED_EVENT, &self.name);
        }
    }

    fn with_current(&self, apply: impl FnOnce(&mut Run)) -> bool {
        let mut inner = self.servers.inner.lock();
        match inner.runs.get_mut(&self.name) {
            Some(run) if run.generation == self.generation => {
                apply(run);
                true
            }
            _ => false,
        }
    }
}

impl McpServers {
    pub fn safe_status(&self) -> Vec<Value> {
        self.inner.lock().runs.iter().map(|(name,run)| {
            let state = match run.state { Some(RunState::Starting) => "starting", Some(RunState::Running) => "running", Some(RunState::Ended{..}) => "ended", Some(RunState::Failed{..}) => "failed", _ => "stopped" };
            serde_json::json!({"server":name,"connection":state,"generation":run.generation,
                "pending": if state == "running" { run.observer.clone().unwrap_or_else(|| serde_json::json!({"state":"unobserved"})) } else { serde_json::json!({"state":"unobserved"}) }})
        }).collect()
    }
    pub fn new() -> Self {
        Self::default()
    }

    /// Start one server under a name, ending whatever run that name had.
    ///
    /// The run it replaces is ended by aborting its task, which drops the
    /// process handle; the handle is `kill_on_drop` (`webhook.rs`).
    fn start(&self, app: &AppHandle, name: &str, server: Server) {
        let _ = self.start_checked(app, name, server, None);
    }

    /// `start`, refused when `expected` is given and is not the generation the
    /// name holds now (0 for none) — checked under the same lock the new
    /// generation is taken under, so two room restarts of one read restart it
    /// once (#620). Answers the new run's generation.
    fn start_checked(&self, app: &AppHandle, name: &str, server: Server, expected: Option<u64>) -> Result<u64, String> {
        let generation = {
            let mut inner = self.inner.lock();
            if let Some(expected) = expected {
                let current = inner.runs.get(name).map(|run| run.generation).unwrap_or(0);
                mcp_servers::room_control::check_generation(expected, current)?;
            }
            inner.next_generation += 1;
            let generation = inner.next_generation;
            let run = inner.runs.entry(name.to_string()).or_default();
            if let Some(task) = run.task.take() {
                task.abort();
            }
            run.generation = generation;
            run.started_with = Some(server.clone());
            run.state = Some(RunState::Starting);
            run.observer = None;
            generation
        };
        let reporter = Reporter {
            app: app.clone(),
            servers: self.clone(),
            name: name.to_string(),
            generation,
        };
        reporter.log(&format!("起動: {}", command_line(&server)));

        let room = app.state::<RoomState>().inner().clone();
        let task_app = app.clone();
        let task_reporter = reporter.clone();
        let task = tauri::async_runtime::spawn(async move {
            let detail = match crate::webhook::receive(&task_app, &room, &server, &task_reporter).await
            {
                Ok(detail) | Err(detail) => detail,
            };
            task_reporter.finish(detail);
        });

        let mut inner = self.inner.lock();
        match inner.runs.get_mut(name) {
            // Still the current run, and not already finished: keep the handle
            // so a restart can end it.
            Some(run) if run.generation == generation && run.state_is_live() => {
                run.task = Some(task)
            }
            // Finished before the handle got here, in which case aborting it is
            // nothing; or replaced already by a restart that ran between the
            // spawn and this lock, which found no handle to abort. Left running,
            // that run would go on posting into the rooms beside the one that
            // replaced it, so it is ended here.
            _ => task.abort(),
        }
        Ok(generation)
    }

    /// The generation and state a name holds now: `(0, None)` for one this app
    /// run has not started.
    fn current(&self, name: &str) -> (u64, Option<RunState>) {
        self.inner
            .lock()
            .runs
            .get(name)
            .map(|run| (run.generation, run.state.clone()))
            .unwrap_or((0, None))
    }

    /// End a server's current run without starting another.
    fn stop(&self, app: &AppHandle, name: &str) {
        let stopped = {
            let mut inner = self.inner.lock();
            inner.next_generation += 1;
            let generation = inner.next_generation;
            match inner.runs.get_mut(name) {
                Some(run) => {
                    if let Some(task) = run.task.take() {
                        task.abort();
                    }
                    run.generation = generation;
                    run.started_with = None;
                    run.state = Some(RunState::Stopped);
                    run.log.push(now_ms(), "停止: 設定ファイルにこのサーバがありません");
                    true
                }
                None => false,
            }
        };
        if stopped {
            let _ = app.emit(CHANGED_EVENT, name);
        }
    }

    /// End a server's current run and drop everything held under its name, the
    /// log included (#200).
    ///
    /// For a server whose account is deleted: nothing names it any more, and a
    /// server made later under the same name is another server, which does not
    /// begin with this one's log. A report from the run ended here finds no run
    /// under the name, or one with a newer generation, and is dropped.
    fn forget(&self, app: &AppHandle, name: &str) {
        let forgotten = {
            let mut inner = self.inner.lock();
            match inner.runs.remove(name) {
                Some(mut run) => {
                    if let Some(task) = run.task.take() {
                        task.abort();
                    }
                    true
                }
                None => false,
            }
        };
        if forgotten {
            let _ = app.emit(CHANGED_EVENT, name);
        }
    }
}

impl Run {
    fn state_is_live(&self) -> bool {
        matches!(self.state, Some(RunState::Starting | RunState::Running))
    }
}

/// Start every server the file lists. Called once, as the app starts.
///
/// A file that cannot be read or written starts nothing and says why on
/// stderr; the account window says it again when it is opened. The room runs either
/// way: servers are an addition to it.
pub fn start_all(app: &AppHandle) {
    let listed = match read_file(app).and_then(|root| mcp_servers::servers(&root)) {
        Ok(listed) => listed,
        Err(err) => {
            eprintln!("[mcp] no server started: {err}");
            return;
        }
    };
    let servers = app.state::<McpServers>().inner().clone();
    for (name, server) in listed {
        servers.start(app, &name, server);
    }
}

/// The names of the servers the file lists, for the accounts that answer to
/// them (#193). Written first when there is no file, as `start_all` has it.
pub fn listed_names(app: &AppHandle) -> Result<Vec<String>, String> {
    let root = read_file(app)?;
    Ok(mcp_servers::servers(&root)?.into_keys().collect())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The line a start is logged as. For reading, not for running: the process is
/// spawned from the parts, with no shell between.
fn command_line(server: &Server) -> String {
    std::iter::once(server.command.as_str())
        .chain(server.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn file_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join(FILE_NAME))
}

/// The file's contents, writing the default first when there is no file.
///
/// The default names the bridge by the path the sidecar walk finds it at
/// (`session::webhook_bridge_script`). Where that walk finds nothing, no file
/// is written: a file listing a path that is not there would be written once
/// and then read as the person's own choice from then on.
fn read_file(app: &AppHandle) -> Result<Value, String> {
    let path = file_path(app)?;
    if !path.exists() {
        let script = crate::session::webhook_bridge_script()
            .map_err(|e| format!("{FILE_NAME} を書き出せませんでした: {e}"))?;
        let root = mcp_servers::default_file(&script.to_string_lossy());
        write_file(&path, &root)?;
        return Ok(root);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{FILE_NAME} を読めませんでした: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{FILE_NAME} を読めませんでした: {e}"))
}

fn write_file(path: &PathBuf, root: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create app data dir: {e}"))?;
    }
    let text = serde_json::to_string_pretty(root)
        .map_err(|e| format!("Failed to serialize {FILE_NAME}: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("{FILE_NAME} に書き込めませんでした: {e}"))
}

/// One server as its account's window draws it, and as the participant list
/// reads its state for the account's row (#193).
#[derive(Serialize)]
pub struct ServerView {
    name: String,
    /// False for a server that is still running under a name the file no longer
    /// lists. It has no fields to edit, and 再起動 stops it.
    listed: bool,
    command: String,
    /// One argument per line (`mcp_servers::args_text`).
    args: String,
    /// One `NAME=value` per line (`mcp_servers::env_text`).
    env: String,
    /// `None` when this app run has not started it.
    state: Option<RunState>,
    /// The file holds something other than what the current run was started
    /// from, so what the window shows is not what is running until 再起動.
    stale: bool,
    log: Vec<LogLine>,
}

#[derive(Serialize)]
pub struct PanelView {
    /// The file's path, shown beside the link that opens it.
    file: String,
    /// Why the file could not be read, when it could not.
    error: Option<String>,
    servers: Vec<ServerView>,
}

#[tauri::command]
pub fn mcp_servers(app: AppHandle, servers: State<McpServers>) -> Result<PanelView, String> {
    let file = file_path(&app)?;
    let (listed, error) = match read_file(&app).and_then(|root| mcp_servers::servers(&root)) {
        Ok(listed) => (listed, None),
        Err(err) => (BTreeMap::new(), Some(err)),
    };

    let inner = servers.inner.lock();
    let mut names: Vec<&String> = listed.keys().chain(inner.runs.keys()).collect();
    names.sort();
    names.dedup();
    let views = names
        .into_iter()
        .map(|name| {
            let entry = listed.get(name);
            let run = inner.runs.get(name);
            let started_with = run.and_then(|run| run.started_with.as_ref());
            ServerView {
                name: name.clone(),
                listed: entry.is_some(),
                command: entry.map(|s| s.command.clone()).unwrap_or_default(),
                args: entry.map(|s| mcp_servers::args_text(&s.args)).unwrap_or_default(),
                env: entry.map(|s| mcp_servers::env_text(&s.env)).unwrap_or_default(),
                state: run.and_then(|run| run.state.clone()),
                stale: matches!((entry, started_with), (Some(entry), Some(started)) if entry != started),
                log: run.map(|run| run.log.lines()).unwrap_or_default(),
            }
        })
        .collect();

    Ok(PanelView {
        file: file.to_string_lossy().to_string(),
        error,
        servers: views,
    })
}

/// Write one server's three fields into the file. The running server is not
/// touched: the edit takes effect on 再起動.
#[tauri::command]
pub fn save_mcp_server(
    app: AppHandle,
    name: String,
    command: String,
    args: String,
    env: String,
) -> Result<(), String> {
    let server = mcp_servers::server_from_fields(&command, &args, &env)?;
    let mut root = read_file(&app)?;
    mcp_servers::set_server(&mut root, &name, &server)
        .map_err(|e| format!("{FILE_NAME} に書き込めませんでした: {e}"))?;
    write_file(&file_path(&app)?, &root)
}

/// The server an account made on the form was listed under (#200), and the id
/// that account is given.
#[derive(Serialize)]
pub struct CreatedServer {
    server: String,
    account_id: String,
}

/// Write a new server into the file for an account being made on the form
/// (#200). Not started here: the screen saves the account first and then starts
/// it (`restart_mcp_server`), so the server's first post is said as the account
/// the screen has just listed, under the name and colour it was given.
///
/// The entry is named after the account (`mcp_servers::new_entry_name`), past
/// every name in use — the entries in the file, and the servers and ids of the
/// accounts in `config.json`. The account's id is taken from the entry's name,
/// as it is for an account the config gives an entry (`mcp_servers::account_id`),
/// so the two ways of arriving at an `mcp` account arrive at one shape.
#[tauri::command]
pub fn create_mcp_server(
    app: AppHandle,
    name: String,
    command: String,
    args: String,
    env: String,
) -> Result<CreatedServer, String> {
    let server = mcp_servers::server_from_fields(&command, &args, &env)?;
    let mut root = read_file(&app)?;
    let listed = mcp_servers::servers(&root)?;
    let config = crate::config::read_config(&app)?;
    let mut names: Vec<String> = listed.into_keys().collect();
    names.extend(config.accounts.iter().filter_map(|account| account.server.clone()));
    let ids: Vec<String> = config.accounts.iter().map(|account| account.id.clone()).collect();
    let entry = mcp_servers::new_entry_name(&name, &names, &ids);
    mcp_servers::set_server(&mut root, &entry, &server)
        .map_err(|e| format!("{FILE_NAME} に書き込めませんでした: {e}"))?;
    write_file(&file_path(&app)?, &root)?;
    Ok(CreatedServer {
        account_id: mcp_servers::account_id(&entry),
        server: entry,
    })
}

/// Take a server out of the file and end its run, for its account being
/// deleted (#200).
///
/// The file first: an entry left listed would be given an account again on the
/// next read of the config (`mcp_servers::migrate_accounts`), and the screen
/// removes the account only once this has answered. A file that does not exist
/// holds no entry, and is not written here only to be emptied. The run is
/// ended whether or not the file still listed it: a server already taken out
/// by hand may still be running under the account being deleted.
#[tauri::command]
pub fn delete_mcp_server(
    app: AppHandle,
    servers: State<McpServers>,
    name: String,
) -> Result<(), String> {
    let path = file_path(&app)?;
    if path.exists() {
        let mut root = read_file(&app)?;
        if mcp_servers::remove_server(&mut root, &name)
            .map_err(|e| format!("{FILE_NAME} に書き込めませんでした: {e}"))?
        {
            write_file(&path, &root)?;
        }
    }
    servers.forget(&app, &name);
    Ok(())
}

/// End a server's run and start it again from what the file holds now.
///
/// Read from the file, not from the window: what is started is what is saved,
/// and a field typed into and not saved is not either. A name the file no
/// longer lists is stopped and not started again.
#[tauri::command]
pub fn restart_mcp_server(
    app: AppHandle,
    servers: State<McpServers>,
    name: String,
) -> Result<(), String> {
    match listed_server(&app, &name)? {
        Some(server) => servers.start(&app, &name, server),
        None => servers.stop(&app, &name),
    }
    Ok(())
}

/// The entry the file lists under a name now, or `None`. What a restart starts
/// from, whether the window or a room participant asked for it.
fn listed_server(app: &AppHandle, name: &str) -> Result<Option<Server>, String> {
    let mut listed = read_file(app).and_then(|root| mcp_servers::servers(&root))?;
    Ok(listed.remove(name))
}

/// How long a restart asked for from the room waits for the new run to answer
/// `initialize` before it reports the run as still starting (#620). Under the
/// sidecar's 15 s wait for an answer, so the caller hears back either way.
const ROOM_RESTART_WAIT: Duration = Duration::from_secs(10);

/// What `notice_recipients` means, carried on every answer that lists them, so
/// the caller does not read more into the list than the app knows (#620).
const RECIPIENTS_SCOPE: &str = "notice_recipients (通知の受け取り先) = AI seats in the rooms this server's notices are posted into while it is starting or running. They are not connected to the server: Pullcept is its only MCP client. A seat's own CLI-loaded copy of a server is not seen here and is not restarted.";

fn live(state: &Option<RunState>) -> bool {
    matches!(state, Some(RunState::Starting | RunState::Running))
}

fn state_word(state: &Option<RunState>) -> &'static str {
    match state {
        None => "not_started",
        Some(RunState::Starting) => "starting",
        Some(RunState::Running) => "running",
        Some(RunState::Ended { .. }) => "ended",
        Some(RunState::Failed { .. }) => "failed",
        Some(RunState::Stopped) => "stopped",
    }
}

fn state_detail(state: &Option<RunState>) -> Option<String> {
    match state {
        Some(RunState::Ended { detail } | RunState::Failed { detail }) => Some(detail.clone()),
        _ => None,
    }
}

/// The seats a live server's notices are typed into: every AI seat in a room
/// that holds one (`room::post_notice`). Each server's notices go to the same
/// rooms, so the list is one for all live servers.
fn notice_recipients(app: &AppHandle) -> Vec<Value> {
    app.state::<RoomState>()
        .notice_recipients()
        .into_iter()
        .map(|(topic_id, seat)| serde_json::json!({ "topic_id": topic_id, "account_id": seat.account, "name": seat.name }))
        .collect()
}

/// `mcp_server_status` (#620): every server the file lists or this run has
/// started, its state, and the seats its notices are posted to. Read-only.
pub fn room_status(app: &AppHandle) -> Result<Value, String> {
    let listed = read_file(app).and_then(|root| mcp_servers::servers(&root))?;
    let recipients = notice_recipients(app);
    let runs: BTreeMap<String, (u64, Option<RunState>, Option<Server>)> = {
        let state = app.state::<McpServers>();
        let inner = state.inner.lock();
        inner.runs.iter().map(|(name, run)| (name.clone(), (run.generation, run.state.clone(), run.started_with.clone()))).collect()
    };
    let mut names: Vec<&String> = listed.keys().chain(runs.keys()).collect();
    names.sort();
    names.dedup();
    let servers: Vec<Value> = names
        .into_iter()
        .map(|name| {
            let entry = listed.get(name);
            let (generation, state, started_with) = runs.get(name).cloned().unwrap_or((0, None, None));
            let speaker = crate::config::mcp_speaker(app, name);
            serde_json::json!({
                "server": name,
                "name": speaker.name,
                "account_id": speaker.account_id,
                "listed": entry.is_some(),
                "state": state_word(&state),
                "detail": state_detail(&state),
                "generation": generation,
                "stale": matches!((entry, started_with.as_ref()), (Some(entry), Some(started)) if entry != started),
                "notice_recipients": if live(&state) { recipients.clone() } else { Vec::new() },
            })
        })
        .collect();
    Ok(serde_json::json!({
        "servers": servers,
        "notice_recipients_scope": RECIPIENTS_SCOPE,
        "retrieved_at": now_ms(),
    }))
}

/// `restart_mcp_server` from the room (#620): restart one listed server whose
/// generation is still the one the caller read, wait for the new run to settle,
/// and say who restarted what and how it went into the rooms of the seats that
/// were receiving its notices and the requester's own.
///
/// The restart is the window's (`start_checked` from what the file lists). A
/// name the file does not list is refused: restarting it would only stop it.
pub async fn room_restart(
    app: AppHandle,
    name: String,
    expected_generation: u64,
    requester: String,
    requester_topic: String,
) -> Result<Value, String> {
    use mcp_servers::room_control::{notice_text, seat_names, unlisted, Outcome};
    let server = listed_server(&app, &name)?.ok_or_else(|| unlisted(&name))?;
    let servers = app.state::<McpServers>().inner().clone();
    let (_, before) = servers.current(&name);
    let recipients = if live(&before) { notice_recipients(&app) } else { Vec::new() };
    let generation = servers.start_checked(&app, &name, server, Some(expected_generation))?;

    let deadline = Instant::now() + ROOM_RESTART_WAIT;
    let outcome = loop {
        let (current, state) = servers.current(&name);
        if current != generation {
            break Outcome::Replaced;
        }
        match state {
            Some(RunState::Running) => break Outcome::Running,
            Some(RunState::Failed { detail }) => break Outcome::Failed(detail),
            Some(RunState::Ended { detail }) => break Outcome::Ended(detail),
            Some(RunState::Starting) => {}
            // Forgotten or stopped under this generation: not this run any more.
            Some(RunState::Stopped) | None => break Outcome::Replaced,
        }
        if Instant::now() >= deadline {
            break Outcome::StillStarting;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    let speaker = crate::config::mcp_speaker(&app, &name);
    let names = seat_names(recipients.iter().filter_map(|seat| seat["name"].as_str()));
    let text = notice_text(&requester, &speaker.name, &name, &outcome, &names);
    let mut rooms: Vec<String> = vec![requester_topic];
    for topic in recipients.iter().filter_map(|seat| seat["topic_id"].as_str()) {
        if !rooms.iter().any(|room| room == topic) {
            rooms.push(topic.to_string());
        }
    }
    let room = app.state::<RoomState>();
    let posted = rooms
        .iter()
        .filter(|room_id| crate::room::post_app_notice(&app, &room, room_id, &text, None).is_some())
        .count();

    Ok(serde_json::json!({
        "server": name,
        "name": speaker.name,
        "previous_state": state_word(&before),
        "state": outcome.state(),
        "came_back": outcome.came_back(),
        "detail": outcome.detail(),
        "generation": generation,
        "notice_recipients": recipients,
        "notice_recipients_scope": RECIPIENTS_SCOPE,
        "room_notices_posted": posted,
    }))
}

/// Open the file in whatever the system opens a `.json` file with. Written
/// first when there is none, so the link always has something to open.
#[tauri::command]
pub fn open_mcp_servers_file(app: AppHandle) -> Result<(), String> {
    read_file(&app)?;
    let path = file_path(&app)?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("{FILE_NAME} を開けませんでした: {e}"))
}
