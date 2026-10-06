//! Prepares a disposable workspace with the same project folder, hook and argv as the app.
use mcp_config::{codex, runtime_launch_args, server_name_for, Cli};
use std::path::PathBuf;
fn main() {
    let dir = PathBuf::from(std::env::var("PULLCEPT_PROBE_WORKSPACE").expect("workspace"));
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let entry = repo.join("sidecar/src/index.ts");
    let runner = repo.join("node_modules/tsx/dist/cli.mjs");
    // The room server rides on the launch line; the room's variables come from the
    // caller's environment, as the app sets them for a seat (#297).
    codex::prepare_project(&dir).unwrap();
    let hook_dir = codex::native_hook_dir(&dir, &dir).unwrap();
    codex::register_hook(&hook_dir, &repo.join("sidecar/src/codex-session.mjs")).unwrap();
    let mut base = vec![
        "--no-alt-screen".into(),
        "-a".into(),
        "never".into(),
        "-s".into(),
        "read-only".into(),
    ];
    if let Ok(id) = std::env::var("PULLCEPT_PROBE_RESUME") {
        base.splice(0..0, ["resume".into(), id]);
    }
    let args = runtime_launch_args(
        &base,
        Some(Cli::CodexCli),
        &server_name_for("codex-test", "codex-topic"),
        None,
        &[],
        None,
        &runner,
        &entry,
    )
    .unwrap();
    println!("{}", serde_json::to_string(&args).unwrap());
}
