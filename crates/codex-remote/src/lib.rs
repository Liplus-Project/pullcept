//! Seat-local relay RPC, isolated from activity and approval ownership.
use mcp_config::codex::remote::{self as protocol, Action};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tungstenite::{
    client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream, Message, WebSocket,
};
type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

pub struct Control {
    url: String,
    token: String,
    epoch: String,
    alive: AtomicBool,
    explicit: AtomicBool,
    serial: Mutex<()>,
    timeout: Duration,
    startup_error: Mutex<Option<String>>,
}

impl Control {
    pub fn new(url: &str, token: &str, epoch: String) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            epoch,
            alive: AtomicBool::new(true),
            explicit: AtomicBool::new(false),
            timeout: Duration::from_secs(30),
            serial: Mutex::new(()),
            startup_error: Mutex::new(None),
        }
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn startup_error(&self) -> Option<String> {
        self.startup_error.lock().clone()
    }
    pub fn clear_startup_error(&self) {
        *self.startup_error.lock() = None;
    }
    pub fn accept(&self, action: Action, epoch: Option<&str>) -> Result<(), String> {
        self.check()?;
        if epoch.is_some_and(|epoch| epoch != self.epoch)
            || (action != Action::Status && epoch.is_none())
        {
            return Err("席が切り替わりました。接続画面を開き直してください。".into());
        }
        if action != Action::Status {
            self.explicit.store(true, Ordering::Release);
            *self.startup_error.lock() = None;
        }
        Ok(())
    }
    pub fn end(&self) {
        self.alive.store(false, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), String> {
        if self.alive.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("この席は終了しました。新しい席から開き直してください。".into())
        }
    }
    pub fn enable_on_start(&self) {
        if let Err(err) = self.call_inner(Action::Enable, true) {
            let mut error = self.startup_error.lock();
            if !self.explicit.load(Ordering::Acquire) {
                *error = Some(err);
            }
        }
    }
    pub fn call(&self, action: Action) -> Result<Value, String> {
        self.call_inner(action, false)
    }
    fn call_inner(&self, action: Action, startup: bool) -> Result<Value, String> {
        let deadline = Instant::now() + self.timeout;
        let _guard = self
            .serial
            .try_lock_for(self.timeout)
            .ok_or("接続操作の応答待ちです。少し待って再試行してください。")?;
        self.check()?;
        // A user operation received before the startup worker acquired this
        // lock supersedes its enable. If startup owns the lock first, the
        // explicit operation runs after it and therefore decides final state.
        if startup && self.explicit.load(Ordering::Acquire) {
            return Ok(Value::Null);
        }
        if Instant::now() >= deadline {
            return Err("接続操作が時間切れになりました。".into());
        }
        // All addresses originate in spawn's loopback banner. Bound connect,
        // handshake and writes too, not only the JSON-RPC response loop.
        let port: u16 = self
            .url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or("接続先が不正です。")?;
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .map_err(|_| "席へ接続できません。")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "接続の制限時間を設定できません。")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| "接続の制限時間を設定できません。")?;
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|_| "接続先が不正です。")?;
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|_| "接続認証を準備できません。")?,
        );
        let (mut socket, _) = tungstenite::client(request, MaybeTlsStream::Plain(stream))
            .map_err(|_| "席との認証接続に失敗しました。")?;
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        }
        let result = (|| {
            self.rpc(
                &mut socket,
                1,
                "initialize",
                mcp_config::codex::app_server::initialize_params(env!("CARGO_PKG_VERSION")),
                deadline,
            )?;
            socket
                .send(Message::text(json!({"method":"initialized"}).to_string()))
                .map_err(|_| "席との接続が切れました。")?;
            let (method, params) = action.request();
            self.rpc(&mut socket, 2, method, params, deadline)
        })();
        // Each call has its own socket and ids. A timed-out reply cannot be
        // consumed by another operation or a replacement seat.
        drop(socket);
        self.check()?;
        result
    }
    fn rpc(
        &self,
        socket: &mut Socket,
        id: u64,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, String> {
        self.check()?;
        socket
            .send(Message::text(
                json!({"id":id,"method":method,"params":params}).to_string(),
            ))
            .map_err(|_| "席へ操作を送れません。")?;
        loop {
            self.check()?;
            if Instant::now() >= deadline {
                return Err("リモート接続の応答が時間切れになりました。再試行できます。".into());
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if let Some(answer) = protocol::response(&value, id) {
                            return answer;
                        }
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return Err("席との接続が切れました。".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::TcpListener,
        sync::{mpsc, Arc},
        thread,
    };
    use tungstenite::handshake::server::{Request, Response};

    fn fake<F>(connections: usize, handler: F) -> (Arc<Control>, thread::JoinHandle<()>)
    where
        F: Fn(usize, &mut tungstenite::WebSocket<TcpStream>) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            for i in 0..connections {
                let limit = Instant::now() + Duration::from_secs(5);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < limit =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("fake backend did not receive connection: {e}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut socket =
                    tungstenite::accept_hdr(stream, |request: &Request, response: Response| {
                        assert_eq!(
                            request.headers()["Authorization"],
                            "Bearer fake-private-token"
                        );
                        Ok(response)
                    })
                    .unwrap();
                let initialize = read(&mut socket);
                assert_eq!(initialize["method"], "initialize");
                assert_eq!(
                    initialize["params"]["capabilities"]["experimentalApi"],
                    true
                );
                send(&mut socket, json!({"id":initialize["id"],"result":{}}));
                assert_eq!(read(&mut socket)["method"], "initialized");
                handler(i, &mut socket);
            }
        });
        (
            Arc::new(Control::new(&url, "fake-private-token", "seat-1".into())),
            worker,
        )
    }
    fn read(socket: &mut tungstenite::WebSocket<TcpStream>) -> Value {
        let text = socket.read().unwrap().into_text().unwrap();
        serde_json::from_str(&text).unwrap()
    }
    fn send(socket: &mut tungstenite::WebSocket<TcpStream>, value: Value) {
        socket.send(Message::text(value.to_string())).unwrap();
    }
    #[test]
    fn authenticated_socket_correlates_replies_and_does_not_answer_approvals() {
        let (control, worker) = fake(1, |_, socket| {
            let request = read(socket);
            assert_eq!(request["method"], "remoteControl/status/read");
            send(socket, json!({"id":99,"result":{"status":"wrong"}}));
            send(
                socket,
                json!({"id":request["id"],"method":"item/commandExecution/requestApproval","params":{"secret":"not logged"}}),
            );
            send(
                socket,
                json!({"method":"thread/status/changed","params":{"status":"active"}}),
            );
            send(
                socket,
                json!({"id":request["id"],"result":{"status":"connected"}}),
            );
            // The caller closes without answering the server's request.
            assert!(socket.read().is_err());
        });
        assert_eq!(control.call(Action::Status).unwrap()["status"], "connected");
        worker.join().unwrap();
    }
    #[test]
    fn timeout_closes_its_socket_and_late_reply_cannot_satisfy_next_operation() {
        let (mut control, worker) = fake(2, |index, socket| {
            let request = read(socket);
            if index == 0 {
                thread::sleep(Duration::from_millis(350));
                let _ = socket.send(Message::text(
                    json!({"id":request["id"],"result":{"status":"stale"}}).to_string(),
                ));
            } else {
                send(
                    socket,
                    json!({"id":request["id"],"result":{"status":"connected"}}),
                );
            }
        });
        Arc::get_mut(&mut control).unwrap().timeout = Duration::from_millis(100);
        let started = Instant::now();
        assert!(control
            .call(Action::Status)
            .unwrap_err()
            .contains("時間切れ"));
        assert!(started.elapsed() < Duration::from_secs(1));
        Arc::get_mut(&mut control).unwrap().timeout = Duration::from_secs(2);
        assert_eq!(control.call(Action::Status).unwrap()["status"], "connected");
        worker.join().unwrap();
    }
    #[test]
    fn stopped_seat_rejects_inflight_reply_and_new_epoch_has_no_authority() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let (control, worker) = fake(1, move |_, socket| {
            let request = read(socket);
            seen_tx.send(()).unwrap();
            release.lock().recv().unwrap();
            let _ = socket.send(Message::text(
                json!({"id":request["id"],"result":{"pairingCode":"must not return"}}).to_string(),
            ));
        });
        assert!(control.accept(Action::Enable, Some("seat-2")).is_err());
        assert!(control.accept(Action::Pair, None).is_err());
        let running = Arc::clone(&control);
        let call = thread::spawn(move || running.call(Action::Pair));
        seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        control.end();
        release_tx.send(()).unwrap();
        assert!(call.join().unwrap().unwrap_err().contains("終了"));
        assert!(control.accept(Action::Status, Some("seat-1")).is_err());
        worker.join().unwrap();
    }
    #[test]
    fn explicit_disable_received_before_startup_lock_suppresses_startup_enable() {
        let (control, worker) = fake(1, |_, socket| {
            let request = read(socket);
            assert_eq!(request["method"], "remoteControl/disable");
            assert_eq!(request["params"], json!({"ephemeral":true}));
            send(
                socket,
                json!({"id":request["id"],"result":{"status":"disabled"}}),
            );
        });
        let guard = control.serial.lock();
        let startup = Arc::clone(&control);
        let thread = thread::spawn(move || startup.enable_on_start());
        control.accept(Action::Disable, Some("seat-1")).unwrap();
        drop(guard);
        thread.join().unwrap();
        assert_eq!(control.call(Action::Disable).unwrap()["status"], "disabled");
        worker.join().unwrap();
    }
    #[test]
    fn explicit_disable_after_startup_begins_runs_last() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let (control, worker) = fake(2, move |index, socket| {
            let request = read(socket);
            if index == 0 {
                assert_eq!(request["method"], "remoteControl/enable");
                seen_tx.send(()).unwrap();
                release.lock().recv().unwrap();
            } else {
                assert_eq!(request["method"], "remoteControl/disable");
            }
            send(
                socket,
                json!({"id":request["id"],"result":{"status":if index==0 {"connected"} else {"disabled"}}}),
            );
        });
        let startup = Arc::clone(&control);
        let thread = thread::spawn(move || startup.enable_on_start());
        seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        control.accept(Action::Disable, Some("seat-1")).unwrap();
        release_tx.send(()).unwrap();
        assert_eq!(control.call(Action::Disable).unwrap()["status"], "disabled");
        thread.join().unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn unsupported_enable_is_sanitized_and_control_connection_remains_retryable() {
        let (control, worker) = fake(2, |index, socket| {
            let request = read(socket);
            if index == 0 {
                send(
                    socket,
                    json!({"id":request["id"],"error":{"code":-32601,"message":"fake-private-token"}}),
                );
            } else {
                send(
                    socket,
                    json!({"id":request["id"],"result":{"status":"disabled"}}),
                );
            }
        });
        control.enable_on_start();
        let error = control.startup_error().unwrap();
        assert!(error.contains("対応"));
        assert!(!error.contains("fake-private-token"));
        assert!(control.check().is_ok());
        assert_eq!(control.call(Action::Status).unwrap()["status"], "disabled");
        worker.join().unwrap();
    }
}
