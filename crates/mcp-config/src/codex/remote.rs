//! Native relay planning and protocol boundary. Pairing values are intentionally
//! neither Debug nor persisted; only the caller's private reply carries them.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Status,
    Enable,
    Disable,
    Pair,
}
impl Action {
    pub fn request(self) -> (&'static str, Value) {
        match self {
            Self::Status => ("remoteControl/status/read", Value::Null),
            Self::Enable => ("remoteControl/enable", json!({"ephemeral": true})),
            Self::Disable => ("remoteControl/disable", json!({"ephemeral": true})),
            Self::Pair => ("remoteControl/pairing/start", json!({"manualCode": true})),
        }
    }
}

// A fixed-size, versioned digest avoids growing two UTF-8 hex components
// past Windows' journal-path limit (#426). Lengths are byte counts, so even
// separators and empty identifiers have an unambiguous encoding.
// Codex 0.162.1's longest DB name and longest auxiliary suffix. Native
// fresh history initialization passed at 251 ASCII units and failed at 252
// during fresh initialization. Keep headroom below that observed boundary.
const HISTORY_DB_FILE: &str = "thread_history_1.sqlite";
const JOURNAL_SUFFIX: &str = "-journal";
const SQLITE_PATH_BUDGET: usize = 248;

pub fn sqlite_home(root: &Path, topic: &str, account: &str) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"pullcept.codex-seat-sqlite.v2\0");
    for id in [topic, account] {
        hash.update((id.len() as u64).to_be_bytes());
        hash.update(id.as_bytes());
    }
    let component = format!("{:x}", hash.finalize());
    let path = root.join("codex-seat-sqlite").join(component);
    let longest = path.join(format!("{HISTORY_DB_FILE}{JOURNAL_SUFFIX}"));
    let length = longest
        .to_str()
        .ok_or("専用DBの保存先を UTF-8 で表せません。")?
        .encode_utf16()
        .count();
    if length > SQLITE_PATH_BUDGET {
        return Err(format!("専用DBの保存先が長すぎます（SQLite補助ファイル込み {length} UTF-16単位、上限 {SQLITE_PATH_BUDGET}）。Pullceptのアプリデータ保存先を短くしてから起動してください。"));
    }
    Ok(path)
}

pub fn isolate(args: &mut Vec<String>, path: &Path) -> Result<(), String> {
    let index = args
        .iter()
        .rposition(|arg| arg == "app-server")
        .ok_or("app-server の起動行がありません。")?;
    let path = path.to_str().ok_or("保存先を UTF-8 で表せません。")?;
    let value = super::instruction_value(path);
    args.splice(index..index, ["-c".into(), format!("sqlite_home={value}")]);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub pairing_code: String,
    pub manual_pairing_code: Option<String>,
    pub expires_at: i64,
}

pub fn pairing(value: &Value, now: i64) -> Result<Pairing, String> {
    let code = value["pairingCode"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096);
    let expires = value["expiresAt"]
        .as_i64()
        .map(|v| if v > 100_000_000_000 { v / 1000 } else { v })
        .filter(|v| *v > now && *v <= now + 3600);
    let manual = value["manualPairingCode"]
        .as_str()
        .filter(|s| s.len() <= 256)
        .map(str::to_owned);
    match (code, expires) {
        (Some(code), Some(expires_at)) => Ok(Pairing {
            pairing_code: code.into(),
            manual_pairing_code: manual,
            expires_at,
        }),
        _ => Err("ペアリング情報が不正、または期限切れです。更新してください。".into()),
    }
}

pub fn response(value: &Value, id: u64) -> Option<Result<Value, String>> {
    if value.get("method").is_some() || value["id"].as_u64() != Some(id) {
        return None;
    }
    if let Some(error) = value.get("error") {
        let message = error["message"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let text = if error["code"].as_i64() == Some(-32601) {
            "この Codex CLI はリモート接続に対応していません。CLI を更新してください。"
        } else if message.contains("auth")
            || message.contains("chatgpt")
            || message.contains("login")
        {
            "ChatGPT アカウントでのログインが必要です。API キー認証では利用できません。"
        } else {
            "リモート接続が拒否されました。アカウントの許可・ネットワーク・CLI の対応状況を確認してください。"
        };
        return Some(Err(text.into()));
    }
    Some(
        value
            .get("result")
            .cloned()
            .ok_or_else(|| "リモート接続の応答が不正です。".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_seat_paths_do_not_share_registration_or_escape() {
        let root = Path::new("C:/private");
        let a = sqlite_home(root, "../a", "日本語/x").unwrap();
        assert_eq!(a, sqlite_home(root, "../a", "日本語/x").unwrap());
        assert_ne!(a, sqlite_home(root, "../a/日本語", "x").unwrap());
        assert_ne!(a, sqlite_home(root, "../a", "other").unwrap());
        assert!(a.starts_with(root));
        assert_eq!(a.components().count(), root.components().count() + 2);
    }
    #[test]
    fn digest_is_fixed_safe_unambiguous_and_stable_for_utf8_identifiers() {
        let root = Path::new("C:/private");
        let a = sqlite_home(root, "ab", "c").unwrap();
        let component = a.file_name().unwrap().to_str().unwrap();
        assert_eq!(component.len(), 64);
        assert!(component
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_ne!(a, sqlite_home(root, "a", "bc").unwrap());
        assert_ne!(a, sqlite_home(root, "c", "ab").unwrap());
        assert_ne!(
            sqlite_home(root, "", "x").unwrap(),
            sqlite_home(root, "x", "").unwrap()
        );
        assert_ne!(
            sqlite_home(root, "a/b", "c").unwrap(),
            sqlite_home(root, "a", "b/c").unwrap()
        );
        assert_eq!(
            sqlite_home(root, &"日本語".repeat(1000), "x")
                .unwrap()
                .file_name()
                .unwrap()
                .len(),
            64
        );
        // Fixed vector protects the namespace and UTF-8 byte-length framing.
        assert_eq!(
            sqlite_home(root, "🌙", "日本語")
                .unwrap()
                .file_name()
                .unwrap(),
            "f99e04d4423e58923875c512b96c43b9fca99be99f40cbacbea8355aaa2a7fb4"
        );
    }

    #[test]
    fn windows_uuid_paths_budget_the_database_and_every_auxiliary_suffix() {
        let root = Path::new(r"C:\Users\smile\AppData\Roaming\org.liplus-project.pullcept");
        let topic = "00000000-0000-4000-8000-000000000426";
        let account = "00000000-0000-4000-8000-000000000419";
        let path = sqlite_home(root, topic, account).unwrap();
        for name in [
            "state_5.sqlite",
            "logs_2.sqlite",
            "goals_1.sqlite",
            "memories_1.sqlite",
            "memories_v2_1.sqlite",
            "queue_1.sqlite",
            HISTORY_DB_FILE,
        ] {
            for suffix in ["", "-journal", "-wal", "-shm"] {
                let whole = path.join(format!("{name}{suffix}"));
                assert!(whole.to_str().unwrap().encode_utf16().count() <= SQLITE_PATH_BUDGET);
            }
        }
        let hex = |id: &str| {
            format!(
                "id-{}",
                id.bytes().map(|b| format!("{b:02x}")).collect::<String>()
            )
        };
        let old = root
            .join("codex-seat-sqlite")
            .join(hex(topic))
            .join(hex(account))
            .join(format!("{HISTORY_DB_FILE}{JOURNAL_SUFFIX}"));
        assert!(
            old.to_str().unwrap().encode_utf16().count() > SQLITE_PATH_BUDGET,
            "the old layout must fail this regression budget"
        );
    }

    #[test]
    fn long_and_non_ascii_roots_are_checked_in_utf16_before_launch() {
        let root = format!(
            "C:/Users/{}/AppData/Roaming/org.liplus-project.pullcept",
            "long-user-".repeat(20)
        );
        assert!(sqlite_home(Path::new(&root), "topic", "account")
            .unwrap_err()
            .contains("保存先を短く"));
        let longer_user = format!(
            "C:/Users/{}/AppData/Roaming/org.liplus-project.pullcept",
            "long-user-".repeat(6)
        );
        assert!(sqlite_home(Path::new(&longer_user), "topic", "account").is_ok());
        let root = Path::new("C:/Users/山田🌙/AppData/Roaming/org.liplus-project.pullcept");
        assert!(sqlite_home(root, "topic", "account").is_ok());
        let baseline = sqlite_home(Path::new("C:/r"), "topic", "account").unwrap();
        let cost = baseline
            .join(format!("{HISTORY_DB_FILE}{JOURNAL_SUFFIX}"))
            .to_str()
            .unwrap()
            .encode_utf16()
            .count();
        let extra = SQLITE_PATH_BUDGET - cost;
        let fits = format!("C:/r{}", "a".repeat(extra));
        assert!(sqlite_home(Path::new(&fits), "topic", "account").is_ok());
        let exceeds = format!("C:/r{}🌙", "a".repeat(extra - 1));
        // Same Unicode scalar count as fits, one extra Windows UTF-16 unit.
        assert!(sqlite_home(Path::new(&exceeds), "topic", "account").is_err());
    }

    #[test]
    fn override_is_last_global_setting_and_auth_listener_is_unchanged() {
        let mut args = vec![
            "-c",
            "sqlite_home=old",
            "app-server",
            "--listen",
            "ws://127.0.0.1:0",
            "--ws-auth",
            "capability-token",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        isolate(&mut args, Path::new("C:/seat db")).unwrap();
        let setting = args[3]
            .split_once('=')
            .unwrap()
            .1
            .parse::<toml_edit::Value>()
            .unwrap();
        assert_eq!(setting.as_str(), Some("C:/seat db"));
        assert!(!args[3].contains(' '));
        assert_eq!(args[4], "app-server");
        assert_eq!(args.last().unwrap(), "capability-token");
        let dangerous = "C:/日本語/%PATH% & (seat)";
        isolate(&mut args, Path::new(dangerous)).unwrap();
        let encoded = args[5].split_once('=').unwrap().1;
        assert_eq!(
            encoded.parse::<toml_edit::Value>().unwrap().as_str(),
            Some(dangerous)
        );
        assert!(!encoded.contains('%'));
        assert!(!encoded.contains('&'));
        assert_eq!(Action::Enable.request().1, json!({"ephemeral":true}));
        assert_eq!(Action::Disable.request().1, json!({"ephemeral":true}));
    }
    #[test]
    fn responses_are_correlated_and_errors_cannot_echo_secrets() {
        assert!(response(&json!({"id":1,"method":"approval","params":{}}), 1).is_none());
        assert!(response(&json!({"id":2,"result":{"pairingCode":"secret"}}), 1).is_none());
        let err = response(
            &json!({"id":1,"error":{"code":-32601,"message":"secret"}}),
            1,
        )
        .unwrap()
        .unwrap_err();
        assert!(!err.contains("secret"));
        assert!(err.contains("対応"));
        assert_eq!(
            response(&json!({"id":1,"result":{"status":"connected"}}), 1)
                .unwrap()
                .unwrap()["status"],
            "connected"
        );
    }
    #[test]
    fn pairing_is_validated_and_extra_private_fields_are_dropped() {
        let value = json!({"pairingCode":"test","manualPairingCode":"1234","expiresAt":1200,"secret":"do not forward"});
        let parsed = pairing(&value, 1000).unwrap();
        let serialized = serde_json::to_value(parsed).unwrap();
        assert!(serialized.get("secret").is_none());
        assert!(pairing(&value, 1200).is_err());
        let milliseconds = json!({"pairingCode":"test","expiresAt":1_800_000_060_000_i64});
        assert_eq!(
            pairing(&milliseconds, 1_800_000_000).unwrap().expires_at,
            1_800_000_060
        );
        assert!(pairing(&json!({"pairingCode":"test","expiresAt":999999}), 1000).is_err());
    }
}
