//! Private invoke replies for a running seat's relay control transport.
use super::*;
pub(super) use codex_remote::Control;
use mcp_config::codex::remote::{self as protocol, Action};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    epoch: String,
    status: Option<String>,
    pairing: Option<protocol::Pairing>,
    error: Option<String>,
    /// Why `errored` / `connecting`, when known: `relay_in_use` = the relay
    /// refused this seat with 409 "already online" (#430).
    reason: Option<&'static str>,
}

#[tauri::command]
pub async fn codex_remote_control(
    app: AppHandle,
    pty_id: String,
    epoch: Option<String>,
    action: Action,
) -> Result<Reply, String> {
    let (control, relay_in_use) = app.state::<CodexServers>().servers.lock().get(&pty_id)
        .and_then(|server| Some((server.remote.clone()?, Arc::clone(&server.relay_in_use))))
        .ok_or("この席ではリモート接続が有効ではありません。アカウント設定を確認し、席を起動し直してください。")?;
    if !app.state::<crate::pty::PtyState>().is_running(&pty_id) {
        control.end();
    }
    control.check()?;
    control.accept(action, epoch.as_deref())?;
    if matches!(action, Action::Enable | Action::Disable) {
        // An explicit operation starts a new connection cycle; an earlier
        // refusal no longer describes it.
        relay_in_use.store(false, Ordering::Release);
    }
    let still_current = Arc::clone(&control);
    let reply = tauri::async_runtime::spawn_blocking(move || {
        let mut reply = Reply {
            epoch: control.epoch().to_string(),
            status: None,
            pairing: None,
            error: None,
            reason: None,
        };
        match control.call(action) {
            Ok(value) if action == Action::Pair => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                match protocol::pairing(&value, now) {
                    Ok(pair) => reply.pairing = Some(pair),
                    Err(err) => reply.error = Some(err),
                }
            }
            Ok(value) => {
                reply.status = value["status"]
                    .as_str()
                    .filter(|s| matches!(*s, "disabled" | "connecting" | "connected" | "errored"))
                    .map(str::to_owned);
                if reply.status.is_none() {
                    reply.error = Some("リモート接続の状態を読み取れません。".into());
                }
                if action == Action::Enable {
                    control.clear_startup_error();
                }
                if reply.status.as_deref() == Some("disabled") {
                    reply.error = control.startup_error().or(reply.error);
                }
                match reply.status.as_deref() {
                    Some("errored" | "connecting") => {
                        if relay_in_use.load(Ordering::Acquire) {
                            reply.reason = Some("relay_in_use");
                        }
                    }
                    Some(_) => relay_in_use.store(false, Ordering::Release),
                    None => {}
                }
            }
            Err(err) => reply.error = Some(err),
        }
        control.check()?;
        Ok::<_, String>(reply)
    })
    .await
    .map_err(|_| "接続操作を完了できませんでした。")??;
    // Removal from the registry, terminal exit and generation all invalidate
    // replies even if they raced the RPC completion.
    if !app.state::<crate::pty::PtyState>().is_running(&pty_id) {
        still_current.end();
    }
    still_current.check()?;
    let current = app.state::<CodexServers>();
    let servers = current.servers.lock();
    if !servers
        .get(&pty_id)
        .and_then(|s| s.remote.as_ref())
        .is_some_and(|c| Arc::ptr_eq(c, &still_current))
    {
        return Err("この席は終了しました。".into());
    }
    Ok(reply)
}
