//! Authenticated room participants request fixed account operations; the app owns execution.
use crate::{
    config::{self, AccountKind},
    pty::PtyState,
    room::{self, RoomState},
    session::{self, LaunchPolicy, RoomSeats},
};
use mcp_config::session_control::{Book, Connection, Mode, Operation, Request};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
struct Journal {
    path: Option<PathBuf>,
    book: Book,
}
#[derive(Default)]
pub struct SessionControl {
    journal: Mutex<Journal>,
    connections: Mutex<BTreeMap<String, Connection>>,
    closing: AtomicBool,
}

fn save_book(path: Option<&PathBuf>, book: &Book) -> Result<(), String> {
    let path = path.ok_or("Session operations journal is unavailable")?;
    let bytes = serde_json::to_vec_pretty(book).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("tmp");
    use std::io::Write;
    let mut file = std::fs::File::create(&temporary)
        .map_err(|e| format!("Cannot write session operations: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Cannot flush session operations: {e}"))?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|e| format!("Cannot save session operations: {e}"))
}

fn save(journal: &Journal) -> Result<(), String> {
    save_book(journal.path.as_ref(), &journal.book)
}

impl SessionControl {
    pub fn initialize(&self, app: &AppHandle) -> Result<(), String> {
        let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let path = root.join("session-operations.json");
        let mut book = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Book>(&bytes)
                .map_err(|e| format!("Cannot read session operations: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Book::default(),
            Err(e) => return Err(e.to_string()),
        };
        book.interrupt(&room::now_iso());
        let journal = Journal {
            path: Some(path),
            book,
        };
        save(&journal)?;
        *self.journal.lock() = journal;
        Ok(())
    }
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }
    pub fn close(&self, app: &AppHandle) {
        self.closing.store(true, Ordering::SeqCst);
        let mut journal = self.journal.lock();
        journal.book.interrupt(&room::now_iso());
        if let Err(error) = save(&journal) {
            crate::room_log::report(app, error);
        }
    }
    pub fn connected(
        &self,
        origin: &str,
        socket: &str,
        topic: &str,
        account: &str,
        capability: &str,
        launch: Option<&str>,
    ) {
        self.connections.lock().insert(
            origin.into(),
            Connection {
                socket: socket.into(),
                topic: topic.into(),
                account: account.into(),
                generation: Some(capability.to_owned()),
                launch: launch.map(str::to_owned),
            },
        );
    }
    pub fn disconnected(&self, origin: &str, socket: &str) {
        let mut connections = self.connections.lock();
        if connections.get(origin).is_some_and(|c| c.socket == socket) {
            connections.remove(origin);
        }
    }
    fn matches(&self, app: &AppHandle, connection: &Connection, pty: &str) -> bool {
        let seats = app.state::<RoomSeats>();
        let live = seats
            .running_pty(&connection.topic, &connection.account)
            .as_deref()
            == Some(pty)
            && app.state::<PtyState>().is_running(pty);
        connection.matches_live(
            live,
            seats
                .generation_of(&connection.topic, &connection.account)
                .as_deref(),
            seats
                .launch_id_of(&connection.topic, &connection.account, pty)
                .as_deref(),
        )
    }
    fn room_connected(&self, app: &AppHandle, topic: &str, account: &str, pty: &str) -> bool {
        self.connections
            .lock()
            .values()
            .any(|c| c.topic == topic && c.account == account && self.matches(app, c, pty))
    }
    /// Also checked by the socket's launch admission before dispatch. This check
    /// excludes unaffiliated clients and sockets from a seat that already ended.
    pub fn authorize(&self, app: &AppHandle, origin: &str, socket: &str) -> Result<(), String> {
        let connections = self.connections.lock();
        let connection = connections
            .get(origin)
            .filter(|c| c.socket == socket)
            .ok_or("Only a connected account session may control sessions")?;
        let pty = app
            .state::<RoomSeats>()
            .running_pty(&connection.topic, &connection.account)
            .ok_or("Requester's session is not running")?;
        connection.authorize(
            socket,
            app.state::<PtyState>().is_running(&pty),
            app.state::<RoomSeats>()
                .generation_of(&connection.topic, &connection.account)
                .as_deref(),
            app.state::<RoomSeats>()
                .launch_id_of(&connection.topic, &connection.account, &pty)
                .as_deref(),
        )
    }
    fn operation_value(&self, app: &AppHandle, operation: &Operation) -> Value {
        let mut result = serde_json::to_value(operation).unwrap_or(Value::Null);
        let pty = operation.new_pty_id.as_deref();
        result["process_running"] =
            json!(pty.is_some_and(|id| app.state::<PtyState>().is_running(id)));
        result["room_connected"] = json!(pty.is_some_and(|id| self.room_connected(
            app,
            &operation.request.topic_id,
            &operation.request.account_id,
            id
        )));
        result
    }
    pub fn status(&self, app: &AppHandle, id: &str) -> Result<Value, String> {
        let op = self
            .journal
            .lock()
            .book
            .operations
            .get(id)
            .cloned()
            .ok_or("Unknown operation_id")?;
        Ok(self.operation_value(app, &op))
    }
    pub fn list(&self, app: &AppHandle) -> Result<Value, String> {
        let config = config::read_config(app)?;
        let accounts: Vec<_> = config
            .accounts
            .iter()
            .map(|a| {
                json!({
                    "account_id": a.id, "name": a.address_name(), "kind": a.kind,
                    "launchable": !matches!(a.kind, AccountKind::Admin | AccountKind::Mcp),
                })
            })
            .collect();
        let seats: Vec<_> = app.state::<RoomSeats>().seated(&app.state::<PtyState>()).iter().map(|seat| {
            let running = seat.session.as_ref();
            json!({ "topic_id": seat.topic_id, "account_id": seat.account_id,
                "state": if running.is_some() { "running" } else { "starting" },
                "pty_id": running.map(|s| &s.pty_id), "started_at": running.map(|s| &s.started_at),
                "resumed_from": running.and_then(|s| s.resumed_from.as_deref()),
                "room_connected": running.is_some_and(|s| self.room_connected(app, &seat.topic_id, &seat.account_id, &s.pty_id)),
            })
        }).collect();
        Ok(
            json!({ "accounts": accounts, "topics": app.state::<RoomState>().open_topics(), "seats": seats }),
        )
    }
    pub fn accept(
        &self,
        app: &AppHandle,
        request: Request,
        account: &str,
        topic: &str,
        name: &str,
    ) -> Result<Value, String> {
        if self.is_closing() {
            return Err("Pullcept is closing".into());
        }
        let id = request.operation_id.clone();
        let is_new = {
            let mut journal = self.journal.lock();
            let path = journal.path.clone();
            journal.book.accept_durable(
                request,
                account,
                topic,
                name,
                &room::now_iso(),
                |book| save_book(path.as_ref(), book),
            )?
        };
        if is_new {
            let worker_app = app.clone();
            let worker_id = id.clone();
            // Owned by the app runtime, never by serve_participant or a sidecar.
            tauri::async_runtime::spawn_blocking(move || {
                let state = worker_app.state::<SessionControl>();
                state.execute(&worker_app, &worker_id);
            });
        }
        self.status(app, &id)
    }
    fn update(
        &self,
        app: &AppHandle,
        id: &str,
        status: &str,
        pty: Option<String>,
        error: Option<String>,
    ) -> Result<(), String> {
        let mut journal = self.journal.lock();
        let op = journal
            .book
            .operations
            .get_mut(id)
            .ok_or("Unknown operation_id")?;
        if self.is_closing() {
            return Err("Pullcept is closing; operation interrupted".into());
        }
        op.status = status.into();
        op.updated_at = room::now_iso();
        op.new_pty_id = pty;
        op.error = error;
        if let Err(err) = save(&journal) {
            let op = journal
                .book
                .operations
                .get_mut(id)
                .ok_or("Unknown operation_id")?;
            if status == "running" {
                op.status = "failed".into();
            }
            op.error = Some(format!("Operation record save failed: {err}"));
            crate::room_log::report(app, err.clone());
            return Err(err);
        }
        Ok(())
    }
    fn notice(&self, app: &AppHandle, id: &str) {
        let Some(op) = self.journal.lock().book.operations.get(id).cloned() else {
            return;
        };
        let target = config::read_config(app)
            .ok()
            .and_then(|c| {
                c.accounts
                    .into_iter()
                    .find(|a| a.id == op.request.account_id)
                    .map(|a| a.address_name().to_string())
            })
            .unwrap_or_else(|| "不明なアカウント".into());
        let content = mcp_config::session_control::notice_text(
            &op.requester_name,
            &target,
            op.request.action,
            &op.status,
            op.error.as_deref(),
            id,
        );
        room::post_app_notice(
            app,
            &app.state::<RoomState>(),
            &op.requester_topic,
            &content,
            None,
        );
        let _ = app.emit("session-operation", self.operation_value(app, &op));
    }
    fn execute(&self, app: &AppHandle, id: &str) {
        let seats = app.state::<RoomSeats>();
        let _guard = seats.lifecycle.lock();
        if self.update(app, id, "running", None, None).is_err() {
            self.notice(app, id);
            return;
        }
        let Some(op) = self.journal.lock().book.operations.get(id).cloned() else {
            return;
        };
        let result = self.run(app, &op.request);
        let (status, pty, error) = match result {
            Ok(Some(pty)) => ("process_started", Some(pty), None),
            Ok(None) => ("stopped", None, None),
            Err(err) => ("failed", None, Some(err)),
        };
        if let Err(err) = self.update(app, id, status, pty, error) {
            crate::room_log::report(app, err);
        }
        self.notice(app, id);
        let _ = app.emit("session-seats-changed", ());
    }
    fn run(&self, app: &AppHandle, request: &Request) -> Result<Option<String>, String> {
        if self.is_closing() {
            return Err("Pullcept is closing".into());
        }
        let account = config::read_config(app)?
            .accounts
            .into_iter()
            .find(|a| a.id == request.account_id)
            .ok_or("Unknown configured account")?;
        if matches!(account.kind, AccountKind::Admin | AccountKind::Mcp) {
            return Err("Target account is not a launchable session".into());
        }
        mcp_config::session_control::execute(request, &mut AppDriver { app, account })
    }
}

struct AppDriver<'a> {
    app: &'a AppHandle,
    account: config::Account,
}
impl mcp_config::session_control::Driver for AppDriver<'_> {
    fn current(
        &mut self,
        request: &Request,
    ) -> Result<Option<mcp_config::session_control::Current>, String> {
        if self
            .app
            .state::<RoomState>()
            .topic_of(&request.topic_id)
            .is_none()
        {
            return Err("Target topic is not open".into());
        }
        let seats = self.app.state::<RoomSeats>();
        let ptys = self.app.state::<PtyState>();
        let held = seats.seated(&ptys);
        let seat = held
            .iter()
            .find(|s| s.topic_id == request.topic_id && s.account_id == request.account_id);
        if seat.is_some_and(|s| s.session.is_none()) {
            return Err("Target seat is starting; wait for its PTY before controlling it".into());
        }
        Ok(seat.map(|seat| mcp_config::session_control::Current {
            pty_id: seat
                .session
                .as_ref()
                .map(|s| s.pty_id.clone())
                .unwrap_or("starting".into()),
            native_id: seat
                .session
                .as_ref()
                .and_then(|s| seats.native_id_of(&request.topic_id, &request.account_id, &s.pty_id))
                .flatten(),
            resumed_from: seat.session.as_ref().and_then(|s| s.resumed_from.clone()),
        }))
    }
    fn resume(&mut self, request: &Request) -> Result<String, String> {
        let topic = self
            .app
            .state::<RoomState>()
            .topic_of(&request.topic_id)
            .ok_or("Target topic is not open")?;
        session::preflight_resume(self.app, &self.account, &topic)
    }
    fn stop(&mut self, request: &Request) -> Result<(), String> {
        let id = request
            .expected_pty_id
            .as_deref()
            .ok_or("Expected PTY required")?;
        self.app.state::<PtyState>().kill_exact(id)?;
        crate::codex_app_server::stop_one(self.app, id);
        Ok(())
    }
    fn start(
        &mut self,
        request: &Request,
        mode: Mode,
        resume_id: Option<&str>,
    ) -> Result<String, String> {
        let policy = match mode {
            Mode::Fresh => LaunchPolicy::Fresh,
            Mode::Resume => {
                LaunchPolicy::Resume(Some(resume_id.ok_or("Resume ID required")?.into()))
            }
        };
        let started = session::start_session_with_policy(
            self.app.clone(),
            self.app.state::<RoomState>(),
            self.app.state::<PtyState>(),
            self.app.state::<RoomSeats>(),
            self.account.clone(),
            request.topic_id.clone(),
            100,
            30,
            policy,
        )?;
        Ok(started.pty_id)
    }
}
