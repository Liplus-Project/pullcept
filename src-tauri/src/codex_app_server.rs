//! Native Codex process ownership and seat leases (#299, #568).
//! Opted-in, compatible logins use an app-owned pooled backend. Each seat
//! owns its authenticated gateway, thread reservation, observer and PTY.
//! Opt-out/unknown-identity seats retain the dedicated process path.
//! See docs/3-accounts.md for detach, mobile subscriber and cold-resume limits.

use mcp_config::codex::activity::{probe_line, probe_of, Activity, Reporter};
use mcp_config::codex::app_server as plan;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream, Message, WebSocket,
};

#[path = "codex_remote.rs"]
pub mod remote;

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
/// The probe's file, under the app's `logs` directory.
const PROBE_FILE: &str = "codex-activity-probe.log";

/// Every running seat server, by its terminal.
#[derive(Default)]
pub struct CodexServers {
    servers: Mutex<HashMap<String, Server>>,
    shared: codex_remote::pool::Pool<SharedBackend>,
}

impl CodexServers {
    pub fn new() -> Self {
        Self::default()
    }
}

/// App-owned native process. Seat leases never stop this owner.
struct SharedBackend {
    url: String,
    token: String,
    child: Arc<Mutex<Child>>,
    remote: Arc<remote::Control>,
    relay_in_use: Arc<AtomicBool>,
    threads: Arc<codex_remote::pool::Threads>,
}
impl SharedBackend {
    fn check(&self) -> Result<(), String> {
        if matches!(self.child.lock().try_wait(), Ok(None)) { Ok(()) }
        else { self.remote.end(); Err("共有 Codex 接続が終了しました。Pullcept を起動し直してください。".into()) }
    }
    fn stop(&self) { self.remote.end(); stop_tree(&mut self.child.lock()); }
}

pub use codex_remote::credential::CredentialFile;

/// The process tree of one seat's server.
struct Server {
    shared: Option<Arc<SharedBackend>>,
    gateway: Option<Arc<codex_remote::gateway::Gateway>>,
    _credential: Option<Arc<CredentialFile>>,
    epoch: String,
    lease: Option<Arc<codex_remote::pool::ThreadLease>>,
    remote_notice: Option<&'static str>,
    alive: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
    remote: Option<Arc<remote::Control>>,
    /// The relay's last connection failure was 409 "already online" (#430),
    /// read off the server's stderr; cleared by the reply that sees it end.
    relay_in_use: Arc<AtomicBool>,
}

impl Server {
    fn stop(self) {
        self.alive.store(false, Ordering::Release);
        if let Some(gateway) = &self.gateway { gateway.end(); }
        if let Some(shared) = &self.shared {
            // The thread reservation is released by its observer after detach.
            let _ = shared; return;
        }
        if let Some(control) = &self.remote { control.end(); }
        stop_tree(&mut self.child.lock());
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
pub fn stop_one(app: &AppHandle, pty_id: &str) {
    let server = app.state::<CodexServers>().servers.lock().remove(pty_id);
    if let Some(server) = server { server.stop(); }
}

pub fn stop_all(app: &AppHandle) {
    let drained: Vec<Server> = app
        .state::<CodexServers>()
        .servers
        .lock()
        .drain()
        .map(|(_, server)| server)
        .collect();
    for server in drained { server.stop(); }
    for shared in app.state::<CodexServers>().shared.close() { shared.stop(); }
}

/// What the seat's launch is handed: the character, the hooks to turn off,
/// the person's options split, and the thread to go back into, if any.
#[derive(Clone)]
pub struct Request<'a> {
    pub shared_key: Option<String>,
    pub shared_home: Option<String>,
    pub shared_identity: Option<String>,
    pub remote_notice: Option<&'static str>,
    pub thread_config: Value,
    pub credential: Option<Arc<CredentialFile>>,
    pub on_spawn: Option<&'a dyn Fn(Arc<Mutex<Child>>)>,
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
    pub remote_enabled: bool,
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
    probe: Vec<(String, Option<String>, String, Option<String>)>,
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
            crate::room_status::activity(&self.app, &event);
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

fn spawn(request: &Request, relay_in_use: &Arc<AtomicBool>) -> Result<(Child, u16), String> {
    let command = request.command;
    let mut cmd = if cfg!(windows) && !command.to_ascii_lowercase().ends_with(".exe") {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/C", command]);
        cmd
    } else {
        Command::new(command)
    };
    // A shared backend must not inherit the launching room from Pullcept's
    // own host environment. Opt-out seats explicitly receive theirs below.
    for (key,_) in std::env::vars().filter(|(key,_)| key.to_ascii_uppercase().starts_with("PULLCEPT_")) { cmd.env_remove(key); }
    cmd.args(&request.server_args)
        .current_dir(request.cwd)
        .env_remove(mcp_config::codex::OUTPUT_STYLE_ENV)
        .env_remove(plan::REMOTE_TOKEN_ENV)
        .envs(request.env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.remote_enabled {
        // The relay's refusal reason is a warning, below the default stderr
        // level; the person's own RUST_LOG, if any, is kept (#430).
        let existing = request
            .env
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case("RUST_LOG"))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var("RUST_LOG").ok());
        cmd.env("RUST_LOG", mcp_config::codex::remote::relay_log_filter(existing.as_deref()));
    }
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
        let relay_in_use = Arc::clone(relay_in_use);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line);
                        if let Some(port) = plan::listening_port(&text) {
                            let _ = tx.send(port);
                        }
                        if let Some(in_use) = mcp_config::codex::remote::relay_failure_in_use(&text) {
                            relay_in_use.store(in_use, Ordering::Release);
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
                        self.probe.push((method, status, crate::room::now_iso(), value["params"]["threadId"].as_str().or_else(||value["params"]["thread"]["id"].as_str()).map(str::to_string)));
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
            thread_id,
            ..
        } = self;
        let alive = Arc::clone(&server.alive);
        let lease = server.lease.clone();
        let mut seat = Seat {
            app: app.clone(),
            reporter: Reporter::new(topic_id, account_id, pty_id),
        };
        let mut probe = Probe {
            file: crate::room_log::open_probe(app, PROBE_FILE),
            topic_id: topic_id.to_string(),
            account_id: account_id.to_string(),
        };
        let (topic, account, pty) = (topic_id.to_string(), account_id.to_string(), pty_id.to_string());
        std::thread::spawn(move || {
            let mut plan = mcp_config::work_status::CodexPlan::default();
            for (method, status, at, owner) in &early {
                if owner.as_deref()==Some(thread_id.as_str()) { probe.write(method, status.as_deref(), at); }
            }
            seat.tell(&activity);
            let mut turn_id: Option<String> = None;
            loop {
                if !alive.load(Ordering::Acquire) {
                    if let Some(turn) = turn_id.as_ref() {
                        let _ = socket.send(Message::text(json!({"id":1000001,"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn}}).to_string()));
                    }
                    let _ = socket.send(Message::text(json!({"id":1000002,"method":"thread/unsubscribe","params":{"threadId":thread_id}}).to_string()));
                    let _ = socket.close(None);
                    break;
                }
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            if value["params"]["threadId"].as_str() == Some(thread_id.as_str()) {
                                if value["method"] == "turn/started" { turn_id = value["params"]["turn"]["id"].as_str().map(str::to_owned); }
                                if value["method"] == "turn/completed" { turn_id = None; }
                            }
                            // Only this seat's notifications enter its activity probe.
                            if value["params"]["threadId"].as_str() != Some(thread_id.as_str()) && value["params"]["thread"]["id"].as_str() != Some(thread_id.as_str()) { continue; }
                            probe.note(&value);
                            if let Some(steps) = plan.feed(&thread_id, &value) { crate::room_status::plan(&seat.app, &topic, &account, &pty, "codex-app-server", steps); }
                            activity.feed(&value);
                            seat.tell(&activity);
                        }
                    }
                    Ok(_) => {}
                    Err(tokio_tungstenite::tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                    Err(_) => break,
                }
            }
            drop(lease);
            activity.disconnect();
            seat.tell(&activity);
        });
        if server.shared.is_none() {
            if let Some(control) = &server.remote {
                let control = Arc::clone(control);
                std::thread::spawn(move || control.enable_on_start());
            }
        }
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
pub fn start(app: &AppHandle, request: Request) -> Result<Started, String> {
    if request.shared_key.is_some() { return start_shared(app, request); }
    let relay_in_use = Arc::new(AtomicBool::new(false));
    let (child, port) = spawn(&request, &relay_in_use)?;
    let url = format!("ws://127.0.0.1:{port}");
    let server = Server {
        shared: None, gateway: None, _credential: request.credential.clone(), epoch: uuid::Uuid::new_v4().to_string(), lease: None, remote_notice: request.remote_notice, alive: Arc::new(AtomicBool::new(true)),
        relay_in_use,
        child: Arc::new(Mutex::new(child)),
        remote: request.remote_enabled.then(|| Arc::new(remote::Control::new(&url, request.token, uuid::Uuid::new_v4().to_string()))),
    };
    if let Some(on_spawn) = request.on_spawn { on_spawn(Arc::clone(&server.child)); }
    let socket = match connect(&url, request.token) {
        Ok(socket) => socket,
        Err(err) => {
            stop_tree(&mut server.child.lock());
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

fn with_config(mut params: Value, extra: &Value) -> Value {
    if let Some(extra) = extra.as_object() {
        if !params["config"].is_object() { params["config"] = json!({}); }
        for (key, value) in extra { params["config"][key] = value.clone(); }
    }
    params
}

fn start_shared(app: &AppHandle, request: Request) -> Result<Started, String> {
    let state = app.state::<CodexServers>();
    let backend = state.shared.acquire(request.shared_home.as_deref().ok_or("認証 HOME がありません。")?, request.shared_identity.as_deref().ok_or("ログイン識別子がありません。")?, request.shared_key.as_deref().unwrap(), || {
        let token = new_token();
        let mut spawning = request.clone();
        let digest = token_sha256(&token);
        let index = spawning.server_args.iter().position(|s| s == "--ws-token-sha256").ok_or("共有接続の認証設定がありません。")?;
        spawning.server_args[index + 1] = digest;
        let relay = Arc::new(AtomicBool::new(false));
        let (child,port) = spawn(&spawning, &relay)?;
        let url = format!("ws://127.0.0.1:{port}");
        let backend = SharedBackend { child: Arc::new(Mutex::new(child)), remote: Arc::new(remote::Control::new(&url, &token, uuid::Uuid::new_v4().to_string())), url, token, relay_in_use: relay, threads: Arc::new(Default::default()) };
        let control = Arc::clone(&backend.remote);
        std::thread::spawn(move || control.enable_on_start());
        Ok(backend)
    })?;
    backend.check()?;
    if let Some(on_spawn) = request.on_spawn { on_spawn(Arc::clone(&backend.child)); }
    let socket = connect(&backend.url, &backend.token)?;
    let server = Server { child: Arc::clone(&backend.child), remote: Some(Arc::clone(&backend.remote)), relay_in_use: Arc::clone(&backend.relay_in_use), shared: Some(Arc::clone(&backend)), gateway: None, _credential: request.credential.clone(), epoch: uuid::Uuid::new_v4().to_string(), lease: None, remote_notice: request.remote_notice, alive: Arc::new(AtomicBool::new(true)) };
    let mut started = Started { server, socket, url: backend.url.clone(), thread_id: String::new(), created: false, next_id: 0, activity: Activity::new(), probe: Vec::new() };
    // A known conversation can only be cold-resumed. Loaded native resume
    // ignores new room/launch settings, even if that call reports success.
    let result = (|| {
        started.call("initialize", plan::initialize_params(env!("CARGO_PKG_VERSION")))?;
        started.socket.send(Message::text(json!({"method":"initialized"}).to_string())).map_err(|_| "共有接続が切れました。")?;
        if let Some(id) = request.resume {
            started.server.lease = Some(Arc::new(codex_remote::pool::ThreadLease::claim(&backend.threads,id)?));
            let mut cursor = Value::Null;
            loop {
                let loaded = started.call("thread/loaded/list", json!({"cursor":cursor,"limit":100}))?;
                if loaded["data"].as_array().is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(id))) {
                    return Err("この会話はスマホなどでまだ開かれています。席の部屋設定を安全に更新するため、スマホで会話を閉じて少し待ち、再試行してください。履歴は保持しています。".into());
                }
                cursor = loaded["nextCursor"].clone();
                if cursor.is_null() { break; }
            }

        }
        prepare_thread(&mut started, &request)?;
        if let Some(file) = &request.credential {
            let marker=file.marker();
            let deadline=Instant::now()+REQUEST_TIMEOUT;
            while !marker.is_file() {
                backend.check()?;
                if Instant::now()>=deadline { return Err("会話へ今回の部屋設定を適用できませんでした。スマホで会話を閉じ、少し待って再試行してください。履歴は保持しています。".into()) }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _=std::fs::remove_file(marker);
        }
        if started.server.lease.is_none() { started.server.lease = Some(Arc::new(codex_remote::pool::ThreadLease::claim(&backend.threads,&started.thread_id)?)); }
        let gateway = Arc::new(codex_remote::gateway::Gateway::start(backend.url.clone(), backend.token.clone(), request.token.to_string(), started.thread_id.clone(), request.thread_cwd.to_path_buf(), request.hook_keys.to_vec())?);
        started.url = gateway.url.clone();
        started.server.gateway = Some(gateway);
        Ok(())
    })();
    match result {
        Ok(()) => Ok(started),
        Err(err) => { started.abandon(); Err(err) }
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
    prepare_thread(started, request)
}

fn prepare_thread(started: &mut Started, request: &Request) -> Result<(), String> {
    let cwd = request.thread_cwd.to_string_lossy().to_string();
    // Before any thread exists: the one hook is off and no style handler is
    // left to deliver the character a second time.
    let mut thread_config = request.thread_config.clone();
    if request.shared_key.is_some() {
        let config = started.call("config/read", json!({"includeLayers":false,"cwd":cwd}))?;
        if let Some(servers) = config["config"]["mcp_servers"].as_object() {
            for name in servers.keys().filter(|name| name.starts_with(mcp_config::SERVER_PREFIX)) {
                if thread_config.get(format!("mcp_servers.{name}")).is_none() { thread_config[format!("mcp_servers.{name}.enabled")] = json!(false); }
            }
        }
    } else {
        let hooks = started.call("hooks/list", json!({"cwds": [cwd]}))?;
        plan::check_hooks(&hooks, request.hook_keys)?;
    }
    let result = match request.resume {
        Some(id) => {
            started.thread_id = id.to_string();
            started.activity.set_thread(id);
            started.call(
                "thread/resume",
                with_config(plan::resume_params(id, &cwd, request.instructions, request.settings), &thread_config),
            )?
        }
        None => started.call(
            "thread/start",
            with_config(plan::start_params(&cwd, request.instructions, request.settings), &thread_config),
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
