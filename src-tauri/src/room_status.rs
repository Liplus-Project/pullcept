//! App-observed status, without issuing commands, waking seats or reading screens.
use crate::{
    pty::PtyState,
    room::{RoomState, SessionStats, UsageBook},
    session::RoomSeats,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct StatusBook(pub Mutex<mcp_config::room_status::Book>);

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
fn current(app: &AppHandle, topic: &str, account: &str, pty: &str) -> bool {
    // Adoption can report while Starting, but only after this launch binds its PTY.
    if !app
        .state::<StatusBook>()
        .0
        .lock()
        .matches(topic, account, pty)
    {
        return false;
    }
    app.state::<RoomSeats>()
        .running_pty(topic, account)
        .is_none_or(|p| p == pty)
        && app.state::<PtyState>().is_running(pty)
}
pub fn stats(app: &AppHandle, stats: &SessionStats) {
    if let Some(pty) = stats
        .pty_id
        .as_deref()
        .filter(|p| current(app, &stats.topic_id, &stats.account_id, p))
    {
        app.state::<StatusBook>().0.lock().stats(
            &stats.topic_id,
            &stats.account_id,
            pty,
            &serde_json::to_value(stats).unwrap_or(Value::Null),
            stats.model_reported,
            now_ms(),
        );
    }
}
pub fn activity(app: &AppHandle, event: &Value) {
    if let (Some(topic), Some(account), Some(pty)) = (
        event["topic_id"].as_str(),
        event["account_id"].as_str(),
        event["pty_id"].as_str(),
    ) {
        if current(app, topic, account, pty) {
            app.state::<StatusBook>().0.lock().activity(event, now_ms());
        }
    }
}

pub fn answer(app: &AppHandle, topic: &str, account_id: Option<&str>) -> Result<Value, String> {
    let room = app.state::<RoomState>();
    if !room.holds(topic) {
        return Err("Topic is no longer open".into());
    }
    let rows = room
        .participants(topic)
        .into_iter()
        .map(|p| {
            let role = crate::room::role_of(app, &room, &p.id, p.account.as_deref());
            json!({"id": p.id, "account_id": p.account, "name": p.name,
            "full_name": p.full_name, "room_connected": true, "role": role})
        })
        .collect();
    let mut rows = mcp_config::room_status::select(rows, account_id)?;
    let now = now_ms();
    for row in &mut rows {
        if row["role"] == "admin" {
            continue;
        }
        let Some(account) = row["account_id"].as_str() else {
            continue;
        };
        let pty = app
            .state::<RoomSeats>()
            .running_pty(topic, account)
            .filter(|p| app.state::<PtyState>().is_running(p));
        let mut usage = app.state::<UsageBook>().snapshot(topic, account);
        if let Some(map) = usage.as_object_mut() {
            map.remove("type");
            map.remove("request_id");
        }
        row["session"] = app.state::<StatusBook>().0.lock().project(
            topic,
            account,
            pty.as_deref(),
            now,
            usage,
            account_id.is_some(),
        );
    }
    Ok(
        json!({"topic_id": topic, "retrieved_at": now, "time_unit": "unix_milliseconds_except_usage_unix_seconds", "participants": rows}),
    )
}
