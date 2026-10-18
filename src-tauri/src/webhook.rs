//! Webhooks received by the app itself, and put into the room (#169).
//!
//! The app runs the `github-webhook-mcp` bridge as its own MCP client, on the
//! bridge's stdio, where a CLI session usually sits. The bridge pushes each
//! event as `notifications/claude/channel` to whoever connected, and the app
//! posts it into every room an AI session is seated in, said as the server's
//! `mcp` account (`room::post_notice`, #193). An event that finds no such room
//! is not posted.
//!
//! **The app never marks an event processed** (#180). The worker's pending set
//! is one for every reader, so an event the app marked would drop out of the
//! backlog a polling session reads. Posting is showing it; whether it has been
//! handled is for the session that handled it. Leaving it pending posts nothing
//! twice: the worker broadcasts an event once, when it takes it in, and a
//! WebSocket that connects is sent no backlog (#180 premise).
//!
//! **The bridge opens its WebSocket only when a token file is already there**
//! (`~/.github-webhook-mcp/oauth-tokens.json`, #169 premise). The app calls no
//! tool until an event has arrived, so on a machine where the bridge has never
//! been authorised nothing arrives and nothing is asked: no browser opens at
//! startup. Authorising stays where it is, with a session's own bridge.
//!
//! **An event whose body a session wrote is read once more, for its signature
//! (#269).** One GitHub account can be shared by several sessions, and what
//! tells them apart is the last line of the body (#270). The push carries no
//! body, so the app calls `get_event`, which only reads — it does not mark the
//! event processed. A signed notice opens with the name; one whose read failed,
//! timed out, or found no signature goes out as it did before. Which events are
//! read, how the answer is read, and the order notices keep while one is read
//! are `webhook_bridge::Notices`.
//!
//! **Every notice carries one line naming its event id for `mark_processed`
//! (#410).** The session that reads a notice of its own operation is to mark it
//! there and then; the app itself still marks nothing (#180). Where the line
//! goes is `webhook_bridge::prompted`, applied by `Notices`.
//!
//! **What sessions already receive is unchanged.** A session that loads its own
//! `github-webhook-mcp` channel keeps it; this is a second path beside it, not a
//! replacement (#169 constraints).
//!
//! What is written to the bridge and how each line from it is read are in the
//! `webhook-bridge` crate, where they are tested. This file is the process and
//! the loop. Which servers are run, and from what, is `app_mcp.rs` (#172): the
//! bridge is the one entry the app lists when it first writes its
//! `mcp-servers.json`, and any server listed there is run by this same loop.

use crate::app_mcp::Reporter;
use crate::room::{self, RoomState};
use mcp_servers::Server;
use std::process::Stdio;
use std::time::Instant;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use webhook_bridge::{
    initialize_request, initialized_notification, read_line, Event, Line, Notices, INITIALIZE_ID,
};

/// Run one server and read it until it ends. `Ok` says how it ended, `Err` why
/// it could not go on; either way the run is over.
///
/// The server is not restarted when it ends. The bridge reconnects its own
/// WebSocket; what ends it is the process itself failing, and a loop restarting
/// that would be the app retrying a failure on its own (`model-loop-safety`).
/// 再起動 in the settings panel, or a restart of the app, starts it again.
pub async fn receive(
    app: &AppHandle,
    room: &RoomState,
    server: &Server,
    report: &Reporter,
) -> Result<String, String> {
    let mut child = spawn(server)?;
    let mut stdin = child.stdin.take().ok_or("the server has no stdin")?;
    let stdout = child.stdout.take().ok_or("the server has no stdout")?;
    if let Some(stderr) = child.stderr.take() {
        // The bridge says what it is doing on stderr — the WebSocket opening,
        // closing, and retrying. Kept as the server's log, so the one place a
        // person can see why nothing is arriving is the settings panel.
        let report = report.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                report.log(&line);
            }
        });
    }

    send(&mut stdin, &initialize_request()).await?;

    let mut lines = BufReader::new(stdout).lines();
    let mut notices = Notices::new();
    let mut ready = false;
    let mut observer = Box::pin(observe(server, report));
    let mut observer_done = false;
    let ended = loop {
        let deadline = notices.next_deadline();
        let line = tokio::select! {
            line = lines.next_line() => line,
            result = &mut observer, if !observer_done => {
                observer_done = true;
                if result.is_err() { report.observer(serde_json::json!({"state":"unobserved","observed_at":crate::room_status::now_ms()})); }
                continue;
            },
            // A read the bridge has not answered in time: the notice goes out
            // unsigned rather than waiting on.
            _ = sleep_until(deadline), if deadline.is_some() => {
                post_due(app, room, report, notices.due(Instant::now()));
                continue;
            }
        };
        let line = match line {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(e) => break Err(format!("reading the server failed: {e}")),
        };
        match read_line(&line) {
            Line::Answer { id, failure, .. } if id == INITIALIZE_ID => {
                if let Some(failure) = failure {
                    break Err(format!("the server refused initialize: {failure}"));
                }
                if let Err(e) = send(&mut stdin, &initialized_notification()).await {
                    break Err(e);
                }
                ready = true;
                report.running();
            }
            Line::Answer { id, failure, text } => {
                notices.answer(id, failure.as_deref(), text.as_deref());
            }
            Line::Other => {}
            Line::Event(event) => {
                if let Some((id, request)) = notices.push(event, ready, Instant::now()) {
                    if let Err(e) = send(&mut stdin, &request).await {
                        report.log(&format!("[pullcept] could not ask for an event's body: {e}"));
                        notices.abandon(id);
                    }
                }
            }
        }
        post_due(app, room, report, notices.due(Instant::now()));
    };
    // Nothing held is lost to the server ending: what was waiting on a read
    // goes out unsigned.
    post_due(app, room, report, notices.drain());
    ended?;

    let status = child
        .wait()
        .await
        .map_err(|e| format!("the server could not be waited on: {e}"))?;
    Ok(format!("the server ended ({status})"))
}

/// Post each notice into every room an AI session is seated in.
fn post_due(app: &AppHandle, room: &RoomState, report: &Reporter, due: Vec<Event>) {
    for event in due {
        // Read now rather than when the server was started: the account's name
        // and colour may have been edited since (#193).
        let speaker = crate::config::mcp_speaker(app, report.server());
        let rooms = room::post_notice(app, room, &speaker, &event.content);
        if rooms == 0 {
            report.log(&format!(
                "[pullcept] event {} left pending: no AI session is seated in any room",
                event.event_id
            ));
        }
    }
}

/// Sleep until `deadline`; never, when there is none. Only polled with one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

/// A separate, noninteractive client for the shipped wrapper only. Its future
/// belongs to receive(), so dropping/restarting that generation kills this child.
/// Notification authentication/refresh and get_event remain on the normal child.
async fn observe(server: &Server, report: &Reporter) -> Result<(), String> {
    let bundled = crate::session::webhook_bridge_script().ok().is_some_and(|path|
        server.command == "node" && server.args.len() == 1 && std::path::Path::new(&server.args[0]) == path);
    if !bundled {
        report.observer(serde_json::json!({"state":"unsupported"})); return Ok(());
    }
    let mut observer_server = server.clone();
    observer_server.env.insert("WEBHOOK_OBSERVER".into(), "1".into());
    observer_server.env.insert("WEBHOOK_CHANNEL".into(), "0".into());
    let mut child = spawn(&observer_server)?;
    let mut stdin = child.stdin.take().ok_or("observer stdin unavailable")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("observer stdout unavailable")?).lines();
    // Drain without saving: observer diagnostics can include private auth paths.
    if let Some(stderr) = child.stderr.take() {
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
    }
    observer_send(&mut stdin, &initialize_request()).await?;
    let initialized = async {
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            if let Line::Answer {id, failure, ..} = read_line(&line) {
                if id == INITIALIZE_ID { return failure.map_or(Ok(()), |_| Err("observer initialize refused".to_string())); }
            }
        }
        Err("observer ended during initialize".into())
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), initialized).await {
        Ok(result) => result?,
        Err(_) => { report.observer(serde_json::json!({"state":"timeout","observed_at":crate::room_status::now_ms()})); return Ok(()); }
    }
    observer_send(&mut stdin, &initialized_notification()).await?;
    let mut observer = mcp_config::mcp_observer::Observer::default();
    observer_send(&mut stdin, &observer.discover(Instant::now())).await?;
    let mut discovering = true;
    let mut poll = tokio::time::interval(std::time::Duration::from_secs(30));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let deadline = observer.deadline();
        tokio::select! {
            _ = sleep_until(deadline), if deadline.is_some() => {
                if observer.expire(Instant::now()) { report.observer(serde_json::json!({"state":"timeout","observed_at":crate::room_status::now_ms()})); }
                if discovering { return Ok(()); }
            },
            _ = poll.tick() => {
                if let Some(request) = observer.poll(Instant::now()) { observer_send(&mut stdin,&request).await?; }
            },
            line = lines.next_line() => {
                let Some(line) = line.map_err(|e| e.to_string())? else { return Err("observer ended".into()); };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if let Some(snapshot) = observer.receive(&value, Instant::now(),crate::room_status::now_ms()) {
                    let unsupported = snapshot["state"] == "unsupported";
                    report.observer(snapshot);
                    if unsupported { return Ok(()); }
                    if discovering {
                        discovering = false;
                        if let Some(request) = observer.poll(Instant::now()) { observer_send(&mut stdin,&request).await?; }
                    }
                }
                // All channel pushes and arbitrary server messages are ignored.
            },
        }
    }
}
async fn observer_send(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    tokio::time::timeout(std::time::Duration::from_secs(1),send(stdin,line)).await.map_err(|_| "observer pipe timeout")?
}

/// The server's command and arguments, with its `env` added to the app's own
/// environment and all three streams piped.
///
/// Spawned directly, with no shell between — the way the bridge was run before
/// it had an entry (`node <script>`, as `mcp_config::spawn_form` has the
/// sidecar). A command that is a script rather than an executable is written
/// with its extension (`npx.cmd`).
fn spawn(server: &Server) -> Result<Child, String> {
    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .envs(&server.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Belt to the script's braces: the bridge's script ends on stdin EOF,
        // which covers the app going away by any route; this covers the handle
        // being dropped while the app is still up — which is how 再起動 ends
        // the run it replaces.
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window for a process nobody types into.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", server.command))
}

async fn send(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    stdin
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|e| format!("writing to the server failed: {e}"))?;
    stdin
        .flush()
        .await
        .map_err(|e| format!("writing to the server failed: {e}"))
}
