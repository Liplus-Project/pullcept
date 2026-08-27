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
//! This crate holds no tauri: it writes into the user's own project directory,
//! which is the part of Pullcept that most needs test coverage, and a test
//! binary linking the tauri tree does not load on the GNU target.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Prefix of the name the sidecar is registered under in `.mcp.json`.
///
/// The full name is per account (`server_name_for`), not one fixed key. Two
/// sessions pointed at the same working directory write into the same file, and
/// a single key means the second launch overwrites the first one's name, hue
/// and room address — the identity the first session was launched with is gone
/// while that session is still running (#40).
pub const SERVER_PREFIX: &str = "pullcept-room";

/// The `.mcp.json` key, and the `server:<name>` tag, for one account.
///
/// A function of the account id alone. The id is what an account is; the name
/// is an attribute of it, and a key derived from the name moved every time the
/// name was edited — the registration a running session was launched against
/// would be orphaned under the old key while the CLI holding that session still
/// names the old tag on its command line (#53). Deriving from the id makes a
/// rename cost nothing, which is what makes the name editable at all.
///
/// Per account rather than per launch: relaunching one account reuses its
/// entry, rather than growing the user's file by one key per launch.
///
/// The slug half is legibility and the hash half is what makes the key total,
/// as it was under the name. An id is opaque, so the slug reads less well than
/// a name did; which account an entry belongs to is read from
/// `PULLCEPT_AGENT_NAME` in its own env instead. Legibility loses to identity
/// here — a key that reads oddly costs one lookup, and a key that moves is a
/// registration nobody can find.
pub fn server_name_for(account_id: &str) -> String {
    let slug = slugify(account_id);
    let hash = fnv1a(account_id);
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
    /// Display name this session speaks under. Written into the env for the
    /// sidecar to declare in `hello`, and it is what says whose entry this is
    /// when the file is read by eye. Not the key: see `server_name_for`.
    pub agent_name: &'a str,
    /// Hue this session declared, in oklch degrees, or `None` when it declared
    /// none. Absent rather than a default: the room derives a hue from the name
    /// for an undeclared participant, and a value written here would be a
    /// declaration the person never made.
    pub agent_hue: Option<f64>,
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
    args.iter()
        .any(|arg| arg.split('=').next().unwrap_or(arg) == SETTINGS_FLAG)
}

/// What an account writes where the id of a CLI session goes.
///
/// The one thing this app knows about resuming a session is that the id is
/// decided here rather than read back out of the CLI's output. Which flag
/// carries it is the CLI's business, and the CLI is per-account
/// (`Account::command`) — so the app substitutes into a line the person wrote
/// instead of holding a flag of its own. `claude` spells the two halves
/// `--session-id <uuid>` and `--resume <uuid>`; another CLI spells them
/// otherwise, or not at all, and an account that writes the placeholder nowhere
/// simply has no session id (#115, decision 4B).
pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";

/// Whether these arguments have somewhere to put a session id.
///
/// What decides whether one is minted at all. Minting unconditionally would
/// hand out an id no launch passes on, and the topic would then record a
/// session that never existed under that name.
pub fn declares_session_id(args: &[String]) -> bool {
    args.iter().any(|arg| arg.contains(SESSION_ID_PLACEHOLDER))
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
/// given its own: the character it speaks as, and the room registrations it
/// must not start.
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
/// A launch with no character and nothing to stop is left untouched and takes
/// the directory's own default. That is now the condition — nothing to stop —
/// rather than "declares no character": a second registration in the directory
/// puts `--settings` on the line whether a character was declared or not.
pub fn settings_launch_args(
    base: &[String],
    character: Option<&str>,
    disabled: &[String],
) -> Vec<String> {
    let mut args = base.to_vec();
    let character = declared_character(character);
    if character.is_none() && disabled.is_empty() {
        return args;
    }
    let mut settings = Map::new();
    if let Some(name) = character {
        settings.insert("outputStyle".into(), json!(name));
    }
    if !disabled.is_empty() {
        settings.insert("disabledMcpjsonServers".into(), json!(disabled));
    }
    args.push(SETTINGS_FLAG.to_string());
    args.push(Value::Object(settings).to_string());
    args
}

/// The whole line one launch runs: the account's options, the room's channel
/// entry, and the settings this session declares about itself.
///
/// One function rather than two calls at each site, because the line shown on
/// screen and the line spawned have to be the same line. They are produced in
/// different places — a preview command and the launch — and every step either
/// one composes for itself is a step the other can be missing.
pub fn launch_args(
    base: &[String],
    server_name: &str,
    character: Option<&str>,
    disabled: &[String],
) -> Vec<String> {
    settings_launch_args(&channel_launch_args(base, server_name), character, disabled)
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
    let server_name = server_name_for(room.account_id);

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
    env.insert("PULLCEPT_ROOM_ID".into(), json!("pullcept"));
    // Only when declared. An undeclared participant is a participant the room
    // derives a hue for, which is not the same state as one who chose that hue.
    if let Some(hue) = room.agent_hue {
        env.insert("PULLCEPT_AGENT_HUE".into(), json!(format!("{hue:.1}")));
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

    fn registration<'a>(entry: &'a Path, runner: &'a Path) -> RoomRegistration<'a> {
        RoomRegistration {
            room_url: "ws://127.0.0.1:1234",
            token: "tok",
            account_id: LIN,
            agent_name: "Lin",
            agent_hue: None,
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
        let server = &json["mcpServers"][server_name_for(LIN)];
        assert_eq!(server["env"]["PULLCEPT_ROOM_URL"], "ws://127.0.0.1:1234");
        assert_eq!(server["env"]["PULLCEPT_ROOM_TOKEN"], "tok");
        assert_eq!(server["env"]["PULLCEPT_AGENT_NAME"], "Lin");
        // The id the sidecar declares in `hello`, so the room can carry it on
        // the seat and the screen can match a participant to one of its
        // accounts without matching on a name (#59). The id, not the key
        // derived from it: the key is a `.mcp.json` concern, and the room has
        // no way back from it to the account.
        assert_eq!(server["env"]["PULLCEPT_ACCOUNT_ID"], LIN);
        assert_eq!(server["env"]["PULLCEPT_ROOM_ID"], "pullcept");
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
        assert!(json["mcpServers"][server_name_for(LIN)].is_object());
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
            agent_name: "Lin",
            agent_hue: Some(145.0),
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &second).expect("second");

        let json = read(&path);
        let server = &json["mcpServers"][server_name_for(LIN)];
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
            agent_name: "リン",
            agent_hue: None,
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &renamed).expect("as リン");

        let json = read(&path);
        let servers = json["mcpServers"].as_object().expect("servers");
        assert_eq!(servers.len(), 1, "a rename must not open a second entry");
        assert_eq!(
            json["mcpServers"][server_name_for(LIN)]["env"]["PULLCEPT_AGENT_NAME"],
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
            agent_name: "Lay",
            agent_hue: Some(25.0),
            sidecar_entry: &entry,
            sidecar_runner: &runner,
        };
        let path = register_sidecar(scratch.path(), &lay).expect("Lay");

        let json = read(&path);
        assert_eq!(
            json["mcpServers"][server_name_for(LIN)]["env"]["PULLCEPT_AGENT_NAME"],
            "Lin",
            "the first session's identity must survive the second launch"
        );
        assert_eq!(
            json["mcpServers"][server_name_for(LAY)]["env"]["PULLCEPT_AGENT_NAME"],
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
        assert!(servers.contains_key(&server_name_for(LIN)));
    }

    #[test]
    fn a_server_name_is_a_function_of_the_account_id() {
        assert_eq!(server_name_for(LIN), server_name_for(LIN));
        assert_ne!(server_name_for(LIN), server_name_for(LAY));
        assert!(server_name_for(LIN).starts_with(SERVER_PREFIX));
        // Total over whatever an id turns out to be, as it was over a name: an
        // input that slugs to nothing still gets a key of its own, and two that
        // slug alike still get two. The uniqueness lives in the hash, and the
        // ids the app mints do not lean on the slug for it.
        assert!(server_name_for("マスター").starts_with("pullcept-room-"));
        assert_ne!(server_name_for("マスター"), server_name_for("ますたー"));
        assert_ne!(server_name_for("Lin"), server_name_for("lin!"));
        // Nothing outside the set a console and a JSON key both leave alone.
        assert!(server_name_for("Lin さん / 2")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
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

        let room = server_name_for(LIN);
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
        let room = server_name_for(LIN);
        let base = vec![CHANNEL_FLAG.to_string(), format!("server:{room}")];
        assert_eq!(channel_launch_args(&base, &room), base);
    }

    #[test]
    fn a_declared_character_rides_in_settings_json() {
        let args = settings_launch_args(&["--verbose".to_string()], Some("character_Lay"), &[]);
        assert_eq!(
            args,
            vec![
                "--verbose".to_string(),
                SETTINGS_FLAG.to_string(),
                r#"{"outputStyle":"character_Lay"}"#.to_string(),
            ]
        );
        // The value has to parse as JSON on the other side: it is handed to the
        // CLI inline rather than written to a file anyone could look at.
        let settled: Value = serde_json::from_str(&args[2]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!("character_Lay"));
    }

    #[test]
    fn a_character_with_a_quote_in_it_stays_one_json_string() {
        // Built rather than formatted, so a name that would otherwise close the
        // string early cannot make the value stop being JSON.
        let args = settings_launch_args(&[], Some(r#"quote"style"#), &[]);
        let settled: Value = serde_json::from_str(&args[1]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!(r#"quote"style"#));
    }

    #[test]
    fn nothing_to_declare_means_the_line_is_left_alone() {
        let base = vec!["--verbose".to_string()];
        // Absent and blank are one state: a cleared field must not launch
        // `{"outputStyle":""}`, which names no style at all.
        assert_eq!(settings_launch_args(&base, None, &[]), base);
        assert_eq!(settings_launch_args(&base, Some(""), &[]), base);
        assert_eq!(settings_launch_args(&base, Some("   "), &[]), base);
        assert_eq!(declared_character(Some("  x  ")), Some("x"));
    }

    #[test]
    fn a_sibling_registration_is_named_as_one_this_launch_does_not_start() {
        // The launch's own entry is not on the list: a session that disabled
        // its own room server would be a session with no room (#103).
        let lay = server_name_for(LAY);
        let args = settings_launch_args(&[], Some("character_Lin"), std::slice::from_ref(&lay));
        let settled: Value = serde_json::from_str(&args[1]).expect("valid JSON");
        assert_eq!(settled["outputStyle"], json!("character_Lin"));
        assert_eq!(settled["disabledMcpjsonServers"], json!([lay]));
    }

    #[test]
    fn a_sibling_registration_puts_settings_on_a_line_with_no_character() {
        // The pass-through condition moved: it is "nothing to stop", not
        // "declares no character". An account with no character in a shared
        // directory still has to stop the other account's sidecar (#103).
        let lay = server_name_for(LAY);
        let args =
            settings_launch_args(&["--verbose".to_string()], None, std::slice::from_ref(&lay));
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
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN))
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
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN))
                .expect("read"),
            others
        );
    }

    #[test]
    fn a_directory_with_no_registrations_names_nothing() {
        let scratch = Scratch::new();
        assert!(
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN))
                .expect("read")
                .is_empty(),
            "no file is not a failure: it is a directory nothing has been registered in yet"
        );

        let entry = PathBuf::from(ENTRY);
        let runner = PathBuf::from(RUNNER);
        register_sidecar(scratch.path(), &registration(&entry, &runner)).expect("register");
        assert!(
            other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN))
                .expect("read")
                .is_empty(),
            "the only entry is this account's own, and disabling it would leave it roomless"
        );
    }

    #[test]
    fn a_config_that_is_not_json_is_refused_before_anything_is_written() {
        let scratch = Scratch::new();
        std::fs::write(scratch.path().join(".mcp.json"), "{ not json").expect("seed");
        let err = other_room_servers(scratch.path(), "ws://127.0.0.1:1234", &server_name_for(LIN))
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
        let room = server_name_for(LIN);
        let line = launch_args(
            &["--verbose".to_string()],
            &room,
            Some("character_Lin"),
            &[],
        );
        assert_eq!(
            line,
            vec![
                "--verbose".to_string(),
                CHANNEL_FLAG.to_string(),
                format!("server:{room}"),
                SETTINGS_FLAG.to_string(),
                r#"{"outputStyle":"character_Lin"}"#.to_string(),
            ]
        );
        assert_eq!(
            launch_args(&["--verbose".to_string()], &room, None, &[]),
            channel_launch_args(&["--verbose".to_string()], &room)
        );
    }

    #[test]
    fn one_line_starts_this_account_and_stops_the_others() {
        // The room entry names this session's own server and the settings name
        // the ones it must leave alone: the same file, read twice, must not
        // disagree about which entry is whose (#103).
        let room = server_name_for(LIN);
        let lay = server_name_for(LAY);
        let line = launch_args(
            &[],
            &room,
            Some("character_Lin"),
            std::slice::from_ref(&lay),
        );
        assert_eq!(line[0], CHANNEL_FLAG);
        assert_eq!(line[1], format!("server:{room}"));
        assert_eq!(line[2], SETTINGS_FLAG);
        let settled: Value = serde_json::from_str(&line[3]).expect("valid JSON");
        assert_eq!(settled["disabledMcpjsonServers"], json!([lay]));
        assert_ne!(
            settled["disabledMcpjsonServers"][0],
            json!(room),
            "the server the channel flag just named must not be disabled"
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

        let args = channel_launch_args(&["--verbose".to_string()], &server_name_for(LIN));
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

}
