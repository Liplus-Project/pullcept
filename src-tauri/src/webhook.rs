//! Webhooks received by the app itself, and put into the room (#169).
//!
//! The app runs the `github-webhook-mcp` bridge as its own MCP client, on the
//! bridge's stdio, where a CLI session usually sits. The bridge pushes each
//! event as `notifications/claude/channel` to whoever connected, and the app
//! posts it into every room an AI session is seated in, under the name
//! `webhook` (`room::post_notice`), then marks it processed. An event that finds
//! no such room is neither posted nor marked, and stays pending on the worker.
//!
//! **The bridge opens its WebSocket only when a token file is already there**
//! (`~/.github-webhook-mcp/oauth-tokens.json`, #169 premise). The app calls no
//! tool until an event has been posted, so on a machine where the bridge has
//! never been authorised nothing arrives and nothing is asked: no browser opens
//! at startup. Authorising stays where it is, with a session's own bridge.
//!
//! **What sessions already receive is unchanged.** A session that loads its own
//! `github-webhook-mcp` channel keeps it; this is a second path beside it, not a
//! replacement (#169 constraints).
//!
//! What is written to the bridge and how each line from it is read are in the
//! `webhook-bridge` crate, where they are tested. This file is the process and
//! the loop.

use crate::room::{self, RoomState};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use webhook_bridge::{
    initialize_request, initialized_notification, mark_processed_request, read_line, Line,
    INITIALIZE_ID,
};

/// Start the receiver, if its script is where the distribution puts it.
///
/// Every failure here is said on stderr and nothing more: webhooks in the room
/// are an addition to it, and a room that could not start them is still a room.
pub fn start(app: AppHandle, room: RoomState) {
    let script = match crate::session::webhook_bridge_script() {
        Ok(script) => script,
        Err(err) => {
            eprintln!("[webhook] not receiving webhooks: {err}");
            return;
        }
    };
    tauri::async_runtime::spawn(async move {
        if let Err(err) = receive(&app, &room, script).await {
            eprintln!("[webhook] receiver stopped: {err}");
        }
    });
}

/// Run the bridge and read it until it ends.
///
/// The bridge is not restarted when it ends. It reconnects its own WebSocket;
/// what ends it is the process itself failing, and a loop restarting that would
/// be the app retrying a failure on its own (`model-loop-safety`). A restart of
/// the app starts it again.
async fn receive(app: &AppHandle, room: &RoomState, script: PathBuf) -> Result<(), String> {
    let mut child = spawn(&script)?;
    let mut stdin = child.stdin.take().ok_or("the bridge has no stdin")?;
    let stdout = child.stdout.take().ok_or("the bridge has no stdout")?;
    if let Some(stderr) = child.stderr.take() {
        // The bridge says what it is doing on stderr — the WebSocket opening,
        // closing, and retrying. Passed through, so the one place a person can
        // see why nothing is arriving is the app's own log.
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    eprintln!("[webhook-bridge] {line}");
                }
            }
        });
    }

    send(&mut stdin, &initialize_request()).await?;

    let mut lines = BufReader::new(stdout).lines();
    let mut next_id = INITIALIZE_ID + 1;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("reading the bridge failed: {e}"))?
    {
        match read_line(&line) {
            Line::Answer { id, failure } if id == INITIALIZE_ID => {
                if let Some(failure) = failure {
                    return Err(format!("the bridge refused initialize: {failure}"));
                }
                send(&mut stdin, &initialized_notification()).await?;
            }
            Line::Answer {
                id,
                failure: Some(failure),
            } => {
                // A `mark_processed` that did not go through. The event is
                // already in the room and stays pending on the worker; saying
                // so is all there is to do, since posting it again would put
                // it in the room twice.
                eprintln!("[webhook] mark_processed (request {id}) failed: {failure}");
            }
            Line::Answer { .. } | Line::Other => {}
            Line::Event(event) => {
                let rooms = room::post_notice(app, room, &event.content);
                if rooms == 0 {
                    eprintln!(
                        "[webhook] event {} left pending: no AI session is seated in any room",
                        event.event_id
                    );
                    continue;
                }
                send(
                    &mut stdin,
                    &mark_processed_request(next_id, &event.event_id),
                )
                .await?;
                next_id += 1;
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("the bridge could not be waited on: {e}"))?;
    Err(format!("the bridge ended ({status})"))
}

/// `node <script>`, with all three streams piped.
///
/// `node` by name, as the sidecar's own line has it (`mcp_config::spawn_form`):
/// an executable rather than a shell script, so no shell is put between.
fn spawn(script: &Path) -> Result<Child, String> {
    let mut command = Command::new("node");
    command
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Belt to the script's braces: the script ends on stdin EOF, which
        // covers the app going away by any route; this covers the handle being
        // dropped while the app is still up.
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console window for a process nobody types into.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map_err(|e| format!("could not start node {}: {e}", script.display()))
}

async fn send(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    stdin
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|e| format!("writing to the bridge failed: {e}"))?;
    stdin
        .flush()
        .await
        .map_err(|e| format!("writing to the bridge failed: {e}"))
}
