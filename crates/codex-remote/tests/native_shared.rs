//! Model-free native contract checks. Set CODEX_TEST_NATIVE to the pinned CLI.
use codex_remote::{
    credential::CredentialFile,
    gateway::Gateway,
    pool::{Pool, ThreadLease, Threads},
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, http::HeaderValue, Message, WebSocket};
struct Native {
    child: Mutex<Child>,
    url: String,
    token: String,
}
impl Drop for Native {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap();
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
}
fn spawn(exe: &str, root: &Path) -> Native {
    std::fs::create_dir_all(root.join("home")).unwrap();
    std::fs::create_dir_all(root.join("work")).unwrap();
    let token = "fixture-private-backend-key".to_string();
    let file = root.join("native-token");
    std::fs::write(&file, &token).unwrap();
    let hook = root.join("style-hook.mjs");
    std::fs::write(
        &hook,
        "import{writeFileSync}from'node:fs';writeFileSync(process.argv[2],'ran');",
    )
    .unwrap();
    let command = format!(
        "node \"{}\" \"{}\"",
        hook.display(),
        root.join("style-ran").display()
    );
    let mut cmd = Command::new(exe);
    cmd.args(["-c",&format!("hooks.SessionStart=[{{matcher='startup|resume|clear|compact',hooks=[{{type='command',command={},additionalContextLimit=0}}]}}]",mcp_config::codex::instruction_value(&command))]);
    cmd.args(["-c","cli_auth_credentials_store=\"file\"","-c","project_doc_max_bytes=0","-c","features.codex_hooks=true","-c","model_provider=\"fixture\"","-c","model=\"gpt-5.4\"","-c","model_providers.fixture={name=\"fixture\",base_url=\"http://127.0.0.1:9/v1\",wire_api=\"responses\",requires_openai_auth=false}","-c","thread_unload_delay_secs=0","app-server","--listen","ws://127.0.0.1:0","--ws-auth","capability-token","--ws-token-file"]).arg(file)
        .env("CODEX_HOME",root.join("home")).env("RUST_LOG","off").current_dir(root.join("work")).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars().filter(|(k, _)| {
        k.starts_with("PULLCEPT_") || k == "OPENAI_API_KEY" || k == "CODEX_API_KEY"
    }) {
        cmd.env_remove(k);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd.spawn().unwrap();
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    for stream in [Box::new(out) as Box<dyn Read + Send>, Box::new(err)] {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Some(port) = mcp_config::codex::app_server::listening_port(&line) {
                    let _ = tx.send(port);
                }
            }
        });
    }
    let mut native = Native {
        child: Mutex::new(child),
        url: String::new(),
        token,
    };
    let port = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("native listener");
    {
        native.url = format!("ws://127.0.0.1:{port}");
        native
    }
}
struct Client {
    ws: WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    next: u64,
    events: Vec<Value>,
}
impl Client {
    fn connect(url: &str, token: &str) -> Self {
        let mut req = url.into_client_request().unwrap();
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let (ws, _) = tungstenite::connect(req).unwrap();
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        }
        let mut c = Self {
            ws,
            next: 0,
            events: Vec::new(),
        };
        c.rpc("initialize",json!({"clientInfo":{"name":"fixture","version":"1"},"capabilities":{"experimentalApi":true}})).unwrap();
        c.send(json!({"method":"initialized"}));
        c
    }
    fn send(&mut self, value: Value) {
        self.ws.send(Message::text(value.to_string())).unwrap();
    }
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        self.send(json!({"id":id,"method":method,"params":params}));
        loop {
            if let Message::Text(text) = self.ws.read().unwrap() {
                let v: Value = serde_json::from_str(&text).unwrap();
                if v["id"] == id && v.get("method").is_none() {
                    return if v.get("error").is_some() {
                        Err(v["error"].clone())
                    } else {
                        Ok(v["result"].clone())
                    };
                }
                self.events.push(v);
            }
        }
    }
}
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "pullcept-native-shared-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn definition(script: &Path, marker: &str, cwd: &Path) -> Value {
    json!({"command":"node","args":[script],"env":{"FIXTURE_MARKER":marker},"cwd":cwd,"enabled":true})
}
fn tools(c: &mut Client, id: &str) -> Vec<String> {
    c.rpc("mcpServerStatus/list", json!({"threadId":id,"limit":100}))
        .unwrap()["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["tools"].as_object().is_some_and(|t| !t.is_empty()))
        .map(|s| s["name"].as_str().unwrap().into())
        .collect()
}
#[test]
fn native_shared_threads_gateway_phone_detach_cold_resume_and_real_tui() {
    let Ok(exe) = std::env::var("CODEX_TEST_NATIVE") else {
        eprintln!("CODEX_TEST_NATIVE unset; native fixture not enabled");
        return;
    };
    assert!(Command::new(&exe)
        .arg("--version")
        .output()
        .unwrap()
        .stdout
        .starts_with(b"codex-cli 0.162.1"));
    let root = root();
    let a_cwd = root.join("A");
    let b_cwd = root.join("B");
    std::fs::create_dir_all(&a_cwd).unwrap();
    std::fs::create_dir_all(&b_cwd).unwrap();
    let fake = root.join("fake.mjs");
    std::fs::write(&fake,r#"import readline from 'node:readline';for await (const line of readline.createInterface({input:process.stdin})){let m=JSON.parse(line);if(m.id===undefined)continue;let result=m.method==='initialize'?{protocolVersion:'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}}:m.method==='tools/list'?{tools:[{name:'echo',description:'fixture',inputSchema:{type:'object'}}]}:m.method==='tools/call'?{content:[{type:'text',text:JSON.stringify({marker:(process.env.PULLCEPT_AGENT_NAME??process.env.FIXTURE_MARKER),cwd:process.cwd(),pid:process.pid})}]}:{};console.log(JSON.stringify({jsonrpc:'2.0',id:m.id,result}));}"#).unwrap();
    let pool = Pool::default();
    let backend = pool
        .acquire("fixture-home", "account", "process", || {
            Ok(spawn(&exe, &root))
        })
        .unwrap();
    let same = pool
        .acquire("fixture-home", "account", "process", || {
            panic!("must not spawn twice")
        })
        .unwrap();
    assert!(Arc::ptr_eq(&backend, &same));
    drop(same);
    let mut owner_a = Client::connect(&backend.url, &backend.token);
    let mut owner_b = Client::connect(&backend.url, &backend.token);
    let hooks = owner_a.rpc("hooks/list", json!({"cwds":[a_cwd]})).unwrap();
    let hook_keys: Vec<String> = hooks["data"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|v| v["hooks"].as_array().unwrap())
        .filter_map(|h| h["key"].as_str().map(str::to_string))
        .collect();
    assert!(
        !hook_keys.is_empty(),
        "fixture style hook must be discoverable"
    );
    let mut trust = serde_json::Map::new();
    for entry in hooks["data"].as_array().unwrap() {
        for hook in entry["hooks"].as_array().unwrap() {
            trust.insert(
                hook["key"].as_str().unwrap().into(),
                json!({"trusted_hash":hook["currentHash"]}),
            );
        }
    }
    owner_a.rpc("config/batchWrite",json!({"edits":[{"keyPath":"hooks.state","value":trust,"mergeStrategy":"upsert"}],"reloadUserConfig":true})).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let bootstrap = repo.join("sidecar/src/shared-seat.mjs");
    let runner = repo.join("node_modules/tsx/dist/cli.mjs");
    let file_a = CredentialFile::create(
        root.join("seat-a.json"),
        &json!({"PULLCEPT_AGENT_NAME":"A","PULLCEPT_ROOM_TOKEN":"fixture-room-secret-A"}),
    )
    .unwrap();
    let file_b = CredentialFile::create(
        root.join("seat-b.json"),
        &json!({"PULLCEPT_AGENT_NAME":"B","PULLCEPT_ROOM_TOKEN":"fixture-room-secret-B"}),
    )
    .unwrap();
    let start = |cwd: &Path, marker: &str, file: &CredentialFile| {
        let mut config = json!({});
        mcp_config::codex::app_server::shared_room_config(
            &mut config,
            &format!("fixture_{}", marker.to_lowercase()),
            &bootstrap,
            &file.path,
            &runner,
            &fake,
            &hook_keys,
        );
        config["model_reasoning_effort"] = json!("low");
        json!({"model":"gpt-5.4","serviceTier":null,"cwd":cwd,"developerInstructions":format!("fixture-{marker}"),"historyMode":"paginated","approvalPolicy":"never","sandbox":"read-only","config":config})
    };
    let a = owner_a
        .rpc("thread/start", start(&a_cwd, "A", &file_a))
        .unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b = owner_b
        .rpc("thread/start", start(&b_cwd, "B", &file_b))
        .unwrap()["thread"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(a, b);
    for (owner, id) in [(&mut owner_a, &a), (&mut owner_b, &b)] {
        owner
            .rpc(
                "thread/inject_items",
                mcp_config::codex::app_server::inject_params(id),
            )
            .unwrap();
    }
    assert!(
        !root.join("style-ran").exists(),
        "no model turn or style handler execution is part of startup fixture"
    );
    let marker_deadline = Instant::now() + Duration::from_secs(10);
    while !file_a.marker().is_file() || !file_b.marker().is_file() {
        assert!(
            Instant::now() < marker_deadline,
            "actual shared bootstrap must run"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let registry = Arc::new(Threads::default());
    let lease_a = ThreadLease::claim(&registry, &a).unwrap();
    let _lease_b = ThreadLease::claim(&registry, &b).unwrap();
    let gateway_a = Gateway::start(
        backend.url.clone(),
        backend.token.clone(),
        "seat-A".into(),
        a.clone(),
        a_cwd.clone(),
        hook_keys.clone(),
    )
    .unwrap();
    let gateway_b = Gateway::start(
        backend.url.clone(),
        backend.token.clone(),
        "seat-B".into(),
        b.clone(),
        b_cwd.clone(),
        hook_keys.clone(),
    )
    .unwrap();
    let mut ca = Client::connect(&gateway_a.url, "seat-A");
    let mut cb = Client::connect(&gateway_b.url, "seat-B");
    for (c, id, name) in [(&mut ca, &a, "fixture_a"), (&mut cb, &b, "fixture_b")] {
        c.rpc("thread/resume", json!({"threadId":id})).unwrap();
        assert_eq!(tools(c, id), vec![name.to_string()]);
        let r = c
            .rpc(
                "mcpServer/tool/call",
                json!({"threadId":id,"server":name,"tool":"echo","arguments":{}}),
            )
            .unwrap();
        let value: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(value["marker"], if name.ends_with('a') { "A" } else { "B" });
    }
    assert!(ca.rpc("thread/read", json!({"threadId":b})).is_err());
    assert!(ca
        .rpc(
            "mcpServer/tool/call",
            json!({"threadId":b,"server":"fixture_b","tool":"echo"})
        )
        .is_err());
    assert!(ca.rpc("thread/list", json!({})).is_err());
    assert!(cb
        .rpc("thread/name/set", json!({"threadId":b,"name":"fixture-B"}))
        .is_ok());
    for e in &ca.events {
        assert_ne!(e["params"]["threadId"].as_str(), Some(b.as_str()));
        assert_ne!(e["params"]["thread"]["id"].as_str(), Some(b.as_str()));
    }
    let mut phone = Client::connect(&backend.url, &backend.token);
    phone.rpc("thread/resume", json!({"threadId":a})).unwrap();
    gateway_a.end();
    drop(file_a);
    drop(ca);
    owner_a
        .rpc("thread/unsubscribe", json!({"threadId":a}))
        .unwrap();
    drop(owner_a);
    drop(lease_a);
    let loaded = phone
        .rpc("thread/loaded/list", json!({"limit":100}))
        .unwrap();
    assert!(loaded["data"].as_array().unwrap().iter().any(|id| id == &a));
    assert_eq!(
        cb.rpc("thread/read", json!({"threadId":b})).unwrap()["thread"]["id"],
        b
    );
    phone
        .rpc("thread/unsubscribe", json!({"threadId":a}))
        .unwrap();
    drop(phone);
    let mut fresh = Client::connect(&backend.url, &backend.token);
    let limit = Instant::now() + Duration::from_secs(20);
    loop {
        let loaded = fresh
            .rpc("thread/loaded/list", json!({"limit":100}))
            .unwrap();
        if !loaded["data"].as_array().unwrap().iter().any(|id| id == &a) {
            break;
        }
        assert!(Instant::now() < limit, "last-subscriber native unload");
        thread::sleep(Duration::from_millis(100));
    }
    let resumed=fresh.rpc("thread/resume",json!({"threadId":a,"cwd":b_cwd,"developerInstructions":"fresh-generation","config":{"mcp_servers.fixture_a":definition(&fake,"FRESH",&b_cwd)}})).unwrap();
    assert_eq!(resumed["thread"]["id"], a);
    let r = fresh
        .rpc(
            "mcpServer/tool/call",
            json!({"threadId":a,"server":"fixture_a","tool":"echo","arguments":{}}),
        )
        .unwrap();
    assert!(r["content"][0]["text"].as_str().unwrap().contains("FRESH"));
    configured_tuis(
        &exe,
        &root,
        &gateway_b.url,
        "seat-B",
        &b,
        &mut owner_b,
        "fresh",
    );
    gateway_b.end();
    drop(cb);
    owner_b
        .rpc("thread/unsubscribe", json!({"threadId":b}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let loaded = owner_b
            .rpc("thread/loaded/list", json!({"limit":100}))
            .unwrap();
        if !loaded["data"].as_array().unwrap().iter().any(|id| id == &b) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "B must actually unload before launcher cold resume"
        );
        thread::sleep(Duration::from_millis(100));
    }
    let mut cold = start(&b_cwd, "B", &file_b);
    cold.as_object_mut().unwrap().remove("historyMode");
    cold["threadId"] = json!(b);
    let restored = owner_b.rpc("thread/resume", cold).unwrap();
    assert_eq!(restored["thread"]["id"], b);
    let cold_gateway = Gateway::start(
        backend.url.clone(),
        backend.token.clone(),
        "seat-B".into(),
        b.clone(),
        b_cwd.clone(),
        hook_keys.clone(),
    )
    .unwrap();
    configured_tuis(
        &exe,
        &root,
        &cold_gateway.url,
        "seat-B",
        &b,
        &mut owner_b,
        "cold",
    );
    cold_gateway.end();
    drop(fresh);
    drop(owner_b);
    drop(gateway_a);
    drop(gateway_b);
    drop(file_b);
    let owners = pool.close();
    assert_eq!(owners.len(), 1);
    drop(backend);
    drop(owners);
    assert!(!root.join("home/auth.json").exists());
    std::fs::remove_dir_all(&root).unwrap();
}
fn configured_tuis(
    exe: &str,
    root: &Path,
    url: &str,
    token: &str,
    id: &str,
    owner: &mut Client,
    phase: &str,
) {
    let before = owner.rpc("thread/resume", json!({"threadId":id})).unwrap();
    assert_eq!(before["model"], "gpt-5.4");
    assert_eq!(before["reasoningEffort"], "low");
    assert_eq!(before["serviceTier"], "default");
    assert_eq!(before["approvalPolicy"], "never");
    assert_eq!(before["sandbox"]["type"], "readOnly");
    for tier in ["default", "fast"] {
        real_tui(exe, root, url, token, id, &format!("{phase}-{tier}"), tier);
        let after = owner.rpc("thread/resume", json!({"threadId":id})).unwrap();
        for field in [
            "model",
            "modelProvider",
            "serviceTier",
            "cwd",
            "runtimeWorkspaceRoots",
            "approvalPolicy",
            "approvalsReviewer",
            "sandbox",
            "activePermissionProfile",
            "reasoningEffort",
        ] {
            assert_eq!(
                after[field], before[field],
                "{phase}/{tier}: launcher {field} changed"
            );
        }
        assert_eq!(tools(owner, id), vec!["fixture_b".to_string()]);
        let r = owner
            .rpc(
                "mcpServer/tool/call",
                json!({"threadId":id,"server":"fixture_b","tool":"echo","arguments":{}}),
            )
            .unwrap();
        let value: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(value["marker"], "B");
        assert_eq!(Path::new(value["cwd"].as_str().unwrap()), root.join("B"));
    }
}
fn real_tui(exe: &str, root: &Path, url: &str, token: &str, id: &str, case: &str, tier: &str) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    let home = root.join(format!("tui-{case}"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), format!("model=\"gpt-6.1-sol\"\nmodel_reasoning_effort=\"medium\"\nservice_tier=\"{tier}\"\ncheck_for_update_on_startup=false\n")).unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(exe);
    command.args([
        "resume",
        id,
        "--remote",
        url,
        "--remote-auth-token-env",
        "PULLCEPT_CODEX_REMOTE_TOKEN",
        "--no-alt-screen",
    ]);
    command.env("PULLCEPT_CODEX_REMOTE_TOKEN", token);
    command.env("CODEX_HOME", &home);
    command.env("TERM", "xterm-256color");
    command.cwd(root.join("B"));
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut writer = pair.master.take_writer().unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    let mut ready = false;
    while Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(250)) {
            if chunk.contains("\u{1b}[6n") {
                writer.write_all(b"\x1b[1;1R").unwrap();
                writer.flush().unwrap();
            }
            text.push_str(&chunk);
            if text.contains("fixture-B") && text.contains("gpt-5.4") {
                ready = true;
                break;
            }
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
    }
    if ready {
        let idle_deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < idle_deadline {
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
                text.push_str(&chunk)
            }
            if child.try_wait().unwrap().is_some() {
                ready = false;
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    // fixture-B exists only on the intended thread's successful native resume
    // response; the initial header is deliberately not sufficient evidence.
    assert!(
        ready
            && !text.contains("failed to")
            && !text.contains("method is unavailable")
            && !text.contains("resume failed")
            && !text.contains("invalid or unsupported seat attach field"),
        "real remote TUI did not settle on intended thread {id}: {text}"
    );
}
