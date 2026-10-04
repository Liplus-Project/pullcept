//! Opt-in native CLI evidence through Pullcept's actual patched ConPTY builder.
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::Read;
fn main() {
    assert!(cfg!(windows), "Windows ConPTY evidence only");
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let fixture: serde_json::Value = serde_json::from_str(&input).unwrap();
    let command = fixture["command"].as_str().unwrap();
    let cwd = fixture["cwd"].as_str().unwrap();
    let home = fixture["home"].as_str().unwrap();
    let output = std::path::Path::new(cwd).join("pty-prompt.json");
    let options: Vec<String> = serde_json::from_value(fixture["options"].clone()).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let args = mcp_config::runtime_launch_args(
        &options,
        Some(mcp_config::Cli::CodexCli),
        "pullcept_probe",
        None,
        &[],
        None,
        &repo.join("node_modules/tsx/dist/cli.mjs"),
        &repo.join("sidecar/src/index.ts"),
    )
    .unwrap();
    mcp_config::codex::check_command_length(command, &args).unwrap();
    let pair = NativePtySystem::default()
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/C");
    cmd.arg("call");
    cmd.arg(command);
    for arg in &args {
        cmd.arg(arg);
    }
    cmd.arg("debug");
    cmd.arg("prompt-input");
    // Redirect only the JSON result; no unrelated prompt content enters diagnostics.
    cmd.arg(format!(">{}", output.display()));
    cmd.arg("2>nul");
    cmd.cwd(cwd);
    cmd.env("CODEX_HOME", home);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let drain = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = reader.read_to_end(&mut sink);
    });
    let writer = pair.master.take_writer().unwrap();
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed().as_secs() > 20 {
            child.kill().unwrap();
            panic!("ConPTY native debug timeout");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    drop(writer);
    drop(pair.master);
    let _ = drain.join();
    assert!(status.success(), "native CLI failed");
    let prompt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    let expected = fixture["expected"].as_str().unwrap();
    let equal = prompt
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "developer")
        .flat_map(|m| m["content"].as_array().unwrap())
        .any(|c| c["text"].as_str() == Some(expected));
    std::fs::remove_file(&output).unwrap();
    assert!(equal, "developer instructions did not match through ConPTY");
    println!("{{\"equal\":true,\"transport\":\"patched-ConPTY-cmd\",\"modelCalls\":0}}");
}
