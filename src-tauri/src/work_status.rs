//! Shared screen/MCP topic journal; no process execution or room posts.
use mcp_config::work_status::{Actor, Book, Request};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
static LOCK: Mutex<()> = Mutex::new(());

fn load(app: &AppHandle, topic: &str) -> Result<(std::path::PathBuf, Book), String> {
    if !app.state::<crate::room::RoomState>().holds(topic) {
        return Err("Topic is no longer open".into());
    }
    let dir = crate::room_log::room_dir(app)?;
    let book: Book = serde_json::from_value(topic_index::work_status::read(&dir, topic)?)
        .map_err(|e| format!("Invalid work journal: {e}"))?;
    book.validate()?;
    Ok((dir, book))
}
pub fn snapshot(app: &AppHandle, topic: &str, account: Option<&str>) -> Result<Value, String> {
    let _guard = LOCK.lock();
    let (_, book) = load(app, topic)?;
    let mut result = book.snapshot(account, 50);
    // A retained manual task is never inferred complete from a CLI's lifecycle.
    for record in result["active"].as_array_mut().unwrap() {
        let running = record["owner"]
            .as_str()
            .and_then(|owner| {
                app.state::<crate::session::RoomSeats>()
                    .running_pty(topic, owner)
            })
            .is_some_and(|p| app.state::<crate::pty::PtyState>().is_running(&p));
        let limited = record["owner"].as_str().and_then(|owner| {
            let pty = app
                .state::<crate::session::RoomSeats>()
                .running_pty(topic, owner);
            app.state::<crate::room_status::StatusBook>()
                .0
                .lock()
                .project(
                    topic,
                    owner,
                    pty.as_deref(),
                    crate::room_status::now_ms(),
                    app.state::<crate::room::UsageBook>().snapshot(topic, owner),
                    false,
                )["limited"]
                .as_bool()
        });
        record["owner_running"] = json!(running);
        record["owner_limited"] = json!(limited);
        record["interrupted"] =
            json!(record["kind"] == "work" && (!running || limited == Some(true)));
    }
    Ok(result)
}
pub fn apply(
    app: &AppHandle,
    topic: &str,
    request: Request,
    actor: Actor<'_>,
    direct_answer: Option<&str>,
) -> Result<Value, String> {
    let _guard = LOCK.lock();
    let (dir, mut book) = load(app, topic)?;
    if request.action == "put"
        && request.expected_revision == 0
        && request.kind.as_deref() == Some("decision")
    {
        let target = request
            .target_account_id
            .as_deref()
            .ok_or("target_account_id required for decisions")?;
        if !crate::config::read_config(app)?
            .accounts
            .iter()
            .any(|a| a.id == target && matches!(a.kind, crate::config::AccountKind::Admin))
        {
            return Err("Target must be a configured human account".into());
        }
    }
    if let Some(id) = request.source_message_id.as_deref() {
        topic_index::work_status::message(&dir, topic, id)?;
    }
    // Realize the topic before its journal: a crash cannot orphan a durable work note.
    let topic_ref = app
        .state::<crate::room::RoomState>()
        .topic_of(topic)
        .ok_or("Topic is no longer open")?;
    crate::room_log::realize_topic(app, &topic_ref)?;
    let now = crate::room::now_iso();
    let answer = if request.action == "answer" {
        if actor.human {
            let text = direct_answer
                .filter(|s| !s.trim().is_empty() && s.len() <= 16000)
                .ok_or("Answer required (maximum 16000 bytes)")?;
            Some(
                json!({"content":text,"account":actor.account,"name":actor.name,"answered_at":now,"source":"screen"}),
            )
        } else {
            Some(topic_index::work_status::human_answer(
                &dir,
                topic,
                request
                    .answer_message_id
                    .as_deref()
                    .ok_or("answer_message_id required")?,
            )?)
        }
    } else {
        None
    };
    let result = book.apply(request, actor, answer, &now, |next| {
        topic_index::work_status::save(
            &dir,
            topic,
            &serde_json::to_value(next).map_err(|e| e.to_string())?,
        )
    })?;
    let _ = app.emit("work-status-changed", json!({"topic_id":topic}));
    Ok(json!(result))
}
#[tauri::command]
pub fn topic_status(
    app: AppHandle,
    topic_id: String,
    account_id: Option<String>,
) -> Result<Value, String> {
    crate::room_status::answer(&app, &topic_id, account_id.as_deref())
}
#[tauri::command]
pub fn update_topic_work(
    app: AppHandle,
    topic_id: String,
    operation: Request,
    answer: Option<String>,
) -> Result<Value, String> {
    let seats = app.state::<crate::session::RoomSeats>();
    let _guard = seats.lifecycle.lock();
    let person = app
        .state::<crate::room::RoomState>()
        .participants(&topic_id)
        .into_iter()
        .find(|p| p.own)
        .ok_or("No screen participant")?;
    let account = person.account.as_deref().ok_or("No screen account")?;
    if crate::config::account_kind_name(&app, account).as_deref() != Some("admin") {
        return Err("Screen account is not human".into());
    }
    apply(
        &app,
        &topic_id,
        operation,
        Actor {
            account,
            name: &person.name,
            human: true,
        },
        answer.as_deref(),
    )
}
