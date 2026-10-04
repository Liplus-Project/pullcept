//! No model call: exposes the same profile discovery adapter as the app.
fn main() {
    let args: Vec<String> =
        serde_json::from_str(&std::env::args().nth(1).expect("JSON options")).unwrap();
    let home =
        std::path::PathBuf::from(std::env::var_os("CODEX_HOME").expect("isolated CODEX_HOME"));
    println!(
        "{}",
        serde_json::to_string(&mcp_config::codex::discovery_options(&args, &home).unwrap())
            .unwrap()
    );
}
