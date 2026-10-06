//! The character file an account wears, edited from the account form (#100).
//!
//! Its place is settled by three of the account's fields and nothing else:
//! `<working directory>/<.claude|.codex>/output-styles/<character>.md`, the
//! folder by the kind. The form shows that file, and 決定 writes what was typed
//! back into it — creating the folder and the file when there are none.
//!
//! A path built from what a person typed is a path that can point anywhere, so
//! everything here refuses before it touches the disk: a name that is not one
//! plain file stem, a working directory that is not a directory, and a place
//! that resolves — through a junction, a link or `..` — outside that directory.
//! The file is a person's own writing, so a save that would overwrite a change
//! made since it was opened stops instead.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// The largest file this reads or writes: the Codex helper's own bound
/// (`LIMIT = 128 * 1024` in Li+ `scripts/codex_output_style.py`), so a file
/// saved here is never one that seat then refuses for its size.
pub const LIMIT: usize = 128 * 1024;

/// The CLIs whose characters are files. `CLI（汎用）`, `admin` and `mcp` have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    /// The CLI an account kind launches, by the kind's stored spelling.
    pub fn of_kind(kind: &str) -> Option<Cli> {
        match kind {
            "claude_code" => Some(Cli::Claude),
            "codex_cli" => Some(Cli::Codex),
            _ => None,
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Cli::Claude => ".claude",
            Cli::Codex => ".codex",
        }
    }
}

/// Whether `name` may be the stem of the file.
///
/// Codex's is its selector's grammar (docs/3-accounts.md, Codex のキャラ選択),
/// so a name the helper would refuse is refused here first. Claude's is any
/// one file name Windows can hold: no separator, no `..`, nothing the file
/// system reserves.
pub fn check_name(cli: Cli, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("キャラクター名が空です。".into());
    }
    if name.trim() != name {
        return Err("キャラクター名の前後に空白があります。".into());
    }
    if name.chars().count() > 128 {
        return Err("キャラクター名は128文字までです。".into());
    }
    if cli == Cli::Codex {
        let mut chars = name.chars();
        let first = chars.next().unwrap_or(' ');
        if !first.is_ascii_alphanumeric()
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(
                "Codex のキャラクター名は、ASCII の英数字で始まる英数字・_・- で書いてください。"
                    .into(),
            );
        }
        return Ok(());
    }
    if name == "." || name == ".." {
        return Err("キャラクター名に . や .. は使えません。".into());
    }
    if name
        .chars()
        .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return Err("キャラクター名に / \\ : * ? \" < > | や制御文字は使えません。".into());
    }
    if name.ends_with('.') {
        return Err("キャラクター名の末尾に . は使えません。".into());
    }
    let device = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&device.as_str())
        || ((device.starts_with("COM") || device.starts_with("LPT"))
            && device.len() == 4
            && device.as_bytes()[3].is_ascii_digit()
            && device.as_bytes()[3] != b'0');
    if reserved {
        return Err(format!("{name} は Windows が予約している名前です。"));
    }
    Ok(())
}

/// The file's place, from the three fields. Touches the disk only to see that
/// the working directory is one.
pub fn locate(cli: Cli, cwd: &str, name: &str) -> Result<PathBuf, String> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return Err("作業ディレクトリが空です。".into());
    }
    let dir = Path::new(cwd);
    if !dir.is_absolute() {
        return Err("作業ディレクトリは絶対パスで書いてください。".into());
    }
    if !dir.is_dir() {
        return Err(format!("作業ディレクトリがありません: {cwd}"));
    }
    check_name(cli, name)?;
    Ok(dir
        .join(cli.folder())
        .join("output-styles")
        .join(format!("{name}.md")))
}

/// Refuse a `path` whose nearest part that exists resolves outside `root`
/// (itself resolved). What does not exist yet is made below that part by this
/// crate, as plain folders, so it cannot lead out.
fn contained(root: &Path, path: &Path) -> Result<(), String> {
    let mut probe = path;
    loop {
        if probe.symlink_metadata().is_ok() {
            let real = fs::canonicalize(probe)
                .map_err(|_| format!("場所を確かめられません: {}", probe.display()))?;
            if !real.starts_with(root) {
                return Err(format!(
                    "キャラクターのファイルが作業ディレクトリの外を指しています: {}",
                    real.display()
                ));
            }
            return Ok(());
        }
        probe = probe
            .parent()
            .ok_or_else(|| "作業ディレクトリを確かめられません。".to_string())?;
    }
}

fn root_of(cwd: &str) -> Result<PathBuf, String> {
    fs::canonicalize(cwd.trim()).map_err(|_| format!("作業ディレクトリを確かめられません: {cwd}"))
}

/// What changed underneath an open file is told apart by this: the length and
/// an FNV-1a of the bytes. Not a defence against anyone, only a fingerprint.
pub fn stamp(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{}:{hash:016x}", bytes.len())
}

/// The file as the form shows it.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Opened {
    /// Where it is, as the three fields put it.
    pub path: String,
    pub exists: bool,
    /// Whether `output-styles` is there. A Codex project that has none is on
    /// the inline selection, and the folder appearing moves every Codex seat in
    /// that directory to file mode (docs/3-accounts.md, Codex のキャラ選択).
    pub folder_exists: bool,
    /// The text with its line ends as `\n`, as a text field holds it.
    pub body: String,
    /// The file's line ends were CRLF; written back that way.
    pub crlf: bool,
    /// The file began with a UTF-8 BOM; written back with it.
    pub bom: bool,
    /// The frontmatter's `name:`, when there is one. Claude selects a style by
    /// it and Codex refuses a file whose name differs from its stem.
    pub name_in_file: Option<String>,
    /// The fingerprint of the bytes read, or none for a file not there.
    pub stamp: Option<String>,
}

fn decode(bytes: &[u8]) -> Result<(String, bool, bool), String> {
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let rest = if bom { &bytes[3..] } else { bytes };
    let text = std::str::from_utf8(rest)
        .map_err(|_| "キャラクターのファイルを UTF-8 として読めません。".to_string())?;
    let crlf = text.contains("\r\n");
    let body = if crlf {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    };
    Ok((body, crlf, bom))
}

/// The bytes `body` is written as: its line ends as the file had them, and the
/// BOM if it had one.
pub fn encode(body: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let plain = body.replace("\r\n", "\n");
    let text = if crlf {
        plain.replace('\n', "\r\n")
    } else {
        plain
    };
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// The frontmatter's `name:` value, quotes taken off.
pub fn front_name(body: &str) -> Option<String> {
    let mut lines = body.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim_end();
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            return Some(unquoted.to_string());
        }
    }
    None
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::metadata(path) {
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(format!(
                "キャラクターのファイルを読めません: {}",
                path.display()
            ))
        }
        Ok(meta) if !meta.is_file() => {
            return Err(format!("ファイルではありません: {}", path.display()))
        }
        Ok(meta) if meta.len() > LIMIT as u64 => {
            return Err("キャラクターのファイルが128 KiBを超えています。".into())
        }
        Ok(_) => {}
    }
    fs::read(path)
        .map(Some)
        .map_err(|_| format!("キャラクターのファイルを読めません: {}", path.display()))
}

/// Read the file the three fields name. One that is not there opens empty: it
/// is made by the first save.
pub fn open(cli: Cli, cwd: &str, name: &str) -> Result<Opened, String> {
    let path = locate(cli, cwd, name)?;
    let root = root_of(cwd)?;
    contained(&root, &path)?;
    let folder_exists = path.parent().is_some_and(Path::is_dir);
    let shown = path.display().to_string();
    let Some(bytes) = read_bounded(&path)? else {
        return Ok(Opened {
            path: shown,
            exists: false,
            folder_exists,
            body: String::new(),
            crlf: false,
            bom: false,
            name_in_file: None,
            stamp: None,
        });
    };
    let (body, crlf, bom) = decode(&bytes)?;
    Ok(Opened {
        path: shown,
        exists: true,
        folder_exists,
        name_in_file: front_name(&body),
        body,
        crlf,
        bom,
        stamp: Some(stamp(&bytes)),
    })
}

/// What a save did.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Saved {
    /// Whether anything was written. A body equal to the file's is not.
    pub written: bool,
    /// The file as it now is.
    pub opened: Opened,
}

/// Write `body` into the file the three fields name.
///
/// `expected` is the stamp the form opened (none for a file that was not
/// there); a file that no longer matches it is left as it is. A file already
/// holding these bytes is not written, and an empty body makes no new file.
///
/// An existing file is written in place, through whatever link points at it,
/// so a style one CLI's folder links to the other's (Li+ `Li+update.md`, the
/// Codex output-style migration) stays one file.
pub fn save(
    cli: Cli,
    cwd: &str,
    name: &str,
    body: &str,
    crlf: bool,
    bom: bool,
    expected: Option<&str>,
) -> Result<Saved, String> {
    let path = locate(cli, cwd, name)?;
    let root = root_of(cwd)?;
    contained(&root, &path)?;
    let bytes = encode(body, crlf, bom);
    if bytes.len() > LIMIT {
        return Err("キャラクターの本文が128 KiBを超えています。".into());
    }
    let current = read_bounded(&path)?;
    if current.as_deref().map(stamp).as_deref() != expected {
        return Err(
            "キャラクターのファイルが、開いた後に変わっています。読み直してから保存してください。"
                .into(),
        );
    }
    let unchanged = match &current {
        Some(now) => *now == bytes,
        None => bytes.is_empty(),
    };
    if unchanged {
        return Ok(Saved {
            written: false,
            opened: open(cli, cwd, name)?,
        });
    }
    let written_to = if current.is_some() {
        let real = fs::canonicalize(&path)
            .map_err(|_| format!("場所を確かめられません: {}", path.display()))?;
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&real)
            .map_err(|err| format!("キャラクターのファイルを書けません: {err}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|err| format!("キャラクターのファイルを書けません: {err}"))?;
        real
    } else {
        let folder = path
            .parent()
            .ok_or_else(|| "キャラクターのフォルダーを確かめられません。".to_string())?;
        fs::create_dir_all(folder)
            .map_err(|err| format!("output-styles のフォルダーを作れません: {err}"))?;
        // Again, now that it exists: what was made must be where it was meant.
        contained(&root, folder)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|err| format!("キャラクターのファイルを作れません: {err}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|err| format!("キャラクターのファイルを書けません: {err}"))?;
        path.clone()
    };
    if fs::read(&written_to).ok().as_deref() != Some(bytes.as_slice()) {
        return Err("書いた内容を読み戻せませんでした。".into());
    }
    Ok(Saved {
        written: true,
        opened: open(cli, cwd, name)?,
    })
}

/// One other account, as far as where its character file is goes.
#[derive(Debug, Clone)]
pub struct Wearer {
    pub name: String,
    pub kind: String,
    pub cwd: String,
    pub character: String,
}

fn same_place(a: &Path, b: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (fs::canonicalize(a), fs::canonicalize(b)) {
        return a == b;
    }
    let plain = |p: &Path| {
        let text: PathBuf = p.components().collect();
        let text = text.display().to_string();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    plain(a) == plain(b)
}

/// The names of the `others` whose character field names the same file as
/// `path` — a save there changes their character too. An account whose field
/// is blank wears its directory's default, which this does not resolve.
pub fn wearers(path: &Path, others: &[Wearer]) -> Vec<String> {
    others
        .iter()
        .filter(|other| {
            let Some(cli) = Cli::of_kind(&other.kind) else {
                return false;
            };
            let character = other.character.trim();
            !character.is_empty()
                && locate(cli, &other.cwd, character)
                    .map(|theirs| same_place(path, &theirs))
                    .unwrap_or(false)
        })
        .map(|other| other.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pullcept-character-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn text(dir: &Path) -> String {
        dir.display().to_string()
    }

    #[test]
    fn kinds_without_a_character_file_have_none() {
        assert_eq!(Cli::of_kind("claude_code"), Some(Cli::Claude));
        assert_eq!(Cli::of_kind("codex_cli"), Some(Cli::Codex));
        for kind in ["cli", "admin", "mcp", ""] {
            assert_eq!(Cli::of_kind(kind), None, "{kind}");
        }
    }

    #[test]
    fn the_place_is_the_three_fields() {
        let dir = scratch();
        assert_eq!(
            locate(Cli::Claude, &text(&dir), "character_Lin").unwrap(),
            dir.join(".claude")
                .join("output-styles")
                .join("character_Lin.md")
        );
        assert_eq!(
            locate(
                Cli::Codex,
                &format!("  {}  ", text(&dir)),
                "character_codex_luna"
            )
            .unwrap(),
            dir.join(".codex")
                .join("output-styles")
                .join("character_codex_luna.md")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_working_directory_that_is_not_one_is_refused() {
        let dir = scratch();
        assert!(locate(Cli::Claude, "", "a").is_err());
        assert!(locate(Cli::Claude, "relative\\dir", "a").is_err());
        assert!(locate(Cli::Claude, &text(&dir.join("missing")), "a").is_err());
        fs::write(dir.join("file"), b"x").unwrap();
        assert!(locate(Cli::Claude, &text(&dir.join("file")), "a").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_name_is_one_plain_stem() {
        for bad in [
            "", " a", "a ", ".", "..", "../x", "..\\x", "a/b", "a\\b", "c:x", "a*", "a?", "a\"",
            "a<", "a>", "a|", "a\n", "a.", "CON", "nul", "Com1", "lpt9.md",
        ] {
            assert!(check_name(Cli::Claude, bad).is_err(), "{bad:?}");
        }
        for good in ["character_Lin", "キャラ", "a.b", "COM0", "CONSOLE", "x-y z"] {
            assert!(check_name(Cli::Claude, good).is_ok(), "{good:?}");
        }
        assert!(check_name(Cli::Claude, &"a".repeat(128)).is_ok());
        assert!(check_name(Cli::Claude, &"a".repeat(129)).is_err());
    }

    #[test]
    fn a_codex_name_is_its_selector_grammar() {
        for bad in ["_a", "-a", "a.b", "a b", "キャラ", "a/b", ".."] {
            assert!(check_name(Cli::Codex, bad).is_err(), "{bad:?}");
        }
        for good in ["character_codex_luna", "A", "0-x_Y"] {
            assert!(check_name(Cli::Codex, good).is_ok(), "{good:?}");
        }
        assert!(check_name(Cli::Codex, &"a".repeat(129)).is_err());
    }

    #[test]
    fn a_missing_file_opens_empty_and_the_first_save_makes_folder_and_file() {
        let dir = scratch();
        let cwd = text(&dir);
        let opened = open(Cli::Claude, &cwd, "new_one").unwrap();
        assert!(!opened.exists);
        assert!(!opened.folder_exists);
        assert_eq!(opened.body, "");
        assert_eq!(opened.stamp, None);
        // Nothing was made by opening.
        assert!(!dir.join(".claude").exists());

        let saved = save(
            Cli::Claude,
            &cwd,
            "new_one",
            "line 1\nline 2\n",
            false,
            false,
            None,
        )
        .unwrap();
        assert!(saved.written);
        assert!(saved.opened.exists && saved.opened.folder_exists);
        let path = dir.join(".claude/output-styles/new_one.md");
        assert_eq!(fs::read(&path).unwrap(), b"line 1\nline 2\n");
        assert_eq!(saved.opened.stamp, Some(stamp(b"line 1\nline 2\n")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_empty_body_makes_no_new_file() {
        let dir = scratch();
        let saved = save(Cli::Codex, &text(&dir), "x", "", false, false, None).unwrap();
        assert!(!saved.written);
        assert!(!dir.join(".codex").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn line_ends_and_bom_are_kept_as_the_file_had_them() {
        let dir = scratch();
        let cwd = text(&dir);
        let folder = dir.join(".codex/output-styles");
        fs::create_dir_all(&folder).unwrap();
        let original = b"\xEF\xBB\xBF---\r\nname: luna\r\n---\r\nhello\r\n";
        fs::write(folder.join("luna.md"), original).unwrap();

        let opened = open(Cli::Codex, &cwd, "luna").unwrap();
        assert!(opened.crlf && opened.bom);
        assert_eq!(opened.body, "---\nname: luna\n---\nhello\n");
        assert_eq!(opened.name_in_file.as_deref(), Some("luna"));

        let body = format!("{}bye\n", opened.body);
        let saved = save(
            Cli::Codex,
            &cwd,
            "luna",
            &body,
            true,
            true,
            opened.stamp.as_deref(),
        )
        .unwrap();
        assert!(saved.written);
        assert_eq!(
            fs::read(folder.join("luna.md")).unwrap(),
            b"\xEF\xBB\xBF---\r\nname: luna\r\n---\r\nhello\r\nbye\r\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unchanged_body_is_not_written() {
        let dir = scratch();
        let cwd = text(&dir);
        let folder = dir.join(".claude/output-styles");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.md"), b"same\r\n").unwrap();
        let opened = open(Cli::Claude, &cwd, "a").unwrap();
        let saved = save(
            Cli::Claude,
            &cwd,
            "a",
            &opened.body,
            opened.crlf,
            opened.bom,
            opened.stamp.as_deref(),
        )
        .unwrap();
        assert!(!saved.written);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_changed_since_it_was_opened_is_not_overwritten() {
        let dir = scratch();
        let cwd = text(&dir);
        let folder = dir.join(".claude/output-styles");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("a.md");
        fs::write(&file, b"first").unwrap();
        let opened = open(Cli::Claude, &cwd, "a").unwrap();
        fs::write(&file, b"someone else").unwrap();
        assert!(save(
            Cli::Claude,
            &cwd,
            "a",
            "mine",
            false,
            false,
            opened.stamp.as_deref()
        )
        .is_err());
        assert_eq!(fs::read(&file).unwrap(), b"someone else");

        // A file that appeared after an empty open is not overwritten either.
        let fresh = open(Cli::Claude, &cwd, "b").unwrap();
        fs::write(folder.join("b.md"), b"theirs").unwrap();
        assert!(save(
            Cli::Claude,
            &cwd,
            "b",
            "mine",
            false,
            false,
            fresh.stamp.as_deref()
        )
        .is_err());
        assert_eq!(fs::read(folder.join("b.md")).unwrap(), b"theirs");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_size_limit_holds_both_ways() {
        let dir = scratch();
        let cwd = text(&dir);
        let big = "x".repeat(LIMIT + 1);
        assert!(save(Cli::Claude, &cwd, "big", &big, false, false, None).is_err());
        assert!(!dir.join(".claude").exists());
        // CRLF counts: LIMIT `\n` become twice as many bytes.
        let lines = "\n".repeat(LIMIT / 2 + 1);
        assert!(save(Cli::Claude, &cwd, "big", &lines, true, false, None).is_err());
        let folder = dir.join(".claude/output-styles");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("big.md"), big.as_bytes()).unwrap();
        assert!(open(Cli::Claude, &cwd, "big").is_err());
        assert!(
            save(
                Cli::Claude,
                &cwd,
                "edge",
                &"y".repeat(LIMIT),
                false,
                false,
                None
            )
            .unwrap()
            .written
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_utf8_is_not_opened() {
        let dir = scratch();
        let folder = dir.join(".claude/output-styles");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("a.md"), b"\x82\xa0").unwrap();
        assert!(open(Cli::Claude, &text(&dir), "a").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_that_leads_outside_the_working_directory_is_refused() {
        let outside = scratch();
        let dir = scratch();
        let cwd = text(&dir);
        assert!(junction(&dir.join(".claude"), &outside));
        assert!(open(Cli::Claude, &cwd, "a").is_err());
        assert!(save(Cli::Claude, &cwd, "a", "body", false, false, None).is_err());
        assert!(!outside.join("output-styles").exists());
        fs::remove_dir(dir.join(".claude")).unwrap();
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_linked_inside_the_working_directory_is_written_through() {
        let dir = scratch();
        let cwd = text(&dir);
        let shared = dir.join("shared-styles");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(dir.join(".claude")).unwrap();
        assert!(junction(
            &dir.join(".claude").join("output-styles"),
            &shared
        ));
        fs::write(shared.join("a.md"), b"old").unwrap();
        let opened = open(Cli::Claude, &cwd, "a").unwrap();
        assert!(
            save(
                Cli::Claude,
                &cwd,
                "a",
                "new",
                false,
                false,
                opened.stamp.as_deref()
            )
            .unwrap()
            .written
        );
        assert_eq!(fs::read(shared.join("a.md")).unwrap(), b"new");
        fs::remove_dir(dir.join(".claude").join("output-styles")).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_frontmatter_name_is_read() {
        assert_eq!(
            front_name("---\nname: character_Lin\ndescription: x\n---\nbody").as_deref(),
            Some("character_Lin")
        );
        assert_eq!(
            front_name("---\nname: \"quoted\"\n---\n").as_deref(),
            Some("quoted")
        );
        assert_eq!(
            front_name("---\nname: 'single'\n---\n").as_deref(),
            Some("single")
        );
        assert_eq!(front_name("---\ndescription: x\n---\nname: body\n"), None);
        assert_eq!(front_name("name: not frontmatter\n"), None);
        assert_eq!(front_name(""), None);
    }

    #[test]
    fn the_accounts_wearing_the_same_file_are_named() {
        let dir = scratch();
        let other = scratch();
        let cwd = text(&dir);
        let path = locate(Cli::Claude, &cwd, "character_Lin").unwrap();
        let wearer = |name: &str, kind: &str, cwd: &str, character: &str| Wearer {
            name: name.into(),
            kind: kind.into(),
            cwd: cwd.into(),
            character: character.into(),
        };
        let upper = if cfg!(windows) {
            cwd.to_uppercase()
        } else {
            cwd.clone()
        };
        let others = [
            wearer("Lay", "claude_code", &cwd, "character_Lin"),
            wearer(
                "Same dir other case",
                "claude_code",
                &format!("{upper}\\"),
                "character_Lin",
            ),
            wearer("Other style", "claude_code", &cwd, "character_Lay"),
            wearer("Codex", "codex_cli", &cwd, "character_Lin"),
            wearer("Other dir", "claude_code", &text(&other), "character_Lin"),
            wearer("Blank", "claude_code", &cwd, ""),
            wearer("Generic", "cli", &cwd, "character_Lin"),
            wearer("Human", "admin", &cwd, "character_Lin"),
        ];
        let named = wearers(&path, &others);
        if cfg!(windows) {
            assert_eq!(named, ["Lay", "Same dir other case"]);
        } else {
            assert_eq!(named, ["Lay"]);
        }
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(other).unwrap();
    }
}
