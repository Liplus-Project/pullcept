//! Fixed diagnostic line from the effective-instructions probe (#622,
//! `sidecar/src/codex-character.mjs`): which call and stage failed, and how
//! long the probe ran. Prompt text, child output and arguments never reach it.
use std::io::Write;

pub const LOG_FILE: &str = "codex-instructions-probe.log";

/// The stage code, only when it is one this build knows.
pub fn code(reply: &serde_json::Value) -> Option<&'static str> {
    let value = reply.get("diagnostic_code")?.as_str()?;
    [
        "call1_shell", "call1_execution", "call1_timeout", "call1_output_limit", "call1_exit", "call1_json",
        "call2_shell", "call2_execution", "call2_timeout", "call2_output_limit", "call2_exit", "call2_json",
        "options", "prompt_shape", "sentinel", "comparison", "unexpected_error",
    ].into_iter().find(|known| *known == value)
}

/// `<UTC time> <code> elapsed_ms=<n>`; elapsed is omitted unless it is a whole number.
pub fn line(reply: &serde_json::Value, now: std::time::SystemTime) -> Option<String> {
    let code = code(reply)?;
    let secs = now.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let at = chrono::DateTime::from_timestamp(secs, 0)?.format("%Y-%m-%dT%H:%M:%SZ");
    Some(match reply.get("elapsed_ms").and_then(serde_json::Value::as_u64) {
        Some(elapsed) => format!("{at} {code} elapsed_ms={elapsed}\n"),
        None => format!("{at} {code}\n"),
    })
}

/// Best effort: opening or writing the probe cannot alter the launch result.
pub fn record<W: Write>(reply: &serde_json::Value, now: std::time::SystemTime, open: impl FnOnce() -> Option<W>) {
    if let Some(line) = line(reply, now) {
        if let Some(mut log) = open() {
            let _ = log.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn known_stage_and_elapsed_reach_the_line_and_nothing_else() {
        let now = UNIX_EPOCH + Duration::from_secs(1_795_000_000);
        let reply = json!({"error":"SECRET", "diagnostic_code":"call2_timeout", "elapsed_ms":15012, "instructions":"PRIVATE"});
        let mut log = Vec::new();
        record(&reply, now, || Some(&mut log));
        let text = String::from_utf8(log).unwrap();
        assert_eq!(text, "2026-11-18T11:06:40Z call2_timeout elapsed_ms=15012\n");
        assert_eq!(line(&json!({"diagnostic_code":"comparison", "elapsed_ms":"9400 SECRET"}), now).unwrap(),
            "2026-11-18T11:06:40Z comparison\n");
        assert_eq!(line(&json!({"diagnostic_code":"comparison", "elapsed_ms":-1}), now).unwrap(),
            "2026-11-18T11:06:40Z comparison\n");
    }

    #[test]
    fn unknown_code_opens_no_file() {
        let now = UNIX_EPOCH + Duration::from_secs(1);
        for reply in [json!({}), json!({"error":"x"}), json!({"diagnostic_code":42}), json!({"diagnostic_code":"call3_exit"}),
            json!({"diagnostic_code":"comparison\nSECRET"}), json!({"diagnostic_code":"/private/path"})] {
            record::<Vec<u8>>(&reply, now, || panic!("unknown code must not open a file"));
        }
    }

    #[test]
    fn unavailable_or_failed_writer_is_ignored() {
        let reply = json!({"diagnostic_code":"call1_exit", "elapsed_ms":3});
        record::<Vec<u8>>(&reply, std::time::SystemTime::now(), || None);
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::ErrorKind::PermissionDenied.into()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        record(&reply, std::time::SystemTime::now(), || Some(Failed));
    }
}
