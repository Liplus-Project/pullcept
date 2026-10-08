//! A Codex seat's own app-server (#299). See docs/3-accounts.md, "Codex の席を
//! app-server 経由で起動する".
//!
//! **One server per seat, started before its terminal and stopped when the
//! terminal ends.** The server runs `codex app-server` on a loopback port the
//! OS picks, with the seat's environment and working directory, the room
//! server on its line, and the token's SHA-256 as the only credential it is
//! given. The app connects as the first client, starts or resumes the thread
//! with the character as `developerInstructions`, and the real TUI attaches
//! to the same thread with `--remote`, reading the token from its own
//! environment.
//!
//! What is decided is `mcp_config::codex::app_server`, free of tauri so that
//! it is tested; what stays here is the process, the socket and the seat.
//!
//! **Stopped on every way a seat closes.** Each server is watched against its
//! terminal and taken down when the terminal is no longer running; the app's
//! close sweeps every one (`stop_all`); and on Windows each server is placed in
//! a job object that ends its whole tree if the app itself goes away first.
//!
//! **What the seat is doing is read off the same connection** (#326). Every
//! notification the server sends the app — while the thread is being started
//! or resumed (`Started::call`) and for the life of the seat after (`adopt`) —
//! is fed to the seat's `Activity` (`mcp_config::codex::activity`), and the
//! screen is told whenever what it says changes (`seat-activity`). Requests
//! the server sends are not answered: approvals stay the terminal's.
//!
//! **Every notification is also written down, without what it says** (#329):
//! one line each in `logs/codex-activity-probe.log` with the seat, the method,
//! the thread status if it carries one and the UTC time (`Probe`). The lines of
//! the start or resume are held until the seat is known and written first.

use mcp_config::codex::activity::{probe_line, probe_of, Activity, Reporter};
use mcp_config::codex::app_server as plan;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream, Message, WebSocket,
};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// How long the server has to print the port it bound.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one request may take, sent to answered.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a running server's terminal is looked at.
const WATCH: Duration = Duration::from_millis(500);
/// Probe lines held while the thread is started, before the seat is known
/// (#329). A start sends a handful; the cap only bounds a server that floods.
const PROBE_EARLY_MAX: usize = 1024;
/// The probe's file, beside `hook-probe.log` (#325).
const PROBE_FILE: &str = "codex-activity-probe.log";

/// Every running seat server, by its terminal.
#[derive(Default)]
pub struct CodexServers {
    servers: Mutex<HashMap<String, Server>>,
}

impl CodexServers {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The process tree of one seat's server.
struct Server {
    child: Child,
}

impl Server {
    fn stop(mut self) {
        stop_tree(&mut self.child);
    }
}

fn stop_tree(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// End every seat server, for the app being closed.
pub fn stop_all(app: &AppHandle) {
    let drained: Vec<Server> = app
        .state::<CodexServers>()
        .servers
        .lock()
        .drain()
        .map(|(_, server)| server)
        .collect();
    for server in drained {
        server.stop();
    }
}

/// What the seat's launch is handed: the character, the hooks to turn off,
/// the person's options split, and the thread to go back into, if any.
pub struct Request<'a> {
    pub command: &'a str,
    pub server_args: Vec<String>,
    /// The process's working directory: the account's.
    pub cwd: &'a Path,
    /// The thread's working directory: the account's after `--cd`.
    pub thread_cwd: &'a Path,
    pub env: Vec<(String, String)>,
    pub instructions: Option<&'a str>,
    pub hook_keys: &'a [String],
    pub settings: &'a plan::ThreadSettings,
    pub resume: Option<&'a str>,
    pub token: &'a str,
}

/// A server up, its thread ready, and the terminal not yet attached.
pub struct Started {
    server: Server,
    socket: Socket,
    pub url: String,
    pub thread_id: String,
    /// The thread was made by this launch, not resumed.
    pub created: bool,
    next_id: u64,
    /// What the seat is doing, fed from the first message on (#326).
    activity: Activity,
    /// Notifications that arrived before the seat was known, as the probe
    /// keeps them: method, thread status, arrival time (#329).
    probe: Vec<(String, Option<String>, String)>,
}

/// One seat's probe lines (#329): the file held open for the life of the
/// connection, each line written whole. A file that cannot be opened or
/// written is dropped silently, as the hook probe's is — a missing line is
/// what the probe reports.
struct Probe {
    file: Option<std::fs::File>,
    topic_id: String,
    account_id: String,
}

impl Probe {
    fn write(&mut self, method: &str, status: Option<&str>, at: &str) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let line = probe_line(method, status, &self.topic_id, &self.account_id, at);
        if file.write_all(format!("{line}\n").as_bytes()).is_err() {
            self.file = None;
        }
    }

    fn note(&mut self, message: &Value) {
        if let Some((method, status)) = probe_of(message) {
            self.write(&method, status.as_deref(), &crate::room::now_iso());
        }
    }
}

/// The seat a reader tells the screen about (#326). What is sent and when —
/// what the seat is doing, and whether its server can still be heard — is
/// `Reporter`'s, which is tested; this only carries it as `seat-activity`.
struct Seat {
    app: AppHandle,
    reporter: Reporter,
}

impl Seat {
    /// Tell the screen, if what the seat says or its connection has changed.
    fn tell(&mut self, activity: &Activity) {
        if let Some(event) = self.reporter.next(activity) {
            let _ = self.app.emit("seat-activity", event);
        }
    }
}

/// A fresh token: two v4 UUIDs (244 random bits), held only in memory and the
/// terminal's environment.
pub fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn token_sha256(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn spawn(request: &Request) -> Result<(Child, u16), String> {
    let command = request.command;
    let mut cmd = if cfg!(windows) && !command.to_ascii_lowercase().ends_with(".exe") {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", command]);
        cmd
    } else {
        Command::new(command)
    };
    cmd.args(&request.server_args)
        .current_dir(request.cwd)
        .env_remove(mcp_config::codex::OUTPUT_STYLE_ENV)
        .env_remove(plan::REMOTE_TOKEN_ENV)
        .envs(request.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Codex app-server を起動できません: {e}"))?;
    job::assign(&child);
    // Both streams are read for the whole life of the server, so a full pipe
    // never stalls it; the first port either names is the one it bound.
    let (tx, rx) = mpsc::channel::<u16>();
    for stream in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if let Some(port) = plan::listening_port(&String::from_utf8_lossy(&line)) {
                            let _ = tx.send(port);
                        }
                    }
                }
            }
        });
    }
    drop(tx);
    let deadline = Instant::now() + LISTEN_TIMEOUT;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(port) => return Ok((child, port)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_tree(&mut child);
                return Err("Codex app-server が待ち受けを始める前に終了しました。診断端末で codex app-server を確認してください。".into());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline || matches!(child.try_wait(), Ok(Some(_))) {
                    stop_tree(&mut child);
                    return Err("Codex app-server の待ち受けを確認できませんでした。起動を止めました。".into());
                }
            }
        }
    }
}

fn connect(url: &str, token: &str) -> Result<Socket, String> {
    let mut request = url
        .into_client_request()
        .map_err(|_| "Codex app-server の接続先が不正です。".to_string())?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "Codex app-server の鍵を送れません。".to_string())?,
    );
    let (socket, _) = tokio_tungstenite::tungstenite::connect(request)
        .map_err(|_| "Codex app-server へ接続できません。".to_string())?;
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    }
    Ok(socket)
}

impl Started {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({"id": id, "method": method, "params": params});
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(|_| format!("Codex app-server へ {method} を送れません。"))?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(format!("Codex app-server が {method} に答えませんでした。"));
            }
            let message = match self.socket.read() {
                Ok(message) => message,
                Err(tokio_tungstenite::tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    continue
                }
                Err(_) => return Err("Codex app-server との接続が切れました。".into()),
            };
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if value["id"].as_u64() != Some(id) || value.get("method").is_some() {
                // Not this answer: a notification sent while the thread was
                // being made is the seat's first state, not noise (#326).
                self.activity.feed(&value);
                if self.probe.len() < PROBE_EARLY_MAX {
                    if let Some((method, status)) = probe_of(&value) {
                        self.probe.push((method, status, crate::room::now_iso()));
                    }
                }
                continue;
            }
            if let Some(error) = value.get("error") {
                let detail = error["message"].as_str().unwrap_or_default();
                return Err(format!("Codex app-server が {method} を受け付けませんでした: {detail}"));
            }
            return Ok(value["result"].clone());
        }
    }

    /// The terminal could not be started: a thread this launch made goes with
    /// the server, so the topic is not left pointing at a conversation nobody
    /// was in.
    pub fn abandon(mut self) {
        if self.created {
            let id = self.thread_id.clone();
            let _ = self.call("thread/delete", json!({"threadId": id}));
        }
        let _ = self.socket.close(None);
        self.server.stop();
    }

    /// The terminal is up on `pty_id`. The app stays a client of the thread
    /// for the life of the seat, reading what the server sends so it never
    /// waits on this connection, and the server is stopped once the terminal
    /// has ended.
    ///
    /// What it reads is what the seat is doing (#326): the state gathered while
    /// the thread was made is told at once, then each change as it comes. A
    /// connection that ends is told as its own state (`connected: false`), so
    /// the screen says 様子不明 rather than falling back to 待機.
    pub fn adopt(self, app: &AppHandle, pty_id: &str, topic_id: &str, account_id: &str) {
        let Started {
            server,
            mut socket,
            mut activity,
            probe: early,
            ..
        } = self;
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            let _ = stream.set_read_timeout(None);
        }
        let mut seat = Seat {
            app: app.clone(),
            reporter: Reporter::new(topic_id, account_id, pty_id),
        };
        let mut probe = Probe {
            file: crate::room_log::open_probe(app, PROBE_FILE),
            topic_id: topic_id.to_string(),
            account_id: account_id.to_string(),
        };
        std::thread::spawn(move || {
            for (method, status, at) in &early {
                probe.write(method, status.as_deref(), at);
            }
            seat.tell(&activity);
            loop {
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            probe.note(&value);
                            activity.feed(&value);
                            seat.tell(&activity);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            activity.disconnect();
            seat.tell(&activity);
        });
        app.state::<CodexServers>()
            .servers
            .lock()
            .insert(pty_id.to_string(), server);
        let app = app.clone();
        let pty_id = pty_id.to_string();
        let spawned = std::thread::Builder::new()
            .name("codex-app-server".into())
            .spawn(move || loop {
                std::thread::sleep(WATCH);
                let running = app.state::<crate::pty::PtyState>().is_running(&pty_id);
                if !running {
                    let server = app.state::<CodexServers>().servers.lock().remove(&pty_id);
                    if let Some(server) = server {
                        server.stop();
                    }
                    return;
                }
            });
        if let Err(err) = spawned {
            eprintln!("[codex-app-server] watcher could not start: {err}");
        }
    }
}

/// Start the seat's server and have its thread ready for the terminal.
pub fn start(request: Request) -> Result<Started, String> {
    let (child, port) = spawn(&request)?;
    let url = format!("ws://127.0.0.1:{port}");
    let mut server = Server { child };
    let socket = match connect(&url, request.token) {
        Ok(socket) => socket,
        Err(err) => {
            stop_tree(&mut server.child);
            return Err(err);
        }
    };
    let mut started = Started {
        server,
        socket,
        url,
        thread_id: String::new(),
        created: false,
        next_id: 0,
        activity: Activity::new(),
        probe: Vec::new(),
    };
    match prepare(&mut started, &request) {
        Ok(()) => Ok(started),
        Err(err) => {
            started.abandon();
            Err(err)
        }
    }
}

fn prepare(started: &mut Started, request: &Request) -> Result<(), String> {
    started.call(
        "initialize",
        plan::initialize_params(env!("CARGO_PKG_VERSION")),
    )?;
    started
        .socket
        .send(Message::text(json!({"method": "initialized"}).to_string()))
        .map_err(|_| "Codex app-server へ initialized を送れません。".to_string())?;
    let cwd = request.thread_cwd.to_string_lossy().to_string();
    // Before any thread exists: the one hook is off and no style handler is
    // left to deliver the character a second time.
    let hooks = started.call("hooks/list", json!({"cwds": [cwd]}))?;
    plan::check_hooks(&hooks, request.hook_keys)?;
    let result = match request.resume {
        Some(id) => {
            started.thread_id = id.to_string();
            started.activity.set_thread(id);
            started.call(
                "thread/resume",
                plan::resume_params(id, &cwd, request.instructions, request.settings),
            )?
        }
        None => started.call(
            "thread/start",
            plan::start_params(&cwd, request.instructions, request.settings),
        )?,
    };
    let (id, _) = plan::thread_of(&result)?;
    if request.resume.is_some_and(|asked| asked != id) {
        return Err("Codex app-server が別の会話を再開しました。起動を止めました。".into());
    }
    started.created = request.resume.is_none();
    started.thread_id = id.clone();
    // What was held for a thread not yet named is read now, then the answer's
    // own status, which is later than any of it (#326).
    started.activity.set_thread(&id);
    started.activity.snapshot(&result["thread"]["status"]);
    plan::check_settings(&result, request.settings)?;
    if started.created {
        // A new thread has no rollout until something is recorded, and the
        // terminal cannot attach to one without it.
        started.call("thread/inject_items", plan::inject_params(&id))?;
    }
    Ok(())
}

/// Windows: end the server's whole tree when the app's process ends, however
/// it ends. Best effort; a failure leaves the other two stops in place. The
/// server joins right after it is created, before `cmd.exe` has started the
/// CLI under it, so the tree it starts is born inside the job.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::jobapi2::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject};
    use winapi::um::winnt::{
        JobObjectExtendedLimitInformation, HANDLE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            // The handle is never closed: the app's exit closes it, and that
            // is what ends every process in it.
            (ok != 0).then_some(Job(handle))
        })
        .as_ref()
    }

    pub fn assign(child: &std::process::Child) {
        let Some(job) = job() else {
            eprintln!("[codex-app-server] no job object; the server is stopped with its seat only");
            return;
        };
        let ok = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            eprintln!("[codex-app-server] the server could not join the job object");
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn assign(_child: &std::process::Child) {}
}
