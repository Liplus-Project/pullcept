use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

/// What kind of participant an account is, and for one that launches, which
/// CLI.
///
/// **Declared when the account is made, never inferred.** The room knows only
/// what kind of connection someone arrived on, and a person joining from
/// another client arrives on the same kind of connection a session does — so
/// inferring this from the connection mistakes one for the other, which is
/// exactly the participant the room was built to stop treating differently
/// (#39). The one moment anyone can say which this is, is the moment the
/// account is created, and there is already a form there (#59).
///
/// **Two of the three launch, and what separates them is what this app knows
/// about the command under them** (#156). `ClaudeCode` names a CLI whose
/// conventions Pullcept holds (`mcp_config::Cli`) — how a session id is handed
/// over, how a session is resumed, how it reports itself. `Cli` is an account
/// this app knows nothing of the sort about: it launches, and the app puts
/// nothing of its own on that line beyond what every session in the room needs.
/// A second CLI is a second variant here and a second arm over there, not a
/// second reading of somebody's launch options.
///
/// What it decides: which group the participant list draws the row under, and
/// which conventions a launch carries. Nothing in the room reads it; the room
/// still has one kind of participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// A person. The one at this keyboard is one of these (#59, which is where
    /// #53 left this open).
    User,
    /// A Claude Code session this app launches, with that CLI's conventions on
    /// its line.
    ClaudeCode,
    /// A session of some CLI this app knows no conventions of.
    ///
    /// The default, and it is the safe one rather than the common one: a kind
    /// nobody declared is a command nobody described, and the line this kind
    /// launches is the person's own.
    #[default]
    Cli,
}

impl AccountKind {
    /// The CLI this kind launches, or `None` when it launches none this app
    /// knows the conventions of — a person, or a command it was told nothing
    /// about.
    pub fn cli(self) -> Option<mcp_config::Cli> {
        match self {
            AccountKind::User | AccountKind::Cli => None,
            AccountKind::ClaudeCode => Some(mcp_config::Cli::ClaudeCode),
        }
    }
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
    /// An account written before the kinds were split carries the one kind
    /// every launched account had, and one written before this field existed
    /// carries nothing at all. Neither is answered here: both are read off the
    /// launch command as the file is loaded
    /// (`mcp_config::migrate_account_kinds`), because that command is what
    /// still says which CLI was being launched, and serde cannot see a second
    /// field while it is deciding this one (#156).
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
    /// **Read only on a kind that names no CLI** (#156, 決定6). How a session is
    /// resumed is that CLI's business, so a kind that names one answers it
    /// (`mcp_config::Cli::resume_command`) and this field is not shown for it —
    /// a line the person has to keep in step with a convention the app already
    /// holds is a line that can disagree with it. Where the kind has no answer,
    /// this is the answer, and the same goes for the other half of the pair: a
    /// fresh launch is handed the id through the conventions, or through a
    /// `{session_id}` the person wrote into their own launch options
    /// (`mcp_config::SESSION_ID_PLACEHOLDER`). A line that names it nowhere is
    /// launched with no id at all, which is the state a CLI with no resume of
    /// its own is permanently in.
    ///
    /// Absent is a real state and the common one. An account that declares no
    /// resume line is launched fresh into a reopened topic and reads back what
    /// it needs through the room's own pull instead (#115, decision 4C).
    #[serde(default)]
    pub resume_command: Option<String>,
}

/// Which of the two panels flanking the room are open.
///
/// Here rather than in the webview's own storage, which is where the two text
/// sizes live (#60 / #68). Those are properties of the screen reading the
/// conversation — two people reading one room have no reason to want the same
/// size — and this is a property of the window's layout, which the person who
/// folded a panel away expects to find folded when they open the app again
/// (#118, decision 1).
///
/// Both open, for a screen that has never folded either. That is what every
/// screen showed before this field existed, so a config saved then opens the
/// way it closed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PanelState {
    /// The topic list, on the left (`#history`).
    pub history: bool,
    /// The account list, on the right (`#participants`).
    pub participants: bool,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState {
            history: true,
            participants: true,
        }
    }
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
    /// Absent in a config written before this field existed, which is every
    /// config saved so far. `Default` fills it with both panels open — the
    /// layout those screens have been showing.
    #[serde(default)]
    pub panels: PanelState,
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
                kind: AccountKind::ClaudeCode,
                character: None,
                // Nothing here, because the kind above holds the way back
                // (`mcp_config::Cli::resume_command`). This field is the
                // generic kind's, where the app has no line of its own to offer
                // and the person writes theirs (#156, 決定6).
                resume_command: None,
            }],
            panels: PanelState::default(),
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
    // unknown field serde ignores. The person's own working directory and
    // launch options are what would have been lost, and they carry over under
    // the names they already had.
    //
    // `resume_command` is the shape of nothing: an account saved before it
    // existed declares no way of resuming, which is what every account did then
    // — there was no topic for a session to be resumed into.
    //
    // `panels` is the same: a config saved before it existed says nothing about
    // which panels are folded, and both of them were open on every screen then
    // (#118).
    //
    // `character` is the same again, and its absence is the state it means:
    // an account saved before it existed declared no character, so its launch
    // reads whatever its working directory's own `settings.json` names — which
    // is what that launch did before this field was here (#99).
    //
    // `kind` is the one that is not. Read as it stands, an account saved before
    // the kinds were split would arrive as the default, and every launched
    // account saved then was launching Claude Code without the app saying so —
    // so the one step below runs before anything is typed (#156).
    //
    // The account the person at the keyboard has is made on the screen rather
    // than migrated either way, because what it is made from — the name and hue
    // they had been joining under — lives in the webview's own storage and
    // never reached this file (#59).
    //
    // No path exists for anything older than that. Pullcept has never
    // shipped a release, and its app data directory is keyed to its own
    // identifier (org.liplus-project.pullcept), so no config in the older
    // left/right pane format from liplus-desktop can reach this app.
    let mut root: Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {e}"))?;
    mcp_config::migrate_account_kinds(&mut root, LEGACY_LAUNCHED_KIND, kind_of_cli);
    serde_json::from_value::<AppConfig>(root).map_err(|e| format!("Failed to parse config: {e}"))
}

/// The kind every launched account carried while there was one of them (#156).
///
/// Read here and written nowhere. The kinds it splits into are what
/// `AccountKind` has variants for, and a legacy variant beside them would be a
/// state every reader of a kind has to carry from now on — the screen's own
/// copy of the enum included.
const LEGACY_LAUNCHED_KIND: &str = "ai";

/// The mapping the load step hands `mcp_config::migrate_account_kinds`, which
/// knows which CLI a saved account was launching and not what this app calls
/// the kind that names it.
///
/// The inverse of `AccountKind::cli`, and both are `match`es the compiler
/// checks: a second CLI is a second arm in each, named at the point it is
/// missing rather than found later by a kind that migrated to the wrong one.
fn kind_of_cli(cli: Option<mcp_config::Cli>) -> Value {
    kind_value(match cli {
        Some(mcp_config::Cli::ClaudeCode) => AccountKind::ClaudeCode,
        None => AccountKind::Cli,
    })
}

/// The JSON one kind is stored as, taken from the enum rather than spelled a
/// second time: two spellings would have to agree, and only one of them is what
/// is read back.
fn kind_value(kind: AccountKind) -> Value {
    serde_json::to_value(kind).expect("a unit variant serializes")
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
