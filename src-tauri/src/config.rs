use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Manager;

mod account;
pub use account::{default_codex_app_server, Account, AccountKind};

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
        // One vendor, by decision rather than by omission: the room was built
        // on a channel capability only this CLI was known to have, and the
        // spec dropped the second vendor to keep that premise out of the
        // design. The room no longer uses the channel (#195); the default
        // stays the CLI whose conventions this app holds. Shipping an account
        // that cannot join the room by default would present a session that
        // never speaks. See docs/0-requirements.md.
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
                nickname: None,
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
                env: Vec::new(),
                server: None,
                avatar: false,
                codex_app_server: false,
                codex_output_style: None,
                codex_remote_control: false,
                resume_notice: true,
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
    read_config(&app)
}

/// The config as it is read: the file, migrated, with an account for each
/// server the app runs (#193) — or the default with the same accounts added,
/// when there is no file.
///
/// The accounts made for the servers are not written here. Their ids are taken
/// from the entry's name (`mcp_servers::account_id`), so the same account comes
/// back on every read until a save writes it down, and a post said as it
/// before then is said as the account the screen later lists.
pub(crate) fn read_config(app: &AppHandle) -> Result<AppConfig, String> {
    let path = config_path(app)?;
    let mut root = if path.exists() {
        read_saved(&path)?
    } else {
        serde_json::to_value(AppConfig::default())
            .map_err(|e| format!("Failed to build config: {e}"))?
    };
    // A file that cannot be read gives no server an account, and costs nothing
    // else: the accounts that are saved are still read, and the server a missing
    // account belongs to is not running either (`app_mcp::start_all`).
    match crate::app_mcp::listed_names(app) {
        Ok(servers) => {
            mcp_servers::migrate_accounts(&mut root, &servers, &kind_value(AccountKind::Mcp))
        }
        Err(err) => eprintln!("[mcp] no server was given an account: {err}"),
    }
    serde_json::from_value::<AppConfig>(root).map_err(|e| format!("Failed to parse config: {e}"))
}

/// `config.json` as JSON, with the kinds migrated (#156).
fn read_saved(path: &PathBuf) -> Result<Value, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read config: {e}"))?;

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
    Ok(root)
}

/// Who a server's posts are said as (#193).
pub struct McpSpeaker {
    pub full_name: Option<String>,
    pub account_id: String,
    pub name: String,
    pub hue: Option<f64>,
}

/// The account a server answers to, read when the server has something to say.
///
/// Read then rather than held from when the server was started: the account's
/// name and colour are edited in its window while the server keeps running, and
/// a post said after that edit is said under what the account is now — the way
/// a person's rename reaches their next post (#40).
///
/// The account the config would be given when it holds none — the entry's name,
/// no colour, the id taken from the name — which is the account the screen
/// lists, whether or not it has been saved yet. The same when the config cannot
/// be read: a server that has something to say is not silenced for it.
pub fn mcp_speaker(app: &AppHandle, server: &str) -> McpSpeaker {
    let found = read_config(app).ok().and_then(|config| {
        config
            .accounts
            .into_iter()
            .find(|account| {
                account.kind == AccountKind::Mcp && account.server.as_deref() == Some(server)
            })
    });
    match found {
        Some(account) => McpSpeaker {
            full_name: (account.address_name() != account.name.trim()).then(|| account.name.trim().to_string()),
            account_id: account.id.clone(),
            name: account.address_name().to_string(),
            hue: account.hue,
        },
        None => McpSpeaker {
            full_name: None,
            account_id: mcp_servers::account_id(server),
            name: server.to_string(),
            hue: None,
        },
    }
}

/// The kind of the account `account_id` names, as it is stored — `claude_code`,
/// `cli`, ... — or `None` when the config holds no such account, or cannot be
/// read (#195).
///
/// Read when a session's post is typed into the others, for its label's
/// `role`, rather than held from the launch: the kind is what the account is
/// now. The stored spelling rather than the enum, because the label carries
/// the word, and `terminal_input` names the kinds it treats apart by that word.
pub fn account_kind_name(app: &AppHandle, account_id: &str) -> Option<String> {
    let config = read_config(app).ok()?;
    let account = config.accounts.into_iter().find(|account| account.id == account_id)?;
    kind_value(account.kind).as_str().map(str::to_string)
}

/// Whether the resume prompt is typed into `account_id`'s seat after its usage
/// limit lifts (#475, `Account::resume_notice`). Read at the moment the prompt
/// is due, as the kind is read above, so the switch is what the account says
/// now. A config that cannot be read, or holds no such account, answers `true`:
/// the behaviour every seat had before the switch.
pub fn resume_notice_enabled(app: &AppHandle, account_id: &str) -> bool {
    read_config(app)
        .ok()
        .and_then(|config| config.accounts.into_iter().find(|account| account.id == account_id))
        .map_or(true, |account| account.resume_notice)
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
        Some(mcp_config::Cli::CodexCli) => AccountKind::CodexCli,
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
pub fn save_config(app: AppHandle, room: tauri::State<crate::room::RoomState>, config: AppConfig) -> Result<(), String> {
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write config: {e}"))?;
    room.refresh_accounts(&app, &config.accounts);
    Ok(())
}

/// The environment field as the account form draws it: `NAME=<mask>` per line
/// (`account_env::render`, #163).
///
/// Opened here, on this side, so the webview is handed a mask and never a
/// stored value. A value that will not open is drawn as
/// `account_env::UNREADABLE` rather than failing the form.
#[tauri::command]
pub fn account_env_text(env: Vec<account_env::EnvVar>) -> String {
    account_env::render(&env)
}

/// Turn what the form's environment field holds back into sealed variables
/// (`account_env::settle`, #163).
///
/// A line left as `account_env_text` drew it keeps the value it had, so an
/// edit that touches other fields never needs the secrets typed again. A line
/// the person wrote is sealed here, before it is stored — the plaintext lives
/// only in the form and this call.
#[tauri::command]
pub fn seal_account_env(
    text: String,
    previous: Vec<account_env::EnvVar>,
) -> Result<Vec<account_env::EnvVar>, String> {
    account_env::settle(&text, &previous, mcp_config::APP_LAUNCH_ENV)
}

// ---------------------------------------------------------------------------
// Account images (#236)
// ---------------------------------------------------------------------------
//
// An account may carry an image, drawn in its circle in place of the initial.
// What is stored is always one small PNG: the screen crops the picked image to
// its centre square and scales it to 128×128 before handing it over, so there
// is no size to bound here and no format to convert. The app keeps the file,
// hands it back, and removes it; it is never put on the roster or the socket —
// the image is this screen's alone.

/// The folder the images are kept in, beside `config.json`.
const AVATAR_DIR: &str = "avatars";

/// The eight bytes every PNG opens with. What the screen hands over is a PNG it
/// made itself, so anything else is refused rather than stored under a `.png`
/// name.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Where `account_id`'s image is kept: `avatars/<id>.png` under the app data
/// directory.
///
/// The id is the file name, with every byte that is not an ASCII letter, digit,
/// `-` or `_` written as `%XX`. The ids the app mints (`account-1`, a UUID) come
/// through as they are. An `mcp` account's id is `mcp-` and the name of an entry
/// in a file the person edits by hand (`mcp_servers::account_id`), and that name
/// may hold a path separator, which would put the file somewhere else. `%` is
/// itself among the bytes written out, so two ids never share a file name.
///
/// Pure logic on the side whose tests do not run (docs/5-development.md), and
/// left here knowingly: it is one pass over the bytes, read rather than run,
/// and #236 keeps its change to this side.
fn avatar_path(app: &AppHandle, account_id: &str) -> Result<PathBuf, String> {
    if account_id.is_empty() {
        return Err("画像の持ち主のアカウントが届きませんでした。".to_string());
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    let mut name = String::with_capacity(account_id.len());
    for byte in account_id.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            name.push(byte as char);
        } else {
            name.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(dir.join(AVATAR_DIR).join(format!("{name}.png")))
}

/// Store an account's image, in place of the one it had (#236).
///
/// The PNG is the request's raw body, the way an attachment's bytes are
/// (`room_log::room_attach_bytes`), and the account rides in the
/// `Pullcept-Account` header as a JSON string with everything outside ASCII
/// escaped. Written before the account's flag is saved: the screen saves the
/// config only once this has answered, so a flag never names a file that was
/// not written.
#[tauri::command]
pub fn save_account_avatar(
    app: AppHandle,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let account_id: String = request
        .headers()
        .get("pullcept-account")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("画像の中身が届きませんでした。".to_string());
    };
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err("画像は PNG で渡してください。".to_string());
    }
    let path = avatar_path(&app, &account_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("画像の置き場所を作れませんでした: {e}"))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("画像を保存できませんでした: {e}"))
}

/// An account's image, as the PNG bytes, or an error when there is none or it
/// will not read (#236).
///
/// Raw bytes rather than a JSON array of numbers, which would be several times
/// the file on the way across; the screen receives an `ArrayBuffer`. A failure
/// is not said on screen: the screen draws the initial instead, which is what
/// an account without an image shows.
#[tauri::command]
pub fn account_avatar(
    app: AppHandle,
    account_id: String,
) -> Result<tauri::ipc::Response, String> {
    let path = avatar_path(&app, &account_id)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("画像を読めませんでした: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Remove an account's image (#236): 外す on its form, and the account itself
/// being deleted. An image that is not there is already removed, and answers
/// as such.
#[tauri::command]
pub fn delete_account_avatar(app: AppHandle, account_id: String) -> Result<(), String> {
    let path = avatar_path(&app, &account_id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("画像を消せませんでした: {e}")),
    }
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

impl Account {
    pub fn address_name(&self) -> &str {
        mcp_config::address_name(&self.name, self.nickname.as_deref())
    }
}
