//! A short-lived control connection to the seat's existing backend. It never
//! resumes threads, answers approvals, emits payloads, or writes diagnostics.
use super::*;
use mcp_config::codex::remote::{self as protocol, Action};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Control {
    url: String,
    token: String,
    epoch: String,
    alive: AtomicBool,
    explicit: AtomicBool,
    serial: Mutex<()>,
    startup_error: Mutex<Option<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    epoch: String,
    status: Option<String>,
    pairing: Option<protocol::Pairing>,
    error: Option<String>,
}

impl Control {
    pub(super) fn new(url: &str, token: &str) -> Self {
        Self { url: url.into(), token: token.into(), epoch: uuid::Uuid::new_v4().to_string(),
            alive: AtomicBool::new(true), explicit: AtomicBool::new(false), serial: Mutex::new(()), startup_error: Mutex::new(None) }
    }
    pub(super) fn end(&self) { self.alive.store(false, Ordering::Release); }
    fn check(&self) -> Result<(), String> {
        if self.alive.load(Ordering::Acquire) { Ok(()) }
        else { Err("この席は終了しました。新しい席から開き直してください。".into()) }
    }
    pub(super) fn enable_on_start(&self) {
        if let Err(err) = self.call_inner(Action::Enable, true) {
            if !self.explicit.load(Ordering::Acquire) { *self.startup_error.lock() = Some(err); }
        }
    }
    fn call(&self, action: Action) -> Result<Value, String> {
        self.call_inner(action, false)
    }
    fn call_inner(&self, action: Action, startup: bool) -> Result<Value, String> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let _guard = self.serial.try_lock_for(REQUEST_TIMEOUT).ok_or("接続操作の応答待ちです。少し待って再試行してください。")?;
        self.check()?;
        // A user operation received before the startup worker acquired this
        // lock supersedes its enable. If startup owns the lock first, the
        // explicit operation runs after it and therefore decides final state.
        if startup && self.explicit.load(Ordering::Acquire) { return Ok(Value::Null); }
        if Instant::now() >= deadline { return Err("接続操作が時間切れになりました。".into()); }
        // All addresses originate in spawn's loopback banner. Bound connect,
        // handshake and writes too, not only the JSON-RPC response loop.
        let port: u16 = self.url.rsplit(':').next().and_then(|p| p.parse().ok()).ok_or("接続先が不正です。")?;
        let address = std::net::SocketAddr::from(([127,0,0,1], port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| "席へ接続できません。")?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| "接続の制限時間を設定できません。")?;
        stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| "接続の制限時間を設定できません。")?;
        let mut request = self.url.as_str().into_client_request().map_err(|_| "接続先が不正です。")?;
        request.headers_mut().insert("Authorization", HeaderValue::from_str(&format!("Bearer {}",self.token)).map_err(|_| "接続認証を準備できません。")?);
        let (mut socket, _) = tokio_tungstenite::tungstenite::client(request, MaybeTlsStream::Plain(stream)).map_err(|_| "席との認証接続に失敗しました。")?;
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() { let _=stream.set_read_timeout(Some(Duration::from_millis(200))); }
        let result = (|| {
            self.rpc(&mut socket, 1, "initialize", plan::initialize_params(env!("CARGO_PKG_VERSION")), deadline)?;
            socket.send(Message::text(json!({"method":"initialized"}).to_string())).map_err(|_| "席との接続が切れました。")?;
            let (method, params) = action.request();
            self.rpc(&mut socket, 2, method, params, deadline)
        })();
        // Each call has its own socket and ids. A timed-out reply cannot be
        // consumed by another operation or a replacement seat.
        drop(socket);
        self.check()?;
        result
    }
    fn rpc(&self, socket: &mut Socket, id: u64, method: &str, params: Value, deadline: Instant) -> Result<Value, String> {
        self.check()?;
        socket.send(Message::text(json!({"id":id,"method":method,"params":params}).to_string())).map_err(|_| "席へ操作を送れません。")?;
        loop {
            self.check()?;
            if Instant::now() >= deadline { return Err("リモート接続の応答が時間切れになりました。再試行できます。".into()); }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if let Some(answer) = protocol::response(&value,id) { return answer; }
                    }
                }
                Ok(_) => {},
                Err(tokio_tungstenite::tungstenite::Error::Io(e)) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut) => {},
                Err(_) => return Err("席との接続が切れました。".into()),
            }
        }
    }
}

#[tauri::command]
pub async fn codex_remote_control(app: AppHandle, pty_id: String, epoch: Option<String>, action: Action) -> Result<Reply,String> {
    let control = app.state::<CodexServers>().servers.lock().get(&pty_id)
        .and_then(|server| server.remote.clone())
        .ok_or("この席ではリモート接続が有効ではありません。アカウント設定を確認し、席を起動し直してください。")?;
    if !app.state::<crate::pty::PtyState>().is_running(&pty_id) { control.end(); }
    control.check()?;
    if epoch.as_deref().is_some_and(|epoch| epoch != control.epoch)
        || (action != Action::Status && epoch.is_none()) {
        return Err("席が切り替わりました。接続画面を開き直してください。".into());
    }
    if action != Action::Status { control.explicit.store(true, Ordering::Release); }
    let still_current = Arc::clone(&control);
    let reply = tauri::async_runtime::spawn_blocking(move || {
        let mut reply = Reply { epoch: control.epoch.clone(), status: None, pairing: None, error: None };
        match control.call(action) {
            Ok(value) if action == Action::Pair => {
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                match protocol::pairing(&value,now) { Ok(pair) => reply.pairing=Some(pair), Err(err) => reply.error=Some(err) }
            }
            Ok(value) => {
                reply.status = value["status"].as_str().filter(|s| matches!(*s,"disabled"|"connecting"|"connected"|"errored")).map(str::to_owned);
                if reply.status.is_none() { reply.error=Some("リモート接続の状態を読み取れません。".into()); }
                if action == Action::Enable { *control.startup_error.lock()=None; }
                if reply.status.as_deref() == Some("disabled") { reply.error=control.startup_error.lock().clone().or(reply.error); }
            }
            Err(err) => reply.error=Some(err),
        }
        control.check()?;
        Ok::<_,String>(reply)
    }).await.map_err(|_| "接続操作を完了できませんでした。")??;
    // Removal from the registry, terminal exit and generation all invalidate
    // replies even if they raced the RPC completion.
    if !app.state::<crate::pty::PtyState>().is_running(&pty_id) { still_current.end(); }
    still_current.check()?;
    let current=app.state::<CodexServers>();
    let servers=current.servers.lock();
    if !servers.get(&pty_id).and_then(|s|s.remote.as_ref()).is_some_and(|c| Arc::ptr_eq(c,&still_current)) {
        return Err("この席は終了しました。".into());
    }
    Ok(reply)
}
