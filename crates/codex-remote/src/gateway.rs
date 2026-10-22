//! Authenticated seat boundary in front of the private native backend.
//! Unknown methods fail closed. A connection owns both RPC directions' IDs.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{
    client::IntoClientRequest,
    handshake::server::{ErrorResponse, Request, Response},
    http::HeaderValue,
    Message, WebSocket,
};

fn key(id: &Value) -> Option<String> {
    (id.is_string() || id.is_number()).then(|| id.to_string())
}
fn thread_id(value: &Value) -> Option<&str> {
    value
        .get("threadId")
        .or_else(|| value.get("thread_id"))
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("thread")
                .and_then(|t| t.get("id"))
                .and_then(Value::as_str)
        })
}
fn foreign_thread(value: &Value, owned: &HashSet<String>) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| {
            if matches!(
                k.as_str(),
                "threadId" | "thread_id" | "conversationId" | "conversation_id" | "sourceThreadId"
            ) {
                v.as_str().is_none_or(|id| !owned.contains(id))
            } else {
                foreign_thread(v, owned)
            }
        }),
        Value::Array(a) => a.iter().any(|v| foreign_thread(v, owned)),
        _ => false,
    }
}

fn one_of(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|s| choices.contains(&s))
}

// Native 0.162.1 ThreadResumeParams. This adapter consumes configuration, not
// applies it: even serviceTier:null would otherwise clear launcher state.
fn attach_params(params: &Value) -> Result<Value, String> {
    let fields = params
        .as_object()
        .ok_or("resume params must be an object")?;
    let mut attach = serde_json::Map::new();
    let mut invalid = Vec::new();
    for (name, value) in fields {
        let valid = match name.as_str() {
            "threadId" => value.is_string(),
            "excludeTurns" => value.is_boolean(),
            "initialTurnsPage" => {
                value.is_null()
                    || value.as_object().is_some_and(|page| {
                        page.iter().all(|(key, v)| match key.as_str() {
                            "limit" => {
                                v.is_null() || v.as_u64().is_some_and(|n| n <= u32::MAX as u64)
                            }
                            "sortDirection" => v.is_null() || one_of(v, &["asc", "desc"]),
                            "itemsView" => {
                                v.is_null() || one_of(v, &["notLoaded", "summary", "full"])
                            }
                            _ => false,
                        })
                    })
            }
            // Consumed settings are checked by JSON type only, never by enum
            // value: the user's codex CLI is not pinned, and a value added by
            // a codex update must not stop seat startup. Nothing here is sent
            // upstream, so a value check would not strengthen the boundary.
            "model"
            | "modelProvider"
            | "serviceTier"
            | "cwd"
            | "permissions"
            | "baseInstructions"
            | "developerInstructions"
            | "sandbox"
            | "approvalsReviewer"
            | "personality" => value.is_null() || value.is_string(),
            "runtimeWorkspaceRoots" => {
                value.is_null()
                    || value
                        .as_array()
                        .is_some_and(|roots| roots.iter().all(Value::is_string))
            }
            "config" => value.is_null() || value.is_object(),
            "approvalPolicy" => value.is_null() || value.is_string() || value.is_object(),
            // Unknown non-null fields stay rejected: an unknown field may be an
            // operation (like history/path, which replace the selected thread's
            // persisted history), so dropping it silently is unsafe. A codex CLI
            // update may therefore need this list checked for compatibility.
            // Unknown null fields are tolerated for native optional-field drift.
            _ => value.is_null(),
        };
        if !valid {
            invalid.push(name.as_str());
        } else if matches!(
            name.as_str(),
            "threadId" | "excludeTurns" | "initialTurnsPage"
        ) {
            attach.insert(name.clone(), value.clone());
        }
    }
    // Field names only: values may carry secrets.
    if !invalid.is_empty() {
        return Err(format!(
            "invalid or unsupported seat attach field: {}",
            invalid.join(", ")
        ));
    }
    if !attach.get("threadId").is_some_and(Value::is_string) {
        return Err("this seat's thread ID is required".into());
    }
    Ok(Value::Object(attach))
}

// Only native relationship metadata can reconstruct an offline/cold seat's
// descendants. Even a backend ignoring ancestorThreadId cannot grant a foreign
// thread: every parent chain must terminate at the original seat root.
fn extend_descendants(owned: &mut HashSet<String>, root: &str, rows: &[Value]) {
    let mut verified = HashSet::from([root.to_string()]);
    loop {
        let before = verified.len();
        for row in rows {
            if let (Some(id), Some(parent)) = (row["id"].as_str(), row["parentThreadId"].as_str()) {
                if verified.contains(parent)
                    && id.len() == 36
                    && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                {
                    verified.insert(id.into());
                }
            }
        }
        if verified.len() == before {
            break;
        }
    }
    owned.extend(verified);
}

fn refresh_descendants(
    url: &str,
    token: &str,
    root: &str,
    owned: &Arc<Mutex<HashSet<String>>>,
    alive: &AtomicBool,
) -> Result<(), ()> {
    let port = url
        .rsplit(':')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(())?;
    let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
        .map_err(|_| ())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| ())?;
    let mut req = url.into_client_request().map_err(|_| ())?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ())?,
    );
    let (mut socket, _) = tungstenite::client(req, stream).map_err(|_| ())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let rpc = |socket: &mut WebSocket<TcpStream>,
               id: u64,
               method: &str,
               params: Value|
     -> Result<Value, ()> {
        socket
            .send(Message::text(
                json!({"id":id,"method":method,"params":params}).to_string(),
            ))
            .map_err(|_| ())?;
        loop {
            if !alive.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(());
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
                    if value.get("method").is_none() && value["id"] == id {
                        return if value.get("error").is_some() {
                            Err(())
                        } else {
                            Ok(value["result"].clone())
                        };
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return Err(()),
            }
        }
    };
    rpc(
        &mut socket,
        1,
        "initialize",
        json!({"clientInfo":{"name":"pullcept-seat-scope","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
    )?;
    socket
        .send(Message::text(json!({"method":"initialized"}).to_string()))
        .map_err(|_| ())?;
    let mut rows = Vec::new();
    let mut cursor = Value::Null;
    let mut id = 1;
    loop {
        id += 1;
        let result = rpc(
            &mut socket,
            id,
            "thread/list",
            json!({"ancestorThreadId":root,"sourceKinds":["subAgent","subAgentReview","subAgentCompact","subAgentThreadSpawn","subAgentOther"],"limit":100,"cursor":cursor,"useStateDbOnly":true}),
        )?;
        rows.extend(result["data"].as_array().ok_or(())?.iter().cloned());
        if rows.len() > 4096 {
            return Err(());
        }
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    extend_descendants(&mut owned.lock(), root, &rows);
    let _ = socket.close(None);
    Ok(())
}

/// Per-client correlation, deliberately not shared across TUI connections.
pub struct Policy {
    owned: Arc<Mutex<HashSet<String>>>,
    cwd: Option<std::path::PathBuf>,
    pending: HashMap<String, String>,
    approvals: HashSet<String>,
}
impl Policy {
    pub fn new(thread: &str) -> Self {
        Self {
            cwd: None,
            owned: Arc::new(Mutex::new(HashSet::from([thread.to_string()]))),
            pending: HashMap::new(),
            approvals: HashSet::new(),
        }
    }
    /// Validate the original request before projecting the actual upstream message.
    pub fn client(&mut self, message: &Value) -> Result<Value, String> {
        let mut upstream = message.clone();
        if let Some(method) = message["method"].as_str() {
            if method == "initialized" && message.get("id").is_none() {
                return Ok(upstream);
            }
            let id = message
                .get("id")
                .and_then(key)
                .ok_or("request ID required")?;
            if self.pending.contains_key(&id) {
                return Err("request ID already pending".into());
            }
            let p = &message["params"];
            if foreign_thread(p, &self.owned.lock()) {
                return Err("another seat's thread is not accessible".into());
            }
            let cwd_read = matches!(method, "skills/list" | "hooks/list");
            if cwd_read {
                let Some(own) = self.cwd.as_ref() else {
                    return Err("seat working directory unavailable".into());
                };
                if p["cwds"].as_array().is_none_or(|paths| {
                    paths.is_empty()
                        || paths.iter().any(|v| {
                            v.as_str()
                                .and_then(|v| std::fs::canonicalize(v).ok())
                                .as_ref()
                                != Some(own)
                        })
                }) {
                    return Err("only this seat's working directory is accessible".into());
                }
            }
            let scoped = matches!(
                method,
                "thread/resume"
                    | "thread/read"
                    | "thread/unsubscribe"
                    | "thread/name/set"
                    | "thread/turns/list"
                    | "thread/items/list"
                    | "thread/compact/start"
                    | "thread/settings/update"
                    | "thread/revert"
                    | "thread/inject_items"
                    | "turn/start"
                    | "turn/steer"
                    | "turn/interrupt"
                    | "mcpServerStatus/list"
                    | "mcpServer/tool/call"
                    | "mcpResource/read"
                    | "thread/backgroundTerminals/list"
                    | "thread/backgroundTerminals/clean"
                    | "thread/backgroundTerminals/terminate"
                    | "thread/goal/get"
                    | "thread/goal/set"
                    | "thread/goal/clear"
                    | "thread/shellCommand"
                    | "thread/attachment/list"
                    | "review/start"
            );
            let global = matches!(
                method,
                "initialize"
                    | "model/list"
                    | "account/read"
                    | "account/rateLimits/read"
                    | "collaborationMode/list"
                    | "experimentalFeature/list"
                    | "configRequirements/read"
            );
            if scoped {
                if thread_id(p).is_none_or(|id| !self.owned.lock().contains(id)) {
                    return Err("this seat's thread ID is required".into());
                }
                if method == "thread/resume" {
                    upstream["params"] = attach_params(p)?;
                }
            } else if !global && !cwd_read {
                return Err("method is unavailable through this seat".into());
            }
            self.pending.insert(id, method.into());
            Ok(upstream)
        } else {
            let id = message
                .get("id")
                .and_then(key)
                .ok_or("response ID required")?;
            if !self.approvals.remove(&id) {
                return Err("approval ID does not belong to this connection".into());
            }
            Ok(upstream)
        }
    }
    pub fn backend(&mut self, message: &Value) -> bool {
        if message.get("method").is_none() {
            return message
                .get("id")
                .and_then(key)
                .is_some_and(|id| self.pending.remove(&id).is_some());
        }
        let p = &message["params"];
        if matches!(
            message["method"].as_str(),
            Some("account/updated" | "account/rateLimits/updated")
        ) && message.get("id").is_none()
        {
            return true;
        }
        // Learn descendants only from native spawn events on an owned parent.
        let parent_owned = thread_id(p).is_some_and(|id| self.owned.lock().contains(id));
        if parent_owned
            && matches!(
                message["method"].as_str(),
                Some("item/started" | "item/completed")
            )
        {
            let item = &p["item"];
            if item["type"] == "collabAgentToolCall"
                && item["tool"] == "spawnAgent"
                && item["senderThreadId"]
                    .as_str()
                    .is_some_and(|id| self.owned.lock().contains(id))
            {
                for id in item["receiverThreadIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                        self.owned.lock().insert(id.into());
                    }
                }
            }
        }
        // Global broadcasts, including thread/started, must not reveal other seats.
        if !parent_owned || foreign_thread(p, &self.owned.lock()) {
            return false;
        }
        if let Some(id) = message.get("id").and_then(key) {
            self.approvals.insert(id);
        }
        true
    }
}

pub struct Gateway {
    pub url: String,
    alive: Arc<AtomicBool>,
    sockets: Arc<Mutex<HashMap<u64, TcpStream>>>,
}
impl Gateway {
    pub fn start(
        backend_url: String,
        backend_token: String,
        seat_token: String,
        thread: String,
        cwd: std::path::PathBuf,
        disabled_hooks: Vec<String>,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| "席の接続口を作れません。")?;
        let url = format!(
            "ws://127.0.0.1:{}",
            listener
                .local_addr()
                .map_err(|_| "席の接続口が不明です。")?
                .port()
        );
        listener
            .set_nonblocking(true)
            .map_err(|_| "席の接続口を設定できません。")?;
        let alive = Arc::new(AtomicBool::new(true));
        let sockets = Arc::new(Mutex::new(HashMap::new()));
        let live = alive.clone();
        let handles = sockets.clone();
        let owned = Arc::new(Mutex::new(HashSet::from([thread.clone()])));
        thread::Builder::new()
            .name("codex-seat-listener".into())
            .spawn(move || {
                let mut sequence = 0u64;
                while live.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if handles.lock().len() >= 16 {
                                drop(stream);
                                continue;
                            }
                            sequence += 1;
                            let id = sequence;
                            if let Ok(handle) = stream.try_clone() {
                                handles.lock().insert(id, handle);
                            }
                            let registry = handles.clone();
                            let owned = owned.clone();
                            let cwd = cwd.clone();
                            let disabled_hooks = disabled_hooks.clone();
                            let (url, private, token, own, live) = (
                                backend_url.clone(),
                                backend_token.clone(),
                                seat_token.clone(),
                                thread.clone(),
                                live.clone(),
                            );
                            thread::spawn(move || {
                                let _ = serve(
                                    stream,
                                    &url,
                                    &private,
                                    &token,
                                    &own,
                                    &live,
                                    owned,
                                    cwd,
                                    &disabled_hooks,
                                );
                                registry.lock().remove(&id);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| "席の接続監視を始められません。")?;
        Ok(Self {
            url,
            alive,
            sockets,
        })
    }
    pub fn end(&self) {
        self.alive.store(false, Ordering::Release);
        for (_, socket) in self.sockets.lock().drain() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.end();
    }
}

fn serve(
    stream: TcpStream,
    url: &str,
    private: &str,
    token: &str,
    own: &str,
    alive: &AtomicBool,
    owned: Arc<Mutex<HashSet<String>>>,
    cwd: std::path::PathBuf,
    disabled_hooks: &[String],
) -> Result<(), ()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| ())?;
    let expected = format!("Bearer {token}");
    let mut local = tungstenite::accept_hdr(stream, move |req: &Request, response: Response| {
        if req
            .headers()
            .get("Authorization")
            .is_some_and(|h| h.as_bytes() == expected.as_bytes())
        {
            Ok(response)
        } else {
            let mut err = ErrorResponse::new(Some("Unauthorized".into()));
            *err.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED;
            Err(err)
        }
    })
    .map_err(|_| ())?;
    if !alive.load(Ordering::Acquire) {
        return Err(());
    }
    // Older backends may not implement relationship queries. Keep the root
    // usable and fail closed for unknown children if metadata is unavailable.
    let _ = refresh_descendants(url, private, own, &owned, alive);
    let mut req = url.into_client_request().map_err(|_| ())?;
    req.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {private}")).map_err(|_| ())?,
    );
    let port = url
        .rsplit(':')
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(())?;
    let native = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
        .map_err(|_| ())?;
    native
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| ())?;
    native
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| ())?;
    let (mut backend, _) = tungstenite::client(req, native).map_err(|_| ())?;
    local.get_mut().set_nonblocking(true).map_err(|_| ())?;
    backend.get_mut().set_nonblocking(true).map_err(|_| ())?;
    let mut policy = Policy::new(own);
    policy.owned = owned;
    policy.cwd = std::fs::canonicalize(cwd).ok();
    while alive.load(Ordering::Acquire) {
        let mut progress = false;
        match read(&mut local)? {
            Some(value) => {
                progress = true;
                match policy.client(&value) {
                    Ok(mut value) => {
                        if value["method"] == "account/read" {
                            value["params"]["refreshToken"] = json!(false);
                        }
                        send(&mut backend, &value)?;
                    }
                    Err(reason) => {
                        if value.get("method").is_some() && value.get("id").is_some() {
                            send(
                                &mut local,
                                &json!({"id":value["id"],"error":{"code":-32600,"message":reason}}),
                            )?;
                        } else {
                            return Err(());
                        }
                    }
                }
            }
            None => {}
        }
        if let Some(mut value) = read(&mut backend)? {
            progress = true;
            if value.get("method").is_none()
                && value
                    .get("id")
                    .and_then(key)
                    .and_then(|id| policy.pending.get(&id))
                    .is_some_and(|method| method == "thread/resume")
                && value.get("error").is_none()
            {
                // Subscription is already installed; snapshots cover earlier
                // children, and subsequent spawn notifications cover the race.
                let _ = refresh_descendants(url, private, own, &policy.owned, alive);
            }
            if value.get("method").is_none()
                && value
                    .get("id")
                    .and_then(key)
                    .and_then(|id| policy.pending.get(&id))
                    .is_some_and(|method| method == "hooks/list")
            {
                if let Some(data) = value["result"]["data"].as_array_mut() {
                    for entry in data {
                        for hook in entry["hooks"].as_array_mut().into_iter().flatten() {
                            if hook["key"].as_str().is_some_and(|k| {
                                disabled_hooks.iter().any(|disabled| disabled == k)
                            }) {
                                hook["enabled"] = json!(false);
                            }
                        }
                    }
                }
            }
            if policy.backend(&value) {
                send(&mut local, &value)?;
            }
        }
        for socket in [&mut local, &mut backend] {
            match socket.flush() {
                Ok(()) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(()),
            }
        }
        if !progress {
            thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}
fn read(socket: &mut WebSocket<TcpStream>) -> Result<Option<Value>, ()> {
    match socket.read() {
        Ok(Message::Text(text)) => serde_json::from_str(&text).map(Some).map_err(|_| ()),
        Ok(Message::Close(_)) => Err(()),
        Ok(_) => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attach_consumes_settings_and_null_clears_in_actual_upstream_message() {
        let params = json!({"threadId":"A", "excludeTurns":true,
            "initialTurnsPage":{"limit":12,"sortDirection":"desc","itemsView":"summary"},
            "model":"different", "modelProvider":"different", "serviceTier":null,
            "cwd":"other", "runtimeWorkspaceRoots":[std::env::temp_dir()],
            "approvalPolicy":{"granular":{"mcp_elicitations":false,"rules":true,"sandbox_approval":false}},
            "approvalsReviewer":"auto_review", "sandbox":"danger-full-access", "permissions":"other",
            "config":{"model_reasoning_effort":"medium","mcp_servers.fake":{"command":"ignored"}},
            "baseInstructions":"other", "developerInstructions":"other", "personality":"friendly",
            "history":null,"path":null,"future":null});
        let expected = json!({"id":1,"method":"thread/resume","params":{
            "threadId":"A","excludeTurns":true,"initialTurnsPage":params["initialTurnsPage"]}});
        let mut policy = Policy::new("A");
        assert_eq!(
            policy
                .client(&json!({"id":1,"method":"thread/resume","params":params}))
                .unwrap(),
            expected
        );
        assert!(policy.backend(&json!({"id":1,"result":{"model":"launcher"}})));
        for name in [
            "model",
            "modelProvider",
            "serviceTier",
            "cwd",
            "runtimeWorkspaceRoots",
            "approvalPolicy",
            "approvalsReviewer",
            "sandbox",
            "permissions",
            "config",
            "baseInstructions",
            "developerInstructions",
            "personality",
        ] {
            for value in [Value::Null, json!(17)] {
                let mut params = json!({"threadId":"A"});
                params[name] = value.clone();
                let result = Policy::new("A")
                    .client(&json!({"id":1,"method":"thread/resume","params":params}));
                if value.is_null() {
                    assert_eq!(result.unwrap()["params"], json!({"threadId":"A"}), "{name}");
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        format!("invalid or unsupported seat attach field: {name}")
                    );
                }
            }
        }
    }
    #[test]
    fn attach_rejects_history_unknown_malformed_and_original_foreign_scope() {
        for extra in [
            json!({"history":[]}),
            json!({"path":"other"}),
            json!({"future":true}),
            json!({"excludeTurns":null}),
            json!({"excludeTurns":1}),
            json!({"initialTurnsPage":{"limit":-1}}),
            json!({"initialTurnsPage":{"limit":4294967296u64}}),
            json!({"initialTurnsPage":{"sortDirection":"sideways"}}),
            json!({"initialTurnsPage":{"itemsView":"other"}}),
            json!({"initialTurnsPage":{"future":null}}),
            json!({"runtimeWorkspaceRoots":"/root"}),
            json!({"runtimeWorkspaceRoots":[1]}),
            json!({"sandbox":true}),
            json!({"approvalsReviewer":{"kind":"user"}}),
            json!({"personality":["friendly"]}),
            json!({"approvalPolicy":false}),
            json!({"config":"model=other"}),
            json!({"config":{"nested":[{"threadId":"B"}]}}),
        ] {
            let mut params = json!({"threadId":"A"});
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let mut policy = Policy::new("A");
            assert!(
                policy
                    .client(&json!({"id":1,"method":"thread/resume","params":params}))
                    .is_err(),
                "{extra}"
            );
            assert!(policy.pending.is_empty());
        }
        assert_eq!(Policy::new("A").client(&json!({"id":1,"method":"thread/resume","params":{"threadId":"A","config":{"threadId":"B"}}})).unwrap_err(), "another seat's thread is not accessible");
        assert!(Policy::new("A")
            .client(&json!({"id":1,"method":"thread/resume","params":{"thread_id":"A"}}))
            .is_err());
    }
    #[test]
    fn attach_accepts_unrecognized_setting_values_by_type() {
        // Values a newer codex CLI might send: only the JSON type is checked.
        let params = json!({"threadId":"A",
            "sandbox":"future-sandbox", "approvalsReviewer":"future_reviewer",
            "personality":"future", "approvalPolicy":"on-failure",
            "runtimeWorkspaceRoots":["relative"]});
        assert_eq!(
            Policy::new("A")
                .client(&json!({"id":1,"method":"thread/resume","params":params}))
                .unwrap()["params"],
            json!({"threadId":"A"})
        );
        let params = json!({"threadId":"A",
            "approvalPolicy":{"granular":{"rules":true,"future_scope":"ask"}}});
        assert_eq!(
            Policy::new("A")
                .client(&json!({"id":1,"method":"thread/resume","params":params}))
                .unwrap()["params"],
            json!({"threadId":"A"})
        );
    }
    #[test]
    fn attach_rejection_names_every_offending_field_without_values() {
        let params = json!({"threadId":"A", "sandbox":"future-sandbox",
            "future":"secret-value", "personality":17, "later":null});
        let mut policy = Policy::new("A");
        let error = policy
            .client(&json!({"id":1,"method":"thread/resume","params":params}))
            .unwrap_err();
        assert_eq!(
            error,
            "invalid or unsupported seat attach field: future, personality"
        );
        assert!(!error.contains("secret-value"));
        assert!(policy.pending.is_empty());
    }
    #[test]
    fn thread_methods_fail_closed_and_approvals_are_correlated_per_connection() {
        let mut a = Policy::new("A");
        let mut b = Policy::new("B");
        assert!(a
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":"B"}}))
            .is_err());
        for method in [
            "thread/list",
            "thread/start",
            "thread/fork",
            "config/write",
            "account/logout",
            "remoteControl/disable",
            "command/exec",
            "future/method",
        ] {
            assert!(
                a.client(&json!({"id":1,"method":method,"params":{}}))
                    .is_err(),
                "{method}"
            );
        }
        assert!(!a.backend(&json!({"method":"thread/started","params":{"thread":{"id":"B"}}})));
        assert!(!a.backend(&json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"B"}})));
        assert!(b.backend(&json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"B"}})));
        assert!(a
            .client(&json!({"id":7,"result":{"decision":"accept"}}))
            .is_err());
        assert!(b
            .client(&json!({"id":7,"result":{"decision":"accept"}}))
            .is_ok());
        assert!(b
            .client(&json!({"id":7,"result":{"decision":"accept"}}))
            .is_err());
        assert!(a
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":"A"}}))
            .is_ok());
        assert!(!a.backend(&json!({"id":2,"result":{}})));
        assert!(a.backend(&json!({"id":1,"result":{}})));
        assert!(!a.backend(&json!({"id":1,"result":{}})));
    }
    #[test]
    fn socket_auth_and_cross_seat_approval_ids_are_enforced() {
        use std::sync::mpsc;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let backend_url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (done_tx, done_rx) = mpsc::channel();
        let backend = thread::spawn(move || {
            let mut workers = Vec::new();
            let mut main_index = 0;
            while main_index < 2 {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut ws = tungstenite::accept_hdr(stream, |r: &Request, response: Response| {
                    assert_eq!(r.headers()["Authorization"], "Bearer private");
                    Ok(response)
                })
                .unwrap();
                let read = |ws: &mut WebSocket<TcpStream>| {
                    serde_json::from_str::<Value>(&ws.read().unwrap().into_text().unwrap()).unwrap()
                };
                let init = read(&mut ws);
                ws.send(Message::text(
                    json!({"id":init["id"],"result":{}}).to_string(),
                ))
                .unwrap();
                assert_eq!(read(&mut ws)["method"], "initialized");
                if init["params"]["clientInfo"]["name"] == "pullcept-seat-scope" {
                    let query = read(&mut ws);
                    assert_eq!(query["method"], "thread/list");
                    ws.send(Message::text(
                        json!({"id":query["id"],"result":{"data":[],"nextCursor":null}})
                            .to_string(),
                    ))
                    .unwrap();
                    continue;
                }
                let index = main_index;
                main_index += 1;
                let tx = done_tx.clone();
                workers.push(thread::spawn(move||{
                    let request=read(&mut ws);assert_eq!(request["method"],"thread/read");
                    ws.send(Message::text(json!({"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":"B"}}).to_string())).unwrap();
                    ws.send(Message::text(json!({"id":request["id"],"result":{"thread":{"id":if index==0{"A"}else{"B"}}}}).to_string())).unwrap();
                    if index==0 {assert!(ws.read().is_err(),"forged approval must not reach backend");tx.send("forged-rejected").unwrap();}
                    else {let response=read(&mut ws);assert_eq!(response["id"],77);assert_eq!(response["result"]["decision"],"accept");tx.send("own-accepted").unwrap();}
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        let cwd = std::env::current_dir().unwrap();
        let a = Gateway::start(
            backend_url.clone(),
            "private".into(),
            "seat-A".into(),
            "A".into(),
            cwd.clone(),
            vec![],
        )
        .unwrap();
        let b = Gateway::start(
            backend_url,
            "private".into(),
            "seat-B".into(),
            "B".into(),
            cwd,
            vec![],
        )
        .unwrap();
        let connect = |url: &str, token: &str| {
            let mut req = url.into_client_request().unwrap();
            req.headers_mut().insert(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            );
            tungstenite::connect(req)
        };
        assert!(connect(&a.url, "seat-B").is_err());
        assert!(connect(&a.url, "private").is_err());
        let mut clients = Vec::new();
        for (url, token, id) in [(&a.url, "seat-A", "A"), (&b.url, "seat-B", "B")] {
            let (mut ws, _) = connect(url, token).unwrap();
            if let tungstenite::stream::MaybeTlsStream::Plain(stream) = ws.get_ref() {
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
            }
            ws.send(Message::text(
                json!({"id":1,"method":"initialize","params":{}}).to_string(),
            ))
            .unwrap();
            let init: Value =
                serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(init["id"], 1);
            ws.send(Message::text(json!({"method":"initialized"}).to_string()))
                .unwrap();
            ws.send(Message::text(
                json!({"id":2,"method":"thread/read","params":{"threadId":id}}).to_string(),
            ))
            .unwrap();
            clients.push(ws);
        }
        let a_reply: Value =
            serde_json::from_str(&clients[0].read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(a_reply["id"], 2);
        clients[0]
            .send(Message::text(
                json!({"id":77,"result":{"decision":"accept"}}).to_string(),
            ))
            .unwrap();
        let b_approval: Value =
            serde_json::from_str(&clients[1].read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(
            b_approval["method"],
            "item/commandExecution/requestApproval"
        );
        clients[1].read().unwrap();
        clients[1]
            .send(Message::text(
                json!({"id":77,"result":{"decision":"accept"}}).to_string(),
            ))
            .unwrap();
        let mut results = vec![
            done_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            done_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        ];
        results.sort();
        assert_eq!(results, vec!["forged-rejected", "own-accepted"]);
        a.end();
        b.end();
        drop(clients);
        backend.join().unwrap();
    }
    #[test]
    fn only_native_spawn_on_owned_parent_grants_child_scope() {
        let child = "01a110c0-c898-75f0-be7d-ffff2e05b9f5";
        let mut a = Policy::new("A");
        let spawn = |parent: &str| json!({"method":"item/completed","params":{"threadId":parent,"item":{"type":"collabAgentToolCall","tool":"spawnAgent","senderThreadId":parent,"receiverThreadIds":[child]}}});
        assert!(!a.backend(&spawn("B")));
        assert!(a
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":child}}))
            .is_err());
        assert!(a.backend(&spawn("A")));
        assert!(a
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":child}}))
            .is_ok());
    }
    #[test]
    fn authoritative_snapshot_reconstructs_offline_children_without_granting_foreign_ids() {
        let child = "01a110c0-c898-75f0-be7d-ffff2e05b9f5";
        let grandchild = "01a110c0-c898-75f0-be7d-ffff2e05b9f6";
        let foreign = "01a110c0-c898-75f0-be7d-ffff2e05b9f7";
        let rows = vec![
            json!({"id":grandchild,"parentThreadId":child}),
            json!({"id":foreign,"parentThreadId":"B"}),
            json!({"id":child,"parentThreadId":"A"}),
        ];
        // A newly created gateway after a cold resume has no live spawn events.
        let mut reconnected = Policy::new("A");
        extend_descendants(&mut reconnected.owned.lock(), "A", &rows);
        assert!(reconnected
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":grandchild}}))
            .is_ok());
        assert!(reconnected
            .client(&json!({"id":2,"method":"thread/read","params":{"threadId":foreign}}))
            .is_err());
        assert!(reconnected.backend(&json!({"id":77,"method":"item/commandExecution/requestApproval","params":{"threadId":child}})));
        assert!(reconnected
            .client(&json!({"id":77,"result":{"decision":"accept"}}))
            .is_ok());
        // Sharing a gateway across TUI reconnects retains the verified set.
        let mut next = Policy::new("A");
        next.owned = reconnected.owned.clone();
        assert!(next
            .client(&json!({"id":1,"method":"thread/read","params":{"threadId":child}}))
            .is_ok());
    }
}
