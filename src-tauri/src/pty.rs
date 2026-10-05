use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::{mpsc, Arc};
use tauri::{AppHandle, Emitter};
use terminal_input::Unsent;
use uuid::Uuid;


struct PtyInstance {
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    killer: Box<dyn portable_pty::MasterPty + Send>,
}

type ProcessMap = Arc<Mutex<HashMap<String, PtyInstance>>>;

/// What `pty-exit-{id}` carries: how the session on that id ended (#121).
///
/// Two things, because the screen has to tell apart two ends that looked the
/// same. An end the app asked for — the row's ❌ (`kill_pty`), a topic being
/// deleted (`kill_each`), the app closing (`kill_all`) — is one the person has
/// just agreed to in a dialog, and is not a thing to go and check. An end
/// nobody asked for — the CLI finishing on its own, or falling over — is.
#[derive(Clone, Serialize)]
pub struct PtyExit {
    /// The child's exit code, or null when it was not collected. Always null
    /// on a requested end: the kill took the child away before it was reaped.
    pub code: Option<u32>,
    /// Whether the app ended this session itself.
    ///
    /// Read off the map, which is where the kill leaves its mark: every kill
    /// path takes the session's entry out before killing it, and the only
    /// other thing that takes an entry out is the session's own reader thread,
    /// at its end. So an entry already gone when that thread looks for it is a
    /// kill having come first. Nothing else is recorded — a second record of
    /// the same fact would be one more place for the two to disagree.
    pub requested: bool,
}

/// One session's input box, as the room reads it before typing into it (#195).
#[derive(Default)]
struct InputBox {
    /// What the person has typed there and not sent (`terminal_input::Unsent`).
    unsent: Unsent,
    /// Posts held back while `unsent` was not empty, oldest first.
    held: VecDeque<String>,
}

type InputMap = Arc<Mutex<HashMap<String, InputBox>>>;

/// What the typist is handed, in the order it is handed.
enum Typing {
    /// Type this post into the session on this id, or hold it.
    Post(String, String),
    /// The session on this id may have an empty input box again: type what
    /// was held for it.
    Release(String),
}

#[derive(Clone)]
pub struct PtyState {
    procs: ProcessMap,
    /// Text the room types into a session, waiting its turn (#183).
    ///
    /// One queue and one thread for every session, so posts are typed in the
    /// order they were handed over. Two posts typed by two threads could each
    /// put its text in before the other's submit key, and the session would
    /// get both as one input.
    ///
    /// A release goes through the same queue rather than typing from the
    /// thread that noticed the box empty, for the same reason: a post handed
    /// over just before the release would otherwise be typed ahead of the
    /// ones held before it (#195).
    typing: mpsc::Sender<Typing>,
    /// Each session's input box, as far as the keys the terminal pane sent
    /// say (#195). An entry exists once the pane has written to a session, and
    /// goes with the session.
    input: InputMap,
}

impl PtyState {
    pub fn new() -> Self {
        let procs: ProcessMap = Arc::new(Mutex::new(HashMap::new()));
        let input: InputMap = Arc::new(Mutex::new(HashMap::new()));
        let (typing, queue) = mpsc::channel::<Typing>();
        let typist = Arc::clone(&procs);
        let boxes = Arc::clone(&input);
        std::thread::spawn(move || {
            // Ends when every sender has gone, which is the app going.
            for next in queue {
                match next {
                    Typing::Post(id, text) => {
                        if hold(&boxes, &id, text.clone()) {
                            continue;
                        }
                        type_one(&typist, &boxes, &id, &text);
                    }
                    Typing::Release(id) => {
                        while let Some(text) = next_held(&boxes, &id) {
                            type_one(&typist, &boxes, &id, &text);
                        }
                    }
                }
            }
        });
        PtyState {
            procs,
            typing,
            input,
        }
    }

    /// Type `text` into the session on `id`, then submit it (#183).
    ///
    /// Answers whether it was queued, which is whether the session is running
    /// now. The typing itself happens on the queue's thread and is not waited
    /// for: it holds a pause (`terminal_input::SUBMIT_PAUSE`), and the caller
    /// is the room delivering a post, which nothing should hold up.
    ///
    /// **Held while the person has something unsent in that session's input
    /// box** (#195, Master 判断 2026-09-28). A post typed then is joined onto
    /// what they wrote. It waits, with any others that arrive behind it, until
    /// they send or erase it, and then all of them are typed in the order they
    /// arrived. Whether to hold is decided on the queue's thread at the moment
    /// of typing, not here: the person may start writing while a post waits
    /// its turn.
    ///
    /// A write that fails once queued is logged and not reported back. A PTY
    /// that refuses a write is a session that has ended between the check and
    /// the write, and there is nobody left to hand the post to.
    pub fn type_in(&self, id: &str, text: String) -> bool {
        if !self.is_running(id) {
            return false;
        }
        self.typing.send(Typing::Post(id.to_string(), text)).is_ok()
    }

    /// Read one write the terminal pane made to the session on `id`, and
    /// release the posts held for it when that left its input box empty.
    fn note_input(&self, id: &str, data: &str) {
        let mut boxes = self.input.lock();
        let input = boxes.entry(id.to_string()).or_default();
        input.unsent.feed(data);
        if input.unsent.is_empty() && !input.held.is_empty() {
            let _ = self.typing.send(Typing::Release(id.to_string()));
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

/// Write `data` to the session on `id`, under the map's lock for this write
/// only.
fn write_to(procs: &ProcessMap, id: &str, data: &[u8]) -> Result<(), String> {
    let mut map = procs.lock();
    let proc = map
        .get_mut(id)
        .ok_or_else(|| format!("Process '{id}' not found"))?;
    proc.writer
        .write_all(data)
        .map_err(|e| format!("Write failed: {e}"))
}

/// Hold `text` for the session on `id` when it cannot be typed now, and answer
/// whether it was held (#195).
///
/// Held when the person has something unsent in that input box, and when
/// posts are already held for it: a post arriving behind held ones goes behind
/// them, so they are typed in the order they arrived. A session the pane has
/// never written to has an empty box and nothing held.
fn hold(boxes: &InputMap, id: &str, text: String) -> bool {
    let mut boxes = boxes.lock();
    match boxes.get_mut(id) {
        Some(input) if !input.unsent.is_empty() || !input.held.is_empty() => {
            input.held.push_back(text);
            true
        }
        _ => false,
    }
}

/// The oldest post held for the session on `id`, when its input box is empty
/// now. Read again before each one: the person may start writing between two
/// held posts, and the rest then wait again.
fn next_held(boxes: &InputMap, id: &str) -> Option<String> {
    let mut boxes = boxes.lock();
    let input = boxes.get_mut(id)?;
    if !input.unsent.is_empty() {
        return None;
    }
    input.held.pop_front()
}

/// Type one post into one session: the text, a pause, then the submit key.
///
/// Two writes, the way the terminal pane's own paste and Enter reach the PTY
/// (`terminal_input`). The lock is not held across the pause: other sessions'
/// keystrokes would wait on it for nothing.
///
/// Once the key has gone, the box is read as sent. Anything the person typed
/// during the pause went in with the post, and the box it was in is empty.
fn type_one(procs: &ProcessMap, boxes: &InputMap, id: &str, text: &str) {
    if let Err(err) = write_to(procs, id, text.as_bytes()) {
        eprintln!("[pty] a room post could not be typed into {id}: {err}");
        return;
    }
    std::thread::sleep(terminal_input::SUBMIT_PAUSE);
    if let Err(err) = write_to(procs, id, terminal_input::SUBMIT.as_bytes()) {
        eprintln!("[pty] a room post typed into {id} could not be submitted: {err}");
        return;
    }
    if let Some(input) = boxes.lock().get_mut(id) {
        input.unsent.sent();
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
    spawn_pty_with_env(app, state, command, args, &[], cols, rows, cwd, false)
}

/// `spawn_pty` with variables set on the child's environment.
///
/// The variables reach the CLI and everything it starts, which is what a launch
/// needs for the values it must hand over without writing them onto the command
/// line: the line is drawn on screen (`session::preview_launch_args`), and the
/// room's token is not a thing to draw (#149).
#[allow(clippy::too_many_arguments)]
pub fn spawn_pty_with_env(
    app: AppHandle,
    state: tauri::State<PtyState>,
    command: String,
    args: Vec<String>,
    env: &[(&str, String)],
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    codex_shell: bool,
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
        // Keep an initial quoted .cmd path intact. Codex options are encoded
        // before reaching this route, including every literal percent sign.
        if codex_shell { c.arg("call"); }
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

    // Add account/app variables to the inherited environment. Codex's reserved
    // style override is cleared first so a blank account uses the project default.
    if codex_shell {
        cmd.env_remove(mcp_config::codex::OUTPUT_STYLE_ENV);
    }
    for (key, value) in env {
        cmd.env(key, value);
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

    // In the map before the reader thread exists, never after. The thread reads
    // an entry already gone as a kill having taken it (`PtyExit::requested`),
    // and a child that dies at once — a command that is not there — can reach
    // the end of its output before a later insert. Its end would then be
    // announced as one the app asked for, and the entry inserted behind it
    // would say forever that a dead session is running (#121).
    state.procs.lock().insert(
        id.clone(),
        PtyInstance {
            writer,
            child,
            killer: pair.master,
        },
    );

    // Spawn background thread to read PTY output and emit events
    let ptys_clone = Arc::clone(&state.procs);
    let input_clone = Arc::clone(&state.input);
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
        // Its input box goes with it, and so do the posts held for it: there
        // is no session left to type them into (#195).
        input_clone.lock().remove(&id_clone);
        let exit = match ended {
            Some(mut pty) => PtyExit {
                code: pty.child.wait().ok().map(|status| status.exit_code()),
                requested: false,
            },
            // Already removed: a kill took it — `kill_pty`, `kill_each`, or the
            // `kill_all` sweep. The exit is this thread's to announce either
            // way; the code is not knowable from here, and the end is one the
            // app asked for (#121).
            None => PtyExit {
                code: None,
                requested: true,
            },
        };
        let _ = app_clone.emit(&format!("pty-exit-{}", id_clone), exit);
    });

    Ok(id)
}

/// Write what the terminal pane sent — a keystroke, or a paste — to the
/// session on `id`.
///
/// Every write is also read for what it leaves in the session's input box
/// (`PtyState::note_input`), which is how the room knows to hold its posts
/// while the person is writing (#195). Only a write that reached the session
/// is read: one that did not changed nothing there.
#[tauri::command]
pub fn write_pty(state: tauri::State<PtyState>, id: String, data: String) -> Result<(), String> {
    write_to(&state.procs, &id, data.as_bytes())?;
    state.note_input(&id, &data);
    Ok(())
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
