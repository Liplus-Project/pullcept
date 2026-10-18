//! Safe, launch-bound projection for the room's read-only status tool (#474).
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Default)]
pub struct Book {
    seats: HashMap<(String, String, String), Held>,
    output: HashMap<String, i64>,
    launches: HashMap<(String, String), String>,
}
#[derive(Default)]
struct Held {
    fields: HashMap<String, Value>,
    activity: Option<(Value, i64)>,
}

impl Book {
    pub fn begin(&mut self, topic: &str, account: &str) {
        self.seats.retain(|(t, a, _), _| t != topic || a != account);
        self.launches.remove(&(topic.into(), account.into()));
    }
    pub fn bind(&mut self, topic: &str, account: &str, pty: &str) {
        self.launches
            .insert((topic.into(), account.into()), pty.into());
    }
    pub fn matches(&self, topic: &str, account: &str, pty: &str) -> bool {
        self.launches
            .get(&(topic.into(), account.into()))
            .is_some_and(|p| p == pty)
    }
    pub fn forget_topic(&mut self, topic: &str) {
        self.seats.retain(|(t, _, _), _| t != topic);
        self.launches.retain(|(t, _), _| t != topic);
    }
    fn seat(&mut self, topic: &str, account: &str, pty: &str) -> &mut Held {
        self.seats
            .entry((topic.into(), account.into(), pty.into()))
            .or_default()
    }
    /// Emit sites replay held stats. Timestamp changes, not those replays.
    pub fn stats(
        &mut self,
        topic: &str,
        account: &str,
        pty: &str,
        stats: &Value,
        model_reported: bool,
        at: i64,
    ) {
        if !self.matches(topic, account, pty) {
            return;
        }
        let held = self.seat(topic, account, pty);
        for key in ["model", "effort", "limited", "limited_source"] {
            let Some(value) = stats.get(key).cloned() else {
                continue;
            };
            // Model snapshots replace nulls too; limit-only snapshots do not.
            if ((key == "model" || key == "effort") && !model_reported)
                || (value.is_null() && key != "model" && key != "effort")
            {
                continue;
            }
            if held.fields.get(key).is_none_or(|old| old["value"] != value) {
                held.fields
                    .insert(key.into(), json!({"value": value, "changed_at": at}));
            }
        }
    }
    /// Only fixed display words cross the boundary; never the long line/tool text.
    pub fn activity(&mut self, event: &Value, at: i64) {
        let (Some(topic), Some(account), Some(pty)) = (
            event["topic_id"].as_str(),
            event["account_id"].as_str(),
            event["pty_id"].as_str(),
        ) else {
            return;
        };
        if !self.matches(topic, account, pty) {
            return;
        }
        let word = event["word"].as_str().filter(|word| {
            matches!(
                *word,
                "許可待ち" | "答え待ち" | "実行中" | "編集中" | "ツール" | "委任中"
            )
        });
        let safe = json!({"connected": event["connected"].as_bool(),
            "thread_status": event["thread_status"].as_str().filter(|s| matches!(*s, "idle" | "active")),
            "word": word, "waiting": event["waiting"].as_bool()});
        let held = self.seat(topic, account, pty);
        if held.activity.as_ref().is_none_or(|(old, _)| old != &safe) {
            held.activity = Some((safe, at));
        }
    }
    pub fn output(&mut self, pty: &str, at: i64) {
        self.output.insert(pty.into(), at);
    }
    pub fn ended(&mut self, pty: &str) {
        self.output.remove(pty);
        self.launches.retain(|_, p| p != pty);
        self.seats.retain(|(_, _, p), _| p != pty);
    }

    pub fn project(
        &self,
        topic: &str,
        account: &str,
        pty: Option<&str>,
        now: i64,
        usage: Value,
        detail: bool,
    ) -> Value {
        let Some(pty) = pty else {
            let mut stopped = json!({"process_running": false, "activity": null, "limited": null, "usage_received_at": usage["received_at"]});
            if detail {
                stopped["usage"] = usage;
            }
            return stopped;
        };
        let held = self.seats.get(&(topic.into(), account.into(), pty.into()));
        let field = |key: &str| {
            held.and_then(|h| h.fields.get(key))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let reported = held.and_then(|h| h.activity.as_ref());
        let limited = field("limited")["value"].as_bool().or_else(|| {
            let windows: Vec<_> = ["five_hour", "seven_day"]
                .iter()
                .filter_map(|k| usage[k]["used_percentage"].as_f64())
                .collect();
            if windows.iter().any(|n| *n >= 100.) {
                Some(true)
            } else if windows.len() == 2 {
                Some(false)
            } else {
                None
            }
        });
        let output_at = self.output.get(pty).copied();
        let outputting = output_at.is_some_and(|at| now.saturating_sub(at) < 1000);
        let source = field("limited_source");
        let authoritative = matches!(source["value"].as_str(), Some("codex" | "claude-parent"))
            && limited == Some(true);
        // Same precedence as seat-status.ts. The webview-only awaitingReply
        // overlay is deliberately absent: output says bytes, not reasoning.
        let word = if authoritative {
            Some("制限中")
        } else if let Some(word) = reported.and_then(|(a, _)| a["word"].as_str()) {
            Some(word)
        } else if reported
            .is_some_and(|(a, _)| a["connected"] == true && a["thread_status"] == "idle")
        {
            Some(if limited == Some(true) {
                "制限中"
            } else {
                "待機"
            })
        } else if outputting {
            Some("出力中")
        } else if limited == Some(true) {
            Some("制限中")
        } else if reported.is_some_and(|(a, _)| a["connected"] == false) {
            Some("様子不明")
        } else if output_at.is_some() {
            Some("待機")
        } else {
            None
        };
        let activity_source = if authoritative {
            "limit_state"
        } else if reported.is_some_and(|(a, _)| a["word"].as_str().is_some()) {
            "structured"
        } else if reported
            .is_some_and(|(a, _)| a["connected"] == true && a["thread_status"] == "idle")
        {
            if limited == Some(true) {
                "usage_or_limit_state"
            } else {
                "structured"
            }
        } else if outputting {
            "pty_output"
        } else if limited == Some(true) {
            "usage_or_limit_state"
        } else if reported.is_some_and(|(a, _)| a["connected"] == false) {
            "structured"
        } else if output_at.is_some() {
            "pty_output"
        } else {
            "unknown"
        };
        let (source_at, source_time_kind) = match activity_source {
            "structured" => (reported.map(|(_, at)| *at), "changed"),
            "pty_output" => (output_at, "output_received"),
            "limit_state" => (field("limited")["changed_at"].as_i64(), "changed"),
            "usage_or_limit_state" if field("limited")["value"].is_boolean() => {
                (field("limited")["changed_at"].as_i64(), "changed")
            }
            "usage_or_limit_state" => (
                usage["received_at"]
                    .as_i64()
                    .map(|at| at.saturating_mul(1000)),
                "usage_received",
            ),
            _ => (None, "unknown"),
        };
        let mut result = json!({"process_running": true, "limited": limited,
            "usage_received_at": usage["received_at"],
            "activity": {"word": word, "reported": reported.map(|(a, _)| a),
                "source": activity_source,
                "reported_changed_at": reported.map(|(_, at)| at), "last_output_at": output_at,
                "source_at": source_at, "source_time_kind": source_time_kind,
                "output_quiet_ms": 1000, "awaiting_reply_overlay": false},
            "limit_state": field("limited")});
        if detail {
            result["model"] = field("model");
            result["effort"] = field("effort");
            result["usage"] = usage;
        }
        result
    }
}

/// Preserve roster connection identity; filtering never searches configured accounts.
pub fn select(participants: Vec<Value>, account: Option<&str>) -> Result<Vec<Value>, String> {
    let rows: Vec<_> = participants
        .into_iter()
        .filter(|p| account.is_none_or(|a| p["account_id"] == a))
        .collect();
    if account.is_some() && rows.is_empty() {
        return Err("account_id is not a participant in this topic".into());
    }
    Ok(rows)
}

/// The socket supplies authorization and its joined topic, never a frame topic.
pub fn request(
    authorize: Result<(), String>,
    joined_topic: Option<&str>,
    read: impl FnOnce(&str) -> Result<Value, String>,
) -> Result<Value, String> {
    authorize?;
    read(joined_topic.ok_or("No requester topic")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authorization_and_join_precede_reads_without_calling_operations() {
        use crate::session_control::Connection;
        let c = Connection {
            socket: "s".into(),
            topic: "own".into(),
            account: "a".into(),
            generation: Some("g".into()),
            launch: Some("l".into()),
        };
        for (socket, live, generation, launch) in [
            ("old", true, Some("g"), Some("l")),
            ("s", false, Some("g"), Some("l")),
            ("s", true, Some("g"), Some("new")),
        ] {
            assert!(request(
                c.authorize(socket, live, generation, launch),
                Some("own"),
                |_| panic!("unauthorized read")
            )
            .is_err());
        }
        let legacy = Connection {
            launch: None,
            ..c.clone()
        };
        assert!(request(
            legacy.authorize("s", true, Some("new"), None),
            Some("own"),
            |_| panic!("old generation read")
        )
        .is_err());
        assert!(request(Ok(()), None, |_| panic!("unjoined read")).is_err());
        let answer = request(
            c.authorize("s", true, Some("g"), Some("l")),
            Some(&c.topic),
            |topic| {
                assert_eq!(topic, "own");
                Ok(json!({"topic":topic}))
            },
        )
        .unwrap();
        assert_eq!(answer["topic"], "own");
    }
    #[test]
    fn connection_identity_and_topic_filter_are_not_names_or_config_accounts() {
        let rows = vec![
            json!({"id":"human", "account_id":"h", "name":"same", "role":"admin"}),
            json!({"id":"one", "account_id":"ai", "name":"same"}),
            json!({"id":"two", "account_id":null, "name":"same"}),
        ];
        assert_eq!(select(rows.clone(), None).unwrap().len(), 3);
        assert_eq!(select(rows.clone(), Some("h")).unwrap()[0]["role"], "admin");
        assert!(select(rows, Some("other-topic-or-config-only")).is_err());
    }
    #[test]
    fn freshness_replays_privacy_and_restart() {
        let mut b = Book::default();
        b.bind("t", "a", "p");
        let stats = json!({"model":"m", "effort":"high", "limited":true, "limited_source":"codex"});
        b.stats("t", "a", "p", &stats, true, 10);
        b.stats("t", "a", "p", &stats, true, 20);
        b.stats("t", "a", "p", &json!({"model":null,"effort":null,"five_hour":null,"context":null,"limited":true,"limited_source":"codex","pty_id":"p"}), false, 30);
        b.activity(&json!({"topic_id":"t","account_id":"a","pty_id":"p","connected":true,"word":"ツール","line":"secret command /path","arguments":"secret"}), 11);
        let p = b.project("t", "a", Some("p"), 100, json!({"received_at":3}), true);
        assert_eq!(p["model"]["changed_at"], 10);
        assert_eq!(p["effort"]["value"], "high");
        assert_eq!(p["effort"]["changed_at"], 10);
        assert_eq!(p["usage"]["received_at"], 3);
        assert_eq!(p["activity"]["word"], "制限中");
        assert!(!p.to_string().contains("secret"));
        assert_eq!(
            b.project("t", "a", Some("new"), 100, json!({}), true)["activity"]["word"],
            Value::Null
        );
        assert_eq!(
            b.project("t", "a", None, 100, json!({}), true)["limited"],
            Value::Null
        );
        b.begin("t", "a");
        assert_eq!(
            b.project("t", "a", Some("p"), 100, json!({}), true)["model"],
            Value::Null
        );
        b.bind("t", "a", "new");
        b.stats("t", "a", "new", &json!({"model":"new-model"}), true, 40);
        b.stats("t", "a", "p", &stats, true, 50);
        assert_eq!(
            b.project("t", "a", Some("new"), 100, json!({}), true)["model"]["value"],
            "new-model"
        );
        assert_eq!(
            b.project("other", "a", Some("new"), 100, json!({}), true)["model"],
            Value::Null
        );
    }
    #[test]
    fn full_model_snapshots_clear_unknown_effort_but_partial_nulls_preserve_clocks() {
        #[derive(serde::Serialize)]
        struct Model<'a> {
            model: Option<&'a str>,
            effort: Option<&'a str>,
            limited: Option<bool>,
        }
        let mut b = Book::default();
        b.bind("t", "a", "p");
        let snapshot = |model, effort, limited| {
            serde_json::to_value(Model {
                model,
                effort,
                limited,
            })
            .unwrap()
        };
        b.stats(
            "t",
            "a",
            "p",
            &snapshot(Some("reasoning"), Some("high"), None),
            true,
            10,
        );
        let partial = snapshot(None, None, Some(true));
        assert!(partial.get("model").unwrap().is_null());
        b.stats("t", "a", "p", &partial, false, 20);
        let p = b.project("t", "a", Some("p"), 100, json!({}), true);
        assert_eq!(p["model"]["changed_at"], 10);
        assert_eq!(p["effort"]["value"], "high");
        b.stats(
            "t",
            "a",
            "p",
            &snapshot(Some("no-effort"), None, None),
            true,
            30,
        );
        let p = b.project("t", "a", Some("p"), 100, json!({}), true);
        assert_eq!(p["model"]["value"], "no-effort");
        assert_eq!(p["effort"], json!({"value":null,"changed_at":30}));
        b.stats("t", "a", "p", &snapshot(None, None, None), true, 40);
        assert_eq!(
            b.project("t", "a", Some("p"), 100, json!({}), true)["model"],
            json!({"value":null,"changed_at":40})
        );
    }
    #[test]
    fn only_explicitly_bound_launch_is_observed_even_while_starting() {
        let mut b = Book::default();
        b.bind("t", "a", "old");
        b.begin("t", "a");
        assert!(!b.matches("t", "a", "old"));
        b.stats("t", "a", "old", &json!({"model":"stale"}), true, 20);
        b.activity(
            &json!({"topic_id":"t","account_id":"a","pty_id":"old","word":"ツール"}),
            20,
        );
        b.bind("t", "a", "new");
        assert!(b.matches("t", "a", "new"));
        b.stats("t", "a", "old", &json!({"model":"stale"}), true, 30);
        b.activity(&json!({"topic_id":"t","account_id":"a","pty_id":"new","connected":true,"thread_status":"idle"}), 30);
        let p = b.project("t", "a", Some("new"), 100, json!({}), true);
        assert!(p["model"].is_null());
        assert_eq!(p["activity"]["word"], "待機");
        b.ended("old");
        assert!(b.matches("t", "a", "new"));
        b.ended("new");
        assert!(!b.matches("t", "a", "new"));
    }
    #[test]
    fn usage_limits_and_stopped_details_keep_original_usage_clocks() {
        let mut b = Book::default();
        b.bind("t", "a", "p");
        b.activity(
            &json!({"topic_id":"t","account_id":"a","pty_id":"p","connected":false}),
            9000,
        );
        let usage = json!({"five_hour":{"used_percentage":100},"received_at":2,"context":42,"context_received_at":3});
        let p = b.project("t", "a", Some("p"), 10000, usage.clone(), true);
        assert_eq!(p["activity"]["word"], "制限中");
        assert_eq!(p["activity"]["source_at"], 2000);
        assert_eq!(p["activity"]["source_time_kind"], "usage_received");
        assert_eq!(p["activity"]["reported_changed_at"], 9000);
        b.activity(&json!({"topic_id":"t","account_id":"a","pty_id":"p","connected":true,"thread_status":"idle"}), 9500);
        let idle_limited = b.project("t", "a", Some("p"), 10000, usage.clone(), false);
        assert_eq!(idle_limited["activity"]["source_at"], 2000);
        assert_eq!(idle_limited["activity"]["word"], "制限中");
        let stopped = b.project("t", "a", None, 10000, usage.clone(), true);
        assert_eq!(stopped["usage"], usage);
        assert!(stopped["activity"].is_null());
        assert!(stopped["model"].is_null());
    }
    #[test]
    fn structured_words_idle_output_disconnect_missing_and_limit_precedence() {
        let mut b = Book::default();
        b.bind("t", "a", "p");
        assert_eq!(
            b.project("t", "a", Some("p"), 100, json!({}), false)["activity"]["word"],
            Value::Null
        );
        b.output("p", 100);
        assert_eq!(
            b.project("t", "a", Some("p"), 101, json!({}), false)["activity"]["word"],
            "出力中"
        );
        assert_eq!(
            b.project("t", "a", Some("p"), 1100, json!({}), false)["activity"]["word"],
            "待機"
        );
        for (connected, thread, word, expected) in [
            (true, "active", Some("許可待ち"), "許可待ち"),
            (true, "idle", None, "待機"),
            (false, "active", None, "出力中"),
        ] {
            b.activity(&json!({"topic_id":"t","account_id":"a","pty_id":"p","connected":connected,"thread_status":thread,"word":word}),1);
            assert_eq!(
                b.project("t", "a", Some("p"), 101, json!({}), false)["activity"]["word"],
                expected
            );
        }
        assert_eq!(
            b.project("t", "a", Some("p"), 1100, json!({}), false)["activity"]["word"],
            "様子不明"
        );
        b.forget_topic("t");
        b.ended("p");
        assert_eq!(
            b.project("t", "a", Some("p"), 1100, json!({}), false)["activity"]["word"],
            Value::Null
        );
    }
}
