//! The environment an account's CLI is launched with (#163).
//!
//! One field on the account, written as `NAME=value` one per line. Every value
//! is sealed before it is stored, and no value is ever drawn whole: the screen
//! shows its head and tail (`GH_TOKEN=github_pat...Nx4h`), the launch line and
//! its preview never carry it, and it reaches the CLI only as a variable on the
//! child's environment. Ordinary values and secret ones are not told apart —
//! one field, one treatment (#163, Master's decision).
//!
//! Sealing is Windows DPAPI (`CryptProtectData`) in the current user's scope:
//! the key is tied to the signed-in Windows user, which is the mechanism
//! Claude Desktop's own store (Electron `safeStorage`) rests on. The Windows
//! Credential Manager is not used (#163, Master's decision).
//!
//! **Off Windows there is no sealing, and nothing is stored instead.** The app
//! is built for Windows; a platform without DPAPI refuses to save a value
//! rather than writing it down in the clear, and a sealed value read there
//! refuses to open. A fallback that stored plaintext would be the one path on
//! which the promise in the first paragraph quietly stops holding.
//!
//! This crate holds no tauri, for the reason `docs/0-requirements.md` gives
//! under テストの配置.

use serde::{Deserialize, Serialize};

/// One variable as the config stores it: the name in the clear, the value
/// sealed.
///
/// The name stays readable because it is not the secret — it is what the
/// person reads to know which line is which — and because the screen draws it
/// whole next to the masked value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    /// `SEALED_PREFIX` followed by the DPAPI blob in lowercase hex.
    pub sealed: String,
}

/// What a sealed value starts with. Names the scheme, so a value sealed some
/// other way later is told apart from this one rather than fed to the wrong
/// decryption.
pub const SEALED_PREFIX: &str = "dpapi:";

/// Drawn in place of a value that could not be opened — sealed under another
/// Windows user or on another machine, or damaged in the file.
///
/// Drawn rather than refused, so the form still opens and the line can be
/// deleted or replaced. Left as it is, the line keeps the sealed value it had
/// (`settle`); the launch is what refuses it (`open_all`).
pub const UNREADABLE: &str = "（復号できません）";

/// How many characters of the head and tail a mask shows, at most.
const HEAD: usize = 10;
const TAIL: usize = 4;

/// Parse the field into `(name, value)` pairs, in the order written.
///
/// A blank line is skipped. Every other line is `NAME=value`, split at the
/// first `=`: a value may itself contain `=`, and a name may not (Windows
/// cannot hold one). The name and the value are both trimmed — a token pasted
/// with a trailing space is the mistake, not a value anyone meant.
///
/// Refused, naming the line: a line with no `=`, an empty name, a name with
/// whitespace or NUL in it, a value with NUL in it, and a name written twice.
/// Names are compared ignoring case, because Windows reads them that way and
/// the second of two spellings would silently be the one that won.
///
/// The value comes back as written: surrounding quotes are not taken off
/// here. `settle` takes them off, after it has matched a line left alone
/// against its mask — a mask drawn from a value stored with a quote in it
/// carries that quote, and must still read back as untouched.
pub fn parse(text: &str) -> Result<Vec<(String, String)>, String> {
    Ok(parse_lines(text)?
        .into_iter()
        .map(|(_, name, value)| (name, value))
        .collect())
}

/// `parse`, keeping each variable's line number for the errors `settle` names.
fn parse_lines(text: &str) -> Result<Vec<(usize, String, String)>, String> {
    let mut out: Vec<(usize, String, String)> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!(
                "環境変数の {line_no} 行目に `=` がありません。`名前=値` の形で書いてください。"
            ));
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            return Err(format!("環境変数の {line_no} 行目に名前がありません。"));
        }
        if name.chars().any(|c| c.is_whitespace() || c == '\0') {
            return Err(format!(
                "環境変数の {line_no} 行目の名前 `{name}` に空白が含まれています。"
            ));
        }
        if value.contains('\0') {
            return Err(format!(
                "環境変数 `{name}` の値に使えない文字（NUL）が含まれています。"
            ));
        }
        if out.iter().any(|(_, seen, _)| seen.eq_ignore_ascii_case(name)) {
            return Err(format!(
                "環境変数 `{name}` が二度書かれています。一つにしてください。"
            ));
        }
        out.push((line_no, name.to_string(), value.to_string()));
    }
    Ok(out)
}

/// The value as the screen draws it: head, `...`, tail.
///
/// At most `HEAD` characters of the head and `TAIL` of the tail, and never
/// more than a quarter of the value at either end — so at least half of any
/// value stays hidden, however short it is. A value of fewer than four
/// characters shows nothing but `...`. Not told apart by whether the value is
/// a secret, since the field does not tell them apart (#163).
///
/// Counted in characters, not bytes, so a value is never cut inside one.
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let head = HEAD.min(n / 4);
    let tail = TAIL.min(n / 4);
    let mut out: String = chars[..head].iter().collect();
    out.push_str("...");
    out.extend(chars[n - tail..].iter());
    out
}

/// What the screen shows for one stored variable's value: its mask, or
/// `UNREADABLE` when it cannot be opened.
pub fn shown(var: &EnvVar) -> String {
    match open(&var.sealed) {
        Ok(value) => mask(&value),
        Err(_) => UNREADABLE.to_string(),
    }
}

/// The field as the form draws it: one `NAME=<shown>` line per variable.
pub fn render(vars: &[EnvVar]) -> String {
    vars.iter()
        .map(|var| format!("{}={}", var.name, shown(var)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Turn what the form holds back into stored variables.
///
/// The form is drawn by `render`, so a line the person left alone reads back
/// as the name and the mask of the value already stored. That line keeps the
/// sealed value it had — the person never saw the value and did not retype it,
/// and nothing here needs them to. Any other line is a value they wrote, and it
/// is sealed now. A variable whose line is gone is gone.
///
/// "Left alone" is exact: the same name, spelled the same, and a value equal to
/// what `shown` draws for it. Anything else was typed, including a value that
/// happens to begin and end like the old one.
///
/// A typed value is taken as written, with two exceptions (#165):
///
/// - One pair of matching quotes around it (`"..."` or `'...'`) is taken off.
///   A quote at one end only is refused, naming the line: it is a paste that
///   lost its partner, and guessing which end was meant would seal a value
///   nobody wrote. A value that really ends in a quote is written inside the
///   other kind (`"abc'"`).
/// - A value for a name already stored that is a damaged form of that name's
///   mask — the mask with a quote added or taken away, or anything that keeps
///   its head and tail around `...` — is refused, naming the line. Sealed, it
///   would replace the real value with the mask's few characters, and the real
///   value is gone for good (the incident in #165). Only names already stored
///   are checked: a new variable, or a value that merely contains `...`, is
///   an ordinary value (paths and options may hold `...`).
///
/// No value appears in any error.
///
/// `reserved` names the variables the app sets on every launch itself. Refused
/// here rather than overwritten at launch: a variable the person set that the
/// launch then quietly replaced would look set and not be.
pub fn settle(text: &str, previous: &[EnvVar], reserved: &[&str]) -> Result<Vec<EnvVar>, String> {
    let mut out = Vec::new();
    for (line_no, name, value) in parse_lines(text)? {
        if reserved.iter().any(|r| r.eq_ignore_ascii_case(&name)) {
            return Err(format!(
                "環境変数 `{name}` はアプリが起動のたびに自分で設定する名前です。別の名前にしてください。"
            ));
        }
        let kept = previous
            .iter()
            .find(|old| old.name == name && shown(old) == value);
        let sealed = match kept {
            Some(old) => old.sealed.clone(),
            None => {
                let edited_mask = previous
                    .iter()
                    .any(|old| old.name.eq_ignore_ascii_case(&name) && is_damaged_mask(&value, &shown(old)));
                if edited_mask {
                    return Err(format!(
                        "環境変数の {line_no} 行目（`{name}`）は、保存済みの値の隠し表示を書き換えたものに見えます。このまま決定すると、本当の値が隠し表示の文字列で上書きされて失われます。値を変えないなら隠し表示をそのまま残し、変えるなら本当の値を貼り直してください。"
                    ));
                }
                let value = unquote(&value).ok_or_else(|| {
                    format!(
                        "環境変数の {line_no} 行目（`{name}`）の値は、片方の端にだけ引用符があります。引用符を外すか、両端を同じ引用符で囲んでください。"
                    )
                })?;
                seal(value).map_err(|err| format!("環境変数 `{name}` を暗号化できませんでした: {err}"))?
            }
        };
        out.push(EnvVar { name, sealed });
    }
    Ok(out)
}

/// How many characters longer than a mask a value may be and still read as
/// that mask retyped (`is_damaged_mask`).
const RETYPE_SLACK: usize = 4;

const QUOTES: [char; 2] = ['"', '\''];

/// The value with one pair of matching surrounding quotes taken off, or as it
/// is when it has none. `None` when a quote stands at one end only (or the two
/// ends hold different quotes): that value is refused, not guessed at.
fn unquote(value: &str) -> Option<&str> {
    let first = value.chars().next();
    let last = value.chars().next_back();
    let opens = first.is_some_and(|c| QUOTES.contains(&c));
    let closes = last.is_some_and(|c| QUOTES.contains(&c));
    match (opens, closes) {
        (false, false) => Some(value),
        (true, true) if value.chars().count() >= 2 && first == last => Some(&value[1..value.len() - 1]),
        _ => None,
    }
}

/// Whether `value` is a damaged form of `mask`, the one `shown` draws for the
/// same name. Quotes at either end are set aside on both sides first, so a
/// quote added to or taken off the mask still reads as the mask. Beyond the
/// mask itself, anything holding `...` that begins with the mask's head and
/// ends with its tail counts, as long as it is no more than `RETYPE_SLACK`
/// characters longer than the mask — the shape of a mask retyped around the
/// ellipsis, not a new value that happens to share a short head and tail
/// (`C:\gh\...\lin` against `C:...in`). A mask that shows no head and no tail (`...`, from a short value)
/// is matched only exactly, so that such a name can still take a value
/// containing `...`.
fn is_damaged_mask(value: &str, mask: &str) -> bool {
    let value = value.trim_matches(QUOTES);
    let mask = mask.trim_matches(QUOTES);
    if mask.is_empty() {
        return false;
    }
    if value == mask {
        return true;
    }
    let Some((head, tail)) = mask.split_once("...") else {
        return false;
    };
    if head.is_empty() && tail.is_empty() {
        return false;
    }
    let (len, mask_len) = (value.chars().count(), mask.chars().count());
    value.contains("...")
        && len >= head.chars().count() + 3 + tail.chars().count()
        && len <= mask_len + RETYPE_SLACK
        && value.starts_with(head)
        && value.ends_with(tail)
}

/// Open every stored variable for a launch, in the order stored.
///
/// One that will not open fails the whole launch, naming the variable and
/// never its value. Launching without it would start a CLI missing something
/// it was declared to have — a token, a config directory — and that CLI would
/// run as somebody else's.
pub fn open_all(vars: &[EnvVar]) -> Result<Vec<(String, String)>, String> {
    vars.iter()
        .map(|var| {
            open(&var.sealed)
                .map(|value| (var.name.clone(), value))
                .map_err(|err| {
                    format!(
                        "環境変数 `{}` を復号できませんでした（{err}）。アカウントの編集で値を入れ直してください。",
                        var.name
                    )
                })
        })
        .collect()
}

/// Seal one value for storage.
pub fn seal(value: &str) -> Result<String, String> {
    let blob = dpapi::protect(value.as_bytes())?;
    Ok(format!("{SEALED_PREFIX}{}", to_hex(&blob)))
}

/// Open one stored value.
pub fn open(sealed: &str) -> Result<String, String> {
    let hex = sealed
        .strip_prefix(SEALED_PREFIX)
        .ok_or_else(|| "暗号化の形式が不明です".to_string())?;
    let blob = from_hex(hex).ok_or_else(|| "保存された値が壊れています".to_string())?;
    let plain = dpapi::unprotect(&blob)?;
    String::from_utf8(plain).map_err(|_| "保存された値が壊れています".to_string())
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(windows)]
mod dpapi {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Mixed into every seal, so a blob this app wrote does not open under a
    /// bare `CryptUnprotectData` from some other program the same user runs.
    /// Not a secret — it is in the source — only a second condition.
    const ENTROPY: &[u8] = b"pullcept/account-env/v1";

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    /// Copy DPAPI's output out and hand its buffer back to `LocalFree`, which
    /// is who owns it.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = if out.pbData.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec()
        };
        if !out.pbData.is_null() {
            LocalFree(out.pbData as _);
        }
        bytes
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(plain);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        // Current-user scope: no CRYPTPROTECT_LOCAL_MACHINE.
        let ok = unsafe {
            CryptProtectData(
                &input,
                null(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(format!(
                "CryptProtectData が失敗しました（{}）",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { take(out) })
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
        let input = blob(sealed);
        let entropy = blob(ENTROPY);
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                null_mut(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            // The OS error is safe to show: it describes the failure, and DPAPI
            // puts nothing of the value into it.
            return Err(format!(
                "CryptUnprotectData が失敗しました（{}）",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod dpapi {
    const UNSUPPORTED: &str =
        "この環境には DPAPI が無いため、環境変数を暗号化できません（Windows でのみ使えます）";

    pub fn protect(_plain: &[u8]) -> Result<Vec<u8>, String> {
        Err(UNSUPPORTED.to_string())
    }

    pub fn unprotect(_sealed: &[u8]) -> Result<Vec<u8>, String> {
        Err(UNSUPPORTED.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_name_value_lines_in_order() {
        let parsed = parse("GH_TOKEN=abc\r\n\n  LI_PLUS_AGENT_KEY = lin  \nGH_CONFIG_DIR=C:\\gh\\lin\n")
            .unwrap();
        assert_eq!(
            parsed,
            vec![
                ("GH_TOKEN".to_string(), "abc".to_string()),
                ("LI_PLUS_AGENT_KEY".to_string(), "lin".to_string()),
                ("GH_CONFIG_DIR".to_string(), "C:\\gh\\lin".to_string()),
            ]
        );
    }

    #[test]
    fn parse_splits_at_the_first_equals_only() {
        assert_eq!(
            parse("OPTS=a=b=c").unwrap(),
            vec![("OPTS".to_string(), "a=b=c".to_string())]
        );
    }

    #[test]
    fn parse_keeps_an_empty_value() {
        assert_eq!(parse("EMPTY=").unwrap(), vec![("EMPTY".to_string(), String::new())]);
    }

    #[test]
    fn parse_of_nothing_is_nothing() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("\n  \r\n").unwrap().is_empty());
    }

    #[test]
    fn parse_refuses_malformed_lines() {
        assert!(parse("GH_TOKEN").unwrap_err().contains("1 行目"));
        assert!(parse("A=1\n=value").unwrap_err().contains("2 行目"));
        assert!(parse("MY VAR=1").is_err());
        assert!(parse("A=x\0y").is_err());
    }

    #[test]
    fn parse_refuses_a_name_written_twice_in_any_case() {
        assert!(parse("GH_TOKEN=a\ngh_token=b").unwrap_err().contains("二度"));
    }

    #[test]
    fn mask_shows_ten_head_and_four_tail_of_a_long_value() {
        let token = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789Nx4h";
        assert_eq!(mask(token), "github_pat...Nx4h");
    }

    #[test]
    fn mask_never_shows_more_than_half_of_a_short_value() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("lin"), "...");
        assert_eq!(mask("abcd"), "a...d");
        assert_eq!(mask("abcdefgh"), "ab...gh");
        assert_eq!(mask("0123456789abcdef"), "0123...cdef");
        for value in ["x", "xy", "secret", "hunter22", "correct-horse"] {
            let shown = mask(value).replace("...", "");
            assert!(shown.chars().count() * 2 <= value.chars().count(), "{value}");
        }
    }

    #[test]
    fn mask_counts_characters_not_bytes() {
        assert_eq!(mask("あいうえおかきくけこさしすせそた"), "あいうえ...すせそた");
    }

    #[test]
    fn hex_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert!(from_hex("abc").is_none());
        assert!(from_hex("zz").is_none());
    }

    #[test]
    fn open_refuses_an_unknown_scheme_or_damaged_value() {
        assert!(open("plain:abc").is_err());
        assert!(open("dpapi:zz").is_err());
    }

    #[test]
    fn an_unreadable_value_is_drawn_and_kept_as_it_was() {
        let broken = EnvVar {
            name: "GH_TOKEN".to_string(),
            sealed: "dpapi:00".to_string(),
        };
        assert_eq!(shown(&broken), UNREADABLE);
        let text = render(std::slice::from_ref(&broken));
        assert_eq!(text, format!("GH_TOKEN={UNREADABLE}"));
        assert_eq!(settle(&text, &[broken.clone()], &[]).unwrap(), vec![broken.clone()]);
        let err = open_all(&[broken]).unwrap_err();
        assert!(err.contains("GH_TOKEN"), "{err}");
    }

    #[test]
    fn settle_refuses_a_name_the_app_sets_itself() {
        let err = settle("pullcept_room_token=x", &[], &["PULLCEPT_ROOM_TOKEN"]).unwrap_err();
        assert!(err.contains("pullcept_room_token"), "{err}");
    }

    #[test]
    fn settle_drops_a_line_that_is_gone() {
        let broken = EnvVar {
            name: "GH_TOKEN".to_string(),
            sealed: "dpapi:00".to_string(),
        };
        assert!(settle("", &[broken], &[]).unwrap().is_empty());
    }

    #[test]
    fn unquote_takes_off_one_matching_pair_only() {
        assert_eq!(unquote("abc"), Some("abc"));
        assert_eq!(unquote("\"abc\""), Some("abc"));
        assert_eq!(unquote("'abc'"), Some("abc"));
        assert_eq!(unquote("\"\"abc\"\""), Some("\"abc\""));
        assert_eq!(unquote("\"\""), Some(""));
        assert_eq!(unquote("\" a b \""), Some(" a b "));
        assert_eq!(unquote("\"abc'\""), Some("abc'"));
        assert_eq!(unquote("it's"), Some("it's"));
        assert_eq!(unquote(""), Some(""));
    }

    #[test]
    fn unquote_refuses_a_quote_at_one_end_only() {
        assert_eq!(unquote("\"github_pat_11ABC"), None);
        assert_eq!(unquote("github_pat_11ABC\""), None);
        assert_eq!(unquote("'abc"), None);
        assert_eq!(unquote("\"abc'"), None);
        assert_eq!(unquote("\""), None);
    }

    #[test]
    fn a_damaged_mask_is_told_from_an_ordinary_value() {
        let mask = "github_pat...Nx4h";
        // The mask itself, with quotes added or taken away.
        assert!(is_damaged_mask(mask, mask));
        assert!(is_damaged_mask("\"github_pat...Nx4h", mask));
        assert!(is_damaged_mask("\"github_pat...Nx4h\"", mask));
        assert!(is_damaged_mask("github_pa...Nx4h", "\"github_pa...Nx4h"));
        // Retyped around the ellipsis, head and tail kept.
        assert!(is_damaged_mask("github_pat_11...Nx4h", mask));
        assert!(is_damaged_mask("github_pat......Nx4h", mask));
        // A real value, or one that merely holds `...`.
        assert!(!is_damaged_mask("github_pat_11ABCDEFG0123456789Nx4h", mask));
        assert!(!is_damaged_mask("github_pat...Zz9q", mask));
        assert!(!is_damaged_mask("C:\\gh\\...\\lin", "C:...in"));
        assert!(is_damaged_mask("C:x...in", "C:...in"));
        assert!(!is_damaged_mask("github_pat_11ABCDEF...Nx4h", mask));
        // A mask that shows nothing is matched only exactly.
        assert!(is_damaged_mask("...", "..."));
        assert!(is_damaged_mask("'...'", "..."));
        assert!(!is_damaged_mask("a...b", "..."));
        assert!(!is_damaged_mask("", ""));
        // The unreadable marker, with quotes added.
        assert!(is_damaged_mask("\"（復号できません）\"", UNREADABLE));
    }

    #[test]
    fn settle_refuses_a_lone_quote_naming_the_line_and_not_the_value() {
        // The incident's first half (#165): the token pasted with a leading
        // quote and no closing one.
        let text = "LI_PLUS_AGENT_KEY=lin\nGH_TOKEN=\"github_pat_11ABCDEFG0123456789";
        let err = settle(text, &[], &[]).unwrap_err();
        assert!(err.contains("2 行目"), "{err}");
        assert!(err.contains("GH_TOKEN"), "{err}");
        assert!(!err.contains("github_pat"), "{err}");
        assert!(settle("A='abc", &[], &[]).unwrap_err().contains("1 行目"));
        assert!(settle("A=abc\"", &[], &[]).is_err());
    }

    #[test]
    fn settle_refuses_an_edited_unreadable_marker() {
        let broken = EnvVar {
            name: "GH_TOKEN".to_string(),
            sealed: "dpapi:00".to_string(),
        };
        let err = settle(&format!("GH_TOKEN=\"{UNREADABLE}\""), std::slice::from_ref(&broken), &[])
            .unwrap_err();
        assert!(err.contains("1 行目") && err.contains("GH_TOKEN"), "{err}");
    }

    #[cfg(windows)]
    mod windows {
        use super::super::*;

        const TOKEN: &str = "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzNx4h";

        #[test]
        fn seal_and_open_round_trip() {
            let sealed = seal(TOKEN).unwrap();
            assert!(sealed.starts_with(SEALED_PREFIX));
            assert!(!sealed.contains(TOKEN));
            assert_eq!(open(&sealed).unwrap(), TOKEN);
            assert_eq!(open(&seal("").unwrap()).unwrap(), "");
            assert_eq!(open(&seal("日本語=値").unwrap()).unwrap(), "日本語=値");
        }

        #[test]
        fn a_tampered_seal_does_not_open() {
            let sealed = seal(TOKEN).unwrap();
            let last = sealed.len() - 1;
            let flipped = if &sealed[last..] == "0" { "1" } else { "0" };
            let tampered = format!("{}{}", &sealed[..last], flipped);
            assert!(open(&tampered).is_err());
        }

        #[test]
        fn settle_seals_new_lines_and_keeps_untouched_ones() {
            let first = settle(&format!("GH_TOKEN={TOKEN}\nLI_PLUS_AGENT_KEY=lin"), &[], &[]).unwrap();
            assert_eq!(first.len(), 2);
            assert!(first.iter().all(|var| !var.sealed.contains(TOKEN)));

            // What the form shows next time, and nothing of the token beyond
            // its mask.
            let drawn = render(&first);
            assert_eq!(drawn, "GH_TOKEN=github_pat...Nx4h\nLI_PLUS_AGENT_KEY=...");

            // Left alone: the same sealed values come back, byte for byte.
            assert_eq!(settle(&drawn, &first, &[]).unwrap(), first);

            // One replaced, one removed, one added.
            let edited = "GH_TOKEN=github_pat_new_value_0000\nGH_CONFIG_DIR=C:\\gh\\lin";
            let second = settle(edited, &first, &[]).unwrap();
            let opened = open_all(&second).unwrap();
            assert_eq!(
                opened,
                vec![
                    ("GH_TOKEN".to_string(), "github_pat_new_value_0000".to_string()),
                    ("GH_CONFIG_DIR".to_string(), "C:\\gh\\lin".to_string()),
                ]
            );
        }

        #[test]
        fn settle_takes_off_matching_surrounding_quotes() {
            let text = format!("GH_TOKEN=\"{TOKEN}\"\nLI_PLUS_AGENT_KEY='lin'\nOPTS=\"a b'\"\nEMPTY=\"\"");
            let opened = open_all(&settle(&text, &[], &[]).unwrap()).unwrap();
            assert_eq!(
                opened,
                vec![
                    ("GH_TOKEN".to_string(), TOKEN.to_string()),
                    ("LI_PLUS_AGENT_KEY".to_string(), "lin".to_string()),
                    ("OPTS".to_string(), "a b'".to_string()),
                    ("EMPTY".to_string(), String::new()),
                ]
            );
        }

        /// The incident in #165, end to end: a token stored with a stray
        /// leading quote, then its mask edited to take the quote away.
        #[test]
        fn the_incident_mask_edit_is_refused_and_the_value_survives() {
            // What the pre-#165 app stored: the value with its leading quote.
            let quoted = format!("\"{TOKEN}");
            let stored = vec![EnvVar {
                name: "GH_TOKEN".to_string(),
                sealed: seal(&quoted).unwrap(),
            }];
            let drawn = render(&stored);
            assert_eq!(drawn, "GH_TOKEN=\"github_pa...Nx4h");

            // Left alone, it still reads back as untouched, quote and all.
            assert_eq!(settle(&drawn, &stored, &[]).unwrap(), stored);

            // The quote taken off the mask: refused, naming the line only.
            let err = settle("GH_TOKEN=github_pa...Nx4h", &stored, &[]).unwrap_err();
            assert!(err.contains("1 行目") && err.contains("GH_TOKEN"), "{err}");
            assert!(!err.contains("github_pa"), "{err}");

            // The real value pasted again goes through, without the quote.
            let fixed = settle(&format!("GH_TOKEN={TOKEN}"), &stored, &[]).unwrap();
            assert_eq!(open_all(&fixed).unwrap(), vec![("GH_TOKEN".to_string(), TOKEN.to_string())]);
        }

        #[test]
        fn edited_masks_of_a_stored_name_are_refused() {
            let stored = settle(&format!("GH_TOKEN={TOKEN}\nLI_PLUS_AGENT_KEY=lin"), &[], &[]).unwrap();
            for line in [
                "GH_TOKEN=\"github_pat...Nx4h\"",
                "GH_TOKEN=\"github_pat...Nx4h",
                "GH_TOKEN=github_pat_11...Nx4h",
                "gh_token=github_pat...Nx4h",
                "LI_PLUS_AGENT_KEY='...'",
            ] {
                let err = settle(line, &stored, &[]).unwrap_err();
                assert!(err.contains("1 行目"), "{line}: {err}");
            }
        }

        #[test]
        fn values_holding_an_ellipsis_are_ordinary_values() {
            let stored = settle("GH_CONFIG_DIR=C:\\gh\\lin\nLI_PLUS_AGENT_KEY=lin", &[], &[]).unwrap();
            // A new variable, and new values for stored names that are not
            // their masks, all hold `...` and all go through.
            let text = "GH_CONFIG_DIR=D:\\a\\...\\b\nLI_PLUS_AGENT_KEY=a...b\nOPTS=--include=src/...";
            let opened = open_all(&settle(text, &stored, &[]).unwrap()).unwrap();
            assert_eq!(
                opened,
                vec![
                    ("GH_CONFIG_DIR".to_string(), "D:\\a\\...\\b".to_string()),
                    ("LI_PLUS_AGENT_KEY".to_string(), "a...b".to_string()),
                    ("OPTS".to_string(), "--include=src/...".to_string()),
                ]
            );
            // A new variable whose value is shaped like a mask is not checked:
            // only names already stored have a mask to be damaged.
            let fresh = settle("NEW_KEY=github_pat...Nx4h", &stored, &[]).unwrap();
            assert_eq!(open_all(&fresh).unwrap(), vec![("NEW_KEY".to_string(), "github_pat...Nx4h".to_string())]);
        }

        #[test]
        fn a_renamed_line_is_sealed_again_from_what_it_says() {
            let first = settle("LI_PLUS_AGENT_KEY=lin", &[], &[]).unwrap();
            // Renamed with the mask left in place: the mask is what was typed,
            // so the mask is the new value. No old value moves across names.
            let second = settle("OTHER_KEY=...", &first, &[]).unwrap();
            assert_eq!(open_all(&second).unwrap(), vec![("OTHER_KEY".to_string(), "...".to_string())]);
        }
    }
}
