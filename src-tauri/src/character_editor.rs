//! The account form's character body (#100): the commands the form reads and
//! writes the file through. Where the file is, what a name may be, and how it is
//! read and written are `crates/character-file`'s; this is only the boundary.

use character_file::{Cli, Opened, Saved, Wearer};
use serde::Deserialize;

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
