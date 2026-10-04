//! Model-free fixture bridge to the same parser/argv functions as the app.
use std::io::Read;
fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let data: serde_json::Value = serde_json::from_str(&input).unwrap();
    let options: Vec<String> = serde_json::from_value(data["options"].clone()).unwrap();
    let mut args = mcp_config::codex::transport_options(&options).unwrap();
    if let Some(source) = data["source"].as_str() {
        mcp_config::codex::apply_character(&mut args, source, data["selected"].as_str().unwrap())
            .unwrap();
    }
    println!("{}", serde_json::to_string(&args).unwrap());
}
