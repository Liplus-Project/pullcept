//! A Codex seat launched through its own app-server (#299). See docs/3-accounts.md,
//! "Codex の席を app-server 経由で起動する".
//!
//! Pure parts only: which of the person's launch options go to the server, to
//! the terminal and to `thread/start`; the two lines; the requests; and the
//! checks read off the server's answers. The process, the socket and the seat
//! are `src-tauri/src/codex_app_server.rs`.
use serde_json::{json, Map, Value};
use toml_edit::Value as TomlValue;

/// The variable the terminal reads the server's token from
/// (`--remote-auth-token-env`). Set by the app on the terminal's environment
/// only; the server is handed the token's SHA-256, never the token.
pub const REMOTE_TOKEN_ENV: &str = "PULLCEPT_CODEX_REMOTE_TOKEN";

/// Loopback, port chosen by the OS. The server prints the port it bound.
pub const LISTEN: &str = "ws://127.0.0.1:0";

/// Named in the account's setting and in the preview.
pub const STYLE_HELPER: &str = "codex-output-style.py";

/// What the thread is started (and resumed) with, read off the person's
/// launch options. `None` leaves the server's own configuration in place.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ThreadSettings {
    pub model: Option<String>,
    pub approval_policy: Option<String>,
    pub sandbox: Option<String>,
    pub effort: Option<String>,
}

/// The person's launch options, split by where each one acts.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Configuration options for the server's line (`-c`, `--enable`, `--disable`).
    pub server: Vec<String>,
    /// Options only the terminal reads (`-c tui.*`, `--no-alt-screen`).
    pub tui: Vec<String>,
    pub thread: ThreadSettings,
}

const APPROVALS: &[&str] = &["untrusted", "on-request", "never"];
const SANDBOXES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];

fn refused(arg: &str) -> String {
    format!("app-server 方式の Codex の席では起動オプション {arg} を引き継げません。起動を止めました。オプションを外すか、このアカウントの「app-server 方式」を切ってください。")
}

fn string_value(raw: &str) -> Option<String> {
    raw.parse::<TomlValue>()
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| Some(raw.to_string()))
}

/// Split options already through `transport_options` (every `-c` is `-c`,
/// `key=value`, its value TOML-encoded).
///
/// Flags win over `-c` for the same setting, as they do in the CLI. Options
/// that have no place on either line are refused, never dropped: a seat that
/// quietly ran without them would look like it had them.
pub fn plan(options: &[String]) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let mut flags = ThreadSettings::default();
    let mut i = 0;
    let value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        options
            .get(*i)
            .cloned()
            .ok_or_else(|| format!("Codex のオプション {flag} には値が必要です。"))
    };
    while i < options.len() {
        let arg = options[i].as_str();
        match arg {
            "-c" | "--config" => {
                let pair = value(&mut i, arg)?;
                let (key, raw) = pair
                    .split_once('=')
                    .ok_or("Codex -c には key=value が必要です。")?;
                let key = key.trim();
                if key.starts_with("tui.") {
                    plan.tui.extend(["-c".into(), pair.clone()]);
                } else {
                    match key {
                        "model" => plan.thread.model = string_value(raw),
                        "approval_policy" => plan.thread.approval_policy = string_value(raw),
                        "sandbox_mode" => plan.thread.sandbox = string_value(raw),
                        "model_reasoning_effort" => plan.thread.effort = string_value(raw),
                        _ => {}
                    }
                    plan.server.extend(["-c".into(), pair.clone()]);
                }
            }
            "--enable" | "--disable" => {
                let feature = value(&mut i, arg)?;
                plan.server.extend([arg.to_string(), feature]);
            }
            "-m" | "--model" => flags.model = Some(value(&mut i, arg)?),
            "-a" | "--ask-for-approval" => flags.approval_policy = Some(value(&mut i, arg)?),
            "-s" | "--sandbox" => flags.sandbox = Some(value(&mut i, arg)?),
            "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => {
                flags.approval_policy = Some("never".into());
                flags.sandbox = Some("danger-full-access".into());
            }
            // Read by `effective_cwd`: the thread's cwd.
            "-C" | "--cd" => {
                value(&mut i, arg)?;
            }
            super::NO_ALT_SCREEN => plan.tui.push(arg.to_string()),
            _ => return Err(refused(arg)),
        }
        i += 1;
    }
    let thread = &mut plan.thread;
    thread.model = flags.model.or(thread.model.take());
    thread.approval_policy = flags.approval_policy.or(thread.approval_policy.take());
    thread.sandbox = flags.sandbox.or(thread.sandbox.take());
    if let Some(policy) = &thread.approval_policy {
        if !APPROVALS.contains(&policy.as_str()) {
            return Err(format!("app-server 方式では承認の指定 {policy} を thread/start に渡せません（{} のどれか）。", APPROVALS.join(" / ")));
        }
    }
    if let Some(sandbox) = &thread.sandbox {
        if !SANDBOXES.contains(&sandbox.as_str()) {
            return Err(format!("app-server 方式では sandbox の指定 {sandbox} を thread/start に渡せません（{} のどれか）。", SANDBOXES.join(" / ")));
        }
    }
    Ok(plan)
}

/// `-c hooks.state={…}` turning off exactly the named hooks for this server.
///
/// The value form, not a dotted key: Codex splits a `-c` key on every `.`
/// (`config/src/overrides.rs`), and a hook key holds the hooks file's path.
/// Merged over the user's own `hooks.state`, so the hook keeps its trust.
pub fn hook_override(keys: &[String]) -> Result<Option<String>, String> {
    if keys.is_empty() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for key in keys {
        if key.is_empty()
            || key.contains(['\'', '%'])
            || !crate::console_safe(key)
        {
            return Err("Codex の output style hook の名前を app-server の起動行で指定できません。起動を止めました。".into());
        }
        entries.push(format!("'{key}'={{enabled=false}}"));
    }
    Ok(Some(format!("hooks.state={{{}}}", entries.join(","))))
}

/// The server's line: the person's configuration, the room server, hooks on,
/// the one hook off, and the authenticated loopback listener.
pub fn server_args(
    plan: &Plan,
    own: &str,
    runner: &std::path::Path,
    entry: &std::path::Path,
    hook_keys: &[String],
    token_sha256: &str,
) -> Result<Vec<String>, String> {
    let paths = super::sidecar_paths(runner, entry)?;
    let mut args = plan.server.clone();
    args.extend(["-c".into(), super::room_server_definition(own, &paths[0], &paths[1])]);
    args.extend(["-c".into(), "features.codex_hooks=true".into()]);
    if let Some(off) = hook_override(hook_keys)? {
        args.extend(["-c".into(), off]);
    }
    args.extend(
        [
            "app-server",
            "--listen",
            LISTEN,
            "--ws-auth",
            "capability-token",
            "--ws-token-sha256",
            token_sha256,
        ]
        .map(String::from),
    );
    Ok(args)
}

/// The terminal's line: the real TUI, attached to the seat's server.
pub fn tui_args(plan: &Plan, thread_id: &str, url: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "resume",
        thread_id,
        "--remote",
        url,
        "--remote-auth-token-env",
        REMOTE_TOKEN_ENV,
    ]
    .map(String::from)
    .to_vec();
    args.extend(plan.tui.iter().cloned());
    if !super::sets_alternate_screen(&plan.tui) {
        args.push(super::NO_ALT_SCREEN.into());
    }
    args
}

/// The port out of the server's `listening on: ws://127.0.0.1:<port>` line.
pub fn listening_port(line: &str) -> Option<u16> {
    let rest = line.split("listening on: ws://127.0.0.1:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|port| *port != 0)
}

pub fn initialize_params(version: &str) -> Value {
    json!({
        "clientInfo": {"name": "pullcept", "title": "Pullcept", "version": version},
        "capabilities": {"experimentalApi": true},
    })
}

fn overrides(settings: &ThreadSettings, params: &mut Map<String, Value>) {
    if let Some(model) = &settings.model {
        params.insert("model".into(), json!(model));
    }
    if let Some(policy) = &settings.approval_policy {
        params.insert("approvalPolicy".into(), json!(policy));
    }
    if let Some(sandbox) = &settings.sandbox {
        params.insert("sandbox".into(), json!(sandbox));
    }
    if let Some(effort) = &settings.effort {
        params.insert("config".into(), json!({"model_reasoning_effort": effort}));
    }
}

/// Append one instruction part after another, preserving nonempty bytes.
/// The seat's mode decides the base via `seat_instructions` (#303).
pub fn developer_instructions(base: Option<&str>, room: Option<&str>) -> Option<String> {
    match (base.filter(|b| !b.is_empty()), room) {
        (Some(base), Some(room)) => Some(format!("{base}\n\n{room}")),
        (Some(base), None) => Some(base.to_string()),
        (None, Some(room)) => Some(room.to_string()),
        (None, None) => None,
    }
}

/// Compose the seat's developer instructions after loader validation (#303).
/// File mode preserves native common instructions before the character. Legacy
/// selection already contains them; characterless seats retain the old fallback.
pub fn seat_instructions(
    mode: &str,
    character: Option<&str>,
    room: Option<&str>,
    effective: impl FnOnce() -> Result<Option<String>, String>,
) -> Result<Option<String>, String> {
    let base = if mode == "file" {
        let common = effective()?;
        developer_instructions(common.as_deref(), character)
    } else if character.is_none() && room.is_some() {
        effective()?
    } else {
        character.map(str::to_string)
    };
    Ok(developer_instructions(base.as_deref(), room))
}

/// `thread/start`: the character as developer instructions, the person's
/// permissions, model and effort.
pub fn start_params(cwd: &str, instructions: Option<&str>, settings: &ThreadSettings) -> Value {
    let mut params = Map::new();
    params.insert("cwd".into(), json!(cwd));
    params.insert("historyMode".into(), json!("paginated"));
    if let Some(text) = instructions {
        params.insert("developerInstructions".into(), json!(text));
    }
    overrides(settings, &mut params);
    Value::Object(params)
}

/// `thread/resume`: the same character again, every time. A resumed thread
/// keeps its first instructions in its history; the ones handed here are the
/// thread's current ones, which compaction puts back.
pub fn resume_params(
    thread_id: &str,
    cwd: &str,
    instructions: Option<&str>,
    settings: &ThreadSettings,
) -> Value {
    let mut params = Map::new();
    params.insert("threadId".into(), json!(thread_id));
    params.insert("cwd".into(), json!(cwd));
    params.insert("excludeTurns".into(), json!(true));
    if let Some(text) = instructions {
        params.insert("developerInstructions".into(), json!(text));
    }
    overrides(settings, &mut params);
    Value::Object(params)
}

/// The text the new thread is given so it has a rollout before the terminal
/// attaches (an empty thread cannot be resumed). Developer, so it puts no
/// words in the character's mouth or in a room member's; model-free.
pub const SEAT_OPENED: &str = "Pullcept opened this conversation as a seat in a room. Room posts arrive as user input.";

pub fn inject_params(thread_id: &str) -> Value {
    json!({
        "threadId": thread_id,
        "items": [{"type": "message", "role": "developer", "content": [{"type": "input_text", "text": SEAT_OPENED}]}],
    })
}

/// The thread id and rollout path out of a `thread/start` or `thread/resume` result.
pub fn thread_of(result: &Value) -> Result<(String, Option<String>), String> {
    let thread = &result["thread"];
    let id = thread["id"]
        .as_str()
        .filter(|id| super::valid_id(id))
        .ok_or("Codex app-server の会話 id を読めません。")?;
    Ok((id.to_string(), thread["path"].as_str().map(str::to_string)))
}

/// What the server says the thread runs with, against what was asked for.
pub fn check_settings(result: &Value, settings: &ThreadSettings) -> Result<(), String> {
    let mismatch = |what: &str| {
        Err(format!(
            "Codex app-server の会話が起動オプションの{what}で始まりませんでした。起動を止めました。"
        ))
    };
    if let Some(model) = &settings.model {
        if result["model"].as_str() != Some(model) {
            return mismatch("モデル");
        }
    }
    if let Some(policy) = &settings.approval_policy {
        if result["approvalPolicy"].as_str() != Some(policy) {
            return mismatch("承認");
        }
    }
    if let Some(sandbox) = &settings.sandbox {
        let active = result["sandbox"]["type"].as_str().unwrap_or_default();
        let expected: &[&str] = match sandbox.as_str() {
            "read-only" => &["readOnly"],
            "workspace-write" => &["workspaceWrite"],
            _ => &["dangerFullAccess", "externalSandbox"],
        };
        if !expected.contains(&active) {
            return mismatch("sandbox");
        }
    }
    if let Some(effort) = &settings.effort {
        if result["reasoningEffort"].as_str() != Some(effort) {
            return mismatch("effort");
        }
    }
    Ok(())
}

/// The seat's `hooks/list` answer: each hook turned off is listed and off,
/// and no Li+ style handler is left on to deliver the character a second time.
pub fn check_hooks(result: &Value, disabled: &[String]) -> Result<(), String> {
    let hooks: Vec<&Value> = result["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .collect();
    for key in disabled {
        let found = hooks.iter().find(|h| h["key"].as_str() == Some(key));
        if found.is_none_or(|h| h["enabled"] != json!(false)) {
            return Err("Codex app-server で Li+ の output style hook を止められませんでした。キャラクターが二重に届くため起動を止めました。".into());
        }
    }
    let style_on = hooks.iter().any(|h| {
        h["eventName"] == json!("sessionStart")
            && h["enabled"] == json!(true)
            && h["command"].as_str().is_some_and(|c| c.contains(STYLE_HELPER))
    });
    if style_on {
        return Err("Codex app-server に止めていない Li+ の output style hook が残っています。キャラクターが二重に届くため起動を止めました。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "01a110c0-c898-75f0-be7d-ffff2e05b9f5";
    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| v.to_string()).collect()
    }
    fn planned(items: &[&str]) -> Result<Plan, String> {
        plan(&crate::codex::transport_options(&s(items)).unwrap())
    }

    #[test]
    fn yolo_effort_and_model_reach_the_thread_and_flags_beat_config() {
        let p = planned(&["--yolo", "-c", "model_reasoning_effort=high", "-c", "model=gpt-a", "-m", "gpt-b"]).unwrap();
        assert_eq!(p.thread, ThreadSettings {
            model: Some("gpt-b".into()),
            approval_policy: Some("never".into()),
            sandbox: Some("danger-full-access".into()),
            effort: Some("high".into()),
        });
        // The configuration still rides on the server's line as written.
        assert!(p.server.iter().any(|a| a == "model_reasoning_effort=\"high\""));
        let p = planned(&["--dangerously-bypass-approvals-and-sandbox"]).unwrap();
        assert_eq!(p.thread.sandbox.as_deref(), Some("danger-full-access"));
        let p = planned(&["-a", "on-request", "-s", "workspace-write", "--enable", "x", "-C", "sub"]).unwrap();
        assert_eq!(p.thread.approval_policy.as_deref(), Some("on-request"));
        assert_eq!(p.thread.sandbox.as_deref(), Some("workspace-write"));
        assert_eq!(p.server, s(&["--enable", "x"]));
        let p = planned(&["-c", "approval_policy=never", "-c", "sandbox_mode=read-only"]).unwrap();
        assert_eq!(p.thread.approval_policy.as_deref(), Some("never"));
        assert_eq!(p.thread.sandbox.as_deref(), Some("read-only"));
    }

    #[test]
    fn options_with_no_place_are_refused_not_dropped() {
        for bad in [
            vec!["--profile", "work"],
            vec!["--add-dir", "x"],
            vec!["-i", "a.png"],
            vec!["--search"],
            vec!["--oss"],
            vec!["--approve-for-me"],
            vec!["--dangerously-bypass-hook-trust"],
            vec!["first prompt"],
            vec!["-a", "on-failure"],
            vec!["-s", "everything"],
        ] {
            assert!(planned(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn terminal_options_stay_on_the_terminal_and_inline_is_kept() {
        let p = planned(&["-c", "tui.theme=dark"]).unwrap();
        assert!(p.server.is_empty());
        let tui = tui_args(&p, ID, "ws://127.0.0.1:5");
        assert_eq!(&tui[..6], &["resume", ID, "--remote", "ws://127.0.0.1:5", "--remote-auth-token-env", REMOTE_TOKEN_ENV]);
        assert_eq!(tui.last().map(String::as_str), Some(crate::codex::NO_ALT_SCREEN));
        let chosen = planned(&["-c", "tui.alternate_screen=always"]).unwrap();
        assert!(!tui_args(&chosen, ID, "ws://x").contains(&crate::codex::NO_ALT_SCREEN.to_string()));
        let own = planned(&["--no-alt-screen"]).unwrap();
        assert_eq!(tui_args(&own, ID, "ws://x").iter().filter(|a| *a == crate::codex::NO_ALT_SCREEN).count(), 1);
    }

    #[test]
    fn server_line_carries_room_hooks_and_auth_but_never_the_token() {
        let p = planned(&["-c", "model_reasoning_effort=low"]).unwrap();
        let key = r"D:\proj\.codex\hooks.json:session_start:0:1".to_string();
        let args = server_args(&p, "own", std::path::Path::new("C:/r/cli.mjs"), std::path::Path::new("C:/r/sidecar.ts"), &[key.clone()], "ab12").unwrap();
        let tail: Vec<&str> = args.iter().rev().take(7).rev().map(String::as_str).collect();
        assert_eq!(tail, ["app-server", "--listen", LISTEN, "--ws-auth", "capability-token", "--ws-token-sha256", "ab12"]);
        assert!(args.iter().any(|a| a.starts_with("mcp_servers.own=")));
        assert!(args.contains(&"features.codex_hooks=true".to_string()));
        let off = args.iter().find(|a| a.starts_with("hooks.state=")).unwrap();
        // The value parses as TOML into exactly the one key, turned off.
        let value: TomlValue = off.split_once('=').unwrap().1.parse().unwrap();
        let table = value.as_inline_table().unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(&key).unwrap().as_inline_table().unwrap().get("enabled").unwrap().as_bool(), Some(false));
        assert!(!args.iter().any(|a| a.contains(NO_TOKEN)));
        assert!(args.iter().all(|a| !a.contains('%')));
        assert_eq!(hook_override(&[]).unwrap(), None);
        assert!(hook_override(&["it's".into()]).is_err());
        assert!(hook_override(&["a%PATH%".into()]).is_err());
    }
    const NO_TOKEN: &str = "secret-token";

    #[test]
    fn requests_carry_the_character_every_time() {
        let settings = ThreadSettings { model: Some("m".into()), approval_policy: Some("never".into()), sandbox: Some("danger-full-access".into()), effort: Some("high".into()) };
        let start = start_params("C:/w", Some("CHAR"), &settings);
        assert_eq!(start["developerInstructions"], "CHAR");
        assert_eq!(start["approvalPolicy"], "never");
        assert_eq!(start["sandbox"], "danger-full-access");
        assert_eq!(start["config"]["model_reasoning_effort"], "high");
        assert_eq!(start["historyMode"], "paginated");
        let resume = resume_params(ID, "C:/w", Some("CHAR"), &settings);
        assert_eq!(resume["developerInstructions"], "CHAR");
        assert_eq!(resume["threadId"], ID);
        assert_eq!(resume["model"], "m");
        let bare = start_params("C:/w", None, &ThreadSettings::default());
        assert!(bare.get("developerInstructions").is_none() && bare.get("config").is_none());
        let inject = inject_params(ID);
        assert_eq!(inject["items"][0]["role"], "developer");
    }

    #[test]
    fn the_room_text_follows_the_character_and_never_changes_it() {
        let room = crate::Cli::CodexCli
            .room_system_prompt("pullcept-room-lin-r1")
            .expect("a safe text");
        // The same text a Claude seat is handed on its launch line.
        assert_eq!(Some(room.clone()), crate::Cli::ClaudeCode.room_system_prompt("pullcept-room-lin-r1"));
        assert!(room.contains("mcp__pullcept-room-lin-r1__say_to_room"));
        assert!(room.contains("instructions of the room MCP server mcp__pullcept-room-lin-r1."));
        let character = "# ルナ\n口調は柔らかく。\n";
        let both = developer_instructions(Some(character), Some(&room)).unwrap();
        // The character's bytes come through whole, so the length and sha256
        // the loader checked are still the character's own.
        assert!(both.starts_with(character));
        assert_eq!(&both[character.len()..], format!("\n\n{room}"));
        assert_eq!(developer_instructions(None, Some(&room)).as_deref(), Some(room.as_str()));
        assert_eq!(developer_instructions(Some("x"), None).as_deref(), Some("x"));
        assert_eq!(developer_instructions(Some(""), None), None);
        assert_eq!(developer_instructions(None, None), None);
        let start = start_params("C:/w", Some(&both), &ThreadSettings::default());
        let resume = resume_params(ID, "C:/w", Some(&both), &ThreadSettings::default());
        assert_eq!(start["developerInstructions"], resume["developerInstructions"]);
        assert!(resume["developerInstructions"].as_str().unwrap().ends_with(&room));
    }

    #[test]
    fn file_seats_keep_common_character_and_room_on_start_and_resume() {
        let common = "共通指示\r\n引用 \" & %PATH% 😀";
        let character = "# ルナ\r\n本文\r\n";
        let room = crate::Cli::CodexCli.room_system_prompt("pullcept-room-luna-r1").unwrap();
        let text = seat_instructions("file", Some(character), Some(&room), || Ok(Some(common.into()))).unwrap().unwrap();
        assert_eq!(text, format!("{common}\n\n{character}\n\n{room}"));
        let start = start_params("C:/w", Some(&text), &ThreadSettings::default());
        let resume = resume_params(ID, "C:/w", Some(&text), &ThreadSettings::default());
        assert_eq!(start["developerInstructions"], text);
        assert_eq!(resume["developerInstructions"], text);
        for common in [None, Some(String::new())] {
            assert_eq!(seat_instructions("file", Some(character), Some(&room), || Ok(common)).unwrap(),
                Some(format!("{character}\n\n{room}")));
        }
        assert_eq!(seat_instructions("file", Some(character), None, || Err("discovery failed".into())), Err("discovery failed".into()));
    }

    #[test]
    fn legacy_selection_and_characterless_fallback_do_not_duplicate_common_text() {
        let selected = "COMMON\n# Selected\nBODY\n";
        assert_eq!(seat_instructions("legacy", Some(selected), Some("ROOM"), || panic!("selection already contains common instructions")).unwrap(),
            Some(format!("{selected}\n\nROOM")));
        for mode in ["legacy", "disabled"] {
            assert_eq!(seat_instructions(mode, None, Some("ROOM"), || Ok(Some("COMMON".into()))).unwrap(), Some("COMMON\n\nROOM".into()));
            assert_eq!(seat_instructions(mode, None, Some("ROOM"), || Ok(None)).unwrap(), Some("ROOM".into()));
            assert!(seat_instructions(mode, None, Some("ROOM"), || Err("discovery failed".into())).is_err());
            assert_eq!(seat_instructions(mode, None, None, || panic!("no override needed")).unwrap(), None);
        }
    }

    #[test]
    fn answers_are_checked_against_what_was_asked() {
        // Shapes as Codex 0.160.1 answered thread/start (measured 2026-10-06).
        let result = json!({"thread":{"id":ID,"path":"C:/h/sessions/r.jsonl"},"model":"gpt-5.2","approvalPolicy":"never",
            "sandbox":{"type":"dangerFullAccess"},"reasoningEffort":"low"});
        assert_eq!(thread_of(&result).unwrap(), (ID.to_string(), Some("C:/h/sessions/r.jsonl".into())));
        let asked = ThreadSettings { model: Some("gpt-5.2".into()), approval_policy: Some("never".into()), sandbox: Some("danger-full-access".into()), effort: Some("low".into()) };
        assert!(check_settings(&result, &asked).is_ok());
        assert!(check_settings(&result, &ThreadSettings::default()).is_ok());
        for wrong in [
            ThreadSettings { sandbox: Some("read-only".into()), ..Default::default() },
            ThreadSettings { effort: Some("high".into()), ..Default::default() },
            ThreadSettings { approval_policy: Some("on-request".into()), ..Default::default() },
            ThreadSettings { model: Some("other".into()), ..Default::default() },
        ] {
            assert!(check_settings(&result, &wrong).is_err());
        }
        assert!(thread_of(&json!({"thread":{"id":"../x"}})).is_err());
    }

    #[test]
    fn hooks_list_must_show_the_style_hook_off_and_no_other_style_handler_on() {
        let key = "C:\\p\\.codex\\hooks.json:session_start:0:1";
        let list = |enabled: bool, extra: Value| json!({"data":[{"cwd":"C:/p","hooks":[
            {"key":"C:\\p\\.codex\\hooks.json:session_start:0:0","eventName":"sessionStart","command":"bash on-session-start.sh","enabled":true},
            {"key":key,"eventName":"sessionStart","command":"python \"C:/p/.codex/hooks/codex-output-style.py\" hook --root \"C:/p\"","enabled":enabled},
            extra]}]});
        let other = json!({"key":"k","eventName":"userPromptSubmit","command":"x","enabled":true});
        assert!(check_hooks(&list(false, other.clone()), &[key.into()]).is_ok());
        assert!(check_hooks(&list(true, other.clone()), &[key.into()]).is_err());
        assert!(check_hooks(&list(false, other.clone()), &["missing".into()]).is_err());
        let second = json!({"key":"k2","eventName":"sessionStart","command":"python codex-output-style.py hook","enabled":true});
        assert!(check_hooks(&list(false, second), &[key.into()]).is_err());
    }

    #[test]
    fn the_bound_port_is_read_from_the_banner() {
        assert_eq!(listening_port("  listening on: ws://127.0.0.1:57367"), Some(57367));
        assert_eq!(listening_port("  readyz: http://127.0.0.1:57367/readyz"), None);
        assert_eq!(listening_port("  listening on: ws://127.0.0.1:0"), None);
    }
}
