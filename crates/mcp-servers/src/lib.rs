//! The MCP servers the app runs itself, as its own client (#172).
//!
//! They are listed in one file, `mcp-servers.json` in the app's data
//! directory, in the shape Claude Desktop's `claude_desktop_config.json` gives
//! its `mcpServers`: a name, and under it `command`, `args` and `env`. The
//! settings panel reads the file and writes one entry back; the file itself is
//! also the person's to edit, which is why a write here touches the three
//! fields it was handed and nothing else in the file.
//!
//! What the panel edits is text — one argument per line, one `NAME=value` per
//! line — and the reading of that text is here rather than in the screen, so
//! there is one reading of it and it is tested.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};

/// The file the servers are listed in, in the app's data directory.
///
/// Its own file rather than a key in `config.json`: this one is opened and
/// edited by hand from the panel's link, and `config.json` is rewritten whole
/// every time an account is saved.
pub const FILE_NAME: &str = "mcp-servers.json";

/// The key the servers sit under, as Claude Desktop names it.
pub const SERVERS_KEY: &str = "mcpServers";

/// The name the bundled webhook bridge (#169) is listed under when the file is
/// first written.
pub const BRIDGE_SERVER: &str = "github-webhook-mcp";

/// How many lines of one server's log the app keeps.
pub const LOG_LINES: usize = 1000;

/// One server: what is run, with what, in what environment.
///
/// `env` is added to the app's own environment, not put in place of it — the
/// same as Claude Desktop, and what lets `node` be found by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Server {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The environment the bundled bridge is listed with when the file is first
/// written: the worker it reaches, and channel push on (#177).
///
/// These are the values `github-webhook-mcp` falls back to when they are not
/// set, written out anyway. The app does not assume that bridge is the one
/// anyone runs, so where it connects is not left to a default hidden in the
/// package: it is in the file, and the panel shows it.
pub const BRIDGE_ENV: [(&str, &str); 2] = [
    ("WEBHOOK_WORKER_URL", "https://github-webhook.smgjp.com"),
    ("WEBHOOK_CHANNEL", "1"),
];

/// The file as it is written when there is none: the bundled bridge, run the
/// way the app ran it before there was a file (`node <webhook-bridge.mjs>`),
/// with its environment written out (`BRIDGE_ENV`).
///
/// Only a file that is not there is written this way. A file already written
/// is the person's, and is read as it stands.
pub fn default_file(bridge_script: &str) -> Value {
    let env: Map<String, Value> = BRIDGE_ENV
        .iter()
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect();
    json!({
        SERVERS_KEY: {
            BRIDGE_SERVER: {
                "command": "node",
                "args": [bridge_script],
                "env": env,
            }
        }
    })
}

/// Read the servers out of the file's contents.
///
/// A file with no `mcpServers` lists none. An entry that is not a server is an
/// error that names it: starting the rest would leave the one the person was
/// editing silently not running.
pub fn servers(root: &Value) -> Result<BTreeMap<String, Server>, String> {
    let Some(listed) = root.get(SERVERS_KEY) else {
        return Ok(BTreeMap::new());
    };
    let listed = listed
        .as_object()
        .ok_or_else(|| format!("{SERVERS_KEY} is not an object"))?;
    let mut servers = BTreeMap::new();
    for (name, entry) in listed {
        let server: Server = serde_json::from_value(entry.clone())
            .map_err(|e| format!("{SERVERS_KEY}.{name}: {e}"))?;
        servers.insert(name.clone(), server);
    }
    Ok(servers)
}

/// Write one server's `command`, `args` and `env` into the file's contents.
///
/// Only those three. Any other field of the entry, any other entry, and any
/// other key of the file stay as they were: the file is also edited by hand,
/// and a save from the panel is not a reason to lose what was written there.
pub fn set_server(root: &mut Value, name: &str, server: &Server) -> Result<(), String> {
    if !root.is_object() {
        return Err("the file is not a JSON object".to_string());
    }
    let listed = root
        .as_object_mut()
        .expect("checked above")
        .entry(SERVERS_KEY)
        .or_insert_with(|| Value::Object(Map::new()));
    let listed = listed
        .as_object_mut()
        .ok_or_else(|| format!("{SERVERS_KEY} is not an object"))?;
    let entry = listed
        .entry(name.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| format!("{SERVERS_KEY}.{name} is not an object"))?;
    entry.insert("command".to_string(), json!(server.command));
    entry.insert("args".to_string(), json!(server.args));
    entry.insert("env".to_string(), json!(server.env));
    Ok(())
}

/// Build a server from what the panel's three fields hold.
///
/// The command is required; it is the one field a server cannot run without.
pub fn server_from_fields(command: &str, args: &str, env: &str) -> Result<Server, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("コマンドを入力してください。".to_string());
    }
    Ok(Server {
        command: command.to_string(),
        args: args_from_text(args),
        env: env_from_text(env)?,
    })
}

/// The arguments field: one argument per line.
///
/// A line is one argument whatever it holds, spaces included — there is no
/// quoting to get wrong, because nothing splits a line. Blank lines are
/// dropped, and so is the space around a line, which a field typed into by hand
/// collects without anyone meaning it.
pub fn args_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The arguments as the field draws them. The inverse of `args_from_text` for
/// any argument that does not begin or end in space.
pub fn args_text(args: &[String]) -> String {
    args.join("\n")
}

/// The environment field: one `NAME=value` per line.
///
/// Split at the first `=`, so a value may hold `=` (a URL's query does). A
/// value wrapped whole in one pair of matching quotes loses them — what a line
/// copied from a shell or a `.env` file carries, and a quote nobody means as
/// part of the value (the account environment reads its field the same way,
/// #165). Blank lines are dropped.
///
/// Refused, with the line named: a line with no `=`, an empty name, a name
/// holding a space, and a name given twice. Each is a line whose meaning the
/// person would otherwise have to guess from what the server then did.
pub fn env_from_text(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let at = index + 1;
        let Some((name, value)) = line.split_once('=') else {
            return Err(format!("環境変数の {at} 行目に = がありません: {line}"));
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("環境変数の {at} 行目に名前がありません: {line}"));
        }
        if name.chars().any(char::is_whitespace) {
            return Err(format!("環境変数の {at} 行目の名前に空白があります: {name}"));
        }
        let value = unquote(value.trim());
        if env.insert(name.to_string(), value.to_string()).is_some() {
            return Err(format!("環境変数 {name} が二度書かれています（{at} 行目）"));
        }
    }
    Ok(env)
}

/// The environment as the field draws it, in name order.
pub fn env_text(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// One line of a server's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogLine {
    /// When it arrived, in milliseconds since the Unix epoch. The screen draws
    /// it in local time.
    pub at_ms: u64,
    pub text: String,
}

/// A server's log: the last `LOG_LINES` lines, oldest first.
///
/// Held by the app across restarts of the server, so the lines that say why the
/// last run ended are still there after the next one starts.
#[derive(Debug, Clone, Default)]
pub struct Log {
    lines: VecDeque<LogLine>,
}

impl Log {
    /// Add a line, dropping the oldest once there are `LOG_LINES`. A blank line
    /// is not kept: it says nothing and takes one of the kept places.
    pub fn push(&mut self, at_ms: u64, text: &str) {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        if self.lines.len() == LOG_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(LogLine {
            at_ms,
            text: text.to_string(),
        });
    }

    pub fn lines(&self) -> Vec<LogLine> {
        self.lines.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_file_is_the_bridge_with_its_environment_written_out() {
        let root = default_file("C:/pullcept/sidecar/src/webhook-bridge.mjs");
        let servers = servers(&root).unwrap();
        assert_eq!(servers.len(), 1);
        let bridge = &servers[BRIDGE_SERVER];
        assert_eq!(bridge.command, "node");
        assert_eq!(bridge.args, ["C:/pullcept/sidecar/src/webhook-bridge.mjs"]);
        // The worker and channel push are written out, not left to the
        // package's own defaults (#177).
        assert_eq!(
            bridge.env,
            BTreeMap::from([
                (
                    "WEBHOOK_WORKER_URL".to_string(),
                    "https://github-webhook.smgjp.com".to_string()
                ),
                ("WEBHOOK_CHANNEL".to_string(), "1".to_string()),
            ])
        );
        // And the panel draws them.
        assert_eq!(
            env_text(&bridge.env),
            "WEBHOOK_CHANNEL=1
WEBHOOK_WORKER_URL=https://github-webhook.smgjp.com"
        );
    }

    #[test]
    fn the_file_is_read_in_claude_desktops_shape() {
        let root: Value = serde_json::from_str(
            r#"{"mcpServers":{"a":{"command":"node","args":["x.mjs"],"env":{"WEBHOOK_WORKER_URL":"https://w.example"}},"b":{"command":"uvx"}}}"#,
        )
        .unwrap();
        let servers = servers(&root).unwrap();
        assert_eq!(servers["a"].env["WEBHOOK_WORKER_URL"], "https://w.example");
        // `args` and `env` may be left out, as Desktop allows.
        assert!(servers["b"].args.is_empty());
        assert!(servers["b"].env.is_empty());
    }

    #[test]
    fn a_file_with_no_servers_lists_none() {
        assert!(servers(&json!({})).unwrap().is_empty());
    }

    #[test]
    fn an_entry_that_is_not_a_server_is_named() {
        let err = servers(&json!({ "mcpServers": { "broken": { "args": [] } } })).unwrap_err();
        assert!(err.contains("broken"), "{err}");
        assert!(servers(&json!({ "mcpServers": [] })).is_err());
    }

    #[test]
    fn a_save_touches_only_the_three_fields_it_was_handed() {
        let mut root: Value = serde_json::from_str(
            r#"{
                "note": "written by hand",
                "mcpServers": {
                    "github-webhook-mcp": {"command":"node","args":["old.mjs"],"env":{"A":"1"},"disabled":false},
                    "other": {"command":"uvx","args":["tool"]}
                }
            }"#,
        )
        .unwrap();
        let server = Server {
            command: "node".to_string(),
            args: vec!["new.mjs".to_string()],
            env: BTreeMap::from([("WEBHOOK_WORKER_URL".to_string(), "https://w".to_string())]),
        };
        set_server(&mut root, BRIDGE_SERVER, &server).unwrap();

        assert_eq!(root["note"], "written by hand");
        assert_eq!(root["mcpServers"]["other"], json!({"command":"uvx","args":["tool"]}));
        let entry = &root["mcpServers"][BRIDGE_SERVER];
        assert_eq!(entry["disabled"], false);
        assert_eq!(entry["args"], json!(["new.mjs"]));
        // Replaced, not merged: a variable removed in the field is removed.
        assert_eq!(entry["env"], json!({"WEBHOOK_WORKER_URL":"https://w"}));
        assert_eq!(servers(&root).unwrap()[BRIDGE_SERVER], server);
    }

    #[test]
    fn a_save_into_a_file_that_cannot_hold_it_is_refused() {
        let server = Server {
            command: "node".to_string(),
            ..Server::default()
        };
        assert!(set_server(&mut json!([]), "a", &server).is_err());
        assert!(set_server(&mut json!({"mcpServers": 1}), "a", &server).is_err());
        let mut root = json!({});
        set_server(&mut root, "a", &server).unwrap();
        assert_eq!(servers(&root).unwrap()["a"], server);
    }

    #[test]
    fn one_line_is_one_argument_spaces_and_all() {
        assert_eq!(
            args_from_text("  C:/Program Files/x/bridge.mjs \r\n\n--flag\n"),
            ["C:/Program Files/x/bridge.mjs", "--flag"]
        );
        let args = vec!["a b".to_string(), "--c=d".to_string()];
        assert_eq!(args_from_text(&args_text(&args)), args);
    }

    #[test]
    fn the_environment_field_reads_name_equals_value() {
        let env = env_from_text(
            "WEBHOOK_WORKER_URL=https://w.example/?a=b\r\n\n  WEBHOOK_CHANNEL = 0 \nQ=\"quoted value\"\nS='x'\nE=",
        )
        .unwrap();
        assert_eq!(env["WEBHOOK_WORKER_URL"], "https://w.example/?a=b");
        assert_eq!(env["WEBHOOK_CHANNEL"], "0");
        assert_eq!(env["Q"], "quoted value");
        assert_eq!(env["S"], "x");
        // An empty value is a value.
        assert_eq!(env["E"], "");
        assert_eq!(env_from_text(&env_text(&env)).unwrap(), env);
    }

    #[test]
    fn a_quote_on_one_side_only_is_kept() {
        let env = env_from_text("A=\"half\nB=\"").unwrap();
        assert_eq!(env["A"], "\"half");
        assert_eq!(env["B"], "\"");
    }

    #[test]
    fn a_line_whose_meaning_would_be_guessed_is_refused() {
        for text in ["NO_EQUALS", "=value", "TWO WORDS=x", "A=1\nA=2"] {
            assert!(env_from_text(text).is_err(), "{text}");
        }
        let err = env_from_text("A=1\n\nbad").unwrap_err();
        assert!(err.contains("3 行目"), "{err}");
    }

    #[test]
    fn a_server_needs_a_command() {
        assert!(server_from_fields("  ", "x", "").is_err());
        let server = server_from_fields(" node ", "x.mjs", "A=1").unwrap();
        assert_eq!(server.command, "node");
        assert_eq!(server.args, ["x.mjs"]);
        assert_eq!(server.env["A"], "1");
        // A bad environment line is not dropped to let the rest through.
        assert!(server_from_fields("node", "", "oops").is_err());
    }

    #[test]
    fn the_log_keeps_the_last_lines_and_skips_blank_ones() {
        let mut log = Log::default();
        log.push(1, "   ");
        assert!(log.lines().is_empty());
        for i in 0..LOG_LINES + 5 {
            log.push(i as u64, &format!("line {i}\r"));
        }
        let lines = log.lines();
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines[0].text, "line 5");
        assert_eq!(lines.last().unwrap().text, format!("line {}", LOG_LINES + 4));
    }
}
