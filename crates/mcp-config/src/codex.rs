//! Codex CLI 0.160: project MCP registration, guarded root hook, native history.
use super::*;
use toml_edit::{value, Array, Document as DocumentMut, Item, Table};
mod character;
pub mod limit;
pub mod status;
pub use character::{apply_character, check_command_length, instruction_value, select_character, transport_options};

pub const LAUNCH_ID_ENV: &str = "PULLCEPT_LAUNCH_ID";
pub const LAUNCH_ROOM_ENV: &str = "PULLCEPT_LAUNCHED_ROOM";
pub const NATIVE_URL_ENV: &str = "PULLCEPT_NATIVE_URL";
pub const OUTPUT_STYLE_ENV: &str = "LI_PLUS_OUTPUT_STYLE";
pub const NATIVE_PATH: &str = "/hooks/codex-session";
pub const SIDECAR_ENV: &[&str] = &[
    ROOM_TOKEN_ENV,
    LAUNCHED_AS_ENV,
    LAUNCH_ROOM_ENV,
    "PULLCEPT_ROOM_URL",
    "PULLCEPT_AGENT_NAME",
    "PULLCEPT_ACCOUNT_ID",
    ROOM_ID_ENV,
    "PULLCEPT_AGENT_HUE",
    "PULLCEPT_UNSEEN_HISTORY",
];

pub fn runtime_args(
    base: &[String],
    own: &str,
    disabled: &[String],
    runner: &Path,
    entry: &Path,
) -> Result<Vec<String>, String> {
    let paths = [runner, entry].map(|p| p.to_string_lossy().replace('\\', "/"));
    if paths.iter().any(|p| !console_safe(p) || p.contains("'''")) {
        return Err("Codex MCP runner path cannot be carried by the Windows launch line".into());
    }
    let mut args = launch_args(&transport_options(base)?, own, disabled);
    let vars = SIDECAR_ENV
        .iter()
        .map(|key| format!("'{key}'"))
        .collect::<Vec<_>>()
        .join(",");
    // Replace the whole table, including any persisted env. Works before project trust,
    // and a trusted project's previous registration cannot override the current launch.
    args.extend(["-c".into(), format!("mcp_servers.{own}={{command='node',args=[{},{}],env_vars=[{vars}],enabled=true,tools={{say_to_room={{approval_mode='approve'}},read_room_history={{approval_mode='approve'}}}}}}", instruction_value(&paths[0]), instruction_value(&paths[1]))]);
    Ok(args)
}

pub fn launch_args(base: &[String], own: &str, disabled: &[String]) -> Vec<String> {
    let mut args = base.to_vec();
    for name in disabled
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(own))
    {
        args.extend([
            "-c".into(),
            format!("mcp_servers.{name}.enabled={}", name == own),
        ]);
    }
    args.extend(["-c".into(), "features.codex_hooks=true".into()]);
    if !sets_alternate_screen(base) {
        args.push(NO_ALT_SCREEN.into());
    }
    args
}

/// Inline TUI: the seat's terminal keeps Codex's output as scrollback (#293).
/// Accepted after `resume <id>` as well as on a fresh line (`codex resume --help`, 0.160).
pub const NO_ALT_SCREEN: &str = "--no-alt-screen";

/// The person already chose the screen mode: the flag itself, or a `-c` value keyed
/// `tui.alternate_screen` in any form the option parsing accepts. Adding ours then
/// would only risk a launch the CLI refuses.
fn sets_alternate_screen(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return false;
        }
        if arg == NO_ALT_SCREEN {
            return true;
        }
        let config = if ["-c", "--config"].contains(&arg) {
            i += 1;
            args.get(i).map(String::as_str)
        } else {
            arg.strip_prefix("--config=").or_else(|| arg.strip_prefix("-c="))
        };
        if config
            .and_then(|c| c.split_once('='))
            .is_some_and(|(key, _)| key.trim() == "tui.alternate_screen")
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Codex resolves a --cd value against the process's original working directory.
pub fn effective_cwd(initial: &Path, args: &[String]) -> Result<PathBuf, String> {
    let mut cwd = initial.to_path_buf();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let target = if ["-C", "--cd"].contains(&arg.as_str()) {
            i += 1;
            Some(
                args.get(i)
                    .ok_or("Codex --cd requires a directory")?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--cd=")
                .or_else(|| arg.strip_prefix("-C="))
        };
        if let Some(target) = target {
            cwd = initial.join(target);
        } else if !arg.contains('=')
            && [
                "-c",
                "--config",
                "-p",
                "--profile",
                "-m",
                "--model",
                "-a",
                "--ask-for-approval",
                "-s",
                "--sandbox",
                "--add-dir",
                "--enable",
                "--disable",
                "-i",
                "--image",
            ]
            .contains(&arg.as_str())
        {
            i += 1;
        }
        i += 1;
    }
    if !cwd.is_dir() {
        return Err(format!(
            "Codex の作業フォルダーがありません: {}",
            cwd.display()
        ));
    }
    Ok(cwd)
}

/// app-server rejects --profile. Only the profile's discovery setting is carried
/// over; the actual interactive command still receives the original profile.
pub fn discovery_options(args: &[String], home: &Path) -> Result<Vec<String>, String> {
    let mut options = Vec::new();
    let mut profile = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if ["-p", "--profile"].contains(&arg.as_str()) {
            i += 1;
            profile = Some(
                args.get(i)
                    .ok_or("Codex --profile requires a name")?
                    .as_str(),
            );
        } else if let Some(name) = arg.strip_prefix("--profile=") {
            profile = Some(name);
        } else {
            options.push(arg.clone());
            if !arg.contains('=')
                && [
                    "-c",
                    "--config",
                    "-m",
                    "--model",
                    "-a",
                    "--ask-for-approval",
                    "-s",
                    "--sandbox",
                    "-C",
                    "--cd",
                    "--add-dir",
                    "--enable",
                    "--disable",
                    "-i",
                    "--image",
                ]
                .contains(&arg.as_str())
            {
                i += 1;
                if let Some(value) = args.get(i) {
                    options.push(value.clone());
                }
            }
        }
        i += 1;
    }
    if let Some(name) = profile {
        if name.is_empty() || name.contains(['/', '\\', ':']) {
            return Err("Codex profile 名にパスは指定できません。".into());
        }
        let file = home.join(format!("{name}.config.toml"));
        let text = std::fs::read_to_string(&file)
            .map_err(|_| format!("Codex profile を読めません: {}", file.display()))?;
        let doc: DocumentMut = text
            .parse()
            .map_err(|_| format!("Codex profile の TOML を読めません: {}", file.display()))?;
        if let Some(markers) = doc.get("project_root_markers") {
            let markers = markers
                .as_array()
                .ok_or("Codex profile の project_root_markers は文字列配列が必要です。")?;
            let markers=markers.iter().map(|v|{
                let s=v.as_str().ok_or("Codex project_root_markers は文字列配列が必要です。")?;
                if s.contains(['\'', '\r', '\n']) {return Err("Codex profile の project_root_markers を discovery に保持できません。診断端末で確認してください。");}
                Ok(format!("'{s}'"))
            }).collect::<Result<Vec<_>,&str>>()?.join(",");
            options.splice(
                0..0,
                ["-c".into(), format!("project_root_markers=[{markers}]")],
            );
        }
    }
    Ok(options)
}

/// Mirrors native 0.160 git-utils/trust.rs ownership checks, then the loader's
/// root_checkout_hooks_folder_for_dir mapping. No unrelated checkout is written.
pub fn native_hook_dir(cwd: &Path, config_dir: &Path) -> Result<PathBuf, String> {
    fn metadata_text(path: &Path) -> Result<String, String> {
        let meta =
            std::fs::symlink_metadata(path).map_err(|_| "Codex hook の Git 境界を読めません。")?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 65536 {
            return Err("Codex hook の Git 境界が不正です。".into());
        }
        std::fs::read_to_string(path).map_err(|_| "Codex hook の Git 境界を読めません。".into())
    }
    fn gitdir(path: &Path) -> Result<PathBuf, String> {
        let text = metadata_text(path)?;
        let target = text
            .trim()
            .strip_prefix("gitdir:")
            .filter(|v| !v.trim().is_empty())
            .ok_or("Codex hook の .git 参照が不正です。")?;
        Ok(path.parent().unwrap().join(target.trim()))
    }
    fn canonical(path: &Path) -> Result<PathBuf, String> {
        std::fs::canonicalize(path).map_err(|_| "Codex hook の Git 境界を確認できません。".into())
    }
    fn mismatch() -> String {
        "Codex hook の root checkout が同じ Git repository に属することを確認できません。診断端末で worktree の登録を確認してください。".into()
    }
    let cwd = canonical(cwd)?;
    let config_dir = canonical(config_dir)?;
    for ancestor in cwd.ancestors() {
        let marker = ancestor.join(".git");
        if marker.is_dir() && marker.join("HEAD").exists() {
            break;
        }
        if marker.is_file() {
            let linked = gitdir(&marker)?;
            if std::fs::symlink_metadata(&linked)
                .map_err(|_| mismatch())?
                .file_type()
                .is_symlink()
            {
                return Err(mismatch());
            }
            let linked = canonical(&linked)?;
            let worktrees = linked.parent().ok_or_else(mismatch)?;
            if worktrees.file_name().and_then(|v| v.to_str()) != Some("worktrees") {
                return Ok(config_dir);
            }
            let common = worktrees.parent().ok_or_else(mismatch)?;
            let registered = linked.join(metadata_text(&linked.join("gitdir"))?.trim());
            if registered.file_name().and_then(|v| v.to_str()) != Some(".git")
                || canonical(registered.parent().ok_or_else(mismatch)?)? != ancestor
                || canonical(&linked.join(metadata_text(&linked.join("commondir"))?.trim()))?
                    != common
            {
                return Err(mismatch());
            }
            let main = common.parent().ok_or_else(mismatch)?;
            let main_marker = main.join(".git");
            let main_git = if main_marker.is_dir() {
                main_marker
            } else {
                gitdir(&main_marker)?
            };
            if canonical(&main_git)? != common {
                return Err(mismatch());
            }
            let relative = config_dir.strip_prefix(ancestor).map_err(|_| mismatch())?;
            let target = main.join(relative);
            let existing = target
                .ancestors()
                .find(|dir| dir.exists())
                .ok_or_else(mismatch)?;
            if !canonical(existing)?.starts_with(main) {
                return Err(mismatch());
            }
            return Ok(target);
        }
    }
    Ok(config_dir)
}

fn check_write_path(dir: &Path, name: &str) -> Result<(), String> {
    let boundary =
        std::fs::canonicalize(dir).map_err(|_| "Codex 設定の配置先を確認できません。")?;
    let target = dir.join(".codex").join(name);
    for existing in target.ancestors() {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => {
                let resolved = std::fs::canonicalize(existing)
                    .map_err(|_| "Codex 設定のリンク先を確認できません。")?;
                if !resolved.starts_with(&boundary) {
                    return Err("Codex 設定／hook のリンク先が認定した配置先の外にあります。登録せず起動を止めました。".into());
                }
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Codex 設定の配置境界を調べられません。".into()),
        }
    }
    Err("Codex 設定の配置境界を確認できません。".into())
}

fn document(dir: &Path) -> Result<DocumentMut, String> {
    check_write_path(dir, "config.toml")?;
    let path = dir.join(".codex/config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => text.parse().map_err(|_| {
            format!(
                "{}: Codex 設定の TOML を読めません。診断端末で設定ファイルを確認してください。",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn other_servers(dir: &Path, own: &str) -> Result<Vec<String>, String> {
    let doc = document(dir)?;
    Ok(doc
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter_map(|(name, entry)| {
                    (name != own
                        && entry
                            .get("env")
                            .and_then(|env| env.get("PULLCEPT_ACCOUNT_ID"))
                            .is_some())
                    .then(|| name.to_string())
                })
                .collect()
        })
        .unwrap_or_default())
}

pub fn register(
    dir: &Path,
    room: &RoomRegistration<'_>,
) -> Result<PathBuf, String> {
    let mut doc = document(dir)?;
    let name = server_name_for(room.account_id, room.room_id);
    if doc.get("mcp_servers").is_none() {
        doc["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = doc["mcp_servers"]
        .as_table_like_mut()
        .ok_or("mcp_servers must be a table")?;
    if let Some(existing) = servers.get(&name) {
        if existing
            .get("env")
            .and_then(|env| env.get("PULLCEPT_ACCOUNT_ID"))
            .and_then(Item::as_str)
            != Some(room.account_id)
        {
            return Err(format!(
                "MCP name {name} belongs to an existing non-Pullcept entry"
            ));
        }
    }
    // Sweep only app-owned registrations from previous runs. Foreign entries stay.
    let stale: Vec<String> = servers
        .iter()
        .filter_map(|(key, entry)| {
            let owned = entry
                .get("env")
                .and_then(|env| env.get("PULLCEPT_ACCOUNT_ID"))
                .is_some();
            let url = entry
                .get("env")
                .and_then(|env| env.get("PULLCEPT_ROOM_URL"))
                .and_then(Item::as_str);
            (owned && key.starts_with(SERVER_PREFIX) && url != Some(room.room_url))
                .then(|| key.to_string())
        })
        .collect();
    for key in stale {
        servers.remove(&key);
    }
    let mut entry = Table::new();
    entry["command"] = value("node");
    let mut args = Array::new();
    args.push(room.sidecar_runner.to_string_lossy().as_ref());
    args.push(room.sidecar_entry.to_string_lossy().as_ref());
    entry["args"] = value(args);
    // Ordinary Codex launches leave every app-owned seat disabled.
    entry["enabled"] = value(false);
    let mut vars = Array::new();
    for key in [ROOM_TOKEN_ENV, LAUNCHED_AS_ENV, LAUNCH_ROOM_ENV] {
        vars.push(key);
    }
    entry["env_vars"] = value(vars);
    let mut env = Table::new();
    for (key, val) in [
        ("PULLCEPT_ROOM_URL", room.room_url),
        ("PULLCEPT_AGENT_NAME", room.agent_name),
        ("PULLCEPT_ACCOUNT_ID", room.account_id),
        (ROOM_ID_ENV, room.room_id),
    ] {
        env[key] = value(val);
    }
    if let Some(hue) = room.agent_hue {
        env["PULLCEPT_AGENT_HUE"] = value(format!("{hue:.1}"));
    }
    if room.unseen_history {
        env["PULLCEPT_UNSEEN_HISTORY"] = value("1");
    }
    entry["env"] = Item::Table(env);
    servers.insert(&name, Item::Table(entry));
    let path = dir.join(".codex/config.toml");
    std::fs::create_dir_all(dir.join(".codex")).map_err(|e| e.to_string())?;
    check_write_path(dir, "config.toml")?;
    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn register_hook(dir: &Path, script: &Path) -> Result<(), String> {
    if !console_safe(&script.to_string_lossy()) {
        return Err("Codex hook path contains shell characters".into());
    }
    // A mapped nested root may not exist yet. Validate its existing ancestry
    // before creating it, then enforce the actual .codex/file write boundary.
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    check_write_path(dir, "hooks.json")?;
    let path = dir.join(".codex/hooks.json");
    let mut root = read_config(&path)?;
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("hooks must be an object")?;
    let handlers = hooks.entry("SessionStart").or_insert_with(|| json!([]));
    let handlers = handlers
        .as_array_mut()
        .ok_or("SessionStart must be an array")?;
    let command = format!("node \"{}\"", script.to_string_lossy().replace('\\', "/"));
    let handler = json!({"hooks":[{"type":"command","command":command,"timeout":10}]});
    // Idempotent without touching other commands or their trust state.
    if !handlers.iter().any(|h| h == &handler) {
        handlers.push(handler);
    }
    std::fs::create_dir_all(dir.join(".codex")).map_err(|e| e.to_string())?;
    check_write_path(dir, "hooks.json")?;
    std::fs::write(path, serde_json::to_string_pretty(&root).unwrap()).map_err(|e| e.to_string())
}

pub fn hook_dir_from_layers(cwd: &Path, layers: &[Value]) -> Result<PathBuf, String> {
    let cwd = std::fs::canonicalize(cwd).map_err(|e| e.to_string())?;
    for layer in layers {
        let Some(folder) = layer.get("folder").and_then(Value::as_str) else {
            continue;
        };
        let folder = PathBuf::from(folder);
        if folder.file_name().and_then(|v| v.to_str()) != Some(".codex") {
            continue;
        }
        let Some(parent) = folder.parent() else {
            continue;
        };
        let root = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
        // Only an actual native layer in this cwd's ancestry can be written.
        if cwd.starts_with(&root) {
            return Ok(root);
        }
    }
    Err("Codex がこの作業フォルダー内の project layer を見つけませんでした。診断端末でフォルダーの信頼と project_root_markers を確認してください。登録先を確認できないため起動しません。".into())
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn native_matches<'a>(
    value: &'a Value,
    room: &str,
    account: &str,
    launch: &str,
    known: Option<&str>,
) -> Option<&'a str> {
    let get = |key: &str| value.get(key).and_then(Value::as_str);
    let id = get("session_id")?;
    (get("hook_event_name") == Some("SessionStart")
        && get("room_id") == Some(room)
        && get("account_id") == Some(account)
        && get("launch_id") == Some(launch)
        && valid_id(id)
        && known.is_none_or(|known| known == id))
    .then_some(id)
}

pub fn transcript_checked(home: &Path, id: &str) -> Result<Option<PathBuf>, std::io::Error> {
    if !valid_id(id) {
        return Ok(None);
    }
    fn find(dir: &Path, suffix: &str, depth: usize) -> Result<Option<PathBuf>, std::io::Error> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_file()
                && entry.file_name().to_string_lossy().ends_with(suffix)
                && entry.metadata()?.len() > 0
            {
                return Ok(Some(path));
            }
            if kind.is_dir() && depth > 0 {
                if let Some(found) = find(&path, suffix, depth - 1)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
    let suffix = format!("-{id}.jsonl");
    match find(&home.join("sessions"), &suffix, 3)? {
        Some(path) => Ok(Some(path)),
        None => find(&home.join("archived_sessions"), &suffix, 0),
    }
}

pub fn transcript(home: &Path, id: &str) -> Option<PathBuf> {
    transcript_checked(home, id).ok().flatten()
}

pub fn supported_version(text: &str) -> bool {
    let parts: Vec<u32> = text
        .trim()
        .strip_prefix("codex-cli ")
        .unwrap_or("")
        .split('.')
        .map(|v| v.parse().unwrap_or(0))
        .collect();
    parts.len() >= 3 && (parts[0] > 0 || parts[1] >= 160)
}

pub fn reject_options(args: &[String]) -> Result<(), String> {
    transport_options(args)?;
    let mut value_next = false;
    for arg in args {
        if value_next {
            value_next = false;
            continue;
        }
        if arg == "--" {
            break;
        }
        let head = arg.split('=').next().unwrap_or(arg);
        if [
            "--last",
            "--all",
            "--session-id",
            "--settings",
            "--remote",
            "--worktree",
        ]
        .contains(&head)
            || arg.contains(SESSION_ID_PLACEHOLDER)
            || (!arg.starts_with('-')
                && [
                    "agents",
                    "exec",
                    "e",
                    "review",
                    "login",
                    "logout",
                    "mcp",
                    "plugin",
                    "app-server",
                    "remote-control",
                    "app",
                    "completion",
                    "update",
                    "doctor",
                    "sandbox",
                    "debug",
                    "apply",
                    "a",
                    "resume",
                    "queue",
                    "archive",
                    "delete",
                    "migrate-rollouts",
                    "unarchive",
                    "fork",
                    "cloud",
                    "exec-server",
                    "features",
                    "help",
                ]
                .contains(&head))
        {
            return Err(format!(
                "Codex の対話起動に {arg} は指定できません。再開 ID はトピックから渡します。"
            ));
        }
        value_next = !arg.contains('=')
            && [
                "-c",
                "--config",
                "-p",
                "--profile",
                "-m",
                "--model",
                "-a",
                "--ask-for-approval",
                "-s",
                "--sandbox",
                "-C",
                "--cd",
                "--add-dir",
                "--enable",
                "--disable",
                "-i",
                "--image",
            ]
            .contains(&head);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "12345678-1234-1234-1234-123456789abc";
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pullcept-codex-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        dir
    }
    fn room<'a>(entry: &'a Path, account: &'a str, room_id: &'a str) -> RoomRegistration<'a> {
        RoomRegistration {
            room_url: "ws://127.0.0.1:12345",
            token: "secret-never-in-argv",
            account_id: account,
            room_id,
            agent_name: "Codex Luna",
            agent_hue: Some(120.),
            unseen_history: true,
            sidecar_entry: entry,
            sidecar_runner: entry,
        }
    }
    #[test]
    fn codex_registration_preserves_foreign_config_and_is_disabled_by_default() {
        let dir = scratch();
        let script = dir.join("sidecar.ts");
        std::fs::write(dir.join(".codex/config.toml"), "# retained comment\nmodel = 'gpt-6'\ndeveloper_instructions = 'existing identity'\n[mcp_servers.foreign]\ncommand='custom'\n").unwrap();
        register(&dir, &room(&script, "a", "t")).unwrap();
        register(&dir, &room(&script, "b", "t")).unwrap();
        register(&dir, &room(&script, "a", "u")).unwrap();
        let doc = document(&dir).unwrap();
        assert!(doc.to_string().contains("# retained comment"));
        assert_eq!(
            doc["developer_instructions"].as_str(),
            Some("existing identity")
        );
        assert_eq!(
            doc["mcp_servers"]["foreign"]["command"].as_str(),
            Some("custom")
        );
        let own = server_name_for("a", "t");
        assert_eq!(doc["mcp_servers"][&own]["enabled"].as_bool(), Some(false));
        assert!(doc["mcp_servers"][&own]["env"].get("PULLCEPT_CHARACTER").is_none());
        assert!(!doc.to_string().contains("secret-never-in-argv"));
        let others = other_servers(&dir, &own).unwrap();
        assert_eq!(others.len(), 2);
        let args = super::super::launch_args(
            &["--model".into(), "gpt-6".into()],
            Some(Cli::CodexCli),
            &own,
            Some("character"),
            &others,
            Some("claude-status"),
        );
        assert!(args.contains(&format!("mcp_servers.{own}.enabled=true")));
        assert!(!args.iter().any(|arg| arg.contains("secret")
            || arg.contains("--settings")
            || arg.contains("--session-id")
            || arg.contains("character")));
        let claude = register_sidecar(&dir, &room(&script, "claude", "t")).unwrap();
        assert!(claude.is_file());
        assert_eq!(document(&dir).unwrap().to_string(), doc.to_string());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn hook_registration_is_idempotent_and_preserves_other_hooks() {
        let dir = scratch();
        let path = dir.join(".codex/hooks.json");
        std::fs::write(&path,r#"{"other":"keep","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"user-hook"}]}],"Stop":[]}}"#).unwrap();
        register_hook(&dir, &dir.join("codex-session.mjs")).unwrap();
        register_hook(&dir, &dir.join("codex-session.mjs")).unwrap();
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(doc["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(
            doc["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "user-hook"
        );
        assert!(doc["hooks"]["Stop"].is_array());
        assert_eq!(doc["other"], "keep");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_foreign_toml_never_returns_source_values_in_diagnostics() {
        let dir = scratch();
        std::fs::write(
            dir.join(".codex/config.toml"),
            "[mcp_servers.foreign.env]\nTOKEN='private-value-do-not-display\n",
        )
        .unwrap();
        let err = other_servers(&dir, "own").unwrap_err();
        assert!(!err.contains("private-value-do-not-display"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn codex_directory_link_cannot_redirect_config_or_hook_outside_the_boundary() {
        let dir = scratch();
        let outside = scratch();
        std::fs::remove_dir(dir.join(".codex")).unwrap();
        let link = dir.join(".codex");
        #[cfg(windows)]
        {
            let result = std::process::Command::new("cmd.exe")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "fixture directory junction must be created"
            );
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(register(&dir, &room(Path::new("script"), "a", "t")).is_err());
        assert!(register_hook(&dir, Path::new("script")).is_err());
        assert!(!outside.join("config.toml").exists());
        assert!(!outside.join("hooks.json").exists());
        #[cfg(windows)]
        std::fs::remove_dir(link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn native_capture_requires_the_current_root_launch_and_never_rebinds() {
        let original = json!({"session_id":ID,"hook_event_name":"SessionStart","room_id":"topic","account_id":"a","launch_id":"nonce"});
        assert_eq!(
            native_matches(&original, "topic", "a", "nonce", None),
            Some(ID)
        );
        assert_eq!(
            native_matches(&original, "topic", "a", "nonce", Some(ID)),
            Some(ID)
        );
        for (key, bad) in [
            ("room_id", "other"),
            ("account_id", "b"),
            ("launch_id", "old"),
            ("session_id", "../wrong"),
            ("hook_event_name", "SubagentStart"),
        ] {
            let mut wrong = original.clone();
            wrong[key] = json!(bad);
            assert_eq!(native_matches(&wrong, "topic", "a", "nonce", None), None);
        }
        assert!(native_matches(
            &original,
            "topic",
            "a",
            "nonce",
            Some("00000000-0000-0000-0000-000000000000")
        )
        .is_none());
    }
    #[test]
    fn explicit_native_resume_and_missing_transcript() {
        let dir = scratch();
        let history = dir.join("sessions/2026/10/04");
        std::fs::create_dir_all(&history).unwrap();
        assert!(transcript(&dir, ID).is_none());
        let file = history.join(format!("rollout-now-{ID}.jsonl"));
        std::fs::write(&file, "native rollout").unwrap();
        assert_eq!(transcript(&dir, ID), Some(file.clone()));
        assert!(transcript(&dir, "../wrong").is_none());
        let args = resume_launch_args(
            &["resume".into(), SESSION_ID_PLACEHOLDER.into()],
            &["--model".into(), "gpt-6".into()],
            Cli::CodexCli,
        );
        let composed = substitute_session_id(&launch_args(&args, "own", &[]), ID);
        assert_eq!(&composed[..4], &["resume", ID, "--model", "gpt-6"]);
        assert!(!composed.contains(&"--last".into()));
        assert_eq!(composed.last().map(String::as_str), Some(NO_ALT_SCREEN));
        std::fs::remove_file(file).unwrap();
        assert!(transcript(&dir, ID).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn version_floor_and_interactive_options_are_explicit() {
        assert!(supported_version("codex-cli 0.160.0\n"));
        assert!(supported_version("codex-cli 1.0.0"));
        assert!(!supported_version("codex-cli 0.120.0"));
        assert!(!supported_version("unknown"));
        for text in [
            "exec",
            "--last",
            "--session-id",
            "--settings",
            SESSION_ID_PLACEHOLDER,
        ] {
            assert!(reject_options(&[text.into()]).is_err());
        }
        assert!(reject_options(&[
            "--profile".into(),
            "resume".into(),
            "-m".into(),
            "exec".into()
        ])
        .is_ok());
    }
    #[test]
    fn cd_values_resolve_from_initial_cwd_and_preserve_option_values() {
        let dir = scratch();
        let other = dir.join("other");
        std::fs::create_dir(&other).unwrap();
        for args in [
            vec!["-C", "other"],
            vec!["--cd", "other"],
            vec!["--cd=other"],
            vec!["-C=other"],
        ] {
            assert_eq!(
                effective_cwd(
                    &dir,
                    &args.into_iter().map(String::from).collect::<Vec<_>>()
                )
                .unwrap(),
                other
            );
        }
        assert_eq!(
            effective_cwd(&dir, &["--profile".into(), "--cd=ignored".into()]).unwrap(),
            dir
        );
        assert!(effective_cwd(&dir, &["--cd".into()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn profile_discovery_uses_selected_home_and_dotted_name_before_user_overrides() {
        let dir = scratch();
        std::fs::write(
            dir.join("custom.v2.config.toml"),
            "project_root_markers=['.root']\nmodel='preserved-by-native-cli'\n",
        )
        .unwrap();
        let options = vec![
            "--profile=custom.v2".into(),
            "-c".into(),
            "project_root_markers=['.git']".into(),
            "--cd=other".into(),
        ];
        assert_eq!(
            discovery_options(&options, &dir).unwrap(),
            vec![
                "-c",
                "project_root_markers=['.root']",
                "-c",
                "project_root_markers=['.git']",
                "--cd=other"
            ]
        );
        assert!(discovery_options(&["--profile=missing".into()], &dir).is_err());
        assert!(discovery_options(&["--profile=../other".into()], &dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linked_worktree_maps_only_the_verified_repository_and_relative_directory() {
        let dir = scratch();
        let main = dir.join("main");
        let checkout = dir.join("checkout");
        let gitdir = main.join(".git/worktrees/owned");
        std::fs::create_dir_all(&gitdir).unwrap();
        let nested = checkout.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(gitdir.join("commondir"), "../..").unwrap();
        std::fs::write(
            gitdir.join("gitdir"),
            checkout.join(".git").to_string_lossy().as_ref(),
        )
        .unwrap();
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();
        assert_eq!(
            native_hook_dir(&nested, &nested).unwrap(),
            std::fs::canonicalize(&main).unwrap().join("nested")
        );
        std::fs::write(
            gitdir.join("gitdir"),
            dir.join("wrong/.git").to_string_lossy().as_ref(),
        )
        .unwrap();
        assert!(native_hook_dir(&nested, &nested).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn declared_codex_accounts_round_trip_and_generic_accounts_stay_generic() {
        let mut data = json!({"accounts":[{"kind":"codex_cli","command":"custom.exe","args":["--model","gpt-6"]},{"kind":"cli","command":"codex","args":[]}]});
        migrate_account_kinds(&mut data, "ai", |cli| match cli {
            Some(Cli::ClaudeCode) => json!("claude_code"),
            Some(Cli::CodexCli) => json!("codex_cli"),
            None => json!("cli"),
        });
        let read: Value = serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        assert_eq!(read["accounts"][0]["kind"], "codex_cli");
        assert_eq!(read["accounts"][0]["command"], "custom.exe");
        assert_eq!(read["accounts"][1]["kind"], "cli");
    }
    #[test]
    fn native_layers_select_only_a_real_ancestor_and_io_errors_are_unknown() {
        let dir = scratch();
        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            hook_dir_from_layers(&nested, &[json!({"folder":dir.join(".codex")})]).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(hook_dir_from_layers(
            &nested,
            &[json!({"folder":std::env::temp_dir().join("other-project/.codex")})]
        )
        .is_err());
        std::fs::write(dir.join("sessions"), "not a directory").unwrap();
        assert!(
            transcript_checked(&dir, ID).is_err(),
            "an I/O failure is not missing history"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn every_line_runs_inline_unless_the_person_chose_the_screen_mode() {
        let count = |args: &[String]| args.iter().filter(|a| *a == NO_ALT_SCREEN).count();
        let fresh = launch_args(&["--model".into(), "gpt-6".into()], "own", &[]);
        assert_eq!(count(&fresh), 1);
        assert_eq!(fresh.last().map(String::as_str), Some(NO_ALT_SCREEN));
        let resumed = launch_args(
            &resume_launch_args(&["resume".into(), ID.into()], &[], Cli::CodexCli),
            "own",
            &[],
        );
        assert_eq!(&resumed[..2], &["resume", ID]);
        assert_eq!(count(&resumed), 1);
        let runtime = runtime_args(
            &[],
            "own",
            &[],
            Path::new("C:/runner/cli.mjs"),
            Path::new("C:/repo/sidecar.ts"),
        )
        .unwrap();
        assert_eq!(count(&runtime), 1);
        let own = launch_args(&[NO_ALT_SCREEN.into()], "own", &[]);
        assert_eq!(count(&own), 1);
        for chosen in [
            vec!["-c".to_string(), "tui.alternate_screen=always".into()],
            vec!["--config".into(), "tui.alternate_screen=\"never\"".into()],
            vec!["-c=tui.alternate_screen=auto".into()],
            vec!["--config=tui.alternate_screen=never".into()],
        ] {
            assert_eq!(count(&launch_args(&chosen, "own", &[])), 0, "{chosen:?}");
            // A form the launch refuses (`-c=`) never reaches the line at all.
            if let Ok(normalized) = transport_options(&chosen) {
                assert_eq!(count(&launch_args(&normalized, "own", &[])), 0, "{chosen:?}");
            }
        }
        let other = launch_args(&["-c".into(), "tui.theme=dark".into()], "own", &[]);
        assert_eq!(count(&other), 1);
        assert!(reject_options(&[NO_ALT_SCREEN.into()]).is_ok());
    }
    #[test]
    fn runtime_table_replaces_stale_env_and_carries_only_current_launch_names() {
        let args = runtime_args(
            &[],
            "own",
            &[],
            Path::new("C:/runner path/cli.mjs"),
            Path::new("C:/repo/sidecar.ts"),
        )
        .unwrap();
        let definition = args.last().unwrap();
        let parsed: DocumentMut = definition.parse().unwrap();
        let own = &parsed["mcp_servers"]["own"];
        assert!(own.get("env").is_none());
        assert_eq!(own["command"].as_str(), Some("node"));
        assert_eq!(
            own["tools"]["say_to_room"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            own["tools"]["read_room_history"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert!(own["env_vars"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(ROOM_TOKEN_ENV)));
        assert!(!definition.contains("secret-never-in-argv"));
    }
}
