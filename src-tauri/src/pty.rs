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
