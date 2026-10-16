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

// Hex-encoded UTF-8 is injective, stable across launches and safe as a path
// component even for imported account/topic identifiers with separators.
pub fn sqlite_home(root: &Path, topic: &str, account: &str) -> PathBuf {
    fn component(s: &str) -> String {
        let mut result = String::from("id-");
        for byte in s.bytes() {
            result.push_str(&format!("{byte:02x}"));
        }
        result
    }
    root.join("codex-seat-sqlite")
        .join(component(topic))
        .join(component(account))
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
        let a = sqlite_home(root, "../a", "日本語/x");
        assert_eq!(a, sqlite_home(root, "../a", "日本語/x"));
        assert_ne!(a, sqlite_home(root, "../a/日本語", "x"));
        assert_ne!(a, sqlite_home(root, "../a", "other"));
        assert!(a.starts_with(root));
        assert_eq!(a.components().count(), root.components().count() + 3);
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
        assert_eq!(encoded.parse::<toml_edit::Value>().unwrap().as_str(), Some(dangerous));
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
