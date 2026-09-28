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
//! tool at all, so on a machine where the bridge has never been authorised
//! nothing arrives and nothing is asked: no browser opens at startup.
//! Authorising stays where it is, with a session's own bridge.
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
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use webhook_bridge::{
    initialize_request, initialized_notification, read_line, Line, INITIALIZE_ID,
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
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("reading the server failed: {e}"))?
    {
        match read_line(&line) {
            Line::Answer { id, failure } if id == INITIALIZE_ID => {
                if let Some(failure) = failure {
                    return Err(format!("the server refused initialize: {failure}"));
                }
                send(&mut stdin, &initialized_notification()).await?;
                report.running();
            }
            Line::Answer { .. } | Line::Other => {}
            Line::Event(event) => {
                // Read now rather than when the server was started: the
                // account's name and colour may have been edited since (#193).
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
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("the server could not be waited on: {e}"))?;
    Ok(format!("the server ended ({status})"))
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
