use serde::{Deserialize, Serialize};

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
    /// Original name for display/recognition, and addressing only without a nickname.
    /// Never identity.
    pub name: String,
    #[serde(default)]
    pub nickname: Option<String>,
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
    /// Missing values select app-server; an explicitly saved false keeps the
    /// compatibility hook path (docs/3-accounts.md). Read only on Codex CLI.
    #[serde(default = "default_codex_app_server")]
    pub codex_app_server: bool,
    /// Opt-in relay; only these seats use their own SQLite directory (#419).
    #[serde(default)]
    pub codex_remote_control: bool,
}

pub fn default_codex_app_server() -> bool {
    true
}
