//! Noninteractive observer wire contract. IDs are strings, separate from event-body numeric IDs.
use serde_json::{json, Value};
pub const META: &str = "io.github.Liplus-Project/github-webhook-mcp/pending-status-observer";
pub const LIST_ID: &str = "pullcept:observer:tools";
pub const POLL_ID: &str = "pullcept:observer:pending";
pub fn list_request() -> String { json!({"jsonrpc":"2.0","id":LIST_ID,"method":"tools/list"}).to_string() }
pub fn poll_request() -> String { json!({"jsonrpc":"2.0","id":POLL_ID,"method":"tools/call","params":{"name":"get_pending_status","arguments":{}}}).to_string() }
pub fn supported(message: &Value) -> bool {
    message["error"].is_null() && message["result"]["tools"].as_array().is_some_and(|tools| tools.iter()
        .any(|t| t["name"] == "get_pending_status" && t["_meta"][META] == 1))
}
pub fn snapshot(message: &Value, at: i64) -> Value {
    let raw = message["result"]["content"].as_array().and_then(|parts| parts.iter().find_map(|p| p["text"].as_str()))
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let Some(raw) = raw else { return json!({"state":"unobserved","observed_at":at}); };
    if raw["error"] == "auth_required" || raw["status"] == "auth_required" || raw["auth_required"] == true {
        return json!({"state":"auth_required","observed_at":at});
    }
    if !message["error"].is_null() || message["result"]["isError"] == true { return json!({"state":"unobserved","observed_at":at}); }
    let Some(count) = raw["pending_count"].as_u64() else { return json!({"state":"unobserved","observed_at":at}); };
    let Some(types) = raw["types"].as_object().filter(|m| m.len() <= 100 && m.iter().all(|(k,v)| k.len() <= 128 && v.as_u64().is_some())) else {
        return json!({"state":"unobserved","observed_at":at});
    };
    let latest = match &raw["latest_received_at"] {
        Value::Null => Value::Null,
        Value::String(s) if s.len() <= 64 && chrono::DateTime::parse_from_rfc3339(s).is_ok() => json!(s),
        _ => return json!({"state":"unobserved","observed_at":at}),
    };
    json!({"state":"observed","pending_count":count,"types":types,"latest_received_at":latest,"observed_at":at})
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn unsafe_old_servers_and_secrets_are_excluded() {
        assert!(!supported(&json!({"result":{"tools":[{"name":"get_pending_status"}]}})));
        assert!(supported(&json!({"result":{"tools":[{"name":"get_pending_status","_meta":{META:1}}]}})));
        let m = json!({"result":{"content":[{"text":json!({"pending_count":0,"types":{},"latest_received_at":null,"token":"secret"}).to_string()}]}});
        let s = snapshot(&m,42); assert_eq!(s["pending_count"],0); assert!(!s.to_string().contains("secret"));
        assert_eq!(snapshot(&json!({"error":{"message":"secret"}}),42)["state"],"unobserved");
        assert_eq!(snapshot(&json!({"result":{"isError":true,"content":[{"text":"{\"error\":\"auth_required\"}"}]}}),42)["state"],"auth_required");
    }
}
