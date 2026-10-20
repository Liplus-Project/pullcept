use mcp_config::codex::style_setup::{install, LOADER};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("pullcept style {}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap(); Self(p)
    }
    fn read(&self, p: &str) -> Vec<u8> { fs::read(self.0.join(p)).unwrap() }
    fn write(&self, p: &str, text: &str) { fs::write(self.0.join(p), text).unwrap(); }
}
impl Drop for Folder { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[test]
fn clean_offline_install_preserves_siblings_after_body_save_and_retries() {
    let f = Folder::new();
    install(&f.0).unwrap();
    // Same order as the account form: setup, then character body, then account.
    fs::create_dir_all(f.0.join(".codex/output-styles")).unwrap();
    f.write(".codex/output-styles/Luna.md", "---\nname: Luna\n---\nCHARACTER_BODY\n");
    let config = f.read(".codex/config.toml");
    let hooks = f.read(".codex/hooks.json");
    install(&f.0).unwrap();
    assert_eq!(config, f.read(".codex/config.toml"));
    assert_eq!(hooks, f.read(".codex/hooks.json"));
    let python = if cfg!(windows) { "python" } else { "python3" };
    for selected in [None, Some("Luna")] {
        let mut cmd = Command::new(python);
        cmd.arg(f.0.join(".codex/hooks/codex-output-style.py")).args(["resolve", "--root"]).arg(&f.0).arg("--cwd").arg(&f.0).env_remove("LI_PLUS_OUTPUT_STYLE");
        if let Some(s) = selected { cmd.args(["--style", s]); }
        let result = cmd.output().unwrap();
        assert!(result.status.success(), "{:?}", result);
        let data: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(data["mode"], if selected.is_some() { "file" } else { "disabled" });
    }
}

#[test]
fn existing_liplus_hooks_and_config_are_preserved_byte_for_byte() {
    let f = Folder::new();
    fs::create_dir_all(f.0.join(".codex/hooks")).unwrap();
    fs::create_dir_all(f.0.join(".codex/output-styles")).unwrap();
    f.write(".codex/hooks/codex-output-style.py", LOADER);
    f.write(".codex/config.toml", "# retain comment\ndeveloper_instructions='USER_BODY'\n[liplus]\noutput_style='Luna'\n");
    let canonical = fs::canonicalize(&f.0).unwrap();
    let text = canonical.to_string_lossy();
    let root = text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/");
    let hooks = json!({"hooks":{"SessionStart":[{"matcher":"startup|resume|clear|compact", "hooks":[
        {"type":"command","command":"user_hook", "timeout":60},
        {"type":"command", "command":format!("python3 \"{root}/.codex/hooks/codex-output-style.py\" hook --root \"{root}\""),
         "commandWindows":format!("python \"{root}/.codex/hooks/codex-output-style.py\" hook --root \"{root}\""),
         "timeout":30, "additionalContextLimit":0, "statusMessage":"Li+ project output style"}]}]}});
    f.write(".codex/hooks.json", &serde_json::to_string_pretty(&hooks).unwrap());
    let before = (f.read(".codex/config.toml"), f.read(".codex/hooks.json"));
    install(&f.0).unwrap();
    assert_eq!(before, (f.read(".codex/config.toml"), f.read(".codex/hooks.json")));
}

#[test]
fn rejects_foreign_and_invalid_files_without_changing_any_bytes() {
    for bad in ["helper", "config", "hooks", "registration"] {
        let f = Folder::new(); install(&f.0).unwrap();
        match bad {
            "helper" => f.write(".codex/hooks/codex-output-style.py", "foreign script"),
            "config" => f.write(".codex/config.toml", "[broken"),
            "hooks" => f.write(".codex/hooks.json", "{broken"),
            _ => f.write(".codex/hooks.json", r#"{"hooks":{"SessionStart":[{"hooks":[{"command":"foreign codex-output-style.py"}]}]}}"#),
        }
        let paths = [".codex/hooks/codex-output-style.py", ".codex/config.toml", ".codex/hooks.json"];
        let before: Vec<_> = paths.iter().map(|p| f.read(p)).collect();
        assert!(install(&f.0).is_err(), "{bad}");
        assert_eq!(before, paths.iter().map(|p| f.read(p)).collect::<Vec<_>>());
    }
}

#[test]
fn explicit_shared_off_and_implicit_folder_default_are_not_rewritten() {
    for text in ["[liplus]\noutput_style=false\n", "# implicit folder default\n"] {
        let f = Folder::new(); fs::create_dir_all(f.0.join(".codex/output-styles")).unwrap();
        f.write(".codex/config.toml", text); install(&f.0).unwrap();
        assert_eq!(f.read(".codex/config.toml"), text.as_bytes());
    }
}

#[cfg(windows)]
#[test]
fn late_write_failure_restores_config_and_removes_new_helper() {
    let f = Folder::new(); fs::create_dir_all(f.0.join(".codex")).unwrap();
    let config = "# user comment\ndeveloper_instructions='KEEP_ME'\n";
    f.write(".codex/config.toml", config);
    f.write(".codex/hooks.json", "{\"hooks\":{}}\n");
    let path = f.0.join(".codex/hooks.json");
    let mut permissions = fs::metadata(&path).unwrap().permissions(); permissions.set_readonly(true); fs::set_permissions(&path, permissions).unwrap();
    let result = install(&f.0);
    let mut permissions = fs::metadata(&path).unwrap().permissions(); permissions.set_readonly(false); fs::set_permissions(&path, permissions).unwrap();
    assert!(result.unwrap_err().contains("hooks.json"));
    assert_eq!(f.read(".codex/config.toml"), config.as_bytes());
    assert_eq!(f.read(".codex/hooks.json"), b"{\"hooks\":{}}\n");
    assert!(!f.0.join(".codex/hooks/codex-output-style.py").exists());
    assert!(!f.0.join(".codex/pullcept-style-setup.lock").exists());
    install(&f.0).unwrap();
}
