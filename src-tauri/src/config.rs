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
/// **Three kinds launch, and what separates them is what this app knows
/// about the command under them** (#156 / #272). `ClaudeCode` and `CodexCli` name CLIs whose
/// conventions Pullcept holds (`mcp_config::Cli`) — how a session id is handed
/// over, how a session is resumed, how it reports itself. `Cli` is an account
/// this app knows nothing of the sort about: it launches, and the app puts
/// nothing of its own on that line beyond what every session in the room needs.
/// A second CLI is a second variant here and a second arm over there, not a
/// second reading of somebody's launch options.
///
/// **A local MCP server is not launched as a CLI: the app runs it itself**
/// (#193). It speaks in the room — what the server pushes is posted as the
/// account's — and no CLI is started under it; the server is started from its
/// entry in `mcp-servers.json`, which the account names as `server`.
///
/// What it decides: which group the participant list draws the row under,
/// which conventions a launch carries, and the `role` on the label a post from
/// a session is typed into the other sessions under (#195,
/// `terminal_input::role`). The room still has one kind of participant: the
/// role does not change what the floor admits or where a post is delivered,
/// only what a session reading it is told about who said it — and `admin` is
/// never read from here, since the screen alone is that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// A person. The one at this keyboard is one of these (#59, which is where
    /// #53 left this open).
    ///
    /// Stored as `admin`; `user` is what it was called until #192 and is still
    /// read as this, so a config saved before the rename keeps its people.
    /// Written back under the new name on the next save.
    #[serde(alias = "user")]
    Admin,
    /// A Claude Code session this app launches, with that CLI's conventions on
    /// its line.
    ClaudeCode,
    /// Codex CLI 0.160+ with native SessionStart identity and project TOML.
    CodexCli,
    /// A session of some CLI this app knows no conventions of.
    ///
    /// The default, and it is the safe one rather than the common one: a kind
    /// nobody declared is a command nobody described, and the line this kind
    /// launches is the person's own.
    #[default]
    Cli,
    /// A local MCP server the app runs as its own client (#193). One account
    /// answers to one entry of `mcp-servers.json`, named by `Account::server`.
    ///
    /// Arrived at two ways: declared on the form when an account is made, which
    /// writes the entry it answers to (#200, `app_mcp::create_mcp_server`), or
    /// given to an entry that has none as the config is read
    /// (`mcp_servers::migrate_accounts`). Either way it is fixed from then on:
    /// no other kind turns into it or out of it. Not launched — `start_session`
    /// refuses it, as it refuses a person.
    Mcp,
}

impl AccountKind {
    /// The CLI this kind launches, or `None` when it launches none this app
    /// knows the conventions of — a person, or a command it was told nothing
    /// about.
    pub fn cli(self) -> Option<mcp_config::Cli> {
        match self {
            AccountKind::Admin | AccountKind::Cli | AccountKind::Mcp => None,
            AccountKind::ClaudeCode => Some(mcp_config::Cli::ClaudeCode),
            AccountKind::CodexCli => Some(mcp_config::Cli::CodexCli),
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
    /// Which character this account speaks as: Claude's output style name, or
    /// Codex's project `.codex/output-styles/` stem (legacy inline heading before
    /// migration). `None` leaves the product's project default in place.
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
    /// Variables set on the environment of the CLI this account launches, each
    /// value sealed with DPAPI (`account_env`, #163).
    ///
    /// The environment rather than the line: the line is drawn on screen
    /// (`session::preview_launch_args`), so a value written there — `--settings`
    /// `env` included — is a value drawn on screen. And a field of the account
    /// rather than a file beside it, because the working directory is shared
    /// between accounts and a file per account is what that sharing was for not
    /// needing (#99).
    ///
    /// The screen never holds a stored value in the clear. It is handed the
    /// masks (`account_env_text`) and hands back what the person typed
    /// (`seal_account_env`); the one place a value is opened for use is the
    /// launch (`session::start_session`).
    ///
    /// Empty for an account saved before this field existed, which is what
    /// every launch then had: nothing added to the environment.
    #[serde(default)]
    pub env: Vec<account_env::EnvVar>,
    /// The entry of `mcp-servers.json` this account answers to, for an account
    /// of kind `mcp`; `None` for every other kind (#193).
    ///
    /// The entry's name, which is what the file is keyed on. The account holds
    /// who speaks — its name and colour — and the entry holds what is run, so
    /// neither file carries a second copy of the other's half.
    #[serde(default)]
    pub server: Option<String>,
    /// Whether this account carries an image to be drawn in its circle in
    /// place of the initial (#236).
    ///
    /// The flag only: the image is a file of its own, `avatars/` beside this
    /// config, named after the id (`avatar_path`). Bytes in the config would be
    /// read and written whole every time any account is saved, and the screen
    /// asks for the image once rather than with every read of the list.
    ///
    /// `false` for an account saved before this field existed, which is what
    /// every account then was: the initial on its colour. A flag that says
    /// `true` over a file that is gone is not repaired here — the screen draws
    /// the initial when the image will not load, and that is the whole of the
    /// fallback (#236).
    #[serde(default)]
    pub avatar: bool,
    /// A Codex CLI account's seat runs through its own app-server, and its
    /// character is handed to the thread as `developerInstructions` instead
    /// of by the Li+ output style SessionStart hook (#299).
    ///
    /// `false` — the hook, as every Codex seat ran before #299 — for an
    /// account saved before this field existed and for every new account:
    /// the hook stays the default until a seat switched to this has been
    /// seen answering in character (docs/3-accounts.md). Read only on a
    /// Codex CLI account.
    #[serde(default)]
    pub codex_app_server: bool,
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
            account_id: account.id,
            name: account.name,
            hue: account.hue,
        },
        None => McpSpeaker {
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
pub fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write config: {e}"))
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
