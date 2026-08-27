use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

/// What kind of participant an account is.
///
/// **Declared when the account is made, never inferred.** The room knows only
/// what kind of connection someone arrived on, and a person joining from
/// another client arrives on the same kind of connection a session does — so
/// inferring this from the connection mistakes one for the other, which is
/// exactly the participant the room was built to stop treating differently
/// (#39). The one moment anyone can say which this is, is the moment the
/// account is created, and there is already a form there (#59).
///
/// It sorts the participant list into groups and does nothing else. Nothing in
/// the room reads it; the room still has one kind of participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    /// A person. The one at this keyboard is one of these (#59, which is where
    /// #53 left this open).
    User,
    /// A CLI session this app launches.
    #[default]
    Ai,
}

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
    /// What kind of participant this account is, declared when it was made.
    ///
    /// Defaulted to `Ai` for an account written before this field existed, and
    /// that is a migration rather than a fallback: every account that could
    /// have been saved then was a launch recipe for a CLI. The person at the
    /// keyboard had no account at all until now.
    #[serde(default)]
    pub kind: AccountKind,
    /// Which character this account speaks as: the `name:` of an output style
    /// in its working directory's `.claude/output-styles/`, or `None` when it
    /// declares none and the directory's own default stands.
    ///
    /// A field of the account rather than a string inside `args`, because the
    /// character is who this account is when it runs — the same axis its name
    /// and hue are on, and both of those left the launch for exactly this
    /// reason (#40). Buried in the launch options it would be an attribute of
    /// the account in name only (#99).
    ///
    /// This is what lets two accounts share one working directory. The only
    /// real difference between the two directories they had been kept in was
    /// one output-style file; carried here, that difference no longer needs a
    /// directory of its own, and the memory keyed to the directory becomes
    /// shared by the same move.
    ///
    /// Absent rather than defaulted, and absent for an account saved before
    /// this field existed: a directory whose `settings.json` names a style
    /// already has an answer, and writing one here would be a declaration
    /// nobody made.
    #[serde(default)]
    pub character: Option<String>,
    /// The whole command line that puts this account back into a session it was
    /// already in, with `{session_id}` where the id goes — for example
    /// `claude --resume {session_id}`. `None` when the account declares none.
    ///
    /// A topic holds which session each account was in while it was open
    /// (`room_log::Topic::sessions`), and reopening one hands that id to this
    /// line. What comes back is the participant's own context, carried by the
    /// CLI rather than read out to it — which is why this is a resume and not a
    /// replay of the log (#115, decision 4B).
    ///
    /// A whole command line rather than options alone, because resuming may not
    /// be the same invocation: it is the line the person would type. It is split
    /// the way launch options are, and the first token is the command.
    ///
    /// The other half of the pair is not a field. A fresh launch hands the CLI
    /// an id this app decided, and where that id goes on the line is the same
    /// per-CLI question this field answers — so it is written into the launch
    /// options with the same `{session_id}` placeholder
    /// (`mcp_config::SESSION_ID_PLACEHOLDER`). An account that writes it
    /// nowhere is launched with no id at all, which is the state a CLI with no
    /// resume of its own is permanently in.
    ///
    /// Absent is a real state and the common one. An account that declares no
    /// resume line is launched fresh into a reopened topic and reads back what
    /// it needs through the room's own pull instead (#115, decision 4C).
    #[serde(default)]
    pub resume_command: Option<String>,
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
                kind: AccountKind::Ai,
                character: None,
                // Nothing, rather than a line guessed from the command above.
                // A resume line naming the wrong flag fails at the one moment
                // it is needed, and the person has no reason to go looking at a
                // field they never filled in.
                resume_command: None,
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
    // A config written before `kind` existed is the same shape of nothing: the
    // field defaults to `Ai`, which every account saved then was. The account
    // the person at the keyboard now has is made on the screen rather than
    // migrated, because what it is made from — the name and hue they had been
    // joining under — lives in the webview's own storage and never reached
    // this file (#59).
    //
    // `resume_command` is the same shape of nothing again: an account saved
    // before it existed declares no way of resuming, which is what every
    // account did then — there was no topic for a session to be resumed into.
    //
    // `character` is the same again, and its absence is the state it means:
    // an account saved before it existed declared no character, so its launch
    // reads whatever its working directory's own `settings.json` names — which
    // is what that launch did before this field was here (#99).
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
