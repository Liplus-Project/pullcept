//! Registering the room sidecar in a project's `.mcp.json`, and the flag
//! guard that keeps a launched session able to receive channel pushes.
//!
//! Both are conditions the round trip does not survive without (see the
//! 成立条件 in `docs/0-requirements.md`):
//!
//!   - The sidecar must be registered **by name**. A config handed over with
//!     `--mcp-config` does not resolve on the channel side.
//!   - The launch must carry `--dangerously-load-development-channels
//!     server:<name>` and nothing else on that axis. Adding `--channels`
//!     registers the same server twice and takes the whole room down.
//!   - The launch must name the room registrations it is **not**, in
//!     `--settings`. The CLI starts every enabled server in the file, and a
//!     shared working directory holds one per account (#103).
//!
//! What the app knows about the CLI itself is here too, and it is one type:
//! `Cli`, which answers how a session id is handed over, how a session is
//! resumed, whether the CLI reports itself through `--settings`, and where the
//! conversation under an id is kept. Every answer is a `match` on that enum, so
//! a second CLI is a second arm in each rather than a search through the app for
//! what assumed the first one (#131, decision 3; #156).
//!
//! This crate holds no tauri: it writes into the user's own project directory,
//! which is the part of Pullcept that most needs test coverage, and a test
//! binary linking the tauri tree does not load on the GNU target.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Prefix of the name the sidecar is registered under in `.mcp.json`.
///
/// The full name is per account and room (`server_name_for`), not one fixed key. Two
/// sessions pointed at the same working directory write into the same file, and
/// a single key means the second launch overwrites the first one's name, hue
/// and room address — the identity the first session was launched with is gone
/// while that session is still running (#40).
pub const SERVER_PREFIX: &str = "pullcept-room";

/// The `.mcp.json` key, and the `server:<name>` tag, for one account in one room.
///
/// A function of the account id and the room id, and of nothing else. The id is
/// what an account is; the name is an attribute of it, and a key derived from
/// the name moved every time the name was edited — the registration a running
/// session was launched against would be orphaned under the old key while the
/// CLI holding that session still names the old tag on its command line (#53).
/// Deriving from the id makes a rename cost nothing, which is what makes the
/// name editable at all.
///
/// **The room is in the key because one account may be in several rooms at
/// once (#141, decision 4).** A room is a topic, and a session started in one
/// topic keeps running while another topic is opened and the same account is
/// started there too (decisions 2 and 3). Both launches write into the account's
/// working directory, and under a key of the account alone the second would
/// overwrite the first one's entry — its room id among it — while that session
/// is still running, which is #40 again one axis over. Keeping the key per
/// room is also what leaves the dead-entry sweep unchanged: every room of one
/// run shares the one address (`survives_registration`), so the entries of the
/// other rooms carry the live address and stay, and nothing there has to learn
/// which rooms still exist.
///
/// Per account and room rather than per launch: relaunching one account into a
/// room it was in reuses its entry, rather than growing the user's file by one
/// key per launch.
///
/// The slug half is legibility and the hash half is what makes the key total.
/// The slug is the account's alone: a room id is a uuid, and spelling it out as
/// well would double the key for nothing a reader could use. Which account and
/// which room an entry belongs to is read from `PULLCEPT_AGENT_NAME` and
/// `PULLCEPT_ROOM_ID` in its own env instead. Legibility loses to identity here
/// — a key that reads oddly costs one lookup, and a key that moves is a
/// registration nobody can find.
pub fn server_name_for(account_id: &str, room_id: &str) -> String {
    let slug = slugify(account_id);
    // A separator no id contains, so `("ab", "c")` and `("a", "bc")` are hashed
    // as two different pairs rather than as one concatenation.
    let hash = fnv1a(&format!("{account_id}\n{room_id}"));
    if slug.is_empty() {
        format!("{SERVER_PREFIX}-{hash:08x}")
    } else {
        format!("{SERVER_PREFIX}-{slug}-{hash:08x}")
    }
}

/// Lowercase ASCII alphanumerics, everything else a single separator.
///
/// This rides in a command-line flag (`server:<name>`) as well as in JSON, so
/// it stays inside the character set every shell and console on the way leaves
/// alone. Legibility only — `server_name_for` carries the uniqueness.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 24 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// FNV-1a, 32-bit. The same stable-spread hash the frontend derives a hue with;
/// one hash idea in the codebase rather than two.
fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in text.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

/// Flags that silently stop channel pushes from arriving.
pub const INCOMPATIBLE_FLAGS: &[&str] =
    &["--channels", "--print", "--input-format", "--output-format"];

/// What a session launch needs to know about the room it is joining.
#[derive(Debug, Clone)]
pub struct RoomRegistration<'a> {
    /// `ws://127.0.0.1:<port>` of the room socket.
    pub room_url: &'a str,
    /// Bearer token the sidecar must present.
    pub token: &'a str,
    /// Id of the account being launched. The registration key derives from
    /// this, so the entry stays put across a rename of the account.
    ///
    /// It is also handed to the sidecar in the env, so the session can name it
    /// in `hello` and the room can carry it on the seat. Carried, not consulted:
    /// the room decides identity, self-suppression and the `speaker` stamp on
    /// the connection, and an account id changes none of that (#39 / #40 /
    /// #47). What it buys is the screen being able to say which of its accounts
    /// a participant is, without matching on a name (#59).
    pub account_id: &'a str,
    /// Id of the room being joined, which is the topic the launch goes into
    /// (#141, decision 3).
    ///
    /// The other half of the registration key (`server_name_for`), and handed to
    /// the sidecar in the env so it can name the room in `hello`. One socket
    /// serves every room of a run, so the address cannot say which room a
    /// connection is for; the room id is what does.
    pub room_id: &'a str,
    /// Display name this session speaks under. Written into the env for the
    /// sidecar to declare in `hello`, and it is what says whose entry this is
    /// when the file is read by eye. Not the key: see `server_name_for`.
    pub agent_name: &'a str,
    /// Hue this session declared, in oklch degrees, or `None` when it declared
    /// none. Absent rather than a default: the room derives a hue from the name
    /// for an undeclared participant, and a value written here would be a
    /// declaration the person never made.
    pub agent_hue: Option<f64>,
    /// Whether this session is being seated in a topic that already holds posts
    /// it does not have.
    ///
    /// True when both hold: the topic has been spoken in, and this launch did
    /// not go in on the resume line. The boundary is the seat that did not
    /// resume, not the seat whose resume record was dropped — a fresh session
    /// entering a topic that has been talked in is as blind as one whose way
    /// back went, and a flag that caught only the second would cover half of
    /// one state (#133).
    ///
    /// Carried so the manners can say it. The room still pushes nothing: what
    /// crosses is the fact that there is something to pull, never the posts
    /// themselves, and whether to pull stays the session's call.
    pub unseen_history: bool,
    /// Absolute path of the sidecar entry point.
    pub sidecar_entry: &'a Path,
    /// Absolute path of the TypeScript runner that executes the entry point.
    pub sidecar_runner: &'a Path,
}

/// Reject a launch whose flags would leave the session unable to hear the room.
///
/// Rejected rather than stripped: a session that launches with the flag quietly
/// removed looks like it worked, and the failure then surfaces as silence.
/// Returns the offending flag.
pub fn reject_incompatible_flags(args: &[String]) -> Result<(), &'static str> {
    for arg in args {
        // `--flag=value` counts; matching the bare flag alone would miss it.
        let head = arg.split('=').next().unwrap_or(arg);
        if let Some(found) = INCOMPATIBLE_FLAGS.iter().find(|flag| **flag == head) {
            return Err(found);
        }
    }
    Ok(())
}

/// The flag that loads channel servers into a session.
pub const CHANNEL_FLAG: &str = "--dangerously-load-development-channels";

/// The launch arguments for a channel-enabled session, given the account's own.
///
/// The room's entry is merged into whatever the person wrote rather than added
/// as a second flag: `--channels` alongside this one registers a server twice
/// and takes the whole room down, and two copies of this flag is the same
/// shape. Merging also means the room's input path cannot be dropped by
/// configuring a different server — losing it is losing the room.
///
/// `server_name` is this account's own (`server_name_for`), so the flag and the
/// `.mcp.json` key stay one fact even though that fact differs per account.
pub fn channel_launch_args(base: &[String], server_name: &str) -> Vec<String> {
    let room = format!("server:{server_name}");
    let mut args = base.to_vec();

    if args.iter().any(|arg| *arg == room) {
        return args;
    }

    match args.iter().position(|arg| arg == CHANNEL_FLAG) {
        // Right after the flag: the values are positional, and keeping them
        // contiguous means a later argument cannot be captured as one.
        Some(index) => args.insert(index + 1, room),
        None => {
            args.push(CHANNEL_FLAG.to_string());
            args.push(room);
        }
    }
    args
}

/// The flag that hands a launch its settings, as a path or as JSON.
pub const SETTINGS_FLAG: &str = "--settings";

/// Whether the launch options already declare settings of their own.
///
/// `--settings=<value>` counts, the same way `reject_incompatible_flags` counts
/// it: matching the bare flag alone would miss half the ways of writing it.
pub fn declares_settings(args: &[String]) -> bool {
    declares_flag(args, SETTINGS_FLAG)
}

/// Whether these arguments name `flag`, as `--flag value` or `--flag=value`.
///
/// Both spellings, for the reason `reject_incompatible_flags` reads both:
/// matching the bare word alone would miss half the ways of writing it.
fn declares_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|arg| arg.split('=').next().unwrap_or(arg) == flag)
}

/// These arguments with `flag`, and the value it carries, taken out.
///
/// `--flag value` loses both words and `--flag=value` loses the one it is. A
/// trailing `--flag` with nothing behind it loses only itself, which is the
/// same rule reaching the end of the line rather than a case of its own.
fn without_flag(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut drop_value = false;
    for arg in args {
        if drop_value {
            drop_value = false;
            continue;
        }
        if arg == flag {
            drop_value = true;
            continue;
        }
        if arg.split_once('=').is_some_and(|(head, _)| head == flag) {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// What an account writes where the id of a CLI session goes.
///
/// The one thing every CLI shares here is that the id is decided by this app
/// rather than read back out of the CLI's output. Which flag carries it is the
/// CLI's business, and that business is `Cli`'s below — the placeholder is what
/// the two meet on, so a line carrying it is filled at spawn whether the
/// conventions put it there or a person did (#115, decision 4B; #156).
pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";

/// A CLI this app knows the conventions of.
///
/// **The conventions are the app's, not the person's** (#156). How a fresh
/// session is handed the id this app minted, how one is resumed, whether the
/// session reports itself through `--settings`, and where the conversation
/// under an id is kept — every one of them is a per-CLI answer, and every one
/// of them used to be either written by hand into an account's launch options
/// or assumed of every launched account alike. A session id that never reached
/// the CLI because nobody typed the flag is what ended that: handing the id
/// over is this app's own doing, so a declaration of it is not the person's to
/// remember.
///
/// What this app tells it about the room before its first turn is a per-CLI
/// answer too, and for the same reason the other four are: the flag that
/// carries it is the CLI's (#147).
///
/// One variant per CLI, and every answer below is a `match` on it: a second CLI
/// is a second arm in each, and the compiler names the ones left unanswered.
/// An account may also name no CLI at all, and that is not a variant here — it
/// is the absence of one (`config::AccountKind::Cli`), and what it means is
/// that this app has established nothing about the command under that account,
/// so it puts nothing of its own on that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cli {
    /// Claude Code. The one vendor the room is built on — see the 成立条件 in
    /// `docs/0-requirements.md`.
    ClaudeCode,
}

impl Cli {
    /// Which CLI a launch command names, or `None` when it names none this app
    /// knows.
    ///
    /// **The one place a kind is inferred, and it runs once**: on an account
    /// saved before the kinds were split, where the command is all that is left
    /// to say what was being launched (#156, 決定4). Everything saved after
    /// that carries a declared kind, for the reason the participant kind is
    /// declared rather than read off the connection — an answer re-derived on
    /// every read is an answer that can change under a session already running
    /// on it.
    ///
    /// The file name without its extension, case-folded: `claude`,
    /// `claude.exe` and an absolute path to either are one answer. Anything
    /// else is `None`, a command that merely contains the word included.
    pub fn of_command(command: &str) -> Option<Cli> {
        let file = command
            .trim()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default();
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        match stem.to_ascii_lowercase().as_str() {
            "claude" => Some(Cli::ClaudeCode),
            _ => None,
        }
    }

    /// What this CLI's conventions put on a fresh launch to hand it the session
    /// id this app minted, `{session_id}` and all.
    ///
    /// The placeholder rather than a slot filled in here: it is the same text a
    /// person used to write into their launch options, so one substitution over
    /// the whole line covers both (`substitute_session_id`), and the line drawn
    /// on screen before anything is launched reads the way it always did.
    ///
    /// Empty for a CLI that has no way of being handed one. Such a launch is
    /// given no id at all, which is the state a CLI with no resume of its own
    /// is permanently in.
    pub fn session_id_args(self) -> &'static [&'static str] {
        match self {
            Cli::ClaudeCode => &["--session-id", SESSION_ID_PLACEHOLDER],
        }
    }

    /// The person's own launch options with what this CLI's conventions now put
    /// on the line taken back out (#156).
    ///
    /// The migration's half of the split: the id used to ride a flag written
    /// into the options by hand, and that field is the person's again. Taken
    /// out rather than left for the merge to notice, because the field is read
    /// as what its owner asked for — a convention sitting in it is one the
    /// conventions can no longer change.
    pub fn without_session_id_args(self, base: &[String]) -> Vec<String> {
        match self.session_id_args().first() {
            Some(flag) => without_flag(base, flag),
            None => base.to_vec(),
        }
    }

    /// The whole line that goes back into a session this CLI was already in,
    /// with `{session_id}` where the id goes — or `None` when this CLI has no
    /// way back.
    ///
    /// A whole line rather than options alone, for the reason the account field
    /// it replaces held one: resuming may not be the same invocation, and the
    /// first token is the command.
    ///
    /// `None` is a real state rather than a gap. A seat of that kind starts
    /// fresh into a reopened topic and reads back what it needs through the
    /// room's own pull, which is the second tier of the two-tier answer and not
    /// a failure (#115, decision 4C).
    pub fn resume_command(self) -> Option<&'static str> {
        match self {
            Cli::ClaudeCode => Some("claude --resume {session_id}"),
        }
    }

    /// Whether this CLI reports what it is doing through the `hooks` and
    /// `statusLine` this app writes into `--settings` (#149 / #155).
    ///
    /// What those two keys are spelled as is Claude Code's
    /// (`limited_hook_settings` / `status_line_settings`). A CLI answering
    /// false is not one that spells them otherwise — it is one this app has
    /// established nothing about, and the safer side for an addition is not to
    /// add it: what is lost is a row that never says 制限中 and five values
    /// reading `—`, and what a settings key a CLI does not know can cost is the
    /// launch.
    pub fn reports_through_settings(self) -> bool {
        match self {
            Cli::ClaudeCode => true,
        }
    }

    /// What this CLI is told about the room before its first turn, or `None`
    /// when it cannot be written onto the launch line (#147).
    ///
    /// **The tool is named in full, because a name is what a session can act
    /// on before it has been handed a tool list.** A session woken only by a
    /// room post calls nothing, and the CLI's tool list and the sidecar's own
    /// `instructions` arrive after the first tool call — so a session that
    /// answers a post and stops never learns the room has a tool at all, and
    /// writes its answer to the terminal where nobody reads it (#147, measured
    /// across three sessions, 2026-09-17). The sidecar's text says the same
    /// thing and says it too late; this says it on the launch line, where it is
    /// there before the first turn.
    ///
    /// **Path-independent, and short.** What it states is that the room is
    /// spoken to through this tool and that terminal output does not reach it —
    /// facts about the room rather than about the channel that delivered the
    /// post, so a room reached some other way does not make this text wrong
    /// (#147, 決定4). Nothing about how to converse: that is the sidecar's
    /// `instructions`, which every session that calls the tool once has, and a
    /// second copy of it here would be paid for on every launch and would drift
    /// from the first.
    ///
    /// `None` on a text this launch line cannot carry (`line_safe_text`). The
    /// safer side for an addition is not to add it, the way the status line
    /// already does it: the launch still runs, and what is lost is the state
    /// this exists to prevent rather than the session.
    pub fn room_system_prompt(self, server_name: &str) -> Option<String> {
        let text = match self {
            Cli::ClaudeCode => format!(
                "You are a participant in a Pullcept room. Terminal output \
                 does not reach the room. The only way to be heard there is \
                 the tool mcp__{server_name}__say_to_room. Call it by that \
                 full name even before any tool list has arrived."
            ),
        };
        line_safe_text(&text).then_some(text)
    }

    /// Where this CLI keeps the transcript of one session, or `None` when this
    /// app cannot name the file.
    ///
    /// The layout itself is below (`transcript_path`), which is Claude Code's
    /// and is the only one this app has.
    pub fn transcript_path(self, home: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf> {
        match self {
            Cli::ClaudeCode => transcript_path(home, cwd, session_id),
        }
    }
}

/// Which CLI an account saved before the kinds were split should be read as
/// naming, given its launch command and the resume line it carries.
///
/// `None` is the kind that names none: a command this app knows nothing about,
/// or one it does know and an account with a way back of its own.
///
/// **A resume line that is not the CLI's own keeps the account off that CLI's
/// kind** (#156, 決定6). The kind holds the way back, so an account whose line
/// says the same thing has nothing of its own to keep; one that says something
/// else has, and the kind that leaves that line standing — stored, shown, and
/// the person's to edit — is the generic one. Reading it the other way would
/// put the line in a field nothing shows and nothing reads.
///
/// Blank is absent, the way it is everywhere a field on that form is read
/// (`declared_character`): a line cleared on screen arrives as an empty string.
fn migrated_cli(command: &str, resume: Option<&str>) -> Option<Cli> {
    let cli = Cli::of_command(command)?;
    match resume.map(str::trim).filter(|line| !line.is_empty()) {
        Some(line) if Some(line) != cli.resume_command() => None,
        _ => Some(cli),
    }
}

/// Give every account in a saved config that carries no declared kind the kind
/// its launch command names, and move the fields that go with it
/// (#156, 決定3 / 決定4 / 決定6).
///
/// **Here rather than beside the file it rewrites.** What this has to know is
/// what this crate knows — which CLI a command names, and what that CLI's
/// conventions carry now — and this crate is the one that can be tested (see
/// the テストの配置 in `docs/0-requirements.md`). A step that rewrites a
/// person's saved accounts is the kind of thing that has to be.
///
/// What it does not know is what the app calls a kind. `legacy_kind` is the
/// value every launched account carried while there was one of them, and
/// `kind_value` turns this crate's answer into the value the app stores. Both
/// come from the app's own enum, so no second spelling of it lives here to be
/// kept in step.
///
/// The two fields that move with the kind:
///
///   - the resume line is cleared where the kind holds one of its own (決定6).
///     An account whose line said something else never reaches here with a CLI
///     — that is what puts it on the kind with no conventions (`migrated_cli`),
///     and that kind keeps every field it arrived with.
///   - the CLI's own session-id argument comes back out of the launch options,
///     which are the person's to write again (決定3). Left in, the flag would
///     be on the line twice.
///
/// An account whose kind was declared is left alone. So is one that is not an
/// object, and one whose `args` holds something that is not a string: the typed
/// parse this runs ahead of is what refuses that file, and rewriting the list
/// would drop the element that says why.
pub fn migrate_account_kinds(
    root: &mut Value,
    legacy_kind: &str,
    kind_value: impl Fn(Option<Cli>) -> Value,
) {
    // Under either name the account list has had. `tabs` is what it was called
    // while an account was a launch recipe, and the app still reads that key —
    // a config that old would otherwise arrive with no kind on any account.
    let key = if root.get("accounts").is_some() {
        "accounts"
    } else {
        "tabs"
    };
    let Some(accounts) = root.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    for account in accounts.iter_mut() {
        let Some(account) = account.as_object_mut() else {
            continue;
        };
        // What is migrated is the one value the split replaced, and the absence
        // of any value at all. A kind that was declared stands.
        let declared = account.get("kind").and_then(Value::as_str);
        if declared.is_some_and(|kind| kind != legacy_kind) {
            continue;
        }
        migrate_one_account(account, &kind_value);
    }
}

fn migrate_one_account(
    account: &mut Map<String, Value>,
    kind_value: &impl Fn(Option<Cli>) -> Value,
) {
    let command = account
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let resume = account.get("resume_command").and_then(Value::as_str);
    let Some(cli) = migrated_cli(command, resume) else {
        account.insert("kind".to_string(), kind_value(None));
        return;
    };

    account.insert("kind".to_string(), kind_value(Some(cli)));
    account.insert("resume_command".to_string(), Value::Null);
    if let Some(args) = account.get("args").and_then(Value::as_array) {
        let own: Vec<String> = args
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if own.len() == args.len() {
            let kept = cli.without_session_id_args(&own);
            account.insert("args".to_string(), Value::from(kept));
        }
    }
}

/// The launch arguments carrying the session id, given the account's own.
///
/// The CLI's convention is appended only when the line has nowhere to put an id
/// yet. A resume line names the id itself, and a person who wrote the
/// placeholder into their options has said where it goes — either way a second
/// copy of the flag would hand the CLI two, and which of two copies of one flag
/// a CLI reads is not something this app has established (the ground the
/// two-`--settings` refusal stands on).
///
/// A kind naming no CLI adds nothing here. That is the whole of what the
/// generic kind is: the line is the person's, and this app has nothing to put
/// on it (#156, 決定5).
pub fn session_id_launch_args(base: &[String], cli: Option<Cli>) -> Vec<String> {
    let mut args = base.to_vec();
    let Some(cli) = cli else {
        return args;
    };
    let convention = cli.session_id_args();
    let Some(flag) = convention.first() else {
        return args;
    };
    if declares_session_id(base) || declares_flag(base, flag) {
        return args;
    }
    args.extend(convention.iter().map(|word| (*word).to_string()));
    args
}

/// Whether these arguments have somewhere to put a session id.
///
/// What decides whether one is minted at all. Minting unconditionally would
/// hand out an id no launch passes on, and the topic would then record a
/// session that never existed under that name.
pub fn declares_session_id(args: &[String]) -> bool {
    args.iter().any(|arg| arg.contains(SESSION_ID_PLACEHOLDER))
}

/// The flag that adds to what a launch tells its session about itself (#147).
pub const APPEND_SYSTEM_PROMPT_FLAG: &str = "--append-system-prompt";

/// The same thing written as a path to a file.
///
/// Named although this app never writes it, because the CLI refuses a launch
/// carrying both forms at once ("Cannot use both --append-system-prompt and
/// --append-system-prompt-file", read out of the CLI's own strings, v2.1.273).
/// A line that already names it is therefore a line this app leaves alone: the
/// cost of adding to it is not a text nobody reads, it is the launch.
const APPEND_SYSTEM_PROMPT_FILE_FLAG: &str = "--append-system-prompt-file";

/// The launch arguments carrying what this CLI's conventions tell a session
/// about the room it is joining, given the account's own (#147).
///
/// The room's own entry and the approval ride on every launched line whatever
/// its kind, because they are what a seat in the room needs. This does not: the
/// flag is the CLI's spelling, and a kind this app has established nothing
/// about is a kind whose launch an unknown flag ends (#156, 決定5). Same line
/// the hook and the status line are drawn on.
///
/// A line already naming either form of the flag is left as it is. The file
/// form would end the launch outright; a second copy of this one is the ground
/// the two-`--settings` refusal stands on — which of two copies of one flag a
/// CLI reads is not something this app has established. Left alone rather than
/// refused, because what the person wrote is about this session and this is an
/// addition to it.
pub fn system_prompt_launch_args(
    base: &[String],
    cli: Option<Cli>,
    server_name: &str,
) -> Vec<String> {
    let mut args = base.to_vec();
    let Some(cli) = cli else {
        return args;
    };
    if declares_flag(base, APPEND_SYSTEM_PROMPT_FLAG)
        || declares_flag(base, APPEND_SYSTEM_PROMPT_FILE_FLAG)
    {
        return args;
    }
    let Some(text) = cli.room_system_prompt(server_name) else {
        return args;
    };
    args.push(APPEND_SYSTEM_PROMPT_FLAG.to_string());
    args.push(text);
    args
}

/// Put the session id where the account said it goes.
///
/// Every occurrence in every argument, and inside a larger argument as well as
/// alone: `--session-id={session_id}` is one argument, and so is
/// `--resume={session_id}`. Arguments naming no placeholder come through
/// untouched.
pub fn substitute_session_id(args: &[String], session_id: &str) -> Vec<String> {
    args.iter()
        .map(|arg| arg.replace(SESSION_ID_PLACEHOLDER, session_id))
        .collect()
}

/// How long a directory slug may be before the CLI stops spelling it out.
///
/// Past this the CLI cuts the slug here and appends a hash of the path, and the
/// hash is a private detail of its own — reproducing it would be copying an
/// implementation rather than a layout. `transcript_path` answers `None` for
/// those instead of guessing (see there).
const SLUG_LIMIT: usize = 200;

/// Where the CLI keeps the transcript of one session.
///
/// **The one place Pullcept depends on where a CLI stores its conversations**
/// (#131, decision 3). The layout is Claude Code's:
/// `<home>/.claude/projects/<slug of the working directory>/<session id>.jsonl`.
/// Another CLI keeps them somewhere else entirely — `codex` writes
/// `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` — so a second CLI is a second
/// arm of `Cli::transcript_path`, which is what callers ask and the only way in
/// here (#156). The dependency is accepted rather than avoided (Master 判断):
/// asking the file system whether the conversation is there is steadier than
/// matching the CLI's refusal text, which is the thing #127 already refuses to
/// read.
///
/// The slug folds every character that is not an ASCII letter or digit into a
/// `-`, the drive's colon and the separators alike: `C:\Users\smile\Code`
/// becomes `C--Users-smile-Code`. Folded per UTF-16 code unit rather than per
/// character, because the rule being copied is a JavaScript regular expression
/// and that is the unit it steps in — a character outside the basic plane is
/// two dashes there and would be one here.
///
/// `None` when the slug would pass `SLUG_LIMIT`: the CLI shortens those and the
/// app cannot name the file. It is not "the transcript is missing" — the caller
/// keeps whatever it would have done without this answer, because a guess that
/// named the wrong file would read as a conversation that is gone.
fn transcript_path(home: &Path, cwd: &Path, session_id: &str) -> Option<PathBuf> {
    Some(
        home.join(".claude")
            .join("projects")
            .join(project_slug(&cwd.to_string_lossy())?)
            .join(format!("{session_id}.jsonl")),
    )
}

/// The working directory as it names a directory under `projects/`.
///
/// The result is ASCII by construction, so its byte length is the length the
/// CLI measures against the limit.
fn project_slug(cwd: &str) -> Option<String> {
    let mut slug = String::new();
    for ch in cwd.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else {
            for _ in 0..ch.len_utf16() {
                slug.push('-');
            }
        }
    }
    (slug.len() <= SLUG_LIMIT).then_some(slug)
}

/// The environment variable a launched CLI carries the room token in.
///
/// Set on the spawned process rather than written onto the line, because the
/// line is shown on screen (`preview_launch_args`) and the token is what makes
/// the room this room. The hook below names the variable, and the CLI resolves
/// it into the header when the hook fires (#149).
pub const ROOM_TOKEN_ENV: &str = "PULLCEPT_ROOM_TOKEN";

/// The path the app answers a session's usage-limit signal on.
pub const LIMITED_HOOK_PATH: &str = "/hooks/limited";

/// The path the app answers a session's status-line report on (#155).
pub const STATUS_HOOK_PATH: &str = "/hooks/status";

/// Where one seat posts to, under one of the paths above.
///
/// **The seat is named in the address, and not read out of what the CLI sends**
/// (#149, decision 3). The CLI's hook input names its own session id, which is
/// not what the app keys a seat on — a seat is a topic and an account, and a
/// launch with no `{session_id}` placeholder has no id the app knows at all.
/// The launch knows both halves, so the launch writes them here.
///
/// Loopback and the room's own port: one listener, which answers a WebSocket
/// upgrade as the room and a POST as this (`room.rs`).
///
/// **The two halves are path segments, and the address carries no query**
/// (#152). This URL is a JSON string value inside the `--settings` argument,
/// and on Windows that argument reaches the CLI through `cmd.exe /C`
/// (`pty::spawn_pty_with_env`). The argument holds `"` of its own, so the
/// quoting escapes each as `\"` and wraps the whole in `"` — and `cmd.exe`
/// does not read `\` as an escape, it only counts `"`. The wrapping quote
/// therefore inverts the parity: every JSON string's *contents* sit outside
/// `cmd.exe`'s quotes, where `&` is a command separator. A `?room=…&account=…`
/// ends the launch line at the `&` and runs the rest as a command, which is
/// what #151 shipped and what stopped every account from starting.
///
/// So nothing on this address may be a character `cmd.exe` acts on. Segments
/// keep it to `/`, and `percent_encode` covers the rest: an id carrying `&`,
/// `|`, `<`, `>`, `^`, `(`, `)` or `/` arrives here as `%XX`. Do not put the
/// halves back in a query — the same break returns.
///
/// `%` is the one character left standing, and it is the encoding's own.
/// `cmd.exe` expands `%NAME%`, so two adjacent escapes are a lookup of the
/// text between them — undefined names are left alone on a command line, and
/// the ids this is called with are uuids, which `percent_encode` does not
/// touch at all. Not closed, and measured on neither side.
///
/// One composer for both paths, rather than a second `format!` beside the
/// second address (#155). Everything above is a property of the address's
/// shape and not of what is posted to it, so a copy would be this paragraph's
/// reasoning held twice and dropped once.
fn seat_url(port: u16, path: &str, room_id: &str, account_id: &str) -> String {
    format!(
        "http://127.0.0.1:{port}{path}/{}/{}",
        percent_encode(room_id),
        percent_encode(account_id)
    )
}

/// The seat a request target names, as `(room id, account id)`, or `None` when
/// it is not under `path` or does not name both halves.
///
/// The inverse of `seat_url`, kept beside it so the two cannot drift: the URL
/// is written into a launch line in one place and read off a socket in
/// another.
///
/// A query is dropped before the path is read, because a request target may
/// carry one whatever this app writes. Nothing above it is: the path is the
/// hook's path and exactly two more segments, and a third segment names no
/// seat. `percent_encode` leaves no `/` inside a half, so the split cannot cut
/// an id in two.
fn parse_seat_target(path: &str, target: &str) -> Option<(String, String)> {
    let target = target.split_once('?').map_or(target, |(path, _)| path);
    let rest = target.strip_prefix(path)?.strip_prefix('/')?;
    let (room, account) = rest.split_once('/')?;
    if account.contains('/') {
        return None;
    }
    let room = percent_decode(room).filter(|id| !id.is_empty())?;
    let account = percent_decode(account).filter(|id| !id.is_empty())?;
    Some((room, account))
}

/// Where one seat's usage-limit hook posts to (#149).
pub fn limited_hook_url(port: u16, room_id: &str, account_id: &str) -> String {
    seat_url(port, LIMITED_HOOK_PATH, room_id, account_id)
}

/// The seat a usage-limit request names, or `None` when it names no seat.
pub fn parse_limited_hook_target(target: &str) -> Option<(String, String)> {
    parse_seat_target(LIMITED_HOOK_PATH, target)
}

/// Where one seat's status line posts to (#155).
pub fn status_hook_url(port: u16, room_id: &str, account_id: &str) -> String {
    seat_url(port, STATUS_HOOK_PATH, room_id, account_id)
}

/// The seat a status-line request names, or `None` when it names no seat.
pub fn parse_status_hook_target(target: &str) -> Option<(String, String)> {
    parse_seat_target(STATUS_HOOK_PATH, target)
}

/// Every byte outside the unreserved set as `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `%XX` back to bytes, or `None` for a malformed escape or a result that is not
/// UTF-8. Nothing else is interpreted: `+` is a plus.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The `hooks` value that has the CLI tell the app its turn stopped on a usage
/// limit.
///
/// `StopFailure`, matched on the error type `rate_limit` and on nothing else
/// (#149, decision 1): the event fires when a turn ends on an API error, and the
/// matcher is what narrows it to the limit. Nothing the CLI printed is read —
/// the signal is the hook firing at this address (#82).
///
/// An HTTP hook rather than a command: the app is already listening, and a
/// command would start a process per signal and put a shell between the CLI
/// and the app. The token reaches the header through the environment
/// (`ROOM_TOKEN_ENV`), so the line carries the variable's name and not its
/// value.
///
/// Added to the user's and the project's hooks rather than in place of them:
/// hook entries merge across settings levels, and `--settings` is merged by the
/// same rules as the files (Claude Code docs, `hooks` and `settings`, read
/// 2026-09-17; not measured on a live CLI).
pub fn limited_hook_settings(url: &str) -> Value {
    json!({
        "StopFailure": [{
            "matcher": "rate_limit",
            "hooks": [{
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer ${{{ROOM_TOKEN_ENV}}}") },
                "allowedEnvVars": [ROOM_TOKEN_ENV],
                "timeout": 5,
            }],
        }],
    })
}

/// Whether one word may be put on a launch line as it is written (#155).
///
/// ASCII letters and digits, and `/ : . _ -`. Everything else is refused,
/// including the space: this word rides inside the `--settings` JSON, and the
/// contents of a JSON string sit outside `cmd.exe`'s quotes (`seat_url`), where
/// `& | < > ^ ( )` are characters it acts on. A directory such as
/// `C:/Program Files (x86)/…` ends the launch line at the `(` — the break #151
/// shipped, one axis over.
///
/// Three shells rather than one, which is why the set is this narrow. The
/// `--settings` argument passes through `cmd.exe`; the status-line command is
/// then run by the CLI through Git Bash where it is installed and PowerShell
/// where it is not (Claude Code docs, `statusline`, read 2026-09-17; not
/// measured on a live CLI). Git Bash reads an unquoted `\` as an escape, so a
/// path arrives with its separators gone — hence a caller folds `\` to `/`
/// before asking. Quoting the word instead would take the parity of the
/// `cmd.exe` scan back the other way, which is a thing to reason about at every
/// later edit; refusing the word costs one row reading `—`.
///
/// Non-ASCII is refused with the rest. A path under a name written in kana
/// reaches `cmd.exe` through a code page this app does not choose, and that is
/// the same unmeasured ground the `%` note in `seat_url` stands on — except
/// that here nothing is lost by declining it.
pub fn line_safe_word(text: &str) -> bool {
    !text.is_empty() && text.chars().all(line_safe_char)
}

/// One character of the set the two checks around it are built from.
///
/// One predicate rather than two spellings of one set: the pair differ by
/// whether the space is in, and a set written twice is a set that drifts once.
fn line_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '/' | ':' | '.' | '_' | '-')
}

/// Whether a whole argument may be put on a launch line as it is written
/// (#147).
///
/// `line_safe_word`'s set with the space added, and that difference is the
/// scope difference between the two. A word rides *inside* the `--settings`
/// JSON, whose contents sit outside `cmd.exe`'s quotes (`seat_url`); and a word
/// carrying no space is written onto the line bare, because the Windows quoting
/// only wraps an argument holding a space, a tab or a `"`
/// (`portable-pty-patch/src/cmdbuilder.rs`, `append_quoted`). Either way there
/// is nothing around it, so `& | < > ^ ( )` are characters `cmd.exe` acts on.
///
/// This is a whole argument of its own, and one holding a space is wrapped in
/// `"` by that same quoting — so its contents would sit *inside* `cmd.exe`'s
/// quotes as long as the quote parity reaching it is even. The set is refused
/// anyway rather than rested on that parity: the parity is a property of every
/// argument written before this one, and what sits before this one is a JSON
/// value whose quote count is even only while it stays JSON. Refusing the set
/// costs nothing here and does not have to be re-reasoned at each later edit,
/// which is the same trade `line_safe_word` takes against quoting the word.
///
/// Non-ASCII is refused with the rest, for the reason it is refused there: a
/// launch line reaches `cmd.exe` through a code page this app does not choose,
/// and this app has not measured what survives it. Nothing is lost by declining
/// it — the reader of this argument is the model, and what it is told is a tool
/// name and one fact about where output goes.
pub fn line_safe_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|ch| ch == ' ' || line_safe_char(ch))
}

/// The program that runs the status-line script.
///
/// By name, the way the sidecar's own `command` is (`spawn_form`): it is an
/// executable rather than a shell script, and the CLI resolves it from the
/// environment the launch handed it.
const STATUS_RUNNER: &str = "node";

/// The status-line command for one seat, or `None` when it cannot be written
/// onto the launch line as it stands (#155).
///
/// `node <script> <url>`, and nothing else on the line: the token is not here,
/// because the line is drawn on screen (`session::preview_launch_args`) and the
/// script reads `ROOM_TOKEN_ENV` out of the environment the launch set, the
/// same place the usage-limit hook's header resolves from.
///
/// `None` rather than a quoted form when either word fails `line_safe_word`.
/// The seat's own address passes by construction — `percent_encode` leaves
/// `%XX`, and `%` is refused here although `seat_url` leaves it standing. The
/// two answers are not in conflict: the hook's address is composed
/// unconditionally and a refusal there would be a launch that cannot report its
/// limit, while this line is an addition, and the safer side for an addition is
/// not to add it.
pub fn status_line_command(script: &Path, url: &str) -> Option<String> {
    let script = script.to_string_lossy().replace('\\', "/");
    (line_safe_word(&script) && line_safe_word(url))
        .then(|| format!("{STATUS_RUNNER} {script} {url}"))
}

/// The `statusLine` value that has the CLI report what it knows about itself.
///
/// **Selected, not merged.** `statusLine` is one value and not a list, so the
/// `--settings` level takes it whole over the user's, the project's and the
/// local file's (Claude Code docs, `settings`, read 2026-09-17; not measured on
/// a live CLI). A session launched from here therefore does not show the status
/// line its person wrote. That is the shape `outputStyle` already has
/// (`settings_launch_args`) rather than the one `hooks` has, and composing the
/// two would mean reading the person's settings files — a second thing to keep
/// in step with the CLI, for a line this app is the only reader of.
pub fn status_line_settings(command: &str) -> Value {
    json!({ "type": "command", "command": command })
}

/// The character an account speaks as, or `None` when it declares none.
///
/// Blank is the same state as absent. The field is a text input on the screen,
/// so an account that had a character and lost it arrives here as an empty
/// string rather than as nothing, and the two have to mean one thing or a
/// cleared field would launch `{"outputStyle":""}`.
pub fn declared_character(character: Option<&str>) -> Option<&str> {
    character.map(str::trim).filter(|name| !name.is_empty())
}

/// The launch arguments carrying what this session declares about itself,
/// given its own: the character it speaks as, the room registration it is,
/// and the room registrations it must not start.
///
/// The character is the `name:` of an output style in the working directory's
/// `.claude/output-styles/`, and it is selected at launch rather than written
/// anywhere: `--settings` takes JSON inline, so two accounts sharing one
/// working directory each get their own character out of the styles already
/// sitting there, and the shared directory gains no per-account file. Gaining
/// one would be the opposite of what sharing the directory is for.
///
/// Selected rather than merged, unlike the channel entry above: `--settings`
/// on the command line wins over the `settings.json` in the directory, so the
/// directory's own default stays as it is and is simply not what this launch
/// reads.
///
/// `own_server` is named in `enabledMcpjsonServers`, which is the approval key
/// (#143). The registration key is per room (`server_name_for`), so every topic
/// is a server name the CLI has never been asked about, and the CLI holds an
/// unapproved `.mcp.json` server at a "New MCP server found" prompt before the
/// session starts. A launch line carrying `--dangerously-skip-permissions`
/// passes that prompt only because the CLI approves pending project servers in
/// that mode when the user settings skip its warning, and a resume line the
/// person wrote without the flag does not — which is why a topic's first launch
/// went straight in and its resume stopped. Approving on the line rather than
/// relying on the mode is what makes both lines one state. It approves for this
/// launch only and writes nothing: the CLI persists an approval into the
/// directory's `settings.local.json`, and one key per topic there is the
/// growth the shared directory is kept free of. It does not override a
/// rejection: a "Continue without" answered earlier is persisted into
/// `disabledMcpjsonServers` and wins over any approval (the CLI checks the
/// rejection first), and it is the person's file to clear, not this app's.
///
/// `disabled` names the sibling accounts' entries (`other_room_servers`). A
/// shared `.mcp.json` holds one entry per account by design (#40), and the CLI
/// starts every enabled server it finds there — so a session in a shared
/// directory spawns the other accounts' sidecars too, and each of those joins
/// the room under the identity it is registered with rather than the one that
/// started it (#103). `disabledMcpjsonServers` is what stops them:
/// `enabledMcpjsonServers` is the approval key, not the start key, and naming
/// only this account's own server there leaves the siblings starting anyway
/// (measured, 2026-08-26).
///
/// Every launch now carries `--settings`, since there is always an own server to
/// approve — with one exception. Options that pass a `--settings` of their own,
/// with no character and nothing to stop, are left untouched: the launch
/// refuses two `--settings` only when a character or a sibling needs the flag,
/// and the approval is not allowed to widen that refusal onto a line that ran
/// before it. Such a line approves through its own settings, or answers the
/// prompt.
///
/// `limited_hook` is the address of this seat's usage-limit signal
/// (`limited_hook_url`), or `None` when the room has no port to name yet. It
/// rides the same way the approval does, and under the same exception: a line
/// left untouched above carries no hook, and its row does not say 制限中
/// (#149). Widening the refusal for it would stop a line that ran before.
///
/// `status_command` is this seat's status-line command (`status_line_command`),
/// or `None` when there is no port to address, or when the script's own path
/// cannot be written onto the line (`line_safe_word`). It rides under the same
/// exception as the two above, and its absence costs the same kind of thing: the
/// panel's five values read `—` for that seat (#155).
pub fn settings_launch_args(
    base: &[String],
    character: Option<&str>,
    own_server: &str,
    disabled: &[String],
    limited_hook: Option<&str>,
    status_command: Option<&str>,
) -> Vec<String> {
    let mut args = base.to_vec();
    let character = declared_character(character);
    if character.is_none() && disabled.is_empty() && declares_settings(base) {
        return args;
    }
    let mut settings = Map::new();
    if let Some(name) = character {
        settings.insert("outputStyle".into(), json!(name));
    }
    settings.insert("enabledMcpjsonServers".into(), json!([own_server]));
    if !disabled.is_empty() {
        settings.insert("disabledMcpjsonServers".into(), json!(disabled));
    }
    if let Some(url) = limited_hook {
        settings.insert("hooks".into(), limited_hook_settings(url));
    }
    if let Some(command) = status_command {
        settings.insert("statusLine".into(), status_line_settings(command));
    }
    args.push(SETTINGS_FLAG.to_string());
    args.push(Value::Object(settings).to_string());
    args
}

/// The whole line one launch runs: the account's options, what the CLI's own
/// conventions put on every line of its kind, the room's channel entry, and the
/// settings this session declares about itself.
///
/// One function rather than two calls at each site, because the line shown on
/// screen and the line spawned have to be the same line. They are produced in
/// different places — a preview command and the launch — and every step either
/// one composes for itself is a step the other can be missing. The conventions
/// go through here for that same reason: they are on the line that runs, so
/// they are on the line the form shows (#156).
///
/// What the kind's conventions put onto the line itself is two things by now:
/// where the session id goes, and what the session is told about the room
/// before its first turn (#147). Both sit between the person's own options and
/// the room's entry. The kind's third contribution here is a gate rather than
/// an argument — whether the hook and the status line ride in the settings at
/// all (`reports_through_settings`).
///
/// `cli` is the CLI this account's kind names, or `None` when its kind names
/// none. A kind naming none is left with the line it would have had before the
/// kinds were split: the room's own entry and the settings the room needs, and
/// nothing this app knows about a CLI. The hook and the status line are the
/// visible half of that — both are Claude Code's spelling, and a kind this app
/// has established nothing about does not get them written onto its line on the
/// chance that they fit (#156, 決定5).
pub fn launch_args(
    base: &[String],
    cli: Option<Cli>,
    server_name: &str,
    character: Option<&str>,
    disabled: &[String],
    limited_hook: Option<&str>,
    status_command: Option<&str>,
) -> Vec<String> {
    let reports = cli.is_some_and(Cli::reports_through_settings);
    settings_launch_args(
        &channel_launch_args(
            &system_prompt_launch_args(&session_id_launch_args(base, cli), cli, server_name),
            server_name,
        ),
        character,
        server_name,
        disabled,
        limited_hook.filter(|_| reports),
        status_command.filter(|_| reports),
    )
}

/// Split a launch-options string the way a shell would, minus the parts a
/// shell does that have no place here.
///
/// Double quotes group, because Windows paths have spaces in them and a bare
/// whitespace split turns one such argument into two without saying so.
/// Nothing else is interpreted: no variable expansion, no globbing, no escape
/// characters — a backslash in a Windows path is a backslash.
pub fn split_launch_options(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;

    for ch in text.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    args
}

/// How the CLI should spawn the sidecar, as a `.mcp.json` command and args.
///
/// Absolute paths and nothing to resolve. The CLI spawns this from the user's
/// own project directory, so anything looked up by name is looked up there:
/// `npx tsx` searched for a package that lives in Pullcept and asked to
/// install it, from a process with no way to answer (#22). `node` is an
/// executable rather than a shell script, so no shell wrapper is needed either.
fn spawn_form(runner: &Path, entry: &Path) -> (&'static str, Vec<String>) {
    (
        "node",
        vec![
            runner.to_string_lossy().to_string(),
            entry.to_string_lossy().to_string(),
        ],
    )
}

/// The `.mcp.json` at `dir`, parsed, or an empty object when there is none.
///
/// Shared by the writer and the reader below so the refusal on a file that is
/// not JSON is one sentence rather than two: the launch reads the file before
/// it writes it, and hearing two different complaints about one file would not
/// tell the person anything the first one did not.
fn read_config(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    // Truncating a file that failed to parse would destroy whatever it held.
    let root: Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{} exists but is not valid JSON ({e}). Fix or move it before starting a session.",
            path.display()
        )
    })?;
    if !root.is_object() {
        return Err(format!("{} is not a JSON object.", path.display()));
    }
    Ok(root)
}

/// Whether one `.mcp.json` entry survives a registration into `room_url`.
///
/// Entries this app wrote in an earlier run can never connect: the room binds a
/// fresh port every run, so their address is dead. Entries carrying the current
/// address are live siblings — the other sessions of this run. Entries with no
/// `PULLCEPT_ROOM_URL` at all are not ours to judge, whatever they are named.
///
/// One predicate rather than two, because `other_room_servers` answers for the
/// file as the registration will leave it and runs before the write. A second
/// copy of this rule would let the list name an entry the write then removed.
fn survives_registration(name: &str, entry: &Value, room_url: &str) -> bool {
    if !name.starts_with(SERVER_PREFIX) {
        return true;
    }
    match entry
        .get("env")
        .and_then(|env| env.get("PULLCEPT_ROOM_URL"))
    {
        Some(Value::String(url)) => url == room_url,
        _ => true,
    }
}

/// The room registrations in `dir` that a launch of `own_server_name` must not
/// start, as the registration into `room_url` will leave the file.
///
/// Every `SERVER_PREFIX` key except this account's own, including one carrying
/// no room address of ours: the CLI starts what the file enables, and it does
/// not consult us about whose entry it is. Enumerated from the file rather than
/// derived from the account list, because an account deleted from the app, or a
/// registration written by some other route, is in the file and not in the list
/// — and it is the file the CLI reads.
///
/// Answers for the post-registration file while running before it: the write
/// only removes entries `survives_registration` rejects and inserts
/// `own_server_name`, which is excluded here either way. Running before means
/// the launch can refuse without having written anything (`--settings` already
/// in the launch options), and a refusal that had already registered would
/// leave behind exactly the entry this issue is about.
pub fn other_room_servers(
    dir: &Path,
    room_url: &str,
    own_server_name: &str,
) -> Result<Vec<String>, String> {
    let root = read_config(&dir.join(".mcp.json"))?;
    let Some(servers) = root.get("mcpServers").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    Ok(servers
        .iter()
        .filter(|(name, entry)| {
            name.starts_with(SERVER_PREFIX)
                && name.as_str() != own_server_name
                && survives_registration(name, entry, room_url)
        })
        .map(|(name, _)| name.clone())
        .collect())
}

/// Merge the room server into the `.mcp.json` at `dir`, preserving whatever
/// else is there. Returns the path written.
///
/// This writes into the user's own project directory, because project scope is
/// where a Claude Code MCP server is normally registered. Only this one key is
/// touched; existing servers and unrelated top-level keys survive verbatim.
pub fn register_sidecar(dir: &Path, room: &RoomRegistration<'_>) -> Result<PathBuf, String> {
    let (command, args) = spawn_form(room.sidecar_runner, room.sidecar_entry);
    let server_name = server_name_for(room.account_id, room.room_id);

    let path = dir.join(".mcp.json");
    let mut root = read_config(&path)?;
    let obj = root.as_object_mut().expect("read_config checked this");
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        return Err(format!("{} has a non-object mcpServers.", path.display()));
    }

    let servers = servers.as_object_mut().expect("checked above");

    // Left in place, a dead entry would have every CLI started in this
    // directory spawn a sidecar that retries nothing forever, and the file
    // would grow by one key per account ever launched here. What survives is
    // `survives_registration`, which `other_room_servers` reads the file
    // through as well.
    servers.retain(|name, entry| survives_registration(name, entry, room.room_url));

    let mut env = Map::new();
    env.insert("PULLCEPT_ROOM_URL".into(), json!(room.room_url));
    env.insert("PULLCEPT_ROOM_TOKEN".into(), json!(room.token));
    env.insert("PULLCEPT_AGENT_NAME".into(), json!(room.agent_name));
    // Unconditional, unlike the hue: a launch always knows which account it is
    // launching, so an absent key here would mean the launch lost it rather
    // than that nobody declared one. Absent on the wire stays a real state —
    // it is what a connection with no account behind it sends (#59).
    env.insert("PULLCEPT_ACCOUNT_ID".into(), json!(room.account_id));
    // Which room this session is in. The address is shared by every room of
    // the run, so without this a connection could not say where it is (#141).
    env.insert("PULLCEPT_ROOM_ID".into(), json!(room.room_id));
    // Only when declared. An undeclared participant is a participant the room
    // derives a hue for, which is not the same state as one who chose that hue.
    if let Some(hue) = room.agent_hue {
        env.insert("PULLCEPT_AGENT_HUE".into(), json!(format!("{hue:.1}")));
    }
    // Only when true, and absence is the other half rather than a launch that
    // lost it: every launch rewrites this entry whole, so a key left over from
    // a run where it did hold cannot survive into one where it does not.
    if room.unseen_history {
        env.insert("PULLCEPT_UNSEEN_HISTORY".into(), json!("1"));
    }

    servers.insert(
        server_name,
        json!({
            "command": command,
            "args": args,
            "env": Value::Object(env),
        }),
    );

    let text = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))?;
    std::fs::write(&path, text).map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("pullcept-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const ENTRY: &str = "C:/pullcept/sidecar/src/index.ts";
    const RUNNER: &str = "C:/pullcept/node_modules/tsx/dist/cli.mjs";
    /// The account `Lin` is launched from. Opaque here as it is in the app: a
    /// test that read a name out of it would be testing the wrong key.
    const LIN: &str = "8f14e45f-ceea-467a-b160-6f14e45fceea";
    const LAY: &str = "2b1c9a70-3d4e-4f80-91a2-b3c4d5e6f708";
    /// The room — the topic — a launch goes into. Opaque, like the accounts.
    const ROOM: &str = "5d41402a-bc4b-4a76-b971-9d911017c592";
    /// A second topic, open at the same time as the first (#141).
    const OTHER_ROOM: &str = "7e2b0a9c-1f3d-4c5e-8a6b-0c1d2e3f4a5b";

    fn registration<'a>(entry: &'a Path, runner: &'a Path) -> RoomRegistration<'a> {
        RoomRegistration {
            room_url: "ws://127.0.0.1:1234",
            token: "tok",
            account_id: LIN,
            room_id: ROOM,
            agent_name: "Lin",
            agent_hue: None,
            unseen_history: false,
            sidecar_entry: entry,
            sidecar_runner: runner,
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("parse")
    }

    #[test]
    fn registers_the_room_server_when_no_config_exists() {
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");

        let json = read(&path);
        let server = &json["mcpServers"][server_name_for(LIN, ROOM)];
        assert_eq!(server["env"]["PULLCEPT_ROOM_URL"], "ws://127.0.0.1:1234");
        assert_eq!(server["env"]["PULLCEPT_ROOM_TOKEN"], "tok");
        assert_eq!(server["env"]["PULLCEPT_AGENT_NAME"], "Lin");
        // The id the sidecar declares in `hello`, so the room can carry it on
        // the seat and the screen can match a participant to one of its
        // accounts without matching on a name (#59). The id, not the key
        // derived from it: the key is a `.mcp.json` concern, and the room has
        // no way back from it to the account.
        assert_eq!(server["env"]["PULLCEPT_ACCOUNT_ID"], LIN);
        // The room this session is in, which the sidecar names in `hello`: one
        // address serves every room of the run, so the address cannot (#141).
        assert_eq!(server["env"]["PULLCEPT_ROOM_ID"], ROOM);
        // Undeclared is the key absent, not a default value: a hue written here
        // would be a declaration this participant never made.
        assert!(
            !server["env"]
                .as_object()
                .expect("env")
                .contains_key("PULLCEPT_AGENT_HUE"),
            "an undeclared hue must leave no key behind"
        );

        // Absolute paths and nothing looked up by name: the CLI runs this from
        // the user's own directory, where `npx tsx` found no tsx and asked to
        // install one (#22).
        assert_eq!(server["command"], "node");
        assert_eq!(
            server["args"].as_array().expect("args"),
            &vec![
                Value::String(RUNNER.to_string()),
                Value::String(ENTRY.to_string()),
            ]
        );
    }

    #[test]
    fn the_unseen_history_flag_is_written_only_for_a_seat_that_has_one() {
        // Both directions, because the sidecar reads presence: a key that stayed
        // behind from a launch where it did hold would tell the next session it
        // is missing something in a topic nobody has spoken in (#133).
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);

        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("no history");
        let json = read(&path);
        assert!(
            !json["mcpServers"][server_name_for(LIN, ROOM)]["env"]
                .as_object()
                .expect("env")
                .contains_key("PULLCEPT_UNSEEN_HISTORY"),
            "a seat with nothing behind it must leave no key"
        );

        let seated_late = RoomRegistration {
            unseen_history: true,
            ..registration(&entry, &runner)
        };
        let path = register_sidecar(scratch.path(), &seated_late).expect("history");
        let json = read(&path);
        assert_eq!(
            json["mcpServers"][server_name_for(LIN, ROOM)]["env"]["PULLCEPT_UNSEEN_HISTORY"],
            "1"
        );

        // And back again, on the same file: the entry is rewritten whole, so
        // the key goes when the state does.
        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("no history");
        let json = read(&path);
        assert!(
            !json["mcpServers"][server_name_for(LIN, ROOM)]["env"]
                .as_object()
                .expect("env")
                .contains_key("PULLCEPT_UNSEEN_HISTORY"),
            "the key must not survive into a launch the state does not hold for"
        );
    }

    #[test]
    fn leaves_everything_else_in_the_config_alone() {
        // This writes into the user's own project directory. Clobbering a
        // server they configured themselves is the failure that matters here.
        let scratch = Scratch::new();
        std::fs::write(
            scratch.path().join(".mcp.json"),
            r#"{"mcpServers":{"theirs":{"command":"their-server"}},"unrelated":42}"#,
        )
        .expect("seed");

        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");

        let json = read(&path);
        assert_eq!(json["mcpServers"]["theirs"]["command"], "their-server");
        assert_eq!(json["unrelated"], 42);
        assert!(json["mcpServers"][server_name_for(LIN, ROOM)].is_object());
    }

    #[test]
    fn re_registering_the_same_account_replaces_its_own_entry() {
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);

        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("first");
        let second = RoomRegistration {
            room_url: "ws://127.0.0.1:1234",
            token: "tok2",
            account_id: LIN,
            room_id: ROOM,
            agent_name: "Lin",
            agent_hue: Some(145.0),
            unseen_history: false,
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &second).expect("second");

        let json = read(&path);
        let server = &json["mcpServers"][server_name_for(LIN, ROOM)];
        assert_eq!(server["env"]["PULLCEPT_ROOM_TOKEN"], "tok2");
        assert_eq!(server["env"]["PULLCEPT_AGENT_HUE"], "145.0");
        assert_eq!(
            json["mcpServers"].as_object().expect("servers").len(),
            1,
            "relaunching one account must not accumulate entries"
        );
    }

    #[test]
    fn renaming_an_account_leaves_its_registration_where_it_was() {
        // The reason the key moved off the name (#53). An account's name is
        // editable, and a key derived from it moved on every edit: the entry
        // the running session was launched against would be orphaned under the
        // old key, while the CLI holding that session still names the old
        // `server:` tag on a command line nothing can go back and change.
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);

        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("as Lin");
        let renamed = RoomRegistration {
            room_url: "ws://127.0.0.1:1234",
            token: "tok",
            account_id: LIN,
            room_id: ROOM,
            agent_name: "リン",
            agent_hue: None,
            unseen_history: false,
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &renamed).expect("as リン");

        let json = read(&path);
        let servers = json["mcpServers"].as_object().expect("servers");
        assert_eq!(servers.len(), 1, "a rename must not open a second entry");
        assert_eq!(
            json["mcpServers"][server_name_for(LIN, ROOM)]["env"]["PULLCEPT_AGENT_NAME"],
            "リン",
            "the new name belongs in the entry the id already had"
        );
    }

    #[test]
    fn two_accounts_in_one_directory_keep_separate_entries() {
        // The failure this key scheme exists for: two sessions pointed at the
        // same working directory. Under one fixed key the second launch
        // overwrote the first one's name while that session was still running,
        // so the room heard one identity twice (#40).
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);

        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("Lin");
        let lay = RoomRegistration {
            room_url: "ws://127.0.0.1:1234",
            token: "tok",
            account_id: LAY,
            room_id: ROOM,
            agent_name: "Lay",
            agent_hue: Some(25.0),
            unseen_history: false,
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &lay).expect("Lay");

        let json = read(&path);
        assert_eq!(
            json["mcpServers"][server_name_for(LIN, ROOM)]["env"]["PULLCEPT_AGENT_NAME"],
            "Lin",
            "the first session's identity must survive the second launch"
        );
        assert_eq!(
            json["mcpServers"][server_name_for(LAY, ROOM)]["env"]["PULLCEPT_AGENT_NAME"],
            "Lay"
        );
        assert_eq!(json["mcpServers"].as_object().expect("servers").len(), 2);
    }

    #[test]
    fn entries_from_a_previous_run_go_and_foreign_ones_stay() {
        // The room binds a fresh port every run, so an entry carrying another
        // address is one no sidecar can reach. Left behind, every CLI started
        // in this directory would spawn one more sidecar retrying a dead port,
        // and the file would grow by one key per name ever used here.
        let scratch = Scratch::new();
        std::fs::write(
            scratch.path().join(".mcp.json"),
            r#"{"mcpServers":{
                 "pullcept-room": {"env":{"PULLCEPT_ROOM_URL":"ws://127.0.0.1:1"}},
                 "pullcept-room-lay-00000000": {"env":{"PULLCEPT_ROOM_URL":"ws://127.0.0.1:1234"}},
                 "pullcept-room-theirs": {"command":"not-ours"},
                 "theirs": {"command":"their-server"}
               }}"#,
        )
        .expect("seed");

        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");

        let json = read(&path);
        let servers = json["mcpServers"].as_object().expect("servers");
        assert!(
            !servers.contains_key("pullcept-room"),
            "an entry from a previous run must go"
        );
        assert!(
            servers.contains_key("pullcept-room-lay-00000000"),
            "a live sibling of this run must stay"
        );
        assert!(
            servers.contains_key("pullcept-room-theirs"),
            "an entry with no room address of ours is not ours to remove"
        );
        assert!(servers.contains_key("theirs"));
        assert!(servers.contains_key(&server_name_for(LIN, ROOM)));
    }

    #[test]
    fn a_server_name_is_a_function_of_the_account_and_the_room() {
        assert_eq!(server_name_for(LIN, ROOM), server_name_for(LIN, ROOM));
        assert_ne!(server_name_for(LIN, ROOM), server_name_for(LAY, ROOM));
        // One account in two topics is two entries (#141, decision 4).
        assert_ne!(server_name_for(LIN, ROOM), server_name_for(LIN, OTHER_ROOM));
        assert!(server_name_for(LIN, ROOM).starts_with(SERVER_PREFIX));
        // The pair is hashed as a pair: moving characters across the boundary
        // between the two ids is a different pair, not the same concatenation.
        assert_ne!(server_name_for("ab", "c"), server_name_for("a", "bc"));
        // Total over whatever an id turns out to be, as it was over a name: an
        // input that slugs to nothing still gets a key of its own, and two that
        // slug alike still get two. The uniqueness lives in the hash, and the
        // ids the app mints do not lean on the slug for it.
        assert!(server_name_for("マスター", ROOM).starts_with("pullcept-room-"));
        assert_ne!(server_name_for("マスター", ROOM), server_name_for("ますたー", ROOM));
        assert_ne!(server_name_for("Lin", ROOM), server_name_for("lin!", ROOM));
        // Nothing outside the set a console and a JSON key both leave alone.
        assert!(server_name_for("Lin さん / 2", "部屋 1")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn one_account_in_two_rooms_keeps_both_entries() {
        // The case #141 opens: a session left running in one topic while the
        // same account is started in another. Both write into the account's own
        // directory, and the second must not take the first one's room away
        // from under it — nor sweep it as dead, since both carry this run's
        // address.
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);

        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("first room");
        let elsewhere = RoomRegistration {
            room_id: OTHER_ROOM,
            ..registration(&entry, &runner)
        };
        let path = register_sidecar(scratch.path(), &elsewhere).expect("second room");

        let json = read(&path);
        assert_eq!(json["mcpServers"].as_object().expect("servers").len(), 2);
        assert_eq!(
            json["mcpServers"][server_name_for(LIN, ROOM)]["env"]["PULLCEPT_ROOM_ID"],
            ROOM,
            "the first room's entry must survive the launch into the second"
        );
        assert_eq!(
            json["mcpServers"][server_name_for(LIN, OTHER_ROOM)]["env"]["PULLCEPT_ROOM_ID"],
            OTHER_ROOM
        );

        // And the session in the second room does not start the first room's
        // sidecar: it is a registration this launch is not, whoever's it is.
        assert_eq!(
            other_room_servers(
                scratch.path(),
                "ws://127.0.0.1:1234",
                &server_name_for(LIN, OTHER_ROOM)
            )
            .expect("read"),
            vec![server_name_for(LIN, ROOM)]
        );
    }

    #[test]
    fn refuses_to_overwrite_a_config_it_cannot_parse() {
        let scratch = Scratch::new();
        let seeded = "{ not json";
        std::fs::write(scratch.path().join(".mcp.json"), seeded).expect("seed");

        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        let err = register_sidecar(scratch.path(), &registration(&entry, &runner)).unwrap_err();
        assert!(err.contains("not valid JSON"), "unexpected error: {err}");
        assert_eq!(
            std::fs::read_to_string(scratch.path().join(".mcp.json")).expect("read"),
            seeded,
            "the unparseable file must be left as it was"
        );
    }

    #[test]
    fn rejects_flags_that_stop_channel_pushes() {
        for arg in ["--channels", "--print", "--input-format", "--output-format"] {
            let args = vec!["--verbose".to_string(), arg.to_string()];
            assert_eq!(reject_incompatible_flags(&args), Err(arg));
        }
    }

    #[test]
    fn rejects_an_incompatible_flag_written_with_an_equals_sign() {
        let args = vec!["--output-format=stream-json".to_string()];
        assert_eq!(reject_incompatible_flags(&args), Err("--output-format"));
    }

    #[test]
    fn accepts_arguments_that_do_not_touch_that_axis() {
        let args = vec!["--verbose".to_string(), "--model=opus".to_string()];
        assert_eq!(reject_incompatible_flags(&args), Ok(()));
    }

    #[test]
    fn merges_the_room_into_a_channel_flag_the_person_already_wrote() {
        // Master's own launch line, which names a different channel server.
        // A second copy of the flag is the `--channels` failure in another
        // shape, and dropping the room entry loses the room's input path.
        let base: Vec<String> = [
            "--dangerously-skip-permissions",
            CHANNEL_FLAG,
            "server:github-webhook-mcp",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let room = server_name_for(LIN, ROOM);
        let merged = channel_launch_args(&base, &room);
        assert_eq!(
            merged,
            vec![
                "--dangerously-skip-permissions".to_string(),
                CHANNEL_FLAG.to_string(),
                format!("server:{room}"),
                "server:github-webhook-mcp".to_string(),
            ]
        );
        assert_eq!(
            merged.iter().filter(|arg| *arg == CHANNEL_FLAG).count(),
            1,
            "the flag must not appear twice"
        );
    }

    #[test]
    fn does_not_add_the_room_twice() {
        let room = server_name_for(LIN, ROOM);
        let base = vec![CHANNEL_FLAG.to_string(), format!("server:{room}")];
        assert_eq!(channel_launch_args(&base, &room), base);
    }

    /// This launch's own registration, as the settings tests name it.
    fn own() -> String {
        server_name_for(LIN, ROOM)
    }

    #[test]
    fn a_declared_character_rides_in_settings_json() {
        let args =
            settings_launch_args(
            &["--verbose".to_string()],
            Some("character_Lay"),
            &own(),
            &[],
            None,
            None,
        );
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "--verbose");
        assert_eq!(args[1], SETTINGS_FLAG);
        // The value has to parse as JSON on the other side: it is handed to the
        // CLI inline rather than written to a file anyone could look at.
        let settled: Value = serde_json::from_str(&args[2]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!("character_Lay"));
    }

    #[test]
    fn a_character_with_a_quote_in_it_stays_one_json_string() {
        // Built rather than formatted, so a name that would otherwise close the
        // string early cannot make the value stop being JSON.
        let args = settings_launch_args(&[], Some(r#"quote"style"#), &own(), &[], None, None);
        let settled: Value = serde_json::from_str(&args[1]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!(r#"quote"style"#));
    }

    #[test]
    fn the_launch_approves_its_own_room_server() {
        // The server name is per topic, so every topic is a name the CLI has
        // never been asked about, and an unapproved one holds the session at a
        // prompt before it starts. The first launch passed only because its
        // line carried the bypass flag; a resume line without it stopped
        // (#143). Approved on every line, so the two lines are one state.
        let resume = split_launch_options("--resume {session_id}");
        let line = launch_args(&resume, Some(Cli::ClaudeCode), &own(), None, &[], None, None);
        // And the CLI's own way of handing over an id is not added on top of
        // it: the line already names where the id goes, and a second flag would
        // hand the CLI two (#156).
        assert!(!line.iter().any(|arg| arg == "--session-id"), "{line:?}");
        let at = line
            .iter()
            .position(|arg| arg == SETTINGS_FLAG)
            .expect("a line with nothing else to declare still carries settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        assert_eq!(settled["enabledMcpjsonServers"], json!([own()]));
        // Only the approval: an undeclared character is not an empty style,
        // and nothing to stop is not an empty list.
        assert_eq!(settled.as_object().expect("object").len(), 1);
    }

    #[test]
    fn a_blank_character_is_not_an_empty_style() {
        // Absent and blank are one state: a cleared field must not launch
        // `{"outputStyle":""}`, which names no style at all.
        for character in [None, Some(""), Some("   ")] {
            let args = settings_launch_args(&["--verbose".to_string()], character, &own(), &[], None, None);
            let settled: Value = serde_json::from_str(&args[2]).expect("valid JSON");
            assert!(settled.get("outputStyle").is_none(), "{character:?}");
        }
        assert_eq!(declared_character(Some("  x  ")), Some("x"));
    }

    #[test]
    fn a_line_with_its_own_settings_and_nothing_else_to_declare_is_left_alone() {
        // The approval does not widen the two-`--settings` refusal: that stays
        // where a character or a sibling needs the flag, so a line that ran
        // before the approval was added runs the same after it.
        let base = vec![SETTINGS_FLAG.to_string(), r#"{"model":"x"}"#.to_string()];
        assert_eq!(settings_launch_args(&base, None, &own(), &[], None, None), base);
        assert_eq!(settings_launch_args(&base, Some("  "), &own(), &[], None, None), base);
        let written = vec!["--settings=C:/x/settings.json".to_string()];
        assert_eq!(settings_launch_args(&written, None, &own(), &[], None, None), written);
    }

    #[test]
    fn a_sibling_registration_is_named_as_one_this_launch_does_not_start() {
        // The launch's own entry is not on the list: a session that disabled
        // its own room server would be a session with no room (#103).
        let lay = server_name_for(LAY, ROOM);
        let args =
            settings_launch_args(
            &[],
            Some("character_Lin"),
            &own(),
            std::slice::from_ref(&lay),
            None,
            None,
        );
        let settled: Value = serde_json::from_str(&args[1]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!("character_Lin"));
        assert_eq!(settled["enabledMcpjsonServers"], json!([own()]));
        assert_eq!(settled["disabledMcpjsonServers"], json!([lay]));
    }

    #[test]
    fn a_sibling_registration_puts_settings_on_a_line_with_no_character() {
        // An account with no character in a shared directory still has to stop
        // the other account's sidecar (#103).
        let lay = server_name_for(LAY, ROOM);
        let args = settings_launch_args(
            &["--verbose".to_string()],
            None,
            &own(),
            std::slice::from_ref(&lay),
            None,
            None,
        );
        assert_eq!(args[0], "--verbose");
        assert_eq!(args[1], SETTINGS_FLAG);
        let settled: Value = serde_json::from_str(&args[2]).expect("valid JSON");
        assert_eq!(settled["disabledMcpjsonServers"], json!([lay]));
        assert!(
            settled.get("outputStyle").is_none(),
            "an undeclared character must not become an empty style"
        );
    }

    #[test]
    fn the_siblings_are_read_from_the_file_as_the_registration_leaves_it() {
        // Enumerated from the file rather than from the account list: an entry
        // the app no longer has an account for is still one the CLI starts.
        let scratch = Scratch::new();
        std::fs::write(
            scratch.path().join(".mcp.json"),
            r#"{"mcpServers":{
                 "pullcept-room": {"env":{"PULLCEPT_ROOM_URL":"ws://127.0.0.1:1"}},
                 "pullcept-room-lay-00000000": {"env":{"PULLCEPT_ROOM_URL":"ws://127.0.0.1:1234"}},
                 "pullcept-room-theirs": {"command":"not-ours"},
                 "theirs": {"command":"their-server"}
               }}"#,
        )
        .expect("seed");

        let others =
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN, ROOM))
                .expect("read");
        assert_eq!(
            others,
            vec![
                "pullcept-room-lay-00000000".to_string(),
                "pullcept-room-theirs".to_string(),
            ],
            "a live sibling and a prefixed entry of unknown origin are both started by the CLI; \
             a dead entry the registration removes is not"
        );

        // Read before the write, answering for the file after it. Registering
        // must not change what the list said.
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");
        assert_eq!(
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN, ROOM))
                .expect("read"),
            others
        );
    }

    #[test]
    fn a_directory_with_no_registrations_names_nothing() {
        let scratch = Scratch::new();
        assert!(
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN, ROOM))
                .expect("read")
                .is_empty(),
            "no file is not a failure: it is a directory nothing has been registered in yet"
        );

        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");
        assert!(
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN, ROOM))
                .expect("read")
                .is_empty(),
            "the only entry is this account's own, and disabling it would leave it roomless"
        );
    }

    #[test]
    fn a_config_that_is_not_json_is_refused_before_anything_is_written() {
        let scratch = Scratch::new();
        std::fs::write(scratch.path().join(".mcp.json"), "{ not json").expect("seed");
        let err = other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN, ROOM))
            .expect_err("must refuse");
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn settings_written_by_hand_is_visible_to_the_caller() {
        // `=` form included: the launch refuses the pair rather than spawning a
        // line whose two `--settings` are read in an order nobody declared.
        assert!(declares_settings(&[SETTINGS_FLAG.to_string()]));
        assert!(declares_settings(&["--settings=C:/x/settings.json".to_string()]));
        assert!(!declares_settings(&["--verbose".to_string()]));
    }

    #[test]
    fn one_line_carries_the_room_and_the_character_together() {
        // The preview and the spawn both come through here. A site composing
        // the two halves for itself is a site the other one can drift from.
        let room = server_name_for(LIN, ROOM);
        let line = launch_args(
            &["--verbose".to_string()],
            Some(Cli::ClaudeCode),
            &room,
            Some("character_Lin"),
            &[],
            None,
            None,
        );
        // The person's own options, then what the kind's conventions carry
        // (#156 / #147), then the room's two halves.
        let told = Cli::ClaudeCode.room_system_prompt(&room).expect("a safe text");
        assert_eq!(
            line[..8],
            [
                "--verbose".to_string(),
                "--session-id".to_string(),
                SESSION_ID_PLACEHOLDER.to_string(),
                APPEND_SYSTEM_PROMPT_FLAG.to_string(),
                told,
                CHANNEL_FLAG.to_string(),
                format!("server:{room}"),
                SETTINGS_FLAG.to_string(),
            ]
        );
        assert_eq!(line.len(), 9);
        let settled: Value = serde_json::from_str(&line[8]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!("character_Lin"));
        // The approval rides with it, naming the server the channel flag names
        // (#143): one fact in the `.mcp.json` key, the tag and the approval.
        assert_eq!(settled["enabledMcpjsonServers"], json!([room]));
    }

    #[test]
    fn one_line_starts_this_account_and_stops_the_others() {
        // The room entry names this session's own server and the settings name
        // the ones it must leave alone: the same file, read twice, must not
        // disagree about which entry is whose (#103).
        let room = server_name_for(LIN, ROOM);
        let lay = server_name_for(LAY, ROOM);
        // On the kind that carries no conventions of its own, so the line is
        // the room's two halves and nothing else — which is what this reads.
        let line = launch_args(
            &[],
            None,
            &room,
            Some("character_Lin"),
            std::slice::from_ref(&lay),
            None,
            None,
        );
        assert_eq!(line[0], CHANNEL_FLAG);
        assert_eq!(line[1], format!("server:{room}"));
        assert_eq!(line[2], SETTINGS_FLAG);
        let settled: Value = serde_json::from_str(&line[3]).expect("valid JSON");
        assert_eq!(settled["enabledMcpjsonServers"], json!([room]));
        assert_eq!(settled["disabledMcpjsonServers"], json!([lay]));
        assert_ne!(
            settled["disabledMcpjsonServers"][0],
            json!(room),
            "the server the channel flag just named must not be disabled"
        );
    }

    #[test]
    fn the_limit_hook_names_the_seat_it_was_launched_for() {
        // The seat is a topic and an account, and the CLI's own hook input
        // carries neither — so the address carries both, and reading it back
        // has to land on the same pair (#149).
        let url = limited_hook_url(1234, ROOM, LIN);
        let target = url
            .strip_prefix("http://127.0.0.1:1234")
            .expect("loopback, on the room's port");
        assert_eq!(
            parse_limited_hook_target(target),
            Some((ROOM.to_string(), LIN.to_string()))
        );
    }

    #[test]
    fn a_target_that_is_not_the_limit_hook_names_no_seat() {
        assert_eq!(parse_limited_hook_target("/hooks/limited"), None);
        assert_eq!(parse_limited_hook_target("/hooks/limited/"), None);
        assert_eq!(parse_limited_hook_target("/other/a/b"), None);
        assert_eq!(parse_limited_hook_target("/hooks/limited-x/a/b"), None);
        // One half is not a seat, and neither is an empty one.
        assert_eq!(parse_limited_hook_target("/hooks/limited/a"), None);
        assert_eq!(parse_limited_hook_target("/hooks/limited//b"), None);
        assert_eq!(parse_limited_hook_target("/hooks/limited/a/"), None);
        // A third segment names no seat: `percent_encode` writes no `/` inside
        // a half, so this is not one.
        assert_eq!(parse_limited_hook_target("/hooks/limited/a/b/c"), None);
        assert_eq!(parse_limited_hook_target("/hooks/limited/%zz/b"), None);
        // The query form #151 shipped is not read back as a seat (#152).
        assert_eq!(parse_limited_hook_target("/hooks/limited?room=a&account=b"), None);
    }

    #[test]
    fn the_limit_hook_address_carries_nothing_a_windows_shell_acts_on() {
        // The address rides in the `--settings` JSON, which reaches the CLI
        // through `cmd.exe /C` on Windows, and the quoting puts every JSON
        // string's contents outside `cmd.exe`'s quotes (see
        // `limited_hook_url`). A `&` there ended the launch line and no
        // account could start (#152).
        let odd = "a&b|c<d>e^f(g)h i/j";
        let url = limited_hook_url(62361, "部屋 1", odd);
        for ch in ['&', '|', '<', '>', '^', '(', ')', '"', ' '] {
            assert!(
                !url.contains(ch),
                "{ch:?} is acted on by cmd.exe and must not reach the launch line: {url}"
            );
        }
        // Encoded rather than dropped: the halves still name the seat.
        let target = url.strip_prefix("http://127.0.0.1:62361").expect("prefix");
        assert_eq!(
            parse_limited_hook_target(target),
            Some(("部屋 1".to_string(), odd.to_string()))
        );
    }

    #[test]
    fn the_limit_hook_rides_in_settings_and_fires_on_the_rate_limit_only() {
        let url = limited_hook_url(1234, ROOM, LIN);
        let line = launch_args(&[], Some(Cli::ClaudeCode), &own(), None, &[], Some(&url), None);
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        let groups = settled["hooks"]["StopFailure"].as_array().expect("StopFailure");
        assert_eq!(groups.len(), 1);
        // Decision 1: the limit, and no other API error.
        assert_eq!(groups[0]["matcher"], json!("rate_limit"));
        let hook = &groups[0]["hooks"][0];
        assert_eq!(hook["type"], json!("http"));
        assert_eq!(hook["url"], json!(url));
        // The variable's name is on the line, never the token: the line is
        // drawn on screen.
        assert_eq!(
            hook["headers"]["Authorization"],
            json!(format!("Bearer ${{{ROOM_TOKEN_ENV}}}"))
        );
        assert_eq!(hook["allowedEnvVars"], json!([ROOM_TOKEN_ENV]));
        // Nothing else of the hooks is declared, so nothing of the person's
        // own is named here to be replaced.
        assert_eq!(settled["hooks"].as_object().expect("hooks").len(), 1);
    }

    #[test]
    fn a_line_with_no_room_port_carries_no_hook() {
        let line = launch_args(&[], Some(Cli::ClaudeCode), &own(), None, &[], None, None);
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        assert!(settled.get("hooks").is_none());
    }

    #[test]
    fn a_line_left_alone_for_its_own_settings_carries_no_hook_either() {
        // The hook does not widen the two-`--settings` refusal any more than
        // the approval does (#143 / #149).
        let base = vec![SETTINGS_FLAG.to_string(), r#"{"model":"x"}"#.to_string()];
        let url = limited_hook_url(1234, ROOM, LIN);
        assert_eq!(settings_launch_args(&base, None, &own(), &[], Some(&url), None), base);
    }

    #[test]
    fn the_status_line_names_the_seat_it_was_launched_for() {
        // The same shape as the limit hook's address, and read back onto the
        // same pair — the status line's own JSON names the CLI's session id,
        // which is not what a seat is keyed on (#155, decision 2).
        let url = status_hook_url(1234, ROOM, LIN);
        let target = url
            .strip_prefix("http://127.0.0.1:1234")
            .expect("the address is the room's own port on loopback");
        assert_eq!(target, format!("{STATUS_HOOK_PATH}/{ROOM}/{LIN}"));
        assert_eq!(
            parse_status_hook_target(target),
            Some((ROOM.to_string(), LIN.to_string()))
        );
    }

    #[test]
    fn the_two_seat_paths_do_not_answer_for_each_other() {
        // One listener, two paths. A status report read as a usage limit would
        // put 制限中 on a row every time the model answered.
        let limited = limited_hook_url(1234, ROOM, LIN);
        let status = status_hook_url(1234, ROOM, LIN);
        let limited = limited.strip_prefix("http://127.0.0.1:1234").expect("prefix");
        let status = status.strip_prefix("http://127.0.0.1:1234").expect("prefix");
        assert_ne!(limited, status);
        assert_eq!(parse_status_hook_target(limited), None);
        assert_eq!(parse_limited_hook_target(status), None);
    }

    #[test]
    fn a_status_line_command_carries_nothing_a_shell_on_the_way_acts_on() {
        // The word set, both directions. Every character here is one `cmd.exe`
        // acts on where it stands outside the quotes — which is where the
        // contents of a JSON string stand (#152) — or one Git Bash reads as an
        // escape.
        assert!(line_safe_word("C:/pullcept/sidecar/src/status.mjs"));
        for hazard in [
            "C:/Program Files (x86)/p/status.mjs",
            "C:/a&b/status.mjs",
            "C:/a|b/status.mjs",
            "C:/a^b/status.mjs",
            "C:/a<b/status.mjs",
            "C:/a>b/status.mjs",
            // A space is refused with them: quoting the word back would take
            // the parity of the `cmd.exe` scan the other way again.
            "C:/my files/status.mjs",
            // A backslash never reaches the line — the caller folds it — and a
            // word still holding one is refused rather than repaired here.
            "C:\\pullcept\\status.mjs",
            "C:/ゆーざ/status.mjs",
            "",
        ] {
            assert!(!line_safe_word(hazard), "{hazard}");
        }
    }

    #[test]
    fn the_status_line_rides_in_settings_and_names_the_script_and_the_seat() {
        let url = status_hook_url(1234, ROOM, LIN);
        let script = PathBuf::from(r"C:\pullcept\sidecar\src\status.mjs");
        let command = status_line_command(&script, &url).expect("a path with nothing to escape");
        // Forward slashes: Git Bash eats an unquoted `\` before the script is
        // ever run (Claude Code docs, `statusline`, read 2026-09-17).
        assert_eq!(command, format!("node C:/pullcept/sidecar/src/status.mjs {url}"));
        let line = launch_args(&[], Some(Cli::ClaudeCode), &own(), None, &[], None, Some(&command));
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        assert_eq!(settled["statusLine"]["type"], json!("command"));
        assert_eq!(settled["statusLine"]["command"], json!(command));
        // The token is not on the line, and neither is the variable that would
        // resolve to it: the script reads it out of the environment the launch
        // set, rather than being handed it the way the hook's header is.
        assert!(!command.contains(ROOM_TOKEN_ENV), "{command}");
        assert!(!command.contains("Bearer"), "{command}");
    }

    #[test]
    fn the_whole_settings_argument_carries_nothing_cmd_exe_acts_on() {
        // What the two word-level checks above are for, asserted on the thing
        // that actually reaches `cmd.exe`: the composed `--settings` value,
        // with everything this launch declares on it at once.
        //
        // `"` is not in the set. `cmd.exe` counts quotes rather than acting on
        // them, and the JSON cannot be written without them — the parity that
        // counting produces is what puts these contents outside the quotes in
        // the first place (`seat_url`), which is why the rest of the set is
        // checked here at all (#152).
        let lay = server_name_for(LAY, ROOM);
        let limited = limited_hook_url(62361, ROOM, LIN);
        let status = status_hook_url(62361, ROOM, LIN);
        let command = status_line_command(
            &PathBuf::from("C:/pullcept/sidecar/src/status.mjs"),
            &status,
        )
        .expect("safe path");
        let line = launch_args(
            &[],
            Some(Cli::ClaudeCode),
            &own(),
            Some("character_Lin"),
            std::slice::from_ref(&lay),
            Some(&limited),
            Some(&command),
        );
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settings = &line[at + 1];
        for ch in ['&', '|', '<', '>', '^', '(', ')'] {
            assert!(
                !settings.contains(ch),
                "{ch:?} is acted on by cmd.exe and must not reach the launch line: {settings}"
            );
        }
        // And it is still JSON on the other side.
        let settled: Value = serde_json::from_str(settings).expect("valid JSON");
        assert_eq!(settled["statusLine"]["command"], json!(command));
        assert_eq!(settled["hooks"]["StopFailure"][0]["hooks"][0]["url"], json!(limited));
    }

    #[test]
    fn a_script_path_that_cannot_be_written_onto_the_line_carries_no_status_line() {
        // The launch still runs; the panel's five values read `—` for that
        // seat. Refusing the line instead would be #151 again.
        let url = status_hook_url(1234, ROOM, LIN);
        let script = PathBuf::from(r"C:\Program Files (x86)\pullcept\status.mjs");
        assert_eq!(status_line_command(&script, &url), None);
        let line = launch_args(&[], Some(Cli::ClaudeCode), &own(), None, &[], None, None);
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        assert!(settled.get("statusLine").is_none());
    }

    #[test]
    fn a_line_left_alone_for_its_own_settings_carries_no_status_line_either() {
        // Same exception as the approval and the hook: a line that ran before
        // this was added runs the same after it (#143 / #149 / #155).
        let base = vec![SETTINGS_FLAG.to_string(), r#"{"model":"x"}"#.to_string()];
        let url = status_hook_url(1234, ROOM, LIN);
        let script = PathBuf::from("C:/pullcept/sidecar/src/status.mjs");
        let command = status_line_command(&script, &url).expect("safe path");
        assert_eq!(
            settings_launch_args(&base, None, &own(), &[], None, Some(&command)),
            base
        );
    }

    #[test]
    fn splits_launch_options_keeping_quoted_arguments_whole() {
        assert_eq!(
            split_launch_options("  --a   --b=1  "),
            vec!["--a".to_string(), "--b=1".to_string()]
        );
        // A Windows path with spaces is one argument, and its backslashes are
        // literal rather than escapes.
        assert_eq!(
            split_launch_options(r#"--add-dir "C:\Program Files\x" --flag"#),
            vec![
                "--add-dir".to_string(),
                r"C:\Program Files\x".to_string(),
                "--flag".to_string(),
            ]
        );
        assert!(split_launch_options("   ").is_empty());
        // An empty quoted argument is an argument, not nothing.
        assert_eq!(
            split_launch_options("--x \"\""),
            vec!["--x".to_string(), String::new()]
        );
    }

    #[test]
    fn the_launch_flag_names_the_server_the_config_registers() {
        // The flag and the `.mcp.json` key are one fact in two places; a drift
        // between them fails as a room that never receives anything.
        let scratch = Scratch::new();
        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        let path =
            register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");
        let registered = read(&path)["mcpServers"]
            .as_object()
            .expect("servers")
            .keys()
            .next()
            .expect("one entry")
            .clone();

        let args = channel_launch_args(&["--verbose".to_string()], &server_name_for(LIN, ROOM));
        assert_eq!(
            args,
            vec![
                "--verbose".to_string(),
                "--dangerously-load-development-channels".to_string(),
                format!("server:{registered}"),
            ]
        );
    }
    #[test]
    fn a_session_id_is_substituted_wherever_the_account_wrote_it() {
        let args = split_launch_options("--resume {session_id} --verbose");
        let filled = substitute_session_id(&args, "0f5a-uuid");
        assert_eq!(filled, vec!["--resume", "0f5a-uuid", "--verbose"]);
    }

    #[test]
    fn a_session_id_is_substituted_inside_one_argument_too() {
        let args = split_launch_options("--session-id={session_id}");
        let filled = substitute_session_id(&args, "0f5a-uuid");
        assert_eq!(filled, vec!["--session-id=0f5a-uuid"]);
    }

    #[test]
    fn options_with_no_placeholder_declare_no_session_id() {
        let args = split_launch_options("--dangerously-skip-permissions");
        assert!(!declares_session_id(&args));
        assert_eq!(substitute_session_id(&args, "0f5a-uuid"), args);
    }

    #[test]
    fn options_naming_the_placeholder_declare_a_session_id() {
        assert!(declares_session_id(&split_launch_options(
            "--session-id {session_id}"
        )));
    }

    /// Both directories were read off a running install (2026-08-29), and they
    /// are what fixes the rule: the drive's colon and each separator are one
    /// dash apiece, so the leading `C:` leaves two.
    #[test]
    fn the_transcript_of_a_session_sits_under_the_slug_of_its_directory() {
        let home = PathBuf::from(r"C:\Users\smile");
        assert_eq!(
            transcript_path(&home, Path::new(r"C:\Users\smile\Code"), "0f5a-uuid"),
            Some(home.join(".claude").join("projects").join("C--Users-smile-Code").join("0f5a-uuid.jsonl"))
        );
        assert_eq!(
            transcript_path(&home, Path::new(r"C:\Users\smile\Claude"), "0f5a-uuid"),
            Some(home.join(".claude").join("projects").join("C--Users-smile-Claude").join("0f5a-uuid.jsonl"))
        );
    }

    /// The fold is not of separators. A dot, a space and a character with no
    /// ASCII spelling all go the same way, which is why the rule is written as
    /// what survives rather than as what is replaced. The last of them takes
    /// two dashes and not one: it is one character here and two units in the
    /// language the rule is written in.
    #[test]
    fn every_character_that_is_not_a_letter_or_a_digit_folds_into_a_dash() {
        let home = PathBuf::from(r"C:\Users\smile");
        assert_eq!(
            transcript_path(&home, Path::new(r"C:\Users\smile\my code.v2\部屋\😀"), "id"),
            Some(
                home.join(".claude")
                    .join("projects")
                    .join("C--Users-smile-my-code-v2------")
                    .join("id.jsonl")
            )
        );
    }

    /// A path the CLI would shorten names no file this app can predict, and the
    /// answer says so rather than pointing at one that is not there.
    #[test]
    fn a_directory_too_long_to_spell_out_names_no_transcript() {
        let long = format!("C:\\{}", "d".repeat(SLUG_LIMIT));
        assert_eq!(
            transcript_path(Path::new("C:\\home"), Path::new(&long), "id"),
            None
        );
        let edge = format!("C:\\{}", "d".repeat(SLUG_LIMIT - 3));
        assert!(transcript_path(Path::new("C:\\home"), Path::new(&edge), "id").is_some());
    }

    /// The layout above is one CLI's, and the way in is that CLI (#156). The
    /// two answers are the same answer, which is what keeps the kind from
    /// having a layout of its own to drift from.
    #[test]
    fn the_transcript_a_kind_names_is_the_layout_of_the_cli_it_names() {
        let home = PathBuf::from(r"C:\Users\smile");
        let cwd = Path::new(r"C:\Users\smile\Code");
        assert_eq!(
            Cli::ClaudeCode.transcript_path(&home, cwd, "0f5a-uuid"),
            transcript_path(&home, cwd, "0f5a-uuid")
        );
    }

    /// The one inference, and what it reads (#156, 決定4). An account saved
    /// before the kinds were split says nothing about which CLI it launches
    /// except by the command it launches.
    #[test]
    fn a_launch_command_names_the_cli_it_is() {
        assert_eq!(Cli::of_command("claude"), Some(Cli::ClaudeCode));
        assert_eq!(Cli::of_command("claude.exe"), Some(Cli::ClaudeCode));
        assert_eq!(Cli::of_command("Claude.CMD"), Some(Cli::ClaudeCode));
        assert_eq!(
            Cli::of_command(r"C:\Users\smile\AppData\claude.exe"),
            Some(Cli::ClaudeCode)
        );
        assert_eq!(Cli::of_command("/usr/local/bin/claude"), Some(Cli::ClaudeCode));
        assert_eq!(Cli::of_command("  claude  "), Some(Cli::ClaudeCode));
        // Not the word wherever it appears: a different program is a different
        // program, and the kind it gets is the one that assumes nothing.
        assert_eq!(Cli::of_command("claude-wrapper"), None);
        assert_eq!(Cli::of_command("codex"), None);
        assert_eq!(Cli::of_command(""), None);
    }

    /// The failure this split is for: the id reached no CLI because nobody had
    /// typed the flag. The kind types it now (#156).
    #[test]
    fn the_claude_code_kind_hands_over_the_session_id_itself() {
        let base = split_launch_options("--dangerously-skip-permissions");
        let line = session_id_launch_args(&base, Some(Cli::ClaudeCode));
        assert_eq!(
            line,
            vec!["--dangerously-skip-permissions", "--session-id", "{session_id}"]
        );
        // And it is the placeholder that lands there, so the one substitution
        // over the composed line fills it.
        assert!(declares_session_id(&line));
        assert_eq!(
            substitute_session_id(&line, "0f5a-uuid").last().map(String::as_str),
            Some("0f5a-uuid")
        );
    }

    /// 決定5: the kind that names no CLI keeps the line it had before the split
    /// — the room's own halves, and nothing this app knows about a CLI.
    #[test]
    fn a_kind_naming_no_cli_is_handed_nothing_of_the_apps() {
        let base = split_launch_options("--dangerously-skip-permissions");
        assert_eq!(session_id_launch_args(&base, None), base);

        let limited = limited_hook_url(1234, ROOM, LIN);
        let status = status_line_command(
            &PathBuf::from("C:/pullcept/sidecar/src/status.mjs"),
            &status_hook_url(1234, ROOM, LIN),
        )
        .expect("safe path");
        let line = launch_args(
            &base,
            None,
            &own(),
            None,
            &[],
            Some(&limited),
            Some(&status),
        );
        let at = line.iter().position(|arg| arg == SETTINGS_FLAG).expect("settings");
        let settled: Value = serde_json::from_str(&line[at + 1]).expect("valid JSON");
        // The room's own approval still rides: that is what every session in
        // the room needs, whatever CLI it is (#143).
        assert_eq!(settled["enabledMcpjsonServers"], json!([own()]));
        assert!(settled.get("hooks").is_none(), "{settled}");
        assert!(settled.get("statusLine").is_none(), "{settled}");
        // And neither is it told what the room's tool is called: the flag that
        // would carry it is Claude Code's, and an unknown flag ends a launch
        // (#147).
        assert!(!line.iter().any(|arg| arg == APPEND_SYSTEM_PROMPT_FLAG), "{line:?}");
    }

    /// Either spelling of the flag is somewhere to put an id, so neither is
    /// given a second one. Both are what the migration takes back out.
    #[test]
    fn a_line_that_already_names_the_flag_is_not_given_a_second_one() {
        for written in ["--session-id {session_id}", "--session-id=abc", "--session-id abc"] {
            let base = split_launch_options(written);
            assert_eq!(
                session_id_launch_args(&base, Some(Cli::ClaudeCode)),
                base,
                "{written}"
            );
        }
    }

    /// The field is the person's again, so what the conventions now carry comes
    /// back out of it (#156, 決定3).
    #[test]
    fn the_conventions_take_their_own_argument_back_out_of_the_options() {
        let written = split_launch_options(
            "--dangerously-skip-permissions --session-id {session_id} --verbose",
        );
        assert_eq!(
            Cli::ClaudeCode.without_session_id_args(&written),
            vec!["--dangerously-skip-permissions", "--verbose"]
        );
        let joined = split_launch_options("--session-id={session_id} --verbose");
        assert_eq!(
            Cli::ClaudeCode.without_session_id_args(&joined),
            vec!["--verbose"]
        );
        // Nothing of the person's own goes with it.
        let theirs = split_launch_options("--verbose");
        assert_eq!(Cli::ClaudeCode.without_session_id_args(&theirs), theirs);
    }

    /// What an account saved before the split is read as, across the four
    /// states it can be in (#156, 決定4 / 決定6).
    #[test]
    fn an_account_with_a_way_back_of_its_own_keeps_it_and_the_generic_kind() {
        let theirs = "claude --resume {session_id} --dangerously-skip-permissions";
        assert_eq!(migrated_cli("claude", Some(theirs)), None);
        // The line the kind itself would use says nothing the kind does not,
        // so it is not a line of their own.
        assert_eq!(
            migrated_cli("claude", Cli::ClaudeCode.resume_command()),
            Some(Cli::ClaudeCode)
        );
        assert_eq!(migrated_cli("claude", None), Some(Cli::ClaudeCode));
        // Cleared on screen arrives as an empty string, and that is the same
        // state as never having written one.
        assert_eq!(migrated_cli("claude", Some("  ")), Some(Cli::ClaudeCode));
        // A command this app knows nothing about, whatever it carries.
        assert_eq!(migrated_cli("codex", None), None);
        assert_eq!(migrated_cli("codex", Some(theirs)), None);
    }

    /// What the app calls its kinds, as the test's stand-in for the enum the
    /// app hands in. The migration is told these rather than spelling them.
    fn kind_of_cli(cli: Option<Cli>) -> Value {
        match cli {
            Some(Cli::ClaudeCode) => json!("claude_code"),
            None => json!("cli"),
        }
    }

    fn migrated(raw: &str) -> Value {
        let mut root: Value = serde_json::from_str(raw).expect("valid JSON");
        migrate_account_kinds(&mut root, "ai", kind_of_cli);
        root
    }

    /// The shape read off a running install (2026-09-17): the session id was
    /// written into the launch options by hand and the resume line said what
    /// the kind now says. Both come back out, and nothing else moves (#156).
    #[test]
    fn an_account_saved_before_the_split_moves_onto_the_kind_of_its_command() {
        let root = migrated(
            r#"{"accounts":[{
                "id":"a","name":"Claude Lay","command":"claude",
                "args":["--dangerously-skip-permissions","--session-id","{session_id}"],
                "cwd":"C:\\Users\\smile\\Claude","hue":250.0,"kind":"ai",
                "character":"character_Lay","resume_command":"claude --resume {session_id}"
            }]}"#,
        );
        let account = &root["accounts"][0];
        assert_eq!(account["kind"], json!("claude_code"));
        assert_eq!(account["resume_command"], Value::Null);
        assert_eq!(account["args"], json!(["--dangerously-skip-permissions"]));
        // Everything that was not about the split is where it was.
        assert_eq!(account["character"], json!("character_Lay"));
        assert_eq!(account["hue"], json!(250.0));
        assert_eq!(account["cwd"], json!(r"C:\Users\smile\Claude"));
    }

    /// 決定6, from the other side: a line of the person's own is a thing to
    /// keep, and the kind that keeps it is the one whose form shows it. Nothing
    /// of that account is rewritten.
    #[test]
    fn an_account_with_its_own_way_back_keeps_every_field_it_arrived_with() {
        let root = migrated(
            r#"{"accounts":[{
                "id":"b","command":"claude","args":["--session-id","{session_id}"],
                "kind":"ai","resume_command":"claude --resume {session_id} --verbose"
            }]}"#,
        );
        let account = &root["accounts"][0];
        assert_eq!(account["kind"], json!("cli"));
        assert_eq!(
            account["resume_command"],
            json!("claude --resume {session_id} --verbose")
        );
        assert_eq!(account["args"], json!(["--session-id", "{session_id}"]));
    }

    /// A kind that was declared is not a kind to decide, and the one value the
    /// split replaced is (#156, 決定4). An account saved before the field
    /// existed says nothing, which is the same question as the legacy value.
    #[test]
    fn only_the_accounts_that_declared_no_kind_of_their_own_are_moved() {
        let root = migrated(
            r#"{"accounts":[
                {"id":"person","command":"claude","args":[],"kind":"user","resume_command":null},
                {"id":"declared","command":"claude","args":[],"kind":"cli","resume_command":"mine"},
                {"id":"absent","command":"claude","args":["--session-id={session_id}"]},
                {"id":"unknown","command":"codex","args":["--session-id","{session_id}"],"kind":"ai"}
            ]}"#,
        );
        let accounts = root["accounts"].as_array().expect("accounts");
        // A person stays a person, and their fields are not touched either.
        assert_eq!(accounts[0]["kind"], json!("user"));
        // A declared kind stands, resume line and all.
        assert_eq!(accounts[1]["kind"], json!("cli"));
        assert_eq!(accounts[1]["resume_command"], json!("mine"));
        // No kind at all is the same question the legacy value asks.
        assert_eq!(accounts[2]["kind"], json!("claude_code"));
        assert_eq!(accounts[2]["args"], json!([]));
        // A command this app knows nothing about keeps its own line, the id it
        // passes by hand included — nothing here knows where else it would go.
        assert_eq!(accounts[3]["kind"], json!("cli"));
        assert_eq!(accounts[3]["args"], json!(["--session-id", "{session_id}"]));
    }

    /// The list under the name it had while an account was a launch recipe. A
    /// config that old would otherwise arrive with no kind on any account.
    #[test]
    fn the_account_list_is_found_under_the_name_it_was_saved_with() {
        let root = migrated(r#"{"tabs":[{"id":"a","command":"claude","args":[]}]}"#);
        assert_eq!(root["tabs"][0]["kind"], json!("claude_code"));
    }

    /// A file the typed parse is going to refuse is left for it to refuse.
    /// Rewriting the list would drop the element that says why.
    #[test]
    fn an_argument_that_is_not_a_string_is_left_where_it_is() {
        let root = migrated(r#"{"accounts":[{"id":"a","command":"claude","args":["--session-id",7]}]}"#);
        assert_eq!(root["accounts"][0]["kind"], json!("claude_code"));
        assert_eq!(root["accounts"][0]["args"], json!(["--session-id", 7]));
    }

    /// The way back is the kind's, and it names where the id goes — the same
    /// placeholder every other line of this app is filled through (#156, 決定6).
    #[test]
    fn a_seat_is_told_the_full_name_of_the_tool_the_room_is_spoken_to_through() {
        // The whole point is the name: a session woken only by a room post
        // calls nothing, so the tool list and the sidecar's `instructions`
        // never arrive, and it answers into its terminal (#147). A name it can
        // call before either has arrived is what this puts on the line.
        let room = server_name_for(LIN, ROOM);
        let told = Cli::ClaudeCode.room_system_prompt(&room).expect("a safe text");
        assert!(told.contains(&format!("mcp__{room}__say_to_room")), "{told}");
        // The room's tool, not the channel that delivered the post: a room
        // reached some other way does not make the text wrong (#147, 決定4).
        assert!(!told.contains("channel"), "{told}");
        // And the fact the terminal is not a way of being heard, which is the
        // half that says why the tool has to be called at all.
        assert!(told.contains("Terminal output does not reach the room"), "{told}");
    }

    #[test]
    fn what_a_seat_is_told_carries_nothing_a_shell_on_the_way_acts_on() {
        // The text set, both directions. Same characters as the word set, plus
        // the space: this is a whole argument rather than a word inside the
        // `--settings` JSON, and one holding a space is what the Windows
        // quoting wraps (`line_safe_text`).
        assert!(line_safe_text("You are a participant in a Pullcept room."));
        for hazard in [
            "a & b",
            "a | b",
            "a ^ b",
            "a < b",
            "a > b",
            "(a) b",
            // A quote would invert the parity of the `cmd.exe` scan, which is
            // the break #152 shipped.
            "say \"hello\"",
            "100% done",
            "部屋へ発言する",
            "",
        ] {
            assert!(!line_safe_text(hazard), "{hazard}");
        }
        // And on the thing that actually reaches `cmd.exe`.
        let told = Cli::ClaudeCode
            .room_system_prompt(&server_name_for(LIN, ROOM))
            .expect("a safe text");
        for ch in ['&', '|', '<', '>', '^', '(', ')', '"', '%'] {
            assert!(!told.contains(ch), "{ch:?} must not reach the launch line: {told}");
        }
    }

    #[test]
    fn a_line_that_already_appends_a_system_prompt_is_not_given_a_second_one() {
        // Which of two copies of one flag a CLI reads is not something this app
        // has established — the ground the two-`--settings` refusal stands on.
        let room = server_name_for(LIN, ROOM);
        let base = split_launch_options("--append-system-prompt \"be brief\"");
        assert_eq!(
            system_prompt_launch_args(&base, Some(Cli::ClaudeCode), &room),
            base
        );
        // The file form for a harder reason: the CLI refuses a launch carrying
        // both at once, so adding to it would cost the launch rather than a
        // text nobody reads.
        let base = split_launch_options("--append-system-prompt-file=C:/p/prompt.txt");
        assert_eq!(
            system_prompt_launch_args(&base, Some(Cli::ClaudeCode), &room),
            base
        );
    }

    #[test]
    fn the_resume_line_of_a_kind_names_where_the_id_goes() {
        let line = Cli::ClaudeCode.resume_command().expect("Claude Code resumes");
        assert!(line.contains(SESSION_ID_PLACEHOLDER), "{line}");
        let mut parts = split_launch_options(line);
        assert_eq!(parts.remove(0), "claude");
        assert!(declares_session_id(&parts), "{parts:?}");
    }
}
