use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

/// One account: someone who exists in this app whether or not they are running.
///
/// The identity is `id`, and only `id`. It is minted once and never changes;
/// everything else here is an attribute the person edits, the name included.
///
/// That is the whole of what an account adds over the launch recipe it replaces
/// (`TabConfig`). A tab was a way of starting something, so the only handle
/// anyone had on a session was the name it took — and two launches off one tab
/// took the same name, which left them indistinguishable and unaddressable
/// (#40). With a structural identity underneath, the name can come down to
/// being a display attribute: it may be edited, and it may collide, without
/// anything losing track of who is who.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Immutable, and the only identity. The `.mcp.json` registration key
    /// derives from this rather than from the name, so a rename cannot move the
    /// key out from under a session already running under it.
    pub id: String,
    /// What the room lists this account under and what a post is addressed to.
    /// Display and addressing; never identity.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// The hue this account is drawn in, in oklch degrees, or `None` when none
    /// was chosen — the screen derives one from the name in that case. Absent
    /// rather than defaulted: chosen and derived are different states (#45).
    ///
    /// Stored here rather than declared per launch, which is where #40 put both
    /// this and the name. #40 was fixing that a session could not be named at
    /// all, and with no identity to hang a name on, declaring at the moment of
    /// joining was the way to reach that. There is an identity now, so the pair
    /// moves onto it and gains what the launch-time form could not have: an
    /// account is named and coloured while it is not running.
    #[serde(default)]
    pub hue: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Also read from `tabs`, the name this key had while an account was a
    /// launch recipe. That alias is the whole migration: id, name, command,
    /// args and working directory carry over as they are, `cli_kind` is
    /// dropped on the floor by serde because nothing is left to read it (#17),
    /// and a hue is simply not declared yet. The next save writes `accounts`.
    #[serde(alias = "tabs")]
    pub accounts: Vec<Account>,
}

// ---------------------------------------------------------------------------
// Session persistence types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedChatMessage {
    pub role: String,
    pub content_type: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub id: String,
    pub name: String,
    pub messages: Vec<SavedChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSessions {
    pub tab_id: String,
    pub active_session_id: Option<String>,
    pub sessions: Vec<SessionData>,
}

impl Default for AppConfig {
    fn default() -> Self {
        // One vendor, by decision rather than by omission: the room is built
        // on a channel capability only this CLI is known to have, and the
        // spec drops the second vendor to keep that premise out of the
        // design. Shipping an account that cannot join the room by default
        // would present a session that never speaks. See
        // docs/0-requirements.md.
        //
        // One account, and its name is the CLI's. That is a starting point
        // sitting in an editable field, not the fixed label it used to be:
        // every session answered to this one name because there was nowhere to
        // change it and nothing else to tell two launches apart (#40). A second
        // account is created on screen and is named there.
        AppConfig {
            accounts: vec![Account {
                id: "account-1".to_string(),
                name: "Claude Code".to_string(),
                command: "claude".to_string(),
                args: vec![],
                cwd: None,
                hue: None,
            }],
        }
    }
}

/// The user's home directory, used to prefill a session's working directory.
///
/// A prefill, not a default: the app never launches a session anywhere the
/// person has not seen on screen.
#[tauri::command]
pub fn home_dir(app: AppHandle) -> Result<String, String> {
    app.path()
        .home_dir()
        .map(|dir| dir.to_string_lossy().to_string())
        .map_err(|e| format!("Failed to resolve home dir: {e}"))
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("config.json"))
}

#[tauri::command]
pub fn load_config(app: AppHandle) -> Result<AppConfig, String> {
    let path = config_path(&app)?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read config: {e}"))?;

    // A config written before accounts existed parses here as it stands: the
    // `tabs` alias on `AppConfig` reads the old key, and `cli_kind` is an
    // unknown field serde ignores. No migration step runs, because there is
    // nothing left for one to do — the person's own working directory and
    // launch options are what would have been lost, and they carry over.
    //
    // No path exists for anything older than that. Pullcept has never
    // shipped a release, and its app data directory is keyed to its own
    // identifier (org.liplus-project.pullcept), so no config in the older
    // left/right pane format from liplus-desktop can reach this app.
    serde_json::from_str::<AppConfig>(&content)
        .map_err(|e| format!("Failed to parse config: {e}"))
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write config: {e}"))
}

// ---------------------------------------------------------------------------
// Session persistence commands
// ---------------------------------------------------------------------------

fn sessions_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("sessions.json"))
}

#[tauri::command]
pub fn save_sessions(app: AppHandle, data: Vec<TabSessions>) -> Result<(), String> {
    let path = sessions_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create sessions dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("Failed to serialize sessions: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write sessions: {e}"))
}

#[tauri::command]
pub fn load_sessions(app: AppHandle) -> Result<Vec<TabSessions>, String> {
    let path = sessions_path(&app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read sessions: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse sessions: {e}"))
}
