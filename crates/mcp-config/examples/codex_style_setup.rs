//! Model-free fixture for the product installer and process-scoped options.
use mcp_config::codex::{style_setup, discovery_options};
fn main() {
    let input: serde_json::Value = serde_json::from_reader(std::io::stdin()).unwrap();
    let cwd = std::path::Path::new(input["cwd"].as_str().unwrap());
    let home = std::path::Path::new(input["home"].as_str().unwrap());
    let options: Vec<String> = serde_json::from_value(input["options"].clone()).unwrap();
    let result = (|| -> Result<_, String> {
        let scope = style_setup::home_scope(cwd, home);
        style_setup::install_scoped(cwd, scope)?;
        let options = style_setup::scoped_options(&options, cwd, home)?;
        let mut discovery = discovery_options(&options, home)?;
        discovery.extend(["-c".into(), "features.codex_hooks=true".into()]);
        Ok(serde_json::json!({"options":discovery,"scope":scope,"hash":style_setup::installed_hash(cwd)?}))
    })();
    match result {
        Ok(value) => println!("{value}"),
        Err(error) => { println!("{}", serde_json::json!({"error":error})); std::process::exit(1); }
    }
}
