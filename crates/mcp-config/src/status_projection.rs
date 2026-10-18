//! Bounded MCP projections over the same snapshot the screen reads.
use serde_json::{json, Value};

fn text(value: &mut Value, field: &str, limit: usize) {
    if let Some(s) = value[field].as_str() {
        if s.chars().count() > limit {
            let shortened: String = s.chars().take(limit).collect();
            value[field] = json!(shortened);
            value[format!("{field}_truncated")] = json!(true);
        }
    }
}
fn record(mut value: Value, detail: bool) -> Value {
    text(&mut value, "text", if detail { 2000 } else { 120 });
    text(&mut value, "progress", if detail { 500 } else { 80 });
    let refs = value["references"].as_array().cloned().unwrap_or_default();
    let limit = if detail { 3 } else { 1 };
    let length = if detail { 512 } else { 180 };
    let truncated = refs.len() > limit
        || refs
            .iter()
            .any(|r| r.as_str().is_some_and(|s| s.chars().count() > length));
    value["references"] = json!(refs
        .into_iter()
        .take(limit)
        .map(|r| r
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(length)
            .collect::<String>())
        .collect::<Vec<_>>());
    if truncated {
        value["references_truncated"] = json!(true);
    }
    if detail && value["answer"].is_object() {
        text(&mut value["answer"], "content", 1000);
    }
    if !detail {
        value.as_object_mut().unwrap().remove("answer");
    }
    value
}
pub fn mcp(mut snapshot: Value, detail: bool) -> Value {
    let work = &mut snapshot["work"];
    let active = work["active"].as_array().cloned().unwrap_or_default();
    work["active_total"] = json!(active.len());
    work["active_limit"] = json!(if detail { 10 } else { 20 });
    work["active_truncated"] = json!(active.len() > if detail { 10 } else { 20 });
    work["active"] = json!(active
        .into_iter()
        .take(if detail { 10 } else { 20 })
        .map(|r| record(r, detail))
        .collect::<Vec<_>>());
    if detail {
        let history = work["history"].as_array().cloned().unwrap_or_default();
        work["history_limit"] = json!(5);
        work["history"] = json!(history
            .into_iter()
            .take(5)
            .map(|r| record(r, true))
            .collect::<Vec<_>>());
        work["history_truncated"] = json!(work["history_total"].as_u64().unwrap_or_default() > 5);
    } else {
        work.as_object_mut().unwrap().remove("history");
        work["history_limit"] = json!(0);
        work["history_omitted"] = json!(true);
        work["detail_requires_account_id"] = json!(true);
    }
    let plans = snapshot["plans"].as_array().cloned().unwrap_or_default();
    snapshot["plans_retained_total"] = json!(plans.len());
    let plans = plans
        .into_iter()
        .filter(|p| detail || p["current_launch"] == true)
        .collect::<Vec<_>>();
    snapshot["plans_total"] = json!(plans.len());
    snapshot["plans_truncated"] = json!(plans.len() > 20);
    snapshot["plans_limit"] = json!(20);
    snapshot["plans"] = json!(plans
        .into_iter()
        .take(20)
        .map(|mut p| {
            let steps = p["steps"].as_array().cloned().unwrap_or_default();
            p["steps_total"] = json!(steps.len());
            p["steps_projection"] = json!(if detail {
                "bounded_steps"
            } else {
                "in_progress_only"
            });
            p["pending_count"] = json!(steps.iter().filter(|s| s["status"] == "pending").count());
            p["completed_count"] =
                json!(steps.iter().filter(|s| s["status"] == "completed").count());
            let selected = steps
                .into_iter()
                .filter(|s| detail || s["status"] == "inProgress")
                .collect::<Vec<_>>();
            p["steps_truncated"] = json!(selected.len() > if detail { 20 } else { 3 });
            p["steps"] = json!(selected
                .into_iter()
                .take(if detail { 20 } else { 3 })
                .map(|mut step| {
                    text(&mut step, "step", if detail { 300 } else { 120 });
                    step
                })
                .collect::<Vec<_>>());
            p
        })
        .collect::<Vec<_>>());
    snapshot["projection"] = json!(if detail {
        "account_detail_bounded"
    } else {
        "current_summary"
    });
    snapshot["full_record_surface"] = json!("participant_details");
    snapshot
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_status_excludes_completed_answers_and_large_plan_bodies() {
        let snapshot = json!({"work":{"active":[{"id":"w","text":"x".repeat(8000),"progress":"p".repeat(2000),"references":[]}],
            "history":[{"id":"h","text":"q","references":[],"answer":{"content":format!("completed-answer-{}","a".repeat(16000))}}],"history_total":1},
            "plans":[{"account_id":"a","current_launch":true,"steps":[{"step":format!("large-completed-plan-{}","z".repeat(2000)),"status":"completed"},{"step":"current","status":"inProgress"}]},{"current_launch":false,"steps":[{"step":"stale-plan","status":"inProgress"}]}]});
        let summary = mcp(snapshot.clone(), false);
        assert!(!summary.to_string().contains("completed-answer"));
        assert!(!summary.to_string().contains("large-completed-plan"));
        assert!(!summary.to_string().contains("stale-plan"));
        assert_eq!(summary["plans"][0]["steps"][0]["step"], "current");
        assert_eq!(summary["work"]["active"][0]["text_truncated"], true);
        let detail = mcp(snapshot.clone(), true);
        assert!(detail["work"]["history"][0]["answer"]["content"]
            .as_str()
            .unwrap()
            .starts_with("completed-answer"));
        assert_eq!(
            detail["work"]["history"][0]["answer"]["content_truncated"],
            true
        );
        assert_eq!(detail["plans"][0]["steps"][0]["step_truncated"], true);
        assert_eq!(
            snapshot["work"]["history"][0]["answer"]["content"]
                .as_str()
                .unwrap()
                .len(),
            16017
        );
        assert!(summary.to_string().len() < 2000);
    }
}
