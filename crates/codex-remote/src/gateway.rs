//! Authenticated seat boundary in front of the private native backend.
//! Unknown methods fail closed. A connection owns both RPC directions' IDs.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, net::{TcpListener, TcpStream}, sync::{Arc, atomic::{AtomicBool, Ordering}}, thread, time::Duration};
use tungstenite::{Message, WebSocket, client::IntoClientRequest, http::HeaderValue, handshake::server::{Request, Response, ErrorResponse}};

fn key(id: &Value) -> Option<String> {
    (id.is_string() || id.is_number()).then(|| id.to_string())
}
fn thread_id(value: &Value) -> Option<&str> {
    value.get("threadId").or_else(|| value.get("thread_id")).and_then(Value::as_str)
        .or_else(|| value.get("thread").and_then(|t| t.get("id")).and_then(Value::as_str))
}
fn foreign_thread(value: &Value, owned: &HashSet<String>) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(k,v)| {
            if matches!(k.as_str(), "threadId" | "thread_id" | "conversationId" | "conversation_id" | "sourceThreadId") {
                v.as_str().is_none_or(|id|!owned.contains(id))
            } else { foreign_thread(v, owned) }
        }),
        Value::Array(a) => a.iter().any(|v| foreign_thread(v, owned)),
        _ => false,
    }
}

/// Per-client correlation, deliberately not shared across TUI connections.
pub struct Policy {
    owned: Arc<Mutex<HashSet<String>>>,
    pending: HashMap<String, String>,
    approvals: HashSet<String>,
}
impl Policy {
    pub fn new(thread: &str) -> Self { Self { owned: Arc::new(Mutex::new(HashSet::from([thread.to_string()]))), pending: HashMap::new(), approvals: HashSet::new() } }
    pub fn client(&mut self, message: &Value) -> Result<(), &'static str> {
        if let Some(method) = message["method"].as_str() {
            if method == "initialized" && message.get("id").is_none() { return Ok(()) }
            let id = message.get("id").and_then(key).ok_or("request ID required")?;
            if self.pending.contains_key(&id) { return Err("request ID already pending") }
            let p = &message["params"];
            if foreign_thread(p, &self.owned.lock()) { return Err("another seat's thread is not accessible") }
            let scoped = matches!(method,
                "thread/resume" | "thread/read" | "thread/unsubscribe" | "thread/name/set" |
                "thread/turns/list" | "thread/items/list" | "thread/compact/start" |
                "thread/settings/update" | "thread/revert" | "thread/inject_items" |
                "turn/start" | "turn/steer" | "turn/interrupt" | "mcpServerStatus/list" |
                "mcpServer/tool/call" | "mcpResource/read" | "thread/backgroundTerminals/list" |
                "thread/backgroundTerminals/clean" | "thread/backgroundTerminals/terminate" |
                "thread/goal/get" | "thread/goal/set" | "thread/goal/clear" | "thread/shellCommand" | "thread/attachment/list" | "review/start");
            let global = matches!(method, "initialize" | "model/list" | "account/read" | "account/rateLimits/read" | "collaborationMode/list" | "experimentalFeature/list" | "configRequirements/read");
            if scoped {
                if thread_id(p).is_none_or(|id|!self.owned.lock().contains(id)) { return Err("this seat's thread ID is required") }
                // Attach is a rejoin only. The launcher owns configuration and history.
                if method == "thread/resume" && p.as_object().is_some_and(|m| m.iter().any(|(k,v)| !v.is_null() && !matches!(k.as_str(), "threadId" | "excludeTurns" | "initialTurnsPage"))) {
                    return Err("resume configuration belongs to the seat launcher")
                }
            } else if !global { return Err("method is unavailable through this seat") }
            self.pending.insert(id, method.into());
            Ok(())
        } else {
            let id = message.get("id").and_then(key).ok_or("response ID required")?;
            if !self.approvals.remove(&id) { return Err("approval ID does not belong to this connection") }
            Ok(())
        }
    }
    pub fn backend(&mut self, message: &Value) -> bool {
        if message.get("method").is_none() {
            return message.get("id").and_then(key).is_some_and(|id| self.pending.remove(&id).is_some())
        }
        let p = &message["params"];
        if matches!(message["method"].as_str(),Some("account/updated"|"account/rateLimits/updated")) && message.get("id").is_none() { return true }
        // Learn descendants only from native spawn events on an owned parent.
        let parent_owned=thread_id(p).is_some_and(|id|self.owned.lock().contains(id));
        if parent_owned && matches!(message["method"].as_str(),Some("item/started"|"item/completed")) {
            let item=&p["item"];
            if item["type"]=="collabAgentToolCall" && item["tool"]=="spawnAgent" && item["senderThreadId"].as_str().is_some_and(|id|self.owned.lock().contains(id)) {
                for id in item["receiverThreadIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if id.len()==36 && id.chars().all(|c|c.is_ascii_hexdigit() || c=='-') { self.owned.lock().insert(id.into()); }
                }
            }
        }
        // Global broadcasts, including thread/started, must not reveal other seats.
        if !parent_owned || foreign_thread(p, &self.owned.lock()) { return false }
        if let Some(id) = message.get("id").and_then(key) { self.approvals.insert(id); }
        true
    }
}

pub struct Gateway {
    pub url: String,
    alive: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<u64,TcpStream>>>,
}
impl Gateway {
    pub fn start(backend_url: String, backend_token: String, seat_token: String, thread: String) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "席の接続口を作れません。")?;
        let url = format!("ws://127.0.0.1:{}", listener.local_addr().map_err(|_| "席の接続口が不明です。")?.port());
        listener.set_nonblocking(true).map_err(|_| "席の接続口を設定できません。")?;
        let alive = Arc::new(AtomicBool::new(true));
        let sockets = Arc::new(Mutex::new(HashMap::new()));
        let live = alive.clone(); let handles = sockets.clone();
        let owned=Arc::new(Mutex::new(HashSet::from([thread.clone()])));
        thread::Builder::new().name("codex-seat-listener".into()).spawn(move || {
            let mut sequence = 0u64;
            while live.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream,_)) => {
                        if handles.lock().len() >= 16 { drop(stream); continue; }
                        sequence += 1; let id = sequence;
                        if let Ok(handle) = stream.try_clone() { handles.lock().insert(id,handle); }
                        let registry = handles.clone();
                        let owned=owned.clone();
                        let (url, private, token, own, live) = (backend_url.clone(),backend_token.clone(),seat_token.clone(),thread.clone(),live.clone());
                        thread::spawn(move || { let _ = serve(stream, &url, &private, &token, &own, &live, owned); registry.lock().remove(&id); });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        }).map_err(|_| "席の接続監視を始められません。")?;
        Ok(Self { url, alive, sockets })
    }
    pub fn end(&self) {
        self.alive.store(false, Ordering::Release);
        for (_,socket) in self.sockets.lock().drain() { let _ = socket.shutdown(std::net::Shutdown::Both); }
    }
}
impl Drop for Gateway { fn drop(&mut self) { self.end(); } }

fn serve(stream: TcpStream, url: &str, private: &str, token: &str, own: &str, alive: &AtomicBool, owned: Arc<Mutex<HashSet<String>>>) -> Result<(), ()> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| ())?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| ())?;
    let expected = format!("Bearer {token}");
    let mut local = tungstenite::accept_hdr(stream, move |req: &Request, response: Response| {
        if req.headers().get("Authorization").is_some_and(|h| h.as_bytes() == expected.as_bytes()) { Ok(response) }
        else { let mut err = ErrorResponse::new(Some("Unauthorized".into())); *err.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED; Err(err) }
    }).map_err(|_| ())?;
    if !alive.load(Ordering::Acquire) { return Err(()) }
    let mut req = url.into_client_request().map_err(|_| ())?;
    req.headers_mut().insert("Authorization", HeaderValue::from_str(&format!("Bearer {private}")).map_err(|_| ())?);
    let port = url.rsplit(':').next().and_then(|s| s.parse::<u16>().ok()).ok_or(())?;
    let native = TcpStream::connect_timeout(&([127,0,0,1],port).into(), Duration::from_secs(2)).map_err(|_| ())?;
    native.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| ())?;
    native.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| ())?;
    let (mut backend, _) = tungstenite::client(req, native).map_err(|_| ())?;
    local.get_mut().set_nonblocking(true).map_err(|_| ())?;
    backend.get_mut().set_nonblocking(true).map_err(|_| ())?;
    let mut policy = Policy::new(own);
    policy.owned = owned;
    while alive.load(Ordering::Acquire) {
        let mut progress = false;
        match read(&mut local)? {
            Some(mut value) => {
                progress = true;
                match policy.client(&value) {
                    Ok(()) => { if value["method"] == "account/read" { value["params"]["refreshToken"] = json!(false); } send(&mut backend,&value)?; }
                    Err(reason) => {
                        if value.get("method").is_some() && value.get("id").is_some() {
                            send(&mut local,&json!({"id":value["id"],"error":{"code":-32600,"message":reason}}))?;
                        } else { return Err(()) }
                    }
                }
            }
            None => {}
        }
        if let Some(value) = read(&mut backend)? {
            progress = true;
            if policy.backend(&value) { send(&mut local,&value)?; }
        }
        for socket in [&mut local,&mut backend] {
            match socket.flush() { Ok(()) => {}, Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}, Err(_) => return Err(()) }
        }
        if !progress { thread::sleep(Duration::from_millis(5)); }
    }
    Ok(())
}
fn read(socket: &mut WebSocket<TcpStream>) -> Result<Option<Value>, ()> {
    match socket.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text).map(Some).map_err(|_| ()),
        Ok(Message::Close(_)) => Err(()), Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(_) => Err(()),
    }
}
fn send(socket: &mut WebSocket<TcpStream>, value: &Value) -> Result<(), ()> {
    match socket.send(Message::text(value.to_string())) {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(_) => Err(()),
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn thread_methods_fail_closed_and_approvals_are_correlated_per_connection() {
        let mut a = Policy::new("A"); let mut b = Policy::new("B");
        assert!(a.client(&json!({"id":1,"method":"thread/read","params":{"threadId":"B"}})).is_err());
        for method in ["thread/list","thread/start","thread/fork","config/write","account/logout","remoteControl/disable","command/exec","future/method"] {
            assert!(a.client(&json!({"id":1,"method":method,"params":{}})).is_err(),"{method}");
        }
        assert!(!a.backend(&json!({"method":"thread/started","params":{"thread":{"id":"B"}}})));
        assert!(!a.backend(&json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"B"}})));
        assert!(b.backend(&json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"B"}})));
        assert!(a.client(&json!({"id":7,"result":{"decision":"accept"}})).is_err());
        assert!(b.client(&json!({"id":7,"result":{"decision":"accept"}})).is_ok());
        assert!(b.client(&json!({"id":7,"result":{"decision":"accept"}})).is_err());
        assert!(a.client(&json!({"id":1,"method":"thread/read","params":{"threadId":"A"}})).is_ok());
        assert!(!a.backend(&json!({"id":2,"result":{}})));
        assert!(a.backend(&json!({"id":1,"result":{}})));
        assert!(!a.backend(&json!({"id":1,"result":{}})));
    }
}
