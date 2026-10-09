//! Notification only (#355). Neither a reset nor a child changes the parent gate.
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
pub const OWNER_ENV: &str = "PULLCEPT_LIMIT_NOTICE_OWNER";
pub const OWNER: &str = "app-v1";
pub const PATH: &str = "/hooks/claude-limit";
pub fn owner(nonce: Option<&str>, args: &[String], registered: bool) -> &'static str {
    if registered && nonce.is_some_and(|n| !n.is_empty()) && !super::declares_settings(args) {
        OWNER
    } else {
        ""
    }
}
pub fn url(port: u16, topic: &str, account: &str) -> String {
    format!(
        "http://127.0.0.1:{port}{PATH}/{}/{}",
        super::percent_encode(topic),
        super::percent_encode(account)
    )
}
pub fn target(path: &str) -> Option<(String, String)> {
    let path = path.split('?').next()?;
    let seat = path.strip_prefix(PATH)?.strip_prefix('/')?;
    super::parse_status_hook_target(&format!("/hooks/status/{seat}"))
}
pub fn reset(value: &Value, at: i64) -> Option<i64> {
    let quota = if value["quotaLimits"].is_object() {
        &value["quotaLimits"]
    } else {
        &value["message"]["quotaLimits"]
    };
    if quota["status"] != "rejected" {
        return None;
    }
    quota["resetsAt"]
        .as_i64()
        .filter(|t| *t >= at / 1000 && *t <= at / 1000 + 8 * 86400)
}
#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub subject: String,
    pub reset: Option<i64>,
    pub episode: Option<String>,
}
/// Only rate_limit StopFailure, validated by the caller's launch/parent/PTY gate.
pub fn child(body: &[u8]) -> Option<Stop> {
    let v: Value = serde_json::from_slice(body).ok()?;
    if v["hook_event_name"] != "StopFailure" || v["error"] != "rate_limit" {
        return None;
    }
    let id = v["agent_id"].as_str().filter(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })?;
    Some(Stop {
        subject: format!("child:{id}"),
        reset: None,
        episode: None,
    })
}
/// Optional child metadata. Adopt only the expected parent's own subagent file.
/// Reads at most 4 MiB, skips its cut first line, never keeps conversational text.
pub fn enrich(root: &Path, parent: &str, stop: &mut Stop, body: &[u8], floor: i64) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(reported_parent) = v["transcript_path"].as_str() else {
        return;
    };
    let Some(parent_path) = super::claude_limit::checked(root, parent, Path::new(reported_parent))
    else {
        return;
    };
    let agent = stop.subject.strip_prefix("child:").unwrap_or_default();
    let expected = parent_path
        .parent()
        .unwrap()
        .join(parent)
        .join("subagents")
        .join(format!("agent-{agent}.jsonl"));
    let reported = v["agent_transcript_path"]
        .as_str()
        .map(Path::new)
        .unwrap_or(&expected);
    let Ok(root) = root.canonicalize() else {
        return;
    };
    let Ok(path) = reported.canonicalize() else {
        return;
    };
    if expected.canonicalize().ok().as_ref() != Some(&path) {
        return;
    }
    let Ok(relative) = path.strip_prefix(&root) else {
        return;
    };
    let parts: Vec<_> = relative.iter().collect();
    if parts.len() != 4
        || parts[1] != parent
        || parts[2] != "subagents"
        || parts[3] != format!("agent-{agent}.jsonl").as_str()
    {
        return;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let Ok(meta) = file.metadata() else {
        return;
    };
    let offset = meta.len().saturating_sub(4 * 1024 * 1024);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return;
    }
    let mut bytes = Vec::new();
    if file.take(4 * 1024 * 1024).read_to_end(&mut bytes).is_err() {
        return;
    }
    let mut latest = floor - 1;
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if i == 0 && offset != 0 {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(at) = super::claude_limit::timestamp(&v["timestamp"]) else {
            continue;
        };
        if at < floor || at < latest || v["type"] != "assistant" {
            continue;
        }
        latest = at;
        if v["isApiErrorMessage"] == true && v["error"] == "rate_limit" {
            stop.reset = reset(&v, at);
        } else if v["isApiErrorMessage"].is_null() || v["isApiErrorMessage"] == false {
            if v["requestId"].as_str().is_some()
                && v["message"]["model"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("claude-"))
            {
                stop.episode = v["uuid"].as_str().map(str::to_string);
                stop.reset = None;
            }
        }
    }
}
#[derive(Default)]
pub struct Notices {
    episodes: HashMap<String, Episode>,
}
struct Episode {
    key: Option<String>,
    reset: Option<i64>,
    reached: bool,
}
impl Notices {
    pub fn known_reset(&self, subject: &str) -> Option<i64> {
        self.episodes.get(subject).and_then(|e| e.reset)
    }
    /// Retry rejection is one episode until a normal response or changed child
    /// normal-response identity. Unknown reset remains unknown, without a timer.
    pub fn stop(&mut self, stop: &Stop) -> bool {
        if let Some(old) = self.episodes.get_mut(&stop.subject) {
            if old.key.is_none() || stop.episode.is_none() || stop.episode == old.key {
                if old.key.is_none() {
                    old.key = stop.episode.clone();
                }
                if stop.reset.is_some() {
                    old.reset = stop.reset;
                }
                return false;
            }
        }
        self.episodes.insert(
            stop.subject.clone(),
            Episode {
                key: stop.episode.clone(),
                reset: stop.reset,
                reached: false,
            },
        );
        true
    }
    pub fn recovered_parent(&mut self) {
        self.episodes.remove("parent");
    }
    pub fn due(&mut self, now: i64) -> Vec<String> {
        self.episodes
            .iter_mut()
            .filter_map(|(subject, episode)| {
                if !episode.reached && episode.reset.is_some_and(|t| now >= t) {
                    episode.reached = true;
                    Some(subject.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}
pub fn stopped(name: &str, stop: &Stop, clock: Option<&str>) -> String {
    let who = if stop.subject == "parent" {
        format!("{name} の親セッション")
    } else {
        format!(
            "{name} のサブエージェント（{}）",
            stop.subject.trim_start_matches("child:")
        )
    };
    let reset = clock
        .map(|c| format!("解除予定は {c} です。"))
        .unwrap_or_else(|| "解除予定は不明です。".into());
    let action = if stop.subject == "parent" {
        "親への部屋の投稿を保留します。端末で手動再開し、正常親応答を確認した後にまとめを渡します。"
    } else {
        "子の停止だけでは親への部屋の投稿を保留しません。"
    };
    format!("{who} は利用上限で停止しました。{reset}{action}")
}
pub fn reached(name: &str, subject: &str) -> String {
    let who = if subject == "parent" {
        format!("{name} の親セッション")
    } else {
        format!(
            "{name} のサブエージェント（{}）",
            subject.trim_start_matches("child:")
        )
    };
    let action = if subject == "parent" {
        "正常親応答を確認するまで親の投稿保留は解除しません。"
    } else {
        "子の予定時刻だけでは親の停止・回復を判定しません。"
    };
    format!("{who} の解除予定時刻になりました。回復は未確認です。端末で再試行できます。{action}")
}
pub fn reset_known(name: &str, stop: &Stop, clock: &str) -> String {
    let who = if stop.subject == "parent" {
        format!("{name} の親セッション")
    } else {
        format!(
            "{name} のサブエージェント（{}）",
            stop.subject.trim_start_matches("child:")
        )
    };
    format!("{who} の解除予定は {clock} と分かりました。回復は未確認です。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn parent_hook_is_ignored_and_child_has_no_gate_authority() {
        let mut v = json!({"hook_event_name":"StopFailure","error":"rate_limit"});
        assert!(child(&serde_json::to_vec(&v).unwrap()).is_none());
        v["agent_id"] = json!("agent-1");
        assert_eq!(
            child(&serde_json::to_vec(&v).unwrap()).unwrap().subject,
            "child:agent-1"
        );
        for (key, val) in [
            ("error", "overloaded"),
            ("hook_event_name", "Notification"),
            ("agent_id", "../other"),
        ] {
            let mut other = v.clone();
            other[key] = json!(val);
            assert!(child(&serde_json::to_vec(&other).unwrap()).is_none());
        }
    }
    #[test]
    fn parent_child_episodes_reset_and_recovery_are_distinct() {
        let mut notices = Notices::default();
        let parent = Stop {
            subject: "parent".into(),
            reset: Some(50),
            episode: None,
        };
        assert!(notices.stop(&parent));
        assert!(!notices.stop(&parent));
        let child = Stop {
            subject: "child:a".into(),
            reset: Some(50),
            episode: None,
        };
        assert!(notices.stop(&child));
        assert!(notices.due(49).is_empty());
        assert_eq!(notices.due(50).len(), 2);
        assert!(notices.due(51).is_empty());
        notices.recovered_parent();
        assert!(notices.stop(&parent));
        assert!(!notices.stop(&child));
        let next = Stop {
            episode: Some("new-normal".into()),
            ..child
        };
        assert!(!notices.stop(&next)); // First metadata adoption refines the same stop.
        assert!(notices.stop(&Stop {
            episode: Some("another-normal".into()),
            ..next
        }));
        assert!(reached("seat", "parent").contains("回復は未確認"));
    }
    #[test]
    fn unknown_or_invalid_reset_does_not_schedule() {
        let mut n = Notices::default();
        n.stop(&Stop {
            subject: "parent".into(),
            reset: None,
            episode: None,
        });
        assert!(n.due(i64::MAX).is_empty());
        let v = json!({"quotaLimits":{"status":"rejected","resetsAt":110}});
        assert_eq!(reset(&v, 100000), Some(110));
        assert_eq!(reset(&v, 111000), None);
        assert_eq!(
            reset(
                &json!({"quotaLimits":{"status":"allowed","resetsAt":110}}),
                100000
            ),
            None
        );
        assert!(stopped(
            "seat",
            &Stop {
                subject: "parent".into(),
                reset: None,
                episode: None
            },
            None
        )
        .contains("解除予定は不明"));
    }
    #[test]
    fn route_is_notification_only_and_requires_both_seat_keys() {
        assert_eq!(
            target("/hooks/claude-limit/t/a"),
            Some(("t".into(), "a".into()))
        );
        for p in [
            "/hooks/claude-limit/t",
            "/hooks/activity/StopFailure/t/a",
            "/hooks/claude-limit/t/a/extra",
        ] {
            assert!(target(p).is_none());
        }
    }
    #[test]
    fn owner_marker_requires_the_app_reporter_to_be_registered() {
        assert_eq!(owner(Some("new-launch"), &[], true), OWNER);
        for args in [
            vec!["--settings".into(), "{}".into()],
            vec!["--settings={}".into()],
        ] {
            assert_eq!(owner(Some("new-launch"), &args, true), "");
        }
        assert_eq!(owner(None, &[], true), "");
        assert_eq!(owner(Some("launch"), &[], false), "");
    }
    #[test]
    fn root_and_message_reset_metadata_keep_parent_gate_and_hook_order_separate() {
        let mut v = json!({"timestamp":"1970-01-01T00:01:40Z","type":"assistant","sessionId":"parent", "isSidechain":false,
            "uuid":"reject","requestId":"req","isApiErrorMessage":true,"apiErrorStatus":429,"error":"rate_limit",
            "quotaLimits":{"status":"rejected","resetsAt":110},"message":{"model":"<synthetic>"}});
        for nested in [false, true] {
            if nested {
                v["message"]["quotaLimits"] = v["quotaLimits"].take();
            }
            let event = super::super::claude_limit::event("parent", &v).unwrap();
            assert!(matches!(
                event,
                super::super::claude_limit::Event::Rejected {
                    reset: Some(110),
                    ..
                }
            ));
            let mut gate = super::super::claude_limit::Gate::new(100000, false);
            let mut notices = Notices::default();
            let hook=serde_json::to_vec(&json!({"session_id":"parent","hook_event_name":"StopFailure","error":"rate_limit"})).unwrap();
            assert!(child(&hook).is_none()); // Hook first creates no parent announcement.
            assert!(gate.state.apply(event.clone()));
            let stop = Stop {
                subject: "parent".into(),
                reset: Some(110),
                episode: None,
            };
            assert!(notices.stop(&stop));
            assert!(!gate.state.apply(event));
            assert!(!notices.stop(&stop));
            assert!(child(&hook).is_none()); // Watcher first also creates no second notice.
            assert_eq!(notices.due(110), vec!["parent"]);
            assert!(gate.is_limited());
            assert!(notices.due(111).is_empty());
        }
    }
    #[test]
    fn child_reset_and_episode_require_own_root_new_metadata() {
        let root = std::env::temp_dir().join(format!("pullcept-notice-{}", uuid::Uuid::new_v4()));
        let childdir = root.join("project/parent/subagents");
        std::fs::create_dir_all(&childdir).unwrap();
        let path = childdir.join("agent-a.jsonl");
        let parent_path = root.join("project/parent.jsonl");
        std::fs::write(&parent_path, "").unwrap();
        let success = json!({"type":"assistant","timestamp":"1970-01-01T00:01:41Z","uuid":"normal-a","requestId":"r","message":{"model":"claude-test"}});
        let rejected = json!({"type":"assistant","timestamp":"1970-01-01T00:01:42Z","isApiErrorMessage":true,"error":"rate_limit","quotaLimits":{"status":"rejected","resetsAt":110}});
        std::fs::write(&path, format!("{success}\n{rejected}\n")).unwrap();
        let body=serde_json::to_vec(&json!({"session_id":"parent","hook_event_name":"StopFailure","error":"rate_limit","agent_id":"a","agent_type":"test","transcript_path":parent_path})).unwrap();
        let mut stop = child(&body).unwrap();
        enrich(&root, "parent", &mut stop, &body, 100000);
        assert_eq!(stop.reset, Some(110));
        assert_eq!(stop.episode, Some("normal-a".into()));
        let mut explicit: Value = serde_json::from_slice(&body).unwrap();
        explicit["agent_transcript_path"] = json!(path);
        let mut same = child(&body).unwrap();
        enrich(
            &root,
            "parent",
            &mut same,
            &serde_json::to_vec(&explicit).unwrap(),
            100000,
        );
        assert_eq!(same, stop);
        explicit["transcript_path"] = json!(root.join("project/foreign.jsonl"));
        let mut invalid = child(&body).unwrap();
        enrich(
            &root,
            "parent",
            &mut invalid,
            &serde_json::to_vec(&explicit).unwrap(),
            100000,
        );
        assert_eq!(invalid.reset, None);
        let mut old = child(&body).unwrap();
        enrich(&root, "parent", &mut old, &body, 200000);
        assert_eq!(old.reset, None);
        assert_eq!(old.episode, None);
        let mut foreign = child(&body).unwrap();
        enrich(&root, "other-parent", &mut foreign, &body, 100000);
        assert_eq!(foreign.reset, None);
        assert!(!super::super::claude_limit::reporter_matches(
            "parent",
            "new",
            Some("old"),
            &body
        ));
        let gate = super::super::claude_limit::Gate::new(100000, false);
        let mut notices = Notices::default();
        assert!(notices.stop(&stop));
        assert!(!notices.stop(&stop));
        notices.due(110);
        assert!(!gate.is_limited());
        std::fs::remove_dir_all(root).unwrap();
    }
}
