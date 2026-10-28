//! Fixed diagnostic codes from the output-style resolver (#576).
use std::io::Write;

/// No caller-provided text reaches the log, including unknown future codes.
pub fn code(reply: &serde_json::Value) -> Option<&'static str> {
    let value = reply.get("diagnostic_code")?.as_str()?;
    [
        "discovery_execution", "discovery_timeout", "discovery_json", "discovery_error",
        "helper_execution", "helper_timeout", "helper_json",
        "hook_execution", "hook_timeout", "hook_json",
        "project_validation", "helper_resolve", "hook_validation", "matcher_validation",
        "metadata_validation", "handler_validation", "hook_response_validation", "unexpected_error",
        "helper_hash_validation", "hook_hash_validation",
    ].into_iter().find(|known| *known == value)
}

/// Best effort: opening or writing the probe cannot alter the launch result.
pub fn record<W: Write>(reply: &serde_json::Value, open: impl FnOnce() -> Option<W>) {
    if let Some(code) = code(reply) {
        if let Some(mut log) = open() {
            let _ = log.write_all(format!("{code}\n").as_bytes());
        }
    }
}

/// Decode the resolver's stdout at the launch/preview boundary.
pub fn decode<W: Write>(success: bool, stdout: &[u8], open: impl FnOnce() -> Option<W>) -> Result<serde_json::Value, String> {
    let style: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|_| "Codex の output style 確認が失敗しました。")?;
    if !success || style.get("error").is_some() {
        record(&style, open);
        return Err(style["error"].as_str().unwrap_or("Codex のキャラクターを確認できません。project の output style と専用 SessionStart hook の登録・信頼、Python 3.11 以上を確認してください。").to_string());
    }
    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn launch_boundary_preserves_error_and_success_and_logs_only_failure() {
        let mut log = Vec::new();
        let reply = br#"{"error":"visible error", "diagnostic_code":"helper_execution", "body":"SECRET"}"#;
        assert_eq!(decode(false, reply, || Some(&mut log)).unwrap_err(), "visible error");
        assert_eq!(log, b"helper_execution\n");
        // A protocol error also fails closed when the child exits successfully.
        assert!(decode(true, reply, || Some(&mut log)).is_err());
        let good = br#"{"mode":"file","body":"SECRET","diagnostic_code":"helper_execution"}"#;
        let value = decode::<Vec<u8>>(true, good, || panic!("success must not open log")).unwrap();
        assert_eq!(value["body"], "SECRET");
        assert!(decode::<Vec<u8>>(false, b"not JSON SECRET", || panic!("invalid JSON has no trusted code")).is_err());
        assert_eq!(decode::<Vec<u8>>(false, reply, || None).unwrap_err(), "visible error");
    }

    #[test]
    fn only_allowlisted_code_reaches_file() {
        let path = std::env::temp_dir().join(format!("style-probe-{}", uuid::Uuid::new_v4()));
        let reply = json!({"error":"SECRET", "diagnostic_code":"hook_timeout", "body":"PRIVATE"});
        record(&reply, || std::fs::OpenOptions::new().create(true).append(true).open(&path).ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hook_timeout\n");
        for reply in [json!({}), json!({"diagnostic_code":null}), json!({"diagnostic_code":42}),
            json!({"diagnostic_code":"hook_timeout\nSECRET"}), json!({"diagnostic_code":"/private/path"})] {
            record::<Vec<u8>>(&reply, || panic!("unknown code must not open a file"));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unavailable_or_failed_writer_is_ignored() {
        let reply = json!({"diagnostic_code":"helper_json"});
        record::<Vec<u8>>(&reply, || None);
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::ErrorKind::PermissionDenied.into()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        record(&reply, || Some(Failed));
    }
}
