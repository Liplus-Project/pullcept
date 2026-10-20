//! The account form's character body (#100): the commands the form reads and
//! writes the file through. Where the file is, what a name may be, and how it is
//! read and written are `crates/character-file`'s; this is only the boundary.

use character_file::{Cli, Opened, Saved, Wearer};
use serde::Deserialize;

/// Explicit on-save: shared assets plus native folder/own-hook trust, model-free.
#[tauri::command]
pub async fn setup_codex_output_style(cwd: String, args: Vec<String>, command: String, env: Vec<account_env::EnvVar>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || setup_style(&cwd, &args, &command, &env)).await.map_err(|e| e.to_string())?
}

fn setup_style(cwd: &str, args: &[String], command: &str, sealed: &[account_env::EnvVar]) -> Result<(), String> {
    let base = std::path::Path::new(&cwd);
    let effective = mcp_config::codex::effective_cwd(base, &args)?;
    let canonical = |p: &std::path::Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    if canonical(base)? != canonical(&effective)? {
        return Err("起動オプションの --cd / -C と本文の保存先が異なります。作業ディレクトリ欄を実際のフォルダーに合わせ、--cd / -C を外してから保存してください。".into());
    }
    use mcp_config::codex::style_setup;
    let env = account_env::open_all(sealed).map_err(|_| "環境変数を読めません。")?;
    let home = style_setup::codex_home(base, &env)?;
    let home_scope = style_setup::home_scope(&effective, &home);
    style_setup::install_scoped(&effective, home_scope)?;
    let mut options = style_setup::scoped_options(args, &effective, &home)?;
    options = mcp_config::codex::discovery_options(&options, &home)?;
    options.extend(["-c".into(), "features.codex_hooks=true".into()]);
    let (entry, _) = crate::session::resolve_sidecar_paths()?;
    let mut child = std::process::Command::new("node");
    child.arg(entry.parent().unwrap().join("codex-style-setup.mjs"))
        .arg(command).arg(base).arg(serde_json::to_string(&options).unwrap())
        .arg(canonical(&effective)?.to_string_lossy().trim_start_matches(r"\\?\"))
        .arg(style_setup::installed_hash(&effective)?)
        .arg(if home_scope { "home" } else { "project" })
        .envs(env).env("CODEX_HOME", home).env_remove(mcp_config::codex::OUTPUT_STYLE_ENV)
        .current_dir(base).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        child.creation_flags(0x08000000);
    }
    let output = child.output().map_err(|_| "Codex の承認設定を起動できません。node と CLI コマンドを確認してください。")?;
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| "Codex の承認設定が失敗しました。")?;
    if !output.status.success() || reply["approved"] != true {
        return Err(reply["error"].as_str().unwrap_or("Codex の承認設定が失敗しました。").to_string());
    }
    Ok(())
}

fn cli(kind: &str) -> Result<Cli, String> {
    Cli::of_kind(kind).ok_or_else(|| "この種別はキャラクターのファイルを持ちません。".to_string())
}

/// The file the three fields name, as the form shows it. A file that is not
/// there opens empty.
#[tauri::command]
pub fn open_character_file(kind: String, cwd: String, name: String) -> Result<Opened, String> {
    character_file::open(cli(&kind)?, &cwd, name.trim())
}

/// Write the body into that file, unless it changed since `stamp` was read.
#[tauri::command]
pub fn save_character_file(
    kind: String,
    cwd: String,
    name: String,
    body: String,
    crlf: bool,
    bom: bool,
    stamp: Option<String>,
) -> Result<Saved, String> {
    character_file::save(
        cli(&kind)?,
        &cwd,
        name.trim(),
        &body,
        crlf,
        bom,
        stamp.as_deref(),
    )
}

/// One other account as the form sends it.
#[derive(Deserialize)]
pub struct Other {
    name: String,
    kind: String,
    cwd: Option<String>,
    character: Option<String>,
}

/// The names of the `others` whose character is the same file, said before a
/// save that would change theirs too.
#[tauri::command]
pub fn character_file_wearers(
    kind: String,
    cwd: String,
    name: String,
    others: Vec<Other>,
) -> Result<Vec<String>, String> {
    let path = character_file::locate(cli(&kind)?, &cwd, name.trim())?;
    let others: Vec<Wearer> = others
        .into_iter()
        .map(|other| Wearer {
            name: other.name,
            kind: other.kind,
            cwd: other.cwd.unwrap_or_default(),
            character: other.character.unwrap_or_default(),
        })
        .collect();
    Ok(character_file::wearers(&path, &others))
}
