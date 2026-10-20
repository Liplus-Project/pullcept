//! Bundled, offline installation; the selector remains a per-seat choice.
use std::{fs, io::Write, path::{Path, PathBuf}};
use serde_json::{json, Value};
use toml_edit::{Document, value};

pub const LOADER: &str = include_str!("../../assets/codex-output-style.py");

/// The installed, byte-preserved LF/CRLF snapshot is pinned for the approval
/// subprocess; all other script edits fail before native trust is written.
pub fn installed_hash(cwd: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = regular(&cwd.join(".codex/hooks/codex-output-style.py"))?.ok_or("同梱 helper がありません。")?;
    if std::str::from_utf8(&bytes).map(|s| s.replace("\r\n", "\n")) != Ok(LOADER.replace("\r\n", "\n")) {
        return Err("同梱版と異なる読み込みスクリプトです。承認を中止しました。".into());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn codex_home(cwd: &Path, env: &[(String, String)]) -> Result<PathBuf, String> {
    let home = env.iter().find(|(k, _)| k.eq_ignore_ascii_case("CODEX_HOME")).map(|(_, v)| PathBuf::from(v))
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|p| PathBuf::from(p).join(".codex")))
        .ok_or("Codex home が見つかりません。")?;
    Ok(if home.is_absolute() { home } else { cwd.join(home) })
}

pub fn home_scope(cwd: &Path, home: &Path) -> bool {
    fs::canonicalize(cwd.join(".codex")).ok().zip(fs::canonicalize(home).ok()).is_some_and(|(a, b)| a == b)
        || (home.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(".codex")) &&
            home.parent().and_then(|p| fs::canonicalize(p).ok()).zip(fs::canonicalize(cwd).ok()).is_some_and(|(a, b)| a == b))
}

/// The same canonical command/index for every home seat. Existing inline
/// registrations are rejected rather than silently replaced or re-indexed.
pub fn scoped_options(options: &[String], cwd: &Path, home: &Path) -> Result<Vec<String>, String> {
    let mut args = super::transport_options(options)?;
    if !home_scope(cwd, home) || !cwd.join(".codex/hooks/codex-output-style.py").exists() { return Ok(args); }
    let mut configs = vec![home.join("config.toml")];
    for (index, arg) in args.iter().enumerate() {
        let profile = if ["-p", "--profile"].contains(&arg.as_str()) { args.get(index + 1).map(String::as_str) } else { arg.strip_prefix("--profile=") };
        if let Some(name) = profile {
            if name.is_empty() || name.contains(['/', '\\', ':']) { return Err("Codex profile 名にパスは指定できません。".into()); }
            configs.push(home.join(format!("{name}.config.toml")));
        }
    }
    for config in configs {
        if let Some(bytes) = regular(&config)? {
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            let doc = text.trim_start_matches('\u{feff}').parse::<Document>().map_err(|e| e.to_string())?;
            if doc.get("hooks").and_then(|h| h.get("SessionStart")).is_some() {
                return Err("既存の inline SessionStart 設定とホーム専用 hook が競合します。既存設定を保ち、起動を中止しました。".into());
            }
        }
    }
    let path = fs::canonicalize(cwd).map_err(|e| e.to_string())?.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    if !crate::console_safe(&path) || path.contains(['%', '\'']) { return Err("ホームの hook パスを起動行へ渡せません。".into()); }
    let command = format!("{} \"{path}/.codex/hooks/codex-output-style.py\" hook --root \"{path}\"", if cfg!(windows) { "python" } else { "python3" });
    let raw_pair = format!("hooks.SessionStart=[{{matcher='startup|resume|clear|compact',hooks=[{{type='command',command={},async=false,additionalContextLimit=0}}]}}]", super::instruction_value(&command));
    let pair = super::transport_options(&["-c".into(), raw_pair])?.pop().unwrap();
    let mut owns_inline = false;
    for window in args.windows(2) {
        if window[0] != "-c" { continue; }
        let key = window[1].split('=').next().unwrap_or("").trim();
        if key == "hooks" || key.starts_with("hooks.SessionStart") || key.starts_with("hooks.state") {
            if window[1] == pair { owns_inline = true; }
            else { return Err("起動オプションの hooks 設定とホーム専用 hook が競合します。既存オプションを保ったまま保存・起動を中止しました。".into()); }
        }
    }
    if !owns_inline { args.extend(["-c".into(), pair]); }
    Ok(args)
}

fn regular(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => Err(format!("{}: 通常のファイルではありません。", path.display())),
        Ok(_) => fs::read(path).map(Some).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// Validate everything before writing. Each successful write is tracked so a
/// later failure restores the original bytes; a retry completes the same plan.
pub fn install(cwd: &Path) -> Result<(), String> {
    install_scoped(cwd, false)
}

/// Home registrations are process flags, never a user-wide hooks.json entry.
pub fn install_scoped(cwd: &Path, home_scope: bool) -> Result<(), String> {
    let root = fs::canonicalize(cwd).map_err(|e| format!("作業フォルダー {}: {e}", cwd.display()))?;
    // Match Node realpath / Codex discovery spelling, not Rust's Windows
    // verbatim prefix (which would produce a different hook command).
    let root = if cfg!(windows) {
        let text = root.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") { PathBuf::from(format!(r"\\{unc}")) }
        else { PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)) }
    } else { root };
    if !root.is_dir() || !crate::console_safe(&root.to_string_lossy()) || root.to_string_lossy().contains(['%', '\'']) {
        return Err("作業フォルダーを hook のコマンドへ安全に渡せません。".into());
    }
    let dot = root.join(".codex");
    for dir in [&dot, &dot.join("hooks"), &dot.join("output-styles")] {
        if let Ok(meta) = fs::symlink_metadata(dir) {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(format!("{}: 通常のディレクトリではありません。", dir.display()));
            }
        }
    }
    let helper = dot.join("hooks/codex-output-style.py");
    let old_helper = regular(&helper)?;
    if old_helper.as_deref().is_some_and(|bytes| std::str::from_utf8(bytes).map(|s| s.replace("\r\n", "\n")) != Ok(LOADER.replace("\r\n", "\n"))) {
        return Err(format!("{}: 同梱版と異なる読み込みスクリプトです。既存ファイルを保つため設置を中止しました。", helper.display()));
    }
    let config = dot.join("config.toml");
    let old_config = regular(&config)?;
    let text = std::str::from_utf8(old_config.as_deref().unwrap_or_default()).map_err(|e| format!("{}: {e}", config.display()))?;
    let mut doc = text.trim_start_matches('\u{feff}').parse::<Document>().map_err(|e| format!("{}: {e}", config.display()))?;
    if doc.get("liplus").is_some_and(|v| !v.is_table_like()) {
        return Err(format!("{}: liplus はテーブルである必要があります。", config.display()));
    }
    if let Some(selector) = doc.get("liplus").and_then(|t| t.get("output_style")) {
        if selector.as_bool() != Some(false) && !selector.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128 && s.as_bytes()[0].is_ascii_alphanumeric() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
            return Err(format!("{}: liplus.output_style が不正です。", config.display()));
        }
    } else if !dot.join("output-styles").exists() {
        // Installing the shared directory must not opt previously characterless
        // sibling seats into the loader's implicit character_instance fallback.
        doc["liplus"]["output_style"] = value(false);
    }
    let hooks_path = dot.join("hooks.json");
    let old_hooks = regular(&hooks_path)?;
    let mut hooks: Value = match &old_hooks {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| format!("{}: {e}", hooks_path.display()))?,
        None => json!({}),
    };
    let object = hooks.as_object_mut().ok_or("hooks.json: ルートは object である必要があります。")?;
    let events = object.entry("hooks").or_insert_with(|| json!({})).as_object_mut().ok_or("hooks.json: hooks は object である必要があります。")?;
    let starts = events.entry("SessionStart").or_insert_with(|| json!([])).as_array_mut().ok_or("hooks.json: SessionStart は配列である必要があります。")?;
    if starts.iter().any(|event| !event.is_object() || !event.get("hooks").is_some_and(Value::is_array)) {
        return Err("hooks.json: SessionStart の各登録には hooks 配列が必要です。".into());
    }
    let path = root.to_string_lossy().replace('\\', "/");
    let handler = json!({"matcher":"startup|resume|clear|compact", "hooks":[{
        "type":"command", "command":format!("python3 \"{path}/.codex/hooks/codex-output-style.py\" hook --root \"{path}\""),
        "commandWindows":format!("python \"{path}/.codex/hooks/codex-output-style.py\" hook --root \"{path}\""),
        "async":false, "additionalContextLimit":0
    }]});
    let existing: Vec<_> = starts.iter().flat_map(|event| event["hooks"].as_array().into_iter().flatten().map(move |hook| (event, hook)))
        .filter(|(_, hook)| hook.to_string().contains("codex-output-style.py")).map(|(event, hook)| (event.clone(), hook.clone())).collect();
    if existing.is_empty() { if !home_scope { starts.push(handler.clone()); } }
    else if existing.len() != 1 || {
        let (event, hook) = &existing[0];
        event["matcher"] != handler["matcher"] || hook["type"] != "command" ||
        hook["command"] != handler["hooks"][0]["command"] || hook["commandWindows"] != handler["hooks"][0]["commandWindows"] ||
        hook.get("async").is_some_and(|v| v != false) || hook["additionalContextLimit"] != 0
    } {
        return Err(format!("{}: 既存の専用 SessionStart 登録が同梱版と異なります。登録を保つため中止しました。", hooks_path.display()));
    }
    if home_scope && !existing.is_empty() {
        // Only migrate the exact handler installed by Pullcept #560. Additional
        // metadata belongs to its author and cannot be inferred to be ours.
        if old_helper.is_none() || starts.iter().filter(|event| event.to_string().contains("codex-output-style.py")).any(|event| event != &handler) {
            return Err("ホームの既存専用 hook は Pullcept 同梱登録と完全一致しません。移行を中止しました。".into());
        }
        starts.retain(|event| event != &handler);
    }
    let config_bytes = if doc.to_string() == text.trim_start_matches('\u{feff}') { old_config.clone().unwrap_or_default() } else { doc.to_string().into_bytes() };
    let hook_bytes = if old_hooks.is_some() && serde_json::from_slice::<Value>(old_hooks.as_ref().unwrap()).ok().as_ref() == Some(&hooks) { old_hooks.clone().unwrap() } else { serde_json::to_vec_pretty(&hooks).unwrap() };
    let helper_bytes = old_helper.clone().unwrap_or_else(|| LOADER.as_bytes().to_vec());
    let mut plan = vec![(helper, old_helper, helper_bytes), (config, old_config, config_bytes)];
    if !home_scope || old_hooks.is_some() { plan.push((hooks_path, old_hooks, hook_bytes)); }
    fs::create_dir_all(dot.join("hooks")).map_err(|e| format!("{}: {e}", dot.display()))?;
    let lock_path = dot.join("pullcept-style-setup.lock");
    let _lock = fs::OpenOptions::new().write(true).create_new(true).open(&lock_path).map_err(|e| format!("{}: 設置ロックを取得できません: {e}", lock_path.display()))?;
    let result = (|| {
        let mut written: Vec<(&PathBuf, &Option<Vec<u8>>)> = Vec::new();
        for (path, before, after) in &plan {
            if before.as_ref() == Some(after) { continue; }
            let attempt = (|| {
                if regular(path)? != *before { return Err(format!("{}: 設置中にファイルが変更されました。", path.display())); }
                let mut file = fs::OpenOptions::new().write(true).create_new(before.is_none()).truncate(before.is_some()).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                written.push((path, before));
                file.write_all(after).map_err(|e| format!("{}: {e}", path.display()))
            })();
            if let Err(mut error) = attempt {
                for (path, before) in written.into_iter().rev() {
                    let rollback = if let Some(bytes) = before { fs::write(path, bytes) } else { fs::remove_file(path) };
                    if let Err(e) = rollback { error.push_str(&format!(" 復旧失敗 {}: {e}", path.display())); }
                }
                return Err(error);
            }
        }
        Ok(())
    })();
    drop(_lock);
    let _ = fs::remove_file(lock_path);
    result
}
