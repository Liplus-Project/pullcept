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
    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        // Truncating a file that failed to parse would destroy whatever it held.
        serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} exists but is not valid JSON ({e}). Fix or move it before starting a session.",
                path.display()
            )
        })?
    } else {
        Value::Object(Map::new())
    };

    if !root.is_object() {
        return Err(format!("{} is not a JSON object.", path.display()));
    }
    let obj = root.as_object_mut().expect("checked above");
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        return Err(format!("{} has a non-object mcpServers.", path.display()));
    }

    let servers = servers.as_object_mut().expect("checked above");

    // Entries this app wrote in an earlier run can never connect: the room
    // binds a fresh port every run, so their address is dead. Left in place
    // they would have every CLI started in this directory spawn a sidecar that
    // retries nothing forever, and the file would grow by one key per account
    // ever launched here. Entries carrying the current address are live
    // siblings — the other sessions of this run — and stay. Entries with no
    // `PULLCEPT_ROOM_URL` at all are not ours to judge, whatever they are named.
    servers.retain(|name, entry| {
        if !name.starts_with(SERVER_PREFIX) {
            return true;
        }
        match entry.get("env").and_then(|env| env.get("PULLCEPT_ROOM_URL")) {
            Some(Value::String(url)) => url == room.room_url,
            _ => true,
        }
    });

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
}
