// Compile the application's actual account serde contract without Tauri's DLLs.
#[allow(dead_code)]
#[path = "../../../src-tauri/src/config/account.rs"]
mod account;

use account::{Account, AccountKind};
use serde_json::json;

#[test]
fn codex_transport_missing_and_explicit_choices_survive_save() {
    for (saved, expected) in [(None, true), (Some(false), false), (Some(true), true)] {
        let mut value =
            json!({"id":"codex", "name":"Codex", "command":"codex", "args":[], "kind":"codex_cli"});
        if let Some(saved) = saved {
            value["codex_app_server"] = json!(saved);
        }
        let loaded: Account = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.codex_app_server, expected);
        assert!(!loaded.codex_remote_control);
        let saved = serde_json::to_value(&loaded).unwrap();
        assert_eq!(saved["codex_app_server"], expected);
        assert_eq!(saved["codex_remote_control"], false);
        let reloaded: Account = serde_json::from_value(saved).unwrap();
        assert_eq!(reloaded.codex_app_server, expected);
        assert_eq!(reloaded.kind, AccountKind::CodexCli);
    }
}

#[test]
fn other_account_kinds_and_remote_opt_in_are_preserved() {
    for kind in ["claude_code", "cli", "admin", "mcp"] {
        let loaded: Account = serde_json::from_value(json!({"id":"other", "name":"Other", "command":"unchanged", "args":["--option"], "kind":kind})).unwrap();
        let saved = serde_json::to_value(loaded).unwrap();
        assert_eq!(saved["kind"], kind);
        assert_eq!(saved["command"], "unchanged");
        assert_eq!(saved["args"], json!(["--option"]));
        assert_eq!(saved["codex_remote_control"], false);
    }
    let loaded: Account = serde_json::from_value(json!({"id":"codex", "name":"Codex", "command":"codex", "args":[], "kind":"codex_cli", "codex_remote_control":true})).unwrap();
    assert!(loaded.codex_remote_control);
}

#[test]
fn resume_notice_missing_is_on_and_explicit_off_survives_save() {
    // #475: an account saved before the switch keeps the prompt it always had.
    for (kind, command) in [("claude_code", "claude"), ("codex_cli", "codex")] {
        let value = json!({"id":"a", "name":"A", "command":command, "args":[], "kind":kind});
        let loaded: Account = serde_json::from_value(value.clone()).unwrap();
        assert!(loaded.resume_notice);
        assert_eq!(serde_json::to_value(&loaded).unwrap()["resume_notice"], true);
        let mut off = value;
        off["resume_notice"] = json!(false);
        let loaded: Account = serde_json::from_value(off).unwrap();
        assert!(!loaded.resume_notice);
        let saved = serde_json::to_value(&loaded).unwrap();
        assert_eq!(saved["resume_notice"], false);
        let reloaded: Account = serde_json::from_value(saved).unwrap();
        assert!(!reloaded.resume_notice);
    }
}
