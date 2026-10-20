//! Durable topic work and human decisions. Identity and answer evidence are supplied by the host.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation_id: String,
    pub id: String,
    pub action: String,
    pub expected_revision: u64,
    pub kind: Option<String>,
    pub text: Option<String>,
    pub progress: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    pub source_message_id: Option<String>,
    pub answer_message_id: Option<String>,
    pub target_account_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub owner: String,
    pub added_by: String,
    pub text: String,
    pub progress: Option<String>,
    pub references: Vec<String>,
    pub source_message_id: Option<String>,
    pub target_account_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub state: String,
    pub answer: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    request: Request,
    actor: String,
    result: Record,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Book {
    pub records: BTreeMap<String, Record>,
    receipts: BTreeMap<String, Receipt>,
}
pub struct Actor<'a> {
    pub account: &'a str,
    pub name: &'a str,
    pub human: bool,
}
fn bounded(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.contains('\0')
}
fn evidence_key(answer: Option<&Value>) -> Value {
    answer.map(|a| json!({"content":a["content"],"account":a["account"],"message_id":a["message_id"],"source":a["source"]})).unwrap_or(Value::Null)
}
impl Book {
    pub fn validate(&self) -> Result<(), String> {
        for (id, r) in &self.records {
            if id != &r.id
                || !bounded(id, 128)
                || !bounded(&r.owner, 128)
                || !bounded(&r.text, 8000)
                || r.revision == 0
                || !matches!(r.kind.as_str(), "work" | "decision")
                || !matches!(
                    r.state.as_str(),
                    "active" | "completed" | "cancelled" | "answered"
                )
                || (r.kind == "decision"
                    && r.target_account_id
                        .as_deref()
                        .is_none_or(|a| !bounded(a, 128)))
                || (r.state == "answered"
                    && (r.kind != "decision"
                        || r.answer.as_ref().is_none_or(|a| {
                            a["account"].as_str() != r.target_account_id.as_deref()
                                || a["content"].as_str().is_none()
                        })))
            {
                return Err("Invalid work journal record; refusing to overwrite".into());
            }
        }
        Ok(())
    }
    /// Clone, validate, persist, then commit: a failed write never becomes a successful memory mutation.
    pub fn apply(
        &mut self,
        r: Request,
        actor: Actor<'_>,
        answer: Option<Value>,
        at: &str,
        persist: impl FnOnce(&Book) -> Result<(), String>,
    ) -> Result<Record, String> {
        if !bounded(&r.operation_id, 128) || !bounded(&r.id, 128) {
            return Err("Invalid operation/id".into());
        }
        if let Some(old) = self.receipts.get(&r.operation_id) {
            return if old.request == r
                && old.actor == actor.account
                && evidence_key(old.result.answer.as_ref()) == evidence_key(answer.as_ref())
            {
                Ok(old.result.clone())
            } else {
                Err("operation_id already used".into())
            };
        }
        if r.references.len() > 20
            || r.references.iter().any(|s| !bounded(s, 2048))
            || r.text.as_ref().is_some_and(|s| !bounded(s, 8000))
            || r.progress.as_ref().is_some_and(|s| s.len() > 2000)
            || r.source_message_id
                .as_ref()
                .is_some_and(|s| !bounded(s, 128))
            || r.answer_message_id
                .as_ref()
                .is_some_and(|s| !bounded(s, 128))
            || r.target_account_id
                .as_ref()
                .is_some_and(|s| !bounded(s, 128))
        {
            return Err("Invalid or oversized content".into());
        }
        let mut next = self.clone();
        let existing = next.records.get(&r.id);
        if existing.map_or(0, |x| x.revision) != r.expected_revision {
            return Err("Stale revision; read status again".into());
        }
        if let Some(old) = existing {
            if old.state != "active" {
                return Err("Record is already in history".into());
            }
            if old.owner != actor.account
                && !(actor.human
                    && old.kind == "decision"
                    && old.target_account_id.as_deref() == Some(actor.account))
            {
                return Err("Only the owner may change this record".into());
            }
        } else if r.action != "put" {
            return Err("Unknown record".into());
        }
        if existing.is_none() {
            let kind = r.kind.as_deref().ok_or("kind required")?;
            if !matches!(kind, "work" | "decision") {
                return Err("Invalid kind".into());
            }
            next.records.insert(
                r.id.clone(),
                Record {
                    id: r.id.clone(),
                    kind: kind.into(),
                    owner: actor.account.into(),
                    added_by: actor.name.into(),
                    text: r.text.clone().ok_or("text required")?,
                    progress: r.progress.clone(),
                    references: r.references.clone(),
                    source_message_id: r.source_message_id.clone(),
                    target_account_id: r.target_account_id.clone(),
                    created_at: at.into(),
                    updated_at: at.into(),
                    revision: 0,
                    state: "active".into(),
                    answer: None,
                },
            );
        }
        let record = next.records.get_mut(&r.id).unwrap();
        match r.action.as_str() {
            "put" => {
                if r.kind.as_ref().is_some_and(|k| k != &record.kind) {
                    return Err("Cannot change kind".into());
                }
                if r.target_account_id
                    .as_ref()
                    .is_some_and(|a| Some(a) != record.target_account_id.as_ref())
                {
                    return Err("Cannot change the target human".into());
                }
                record.text = r.text.clone().ok_or("text required")?;
                record.progress = r.progress.clone();
                record.references = r.references.clone();
                record.source_message_id = r.source_message_id.clone();
            }
            "complete" if record.kind == "work" => record.state = "completed".into(),
            "cancel" => record.state = "cancelled".into(),
            "answer" if record.kind == "decision" => {
                let answer = answer.ok_or("Verified human answer required")?;
                if record.target_account_id.as_deref() != answer["account"].as_str() {
                    return Err("Answer is from a different human account".into());
                }
                record.answer = Some(answer);
                record.state = "answered".into();
            }
            _ => return Err("Invalid action for record kind".into()),
        }
        record.revision += 1;
        record.updated_at = at.into();
        let result = record.clone();
        next.receipts.insert(
            r.operation_id.clone(),
            Receipt {
                request: r,
                actor: actor.account.into(),
                result: result.clone(),
            },
        );
        persist(&next)?;
        *self = next;
        Ok(result)
    }
    pub fn snapshot(&self, account: Option<&str>, limit: usize) -> Value {
        let mut active = vec![];
        let mut history = vec![];
        for record in self.records.values().filter(|r| {
            account.is_none_or(|a| {
                r.owner == a || (r.kind == "decision" && r.target_account_id.as_deref() == Some(a))
            })
        }) {
            if record.state == "active" {
                active.push(record);
            } else {
                history.push(record);
            }
        }
        history.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let history_total = history.len();
        history.truncate(limit.min(100));
        json!({"active":active,"history":history,"history_limit":limit.min(100),"history_total":history_total,
            "pending_decisions":active.iter().filter(|r| r.kind == "decision").count()})
    }
}

/// Strict parent plan decoding; arbitrary payload fields never reach the status surface.
pub fn plan_steps(value: &Value) -> Result<Value, String> {
    let steps = value
        .as_array()
        .filter(|v| v.len() <= 100)
        .ok_or("Invalid plan")?;
    let mut out = Vec::new();
    for step in steps {
        let text = step["step"]
            .as_str()
            .filter(|s| bounded(s, 2000))
            .ok_or("Invalid plan step")?;
        let status = step["status"]
            .as_str()
            .filter(|s| matches!(*s, "pending" | "inProgress" | "completed"))
            .ok_or("Invalid plan status")?;
        out.push(json!({"step":text,"status":status}));
    }
    Ok(json!(out))
}

#[derive(Default)]
pub struct CodexPlan {
    turn: Option<String>,
}
impl CodexPlan {
    pub fn feed(&mut self, thread: &str, message: &Value) -> Option<Value> {
        let p = &message["params"];
        if p["threadId"] != thread {
            return None;
        }
        match message["method"].as_str()? {
            "turn/started" => {
                self.turn = p["turn"]["id"].as_str().map(str::to_string);
                None
            }
            "turn/completed" => {
                if p["turn"]["id"].as_str() == self.turn.as_deref() {
                    self.turn = None;
                }
                None
            }
            "turn/plan/updated"
                if self.turn.is_some() && p["turnId"].as_str() == self.turn.as_deref() =>
            {
                plan_steps(&p["plan"]).ok()
            }
            _ => None,
        }
    }
}
/// A Claude Code seat's plan, heard from its own PostToolUse hooks: TodoWrite
/// sends the whole list, while the task tools (`CLAUDE_CODE_ENABLE_TODO_TOOLS`,
/// #551) send one task at a time, so their list is kept here.
#[derive(Default)]
pub struct ClaudePlan {
    tasks: Vec<(String, String, String)>,
}
impl ClaudePlan {
    pub fn feed(&mut self, event: &str, body: &[u8]) -> Option<Value> {
        if event != "PostToolUse" {
            return None;
        }
        let p: Value = serde_json::from_slice(body).ok()?;
        if p["hook_event_name"] != event || !p["agent_id"].is_null() || p.get("error").is_some() {
            return None;
        }
        let status = |s: &Value| match s.as_str() {
            Some("pending") => "pending",
            Some("in_progress") => "inProgress",
            Some("completed") => "completed",
            _ => "invalid",
        };
        match p["tool_name"].as_str()? {
            "TodoWrite" => {
                let todos = p["tool_input"]["todos"].as_array()?;
                let steps = todos.iter().map(|t| json!({"step":t["content"],"status":status(&t["status"])})).collect::<Vec<_>>();
                return plan_steps(&json!(steps)).ok();
            }
            "TaskCreate" => {
                let id = p["tool_response"]["task"]["id"].as_str()?;
                let subject = p["tool_input"]["subject"].as_str()?;
                self.tasks.retain(|t| t.0 != id);
                self.tasks.push((id.into(), subject.into(), "pending".into()));
            }
            "TaskUpdate" => {
                if p["tool_response"]["success"] != true {
                    return None;
                }
                let input = &p["tool_input"];
                let id = input["taskId"].as_str()?;
                if input["status"] == "deleted" {
                    self.tasks.retain(|t| t.0 != id);
                } else {
                    let task = self.tasks.iter_mut().find(|t| t.0 == id)?;
                    if let Some(subject) = input["subject"].as_str() {
                        task.1 = subject.into();
                    }
                    if let Some(s) = input["status"].as_str() {
                        task.2 = s.into();
                    }
                }
            }
            _ => return None,
        }
        let steps = self.tasks.iter().map(|t| json!({"step":t.1,"status":status(&json!(t.2))})).collect::<Vec<_>>();
        plan_steps(&json!(steps)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, action: &str, revision: u64) -> Request {
        Request {
            operation_id: id.into(),
            id: "w".into(),
            action: action.into(),
            expected_revision: revision,
            kind: Some("work".into()),
            text: Some("task".into()),
            progress: None,
            references: vec![],
            source_message_id: None,
            answer_message_id: None,
            target_account_id: Some("human".into()),
        }
    }
    fn actor(account: &str) -> Actor<'_> {
        Actor {
            account,
            name: account,
            human: false,
        }
    }
    #[test]
    fn durable_retry_revision_and_ownership() {
        let mut b = Book::default();
        let r = request("a", "put", 0);
        assert!(b
            .apply(r.clone(), actor("owner"), None, "1", |_| Err("disk".into()))
            .is_err());
        assert!(b.records.is_empty());
        b.apply(r.clone(), actor("owner"), None, "1", |_| Ok(()))
            .unwrap();
        b.apply(r, actor("owner"), None, "2", |_| panic!("retry write"))
            .unwrap();
        assert!(b
            .apply(
                request("b", "put", 0),
                actor("owner"),
                None,
                "2",
                |_| Ok(())
            )
            .is_err());
        assert!(b
            .apply(
                request("b", "complete", 1),
                actor("other"),
                None,
                "2",
                |_| Ok(())
            )
            .is_err());
        let serialized = serde_json::to_vec(&b).unwrap();
        let mut restarted: Book = serde_json::from_slice(&serialized).unwrap();
        restarted
            .apply(
                request("b", "complete", 1),
                actor("owner"),
                None,
                "3",
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(restarted.snapshot(None, 1000)["history_limit"], 100);
        assert_eq!(
            restarted.snapshot(None, 10)["history"][0]["state"],
            "completed"
        );
        assert!(serde_json::from_slice::<Book>(b"{broken").is_err());
    }
    #[test]
    fn decision_answers_require_host_evidence_and_owner() {
        let mut b = Book::default();
        let mut r = request("a", "put", 0);
        r.kind = Some("decision".into());
        b.apply(r, actor("ai"), None, "1", |_| Ok(())).unwrap();
        let r = request("b", "answer", 1);
        assert!(b
            .apply(r.clone(), actor("ai"), None, "2", |_| Ok(()))
            .is_err());
        assert!(b
            .apply(
                r.clone(),
                actor("other"),
                Some(json!({"content":"forged"})),
                "2",
                |_| Ok(())
            )
            .is_err());
        b.apply(
            r,
            Actor {
                account: "human",
                name: "human",
                human: true,
            },
            Some(json!({"content":"yes","account":"human"})),
            "2",
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            b.snapshot(None, 10)["history"][0]["answer"]["content"],
            "yes"
        );
        assert_eq!(
            b.snapshot(Some("human"), 10)["history"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(b.snapshot(Some("other"), 10)["history"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn plan_is_bounded_and_allowlisted() {
        assert!(plan_steps(&json!([{"step":"a","status":"bad"}])).is_err());
        assert_eq!(
            plan_steps(&json!([{"step":"a","status":"pending","secret":"no"}])).unwrap(),
            json!([{"step":"a","status":"pending"}])
        );
    }
    #[test]
    fn parent_thread_turn_and_successful_parent_hook_only() {
        let mut p = CodexPlan::default();
        let plan = json!({"method":"turn/plan/updated","params":{"threadId":"parent","turnId":"one","plan":[{"step":"task","status":"pending"}]}});
        assert!(p.feed("parent", &plan).is_none());
        p.feed(
            "parent",
            &json!({"method":"turn/started","params":{"threadId":"parent","turn":{"id":"one"}}}),
        );
        assert!(p.feed("child", &plan).is_none());
        assert!(p.feed("parent", &plan).is_some());
        p.feed(
            "parent",
            &json!({"method":"turn/started","params":{"threadId":"parent","turn":{"id":"two"}}}),
        );
        assert!(p.feed("parent", &plan).is_none());
        let hook = json!({"hook_event_name":"PostToolUse","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"task","status":"in_progress"}]}});
        assert_eq!(
            ClaudePlan::default().feed("PostToolUse", &serde_json::to_vec(&hook).unwrap()).unwrap()[0]["status"],
            "inProgress"
        );
        assert!(ClaudePlan::default().feed("PostToolUseFailure", &serde_json::to_vec(&hook).unwrap()).is_none());
        let mut child = hook;
        child["agent_id"] = "child".into();
        assert!(ClaudePlan::default().feed("PostToolUse", &serde_json::to_vec(&child).unwrap()).is_none());
        assert!(ClaudePlan::default().feed("PostToolUse", b"malformed").is_none());
        let mut c = ClaudePlan::default();
        let hear = |c: &mut ClaudePlan, v: Value| c.feed("PostToolUse", &serde_json::to_vec(&v).unwrap());
        let create = |id: &str, s: &str| json!({"hook_event_name":"PostToolUse","tool_name":"TaskCreate","tool_input":{"subject":s},"tool_response":{"task":{"id":id,"subject":s}}});
        hear(&mut c, create("1", "A"));
        assert_eq!(hear(&mut c, create("2", "B")).unwrap(), json!([{"step":"A","status":"pending"},{"step":"B","status":"pending"}]));
        let update = |id: &str, input: Value, ok: bool| { let mut i = input; i["taskId"] = id.into(); json!({"hook_event_name":"PostToolUse","tool_name":"TaskUpdate","tool_input":i,"tool_response":{"success":ok}}) };
        assert_eq!(hear(&mut c, update("1", json!({"status":"in_progress"}), true)).unwrap()[0]["status"], "inProgress");
        assert!(hear(&mut c, update("1", json!({"status":"completed"}), false)).is_none());
        assert!(hear(&mut c, update("9", json!({"status":"completed"}), true)).is_none());
        assert_eq!(hear(&mut c, update("1", json!({"status":"deleted"}), true)).unwrap(), json!([{"step":"B","status":"pending"}]));
        let mut sub = create("3", "C");
        sub["agent_id"] = "child".into();
        assert!(hear(&mut c, sub).is_none());
    }
    #[test]
    fn human_target_ownership_filter_and_payload_identity() {
        let mut b = Book::default();
        let mut r = request("create", "put", 0);
        r.kind = Some("decision".into());
        b.apply(r.clone(), actor("ai"), None, "1", |_| Ok(()))
            .unwrap();
        assert_eq!(b.snapshot(Some("human"), 10)["pending_decisions"], 1);
        assert!(b.snapshot(Some("other-ai"), 10)["active"]
            .as_array()
            .unwrap()
            .is_empty());
        r.text = Some("changed retry".into());
        assert!(b.apply(r, actor("ai"), None, "2", |_| Ok(())).is_err());
        assert!(b
            .apply(
                request("a", "answer", 1),
                actor("ai"),
                Some(json!({"account":"other-human","content":"yes"})),
                "2",
                |_| Ok(())
            )
            .is_err());
        assert!(b
            .apply(
                request("a", "cancel", 1),
                Actor {
                    account: "other-human",
                    name: "x",
                    human: true
                },
                None,
                "2",
                |_| Ok(())
            )
            .is_err());
    }
    #[test]
    fn direct_answer_retry_binds_text_but_not_generated_timestamp() {
        let mut b = Book::default();
        let mut r = request("create", "put", 0);
        r.kind = Some("decision".into());
        b.apply(r, actor("ai"), None, "1", |_| Ok(())).unwrap();
        let r = request("answer", "answer", 1);
        let human = || Actor {
            account: "human",
            name: "human",
            human: true,
        };
        b.apply(
            r.clone(),
            human(),
            Some(json!({"account":"human","content":"yes","answered_at":"2"})),
            "2",
            |_| Ok(()),
        )
        .unwrap();
        b.apply(
            r.clone(),
            human(),
            Some(json!({"account":"human","content":"yes","answered_at":"3"})),
            "3",
            |_| panic!("retry writes"),
        )
        .unwrap();
        assert!(b
            .apply(
                r,
                human(),
                Some(json!({"account":"human","content":"changed"})),
                "4",
                |_| Ok(())
            )
            .is_err());
    }
}
