use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;


struct PtyInstance {
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    killer: Box<dyn portable_pty::MasterPty + Send>,
}

type ProcessMap = Arc<Mutex<HashMap<String, PtyInstance>>>;

#[derive(Clone)]
pub struct PtyState {
    procs: ProcessMap,
}

impl PtyState {
    pub fn new() -> Self {
        PtyState {
            procs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Whether the session on `id` is still running.
    ///
    /// Membership is liveness, not history: a session is taken out of the map
    /// by its own reader thread the moment it is reaped. That is what lets a
    /// seat in the room release itself (`session::RoomSeats`) instead of
    /// depending on someone remembering to call for its release — a release
    /// that never came would lock an account out of the room for the rest of
    /// the run, with no way back short of restarting the app.
    pub fn is_running(&self, id: &str) -> bool {
        self.procs.lock().contains_key(id)
    }

    /// Kill every session that is running.
    ///
    /// One acquisition of the lock over the whole map, for the same reason
    /// `RoomSeats::claim` sweeps and claims under one: a list of ids taken
    /// first and killed after would miss whatever was spawned in between, and
    /// the session nobody asked about is the one that outlives the app (#85).
    ///
    /// Drained rather than iterated and cleared, because taking the entry out
    /// is what drops it and the drop is the half that does the work. Killing
    /// the child does not take the tree down on its own — what does is the
    /// `PtyInstance` going away, which closes the master and takes every
    /// process attached to that ConPTY with it. Measured on the row's own
    /// `kill_pty` path (2026-08-25, AI operating): `cmd.exe` and the
    /// `claude.exe` under it were both gone from the process list afterwards.
    /// This sweep drops each instance the same way, so it ends the same tree.
    ///
    /// **There is no failure to report, so nothing here reports one.** On
    /// Windows `Child::kill` resolves to `WinChild::kill`, which discards the
    /// result of its own `TerminateProcess` call and returns `Ok(())`
    /// unconditionally (`portable-pty-patch/src/win/mod.rs`). A caller
    /// collecting the sessions that resisted would collect nothing, forever,
    /// and the report built on it would be a protection that reads as present
    /// and is not. The root of that — `do_kill` also has its success test
    /// inverted — is #96. **If #96 lands, `kill()` can start failing for real,
    /// and a failure path here becomes worth having again.** Until then it
    /// would be dead code claiming to be a safety net.
    pub fn kill_all(&self) {
        for (_, mut pty) in self.procs.lock().drain() {
            let _ = pty.child.kill();
        }
    }

    /// Kill the named sessions.
    ///
    /// `kill_all` narrowed to a list, and the same three things hold about it:
    /// one acquisition of the lock over the whole set, the entry taken out
    /// rather than left in place because dropping the instance is the half that
    /// ends the tree, and no failure to report (see `kill_all` for why, and for
    /// what #96 would have to change first).
    ///
    /// An id that is not in the map is a session that has already exited. That
    /// is not an error here: the caller's list came from the seats, and a seat
    /// is released by its session ending rather than by anyone calling for it
    /// (`session::RoomSeats`).
    pub fn kill_each(&self, ids: &[String]) {
        let mut map = self.procs.lock();
        for id in ids {
            if let Some(mut pty) = map.remove(id) {
                let _ = pty.child.kill();
            }
        }
    }
}

#[tauri::command]
pub fn spawn_pty(
    app: AppHandle,
    state: tauri::State<PtyState>,
    command: String,
    args: Vec<String>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
) -> Result<String, String> {
    let pty_system = NativePtySystem::default();

    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = pty_system
        .openpty(size)
        .map_err(|e| format!("Failed to open PTY: {e}"))?;

    // On Windows, .cmd/.bat scripts (like npm-installed CLIs) cannot be spawned directly.
    // Wrap them with cmd.exe /C so Windows resolves the command via PATH and PATHEXT.
    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("cmd.exe");
        c.arg("/C");
        c.arg(&command);
        for arg in &args {
            c.arg(arg);
        }
        c
    } else {
        let mut c = CommandBuilder::new(&command);
        for arg in &args {
            c.arg(arg);
        }
        c
    };

    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    // Spawn the child process in the PTY
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn '{command}': {e}"))?;

    // Get writer (stdin to the PTY master)
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to get PTY writer: {e}"))?;

    // Get reader (stdout from the PTY master) — clone master for resize later
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to get PTY reader: {e}"))?;

    let id = Uuid::new_v4().to_string();
    let id_clone = id.clone();
    let app_clone = app.clone();

    // Spawn background thread to read PTY output and emit events
    let ptys_clone = Arc::clone(&state.procs);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app_clone.emit(&format!("pty-data-{}", id_clone), data);
                }
                Err(_) => break,
            }
        }
        // Collect exit code before emitting exit event.
        //
        // Taken out of the map first, for two reasons. The map is what says a
        // session is running, and a session that has ended must stop saying so
        // — a seat in the room is held for exactly as long as this entry is
        // here. And the wait happens outside the lock, so a child that is slow
        // to be reaped no longer blocks every other session's input.
        //
        // The instance is dropped after the wait, never before: dropping the
        // master closes the PTY, and closing it under a child that has not been
        // reaped is how an exit code goes missing.
        // Bound before the match: a guard held in the scrutinee lives as long
        // as the match does, which would put the wait back inside the lock.
        let ended = ptys_clone.lock().remove(&id_clone);
        let exit_code: Option<u32> = match ended {
            Some(mut pty) => pty.child.wait().ok().map(|status| status.exit_code()),
            // Already removed: `kill_pty` took it. The exit is this thread's to
            // announce either way; the code is not knowable from here.
            None => None,
        };
        // Emit exit event with exit code payload (None if killed by signal/unknown)
        let _ = app_clone.emit(&format!("pty-exit-{}", id_clone), exit_code);
    });

    state.procs.lock().insert(
        id.clone(),
        PtyInstance {
            writer,
            child,
            killer: pair.master,
        },
    );

    Ok(id)
}

#[tauri::command]
pub fn write_pty(state: tauri::State<PtyState>, id: String, data: String) -> Result<(), String> {
    let mut map = state.procs.lock();
    let proc = map
        .get_mut(&id)
        .ok_or_else(|| format!("Process '{id}' not found"))?;
    proc.writer
        .write_all(data.as_bytes())
        .map_err(|e| format!("Write failed: {e}"))
}

#[tauri::command]
pub fn resize_pty(
    state: tauri::State<PtyState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = state.procs.lock();
    let proc = map
        .get(&id)
        .ok_or_else(|| format!("Process '{id}' not found"))?;
    proc.killer
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| format!("Resize failed: {e}"))
}

#[tauri::command]
pub fn kill_pty(state: tauri::State<PtyState>, id: String) -> Result<(), String> {
    let mut map = state.procs.lock();
    if let Some(mut pty) = map.remove(&id) {
        pty.child.kill().map_err(|e| format!("Kill failed: {e}"))?;
    }
    Ok(())
}

/// End every running session at once, for the app being closed (#85).
///
/// Its own command rather than a loop over `kill_pty` on the screen's side. The
/// screen's list of sessions is a copy of the app's, and a copy is what would
/// leave running whatever the screen had not heard of yet — a launch still in
/// flight when the window was asked to close is exactly that. Sweeping the map
/// itself has no such gap.
///
/// The close is held open on the screen's side, not here. Tauri prevents the
/// close on its own as soon as the webview listens for `CloseRequested`
/// (`tauri::manager::window::on_window_event` calls `api.prevent_close()` when
/// `window.has_js_listener` finds one), and closes the window once the
/// listener returns without preventing. So there is no `on_window_event`
/// wiring on this side to arrange: the question is asked and answered where
/// the dialog is, and this is only the act the answer authorises.
///
/// Returns nothing, including no error. The sweep it calls cannot fail — see
/// `PtyState::kill_all` for why, and for what would have to change (#96)
/// before a failure is a thing this could report.
#[tauri::command]
pub fn kill_all_ptys(state: tauri::State<PtyState>) {
    state.kill_all();
}
