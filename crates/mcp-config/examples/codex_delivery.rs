//! Model-free bridge through the app's delivery composition and thread params.
use mcp_config::codex::app_server::{
    resume_params, seat_instructions, start_params, ThreadSettings,
};
use std::io::Read;
fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let data: serde_json::Value = serde_json::from_str(&input).unwrap();
    let text = seat_instructions(
        data["mode"].as_str().unwrap(),
        data["character"].as_str(),
        data["room"].as_str(),
        || Ok(data["common"].as_str().map(str::to_string)),
    )
    .unwrap();
    let settings = ThreadSettings::default();
    println!(
        "{}",
        serde_json::json!({
            "start": start_params("C:/fixture", text.as_deref(), &settings),
            "resume": resume_params("019db4af-794c-79a4-a277-c974c041cfdb", "C:/fixture", text.as_deref(), &settings),
        })
    );
}
