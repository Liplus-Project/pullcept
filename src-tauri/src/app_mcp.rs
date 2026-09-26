//! The MCP servers the app runs itself, and the settings panel over them (#172).
//!
//! The servers are listed in `mcp-servers.json` in the app's data directory
//! (`mcp_servers::FILE_NAME`), in Claude Desktop's `mcpServers` shape. When
//! there is no file, one is written holding the bundled webhook bridge (#169)
//! the way the app ran it before there was a file, so a first start behaves as
//! the last one did.
//!
//! Each listed server is started when the app starts and run by the receiver in
//! `webhook.rs`, which is the one client the app has: it reads a server's
//! channel pushes into the rooms and marks them processed. This file holds what
//! is around that — which run is current, what state it is in, and its log —
//! and the commands the panel reaches it through.
//!
//! **An edit takes effect when that server is restarted, not when it is
//! saved** (#172, AI 判断). The panel puts 再起動 beside 保存 and says when the
//! running server was started from something other than what the file now
//! holds. Nothing restarts a server on its own: a server that ended stays ended
//! until someone presses the button or starts the app again, for the reason
//! `webhook.rs` gives for not restarting the bridge.
//!
//! What the file holds and how the panel's text is read are in the
//! `mcp-servers` crate, where they are tested.

use crate::room::RoomState;
use mcp_servers::{Log, LogLine, Server, FILE_NAME};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

/// Emitted with the server's name whenever its state or its log changes. The
/// panel reads the whole view again on it; nothing else listens.
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
    /// What the current run was started from, to tell the panel when the file
    /// has moved on from it.
    started_with: Option<Server>,
    state: Option<RunState>,
    /// Kept across runs, so what the last run said before it ended is still
    /// there once the next one has started.
    log: Log,
    task: Option<JoinHandle<()>>,
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Start one server under a name, ending whatever run that name had.
    ///
    /// The run it replaces is ended by aborting its task, which drops the
    /// process handle; the handle is `kill_on_drop` (`webhook.rs`).
    fn start(&self, app: &AppHandle, name: &str, server: Server) {
        let generation = {
            let mut inner = self.inner.lock();
            inner.next_generation += 1;
            let generation = inner.next_generation;
            let run = inner.runs.entry(name.to_string()).or_default();
            if let Some(task) = run.task.take() {
                task.abort();
            }
            run.generation = generation;
            run.started_with = Some(server.clone());
            run.state = Some(RunState::Starting);
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
            // Finished before the handle got here, or replaced already. Nothing
            // to keep.
            _ => {}
        }
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
}

impl Run {
    fn state_is_live(&self) -> bool {
        matches!(self.state, Some(RunState::Starting | RunState::Running))
    }
}

/// Start every server the file lists. Called once, as the app starts.
///
/// A file that cannot be read or written starts nothing and says why on
/// stderr; the panel says it again when it is opened. The room runs either
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

/// One server as the panel draws it.
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
    /// from, so what the panel shows is not what is running until 再起動.
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

/// End a server's run and start it again from what the file holds now.
///
/// Read from the file, not from the panel: what is started is what is saved,
/// and a field typed into and not saved is not either. A name the file no
/// longer lists is stopped and not started again.
#[tauri::command]
pub fn restart_mcp_server(
    app: AppHandle,
    servers: State<McpServers>,
    name: String,
) -> Result<(), String> {
    let listed = read_file(&app).and_then(|root| mcp_servers::servers(&root))?;
    match listed.get(&name) {
        Some(server) => servers.start(&app, &name, server.clone()),
        None => servers.stop(&app, &name),
    }
    Ok(())
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
