//! Current Claude parent transcript metadata (#342). Never infer from PTY text.
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const LAUNCH_ENV: &str = "PULLCEPT_CLAUDE_LAUNCH";
// Oversized lines are skipped safely; a later normal parent response can recover.
const LINE_MAX: usize = 4 * 1024 * 1024;

/// The allowed transcript root, including account-specific config directories.
pub fn projects(home: &Path, cwd: &Path, env: &[(String, String)]) -> PathBuf {
    let config = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .map(|(_, v)| PathBuf::from(v))
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home.join(".claude"));
    let config = if config.is_absolute() {
        config
    } else {
        cwd.join(config)
    };
    config.join("projects")
}

/// Only a canonical parent file directly under a project directory is adopted.
/// A subagent path, a traversal or a symlink escaping this root cannot qualify.
pub fn checked(root: &Path, parent: &str, reported: &Path) -> Option<PathBuf> {
    if parent.is_empty()
        || parent
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && c != '-')
    {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let path = reported.canonicalize().ok()?;
    let rel = path.strip_prefix(&root).ok()?;
    let parts: Vec<_> = rel.components().collect();
    if parts.len() != 2
        || path.file_name()?.to_str()? != format!("{parent}.jsonl")
        || !path.is_file()
    {
        return None;
    }
    Some(path)
}

/// Find an existing parent before spawn, so resume starts at its then EOF.
#[derive(Debug, PartialEq)]
pub enum Search {
    Found(PathBuf),
    Missing,
    Unknown,
}
pub fn search(root: &Path, parent: &str) -> Search {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Search::Unknown;
    };
    let mut found = None;
    for entry in entries {
        let Ok(project) = entry else {
            return Search::Unknown;
        };
        let Ok(kind) = project.file_type() else {
            return Search::Unknown;
        };
        if !kind.is_dir() {
            continue;
        }
        let candidate = project.path().join(format!("{parent}.jsonl"));
        match std::fs::metadata(&candidate) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Search::Unknown,
            Ok(_) => {}
        }
        let Some(path) = checked(root, parent, &candidate) else {
            return Search::Unknown;
        };
        if found.is_some() {
            return Search::Unknown;
        }
        found = Some(path);
    }
    found.map(Search::Found).unwrap_or(Search::Missing)
}
pub fn existing(root: &Path, parent: &str) -> Option<PathBuf> {
    match search(root, parent) {
        Search::Found(path) => Some(path),
        _ => None,
    }
}

/// Reporter identity does not depend on the transcript having been created.
/// Unknown native parents keep their old metrics/activity, without a watcher.
pub fn reporter_matches(
    parent: &str,
    expected_nonce: &str,
    nonce: Option<&str>,
    body: &[u8],
) -> bool {
    if nonce != Some(expected_nonce) {
        return false;
    }
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(id) = v["session_id"].as_str().filter(|s| !s.is_empty()) else {
        return false;
    };
    parent.is_empty() || id == parent
}

/// `native` is read by RoomSeats using this launch's PTY, not merely its seat.
pub fn running_matches(parent: &str, native: Option<Option<String>>, live: bool) -> bool {
    live && native.is_some_and(|id| parent.is_empty() || id.as_deref() == Some(parent))
}

/// Shared validation for Starting and Running reporters. Event/agent names do
/// not select transcripts: SubagentStop also reports the parent's path.
pub fn report_path(
    root: &Path,
    parent: &str,
    expected_nonce: &str,
    nonce: Option<&str>,
    body: &[u8],
    current: Option<&Path>,
) -> Option<PathBuf> {
    if parent.is_empty() || !reporter_matches(parent, expected_nonce, nonce, body) {
        return None;
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    let path = checked(root, parent, Path::new(v["transcript_path"].as_str()?))?;
    if current.is_some_and(|p| p != path) {
        return None;
    }
    Some(path)
}

pub fn timestamp(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.timestamp_millis())
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Rejected {
        at: i64,
        id: String,
        reset: Option<i64>,
    },
    Succeeded {
        at: i64,
        id: String,
    },
}

/// UUID/request identity is deduplicated; content is never copied or logged.
pub fn event(parent: &str, value: &Value) -> Option<Event> {
    if value["type"] != "assistant" || value["isSidechain"] != false {
        return None;
    }
    let ids = [value["sessionId"].as_str(), value["session_id"].as_str()];
    if !ids.iter().any(|id| *id == Some(parent)) || ids.iter().flatten().any(|id| *id != parent) {
        return None;
    }
    let at = timestamp(&value["timestamp"])?;
    let id = value["uuid"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["requestId"].as_str().filter(|s| !s.is_empty()))?
        .to_string();
    if value["isApiErrorMessage"] == true
        && value["error"] == "rate_limit"
        && (value["apiErrorStatus"] == 429 || value["quotaLimits"]["status"] == "rejected")
    {
        return Some(Event::Rejected {
            at,
            id,
            reset: super::claude_notice::reset(value, at),
        });
    }
    let model = value["message"]["model"].as_str()?;
    if (!value["isApiErrorMessage"].is_null() && value["isApiErrorMessage"] != false)
        || !value["error"].is_null()
        || model.is_empty()
        || model.starts_with('<')
        || value["requestId"].as_str().is_none_or(|s| s.is_empty())
    {
        return None;
    }
    Some(Event::Succeeded { at, id })
}

/// How far back from a restarted launch's baseline the last parent event is
/// looked for (#366). A limited parent writes no responses, so its rejection
/// sits near the end; one older than this window restores nothing.
pub const RESTORE_WINDOW: u64 = 4 * 1024 * 1024;

/// The parent's last rejection before `end`, when it is the parent's last
/// event there (#366): `(at_ms, reset)`. A normal response after it, another
/// parent's or a child's line, and an unreadable file all give `None`.
pub fn last_rejection(path: &Path, parent: &str, end: u64) -> Option<(i64, Option<i64>)> {
    let mut f = std::fs::File::open(path).ok()?;
    let end = end.min(f.metadata().ok()?.len());
    let start = end.saturating_sub(RESTORE_WINDOW);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    f.take(end - start).read_to_end(&mut bytes).ok()?;
    let mut lines = bytes.split(|b| *b == b'\n');
    if start > 0 {
        lines.next(); // A line cut by the window is not read.
    }
    let mut last = None;
    for line in lines {
        if let Some(e) = serde_json::from_slice(line)
            .ok()
            .and_then(|v| event(parent, &v))
        {
            last = Some(e);
        }
    }
    match last? {
        Event::Rejected { at, reset, .. } => Some((at, reset)),
        Event::Succeeded { .. } => None,
    }
}

/// One launch's chronological state. A reset/status report cannot change it.
pub struct State {
    pub limited: bool,
    floor: i64,
    latest: i64,
    rejected: Option<i64>,
    seen: HashSet<String>,
    order: VecDeque<String>,
}
impl State {
    pub fn new(floor: i64, persisted: bool) -> Self {
        Self {
            limited: persisted,
            floor,
            latest: floor - 1,
            rejected: None,
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }
    pub fn apply(&mut self, event: Event) -> bool {
        let (at, id, rejection) = match event {
            Event::Rejected { at, id, .. } => (at, id, true),
            Event::Succeeded { at, id } => (at, id, false),
        };
        if at < self.floor || at < self.latest || self.seen.contains(&id) {
            return false;
        }
        self.seen.insert(id.clone());
        self.order.push_back(id);
        if self.order.len() > 4096 {
            if let Some(id) = self.order.pop_front() {
                self.seen.remove(&id);
            }
        }
        self.latest = at;
        if rejection {
            self.rejected = Some(at);
            self.limited = true;
        } else if self.rejected.is_none_or(|rejected| at > rejected) {
            self.limited = false;
        }
        true
    }
}

/// The room delivery gate. Kept held through the recovery notice and closed
/// only while enqueuing its one digest, under the shared Claude table lock.
pub struct Gate {
    pub state: State,
    held: Vec<super::codex::limit::Held>,
    recovery_hold: bool,
}
impl Gate {
    pub fn new(floor: i64, persisted: bool) -> Self {
        Self {
            state: State::new(floor, persisted),
            held: Vec::new(),
            recovery_hold: false,
        }
    }
    pub fn hold(&mut self, post: impl FnOnce() -> super::codex::limit::Held) -> Option<String> {
        if !self.is_limited() {
            return None;
        }
        let post = post();
        let id = post.message_id.clone();
        if !self.held.iter().any(|p| p.message_id == id) {
            self.held.push(post);
        }
        Some(id)
    }
    pub fn is_limited(&self) -> bool {
        self.state.limited || self.recovery_hold
    }
    pub fn keep_holding(&mut self) {
        self.recovery_hold = true;
    }
    pub fn held(&self) -> Vec<super::codex::limit::Held> {
        self.held.clone()
    }
    pub fn release(&mut self) -> Vec<super::codex::limit::Held> {
        self.state.limited = false;
        self.recovery_hold = false;
        std::mem::take(&mut self.held)
    }
}

/// Incremental, bounded reader, retaining only an incomplete metadata line.
/// Missing files leave the state alone. Replacement restarts reading, while
/// launch timestamp and UUID guards prevent historical/duplicate recovery.
pub struct Tail {
    offset: u64,
    partial: Vec<u8>,
    skipping: bool,
    checkpoint: Option<u64>,
    caught_up: bool,
    created: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
}
impl Tail {
    pub fn new(path: &Path, offset: u64) -> Self {
        Self {
            offset,
            partial: Vec::new(),
            skipping: false,
            caught_up: false,
            created: std::fs::metadata(path).ok().and_then(|m| m.created().ok()),
            modified: std::fs::metadata(path).ok().and_then(|m| m.modified().ok()),
            checkpoint: std::fs::File::open(path)
                .ok()
                .and_then(|mut f| fingerprint(&mut f, offset).ok()),
        }
    }
    pub fn caught_up(&self) -> bool {
        self.caught_up
    }
    pub fn feed(&mut self, parent: &str, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                if !self.skipping {
                    if let Ok(v) = serde_json::from_slice(&self.partial) {
                        if let Some(e) = event(parent, &v) {
                            events.push(e);
                        }
                    }
                }
                self.partial.clear();
                self.skipping = false;
            } else if !self.skipping {
                if self.partial.len() == LINE_MAX {
                    self.partial.clear();
                    self.skipping = true;
                } else {
                    self.partial.push(b);
                }
            }
        }
        events
    }
    pub fn read(&mut self, path: &Path, parent: &str) -> std::io::Result<Vec<Event>> {
        self.caught_up = false;
        let mut f = std::fs::File::open(path)?;
        let meta = f.metadata()?;
        if meta.len() < self.offset
            || self
                .created
                .is_some_and(|old| meta.created().ok() != Some(old))
            || (meta.len() == self.offset
                && self
                    .modified
                    .is_some_and(|old| meta.modified().ok() != Some(old)))
            || self
                .checkpoint
                .is_some_and(|old| fingerprint(&mut f, self.offset).ok() != Some(old))
        {
            self.offset = 0;
            self.partial.clear();
            self.skipping = false;
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        f.by_ref().take(1024 * 1024).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        self.created = meta.created().ok();
        self.modified = meta.modified().ok();
        self.checkpoint = Some(fingerprint(&mut f, self.offset)?);
        let events = self.feed(parent, &bytes);
        self.caught_up =
            self.offset == f.metadata()?.len() && self.partial.is_empty() && !self.skipping;
        Ok(events)
    }
}
fn fingerprint(f: &mut std::fs::File, offset: u64) -> std::io::Result<u64> {
    use std::hash::{Hash, Hasher};
    let start = offset.saturating_sub(128);
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (offset - start) as usize];
    f.read_exact(&mut bytes)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    f.seek(SeekFrom::Start(0))?;
    let mut prefix = vec![0; offset.min(128) as usize];
    f.read_exact(&mut prefix)?;
    prefix.hash(&mut hash);
    Ok(hash.finish())
}

#[cfg(test)]
mod tests {
    use super::super::codex::limit::{Held, Mailboxes};
    use super::*;
    use serde_json::json;
    const PARENT: &str = "512c102e-fe28-4e8e-8f23-6cc4226ce2fc";
    fn line(second: u32, rejected: bool) -> Value {
        let mut v = json!({"type":"assistant", "sessionId":PARENT, "session_id":PARENT,
            "isSidechain":false, "timestamp":format!("2026-10-09T14:00:{second:02}.000Z"),
            "uuid":format!("event-{second}"), "requestId":format!("request-{second}"),
            "message":{"model":"claude-opus-5-5"}});
        if rejected {
            v["isApiErrorMessage"] = json!(true);
            v["error"] = json!("rate_limit");
            v["apiErrorStatus"] = json!(429);
            v["quotaLimits"] = json!({"status":"rejected"});
            v["message"]["model"] = json!("<synthetic>");
        }
        v
    }
    fn bytes(v: &Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(v).unwrap();
        b.push(b'\n');
        b
    }
    fn dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("pullcept-claude-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn held(id: &str) -> Held {
        Held {
            message_id: id.into(),
            speaker: "other".into(),
            at: None,
            content: id.into(),
            addressed: true,
        }
    }

    #[test]
    fn observed_parent_reject_normal_without_error_flag_and_reject_again() {
        let mut state = State::new(timestamp(&line(0, false)["timestamp"]).unwrap(), false);
        state.apply(event(PARENT, &line(1, true)).unwrap());
        assert!(state.limited);
        state.apply(event(PARENT, &line(2, false)).unwrap());
        assert!(!state.limited);
        state.apply(event(PARENT, &line(3, true)).unwrap());
        assert!(state.limited);
        state.apply(event(PARENT, &line(2, false)).unwrap());
        assert!(state.limited);
        let mut duplicate = line(2, false);
        duplicate["timestamp"] = line(4, false)["timestamp"].clone();
        state.apply(event(PARENT, &duplicate).unwrap());
        assert!(state.limited);
        state.apply(event(PARENT, &line(4, false)).unwrap());
        assert!(!state.limited);
    }
    #[test]
    fn child_other_parent_synthetic_errors_and_usage_are_not_success() {
        for (key, value) in [
            ("isSidechain", json!(true)),
            ("sessionId", json!("another-parent")),
            ("isApiErrorMessage", json!(true)),
            ("requestId", Value::Null),
        ] {
            let mut v = line(1, false);
            v[key] = value;
            assert_eq!(event(PARENT, &v), None, "{key}");
        }
        let mut v = line(1, false);
        v["message"]["model"] = json!("<synthetic>");
        assert_eq!(event(PARENT, &v), None);
        assert_eq!(
            event(
                PARENT,
                &json!({"type":"status", "rate_limits":{"five_hour":{"used_percentage":0}}})
            ),
            None
        );
        let mut v = line(1, false);
        v["isApiErrorMessage"] = json!(false);
        assert!(event(PARENT, &v).is_some());
    }
    #[test]
    fn history_and_equal_timestamp_do_not_recover_current_rejection() {
        let floor = timestamp(&line(10, false)["timestamp"]).unwrap();
        let mut state = State::new(floor, true);
        state.apply(event(PARENT, &line(9, false)).unwrap());
        assert!(state.limited);
        state.apply(event(PARENT, &line(10, true)).unwrap());
        let mut same_time = line(10, false);
        same_time["uuid"] = json!("same-time");
        state.apply(event(PARENT, &same_time).unwrap());
        assert!(state.limited);
        state.apply(event(PARENT, &line(11, false)).unwrap());
        assert!(!state.limited);
    }
    #[test]
    fn starting_and_bound_reports_validate_generation_parent_and_root_not_event_name() {
        let root = dir();
        let project = root.join("custom-project-name");
        std::fs::create_dir(&project).unwrap();
        let path = project.join(format!("{PARENT}.jsonl"));
        std::fs::write(&path, "").unwrap();
        let body = serde_json::to_vec(&json!({"session_id":PARENT, "transcript_path":path,
            "hook_event_name":"SubagentStop", "agent_id":"child"}))
        .unwrap();
        let current =
            report_path(&root, PARENT, "new-launch", Some("new-launch"), &body, None).unwrap();
        assert!(report_path(
            &root,
            PARENT,
            "new-launch",
            Some("new-launch"),
            &body,
            Some(&current)
        )
        .is_some());
        for nonce in [None, Some("old-launch")] {
            assert!(report_path(&root, PARENT, "new-launch", nonce, &body, None).is_none());
        }
        assert!(report_path(
            &root,
            "other-parent",
            "new-launch",
            Some("new-launch"),
            &body,
            None
        )
        .is_none());
        let child = project.join("subagents");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join(format!("{PARENT}.jsonl")), "").unwrap();
        assert!(checked(&root, PARENT, &child.join(format!("{PARENT}.jsonl"))).is_none());
        let outside = root.join(format!("{PARENT}.jsonl"));
        std::fs::write(&outside, "").unwrap();
        assert!(checked(&root, PARENT, &outside).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn config_scope_honors_relative_custom_config_and_named_project() {
        let base = dir();
        let cwd = base.join("work");
        std::fs::create_dir(&cwd).unwrap();
        let root = projects(
            &base,
            &cwd,
            &[("CLAUDE_CONFIG_DIR".into(), "../custom-config".into())],
        );
        let project = root.join("named-project");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{PARENT}.jsonl"));
        std::fs::write(&path, "").unwrap();
        assert_eq!(existing(&root, PARENT), path.canonicalize().ok());
        std::fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn missing_transcript_preserves_reporters_and_can_be_adopted_later() {
        let root = dir();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let path = project.join(format!("{PARENT}.jsonl"));
        let body = serde_json::to_vec(&json!({"session_id":PARENT,"transcript_path":path,"hook_event_name":"UserPromptSubmit"})).unwrap();
        assert!(reporter_matches(PARENT, "launch", Some("launch"), &body));
        assert!(report_path(&root, PARENT, "launch", Some("launch"), &body, None).is_none());
        std::fs::write(&path, "").unwrap();
        assert!(report_path(&root, PARENT, "launch", Some("launch"), &body, None).is_some());
        assert!(reporter_matches("", "launch", Some("launch"), &body));
        assert!(report_path(&root, "", "launch", Some("launch"), &body, None).is_none());
        assert!(!reporter_matches(PARENT, "launch", Some("previous"), &body));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn snapshots_require_current_live_pty_and_expected_parent() {
        assert!(running_matches(PARENT, Some(Some(PARENT.into())), true));
        assert!(!running_matches(PARENT, None, true)); // Old PTY after relaunch.
        assert!(!running_matches(
            PARENT,
            Some(Some("other-parent".into())),
            true
        ));
        assert!(!running_matches(PARENT, Some(Some(PARENT.into())), false));
        assert!(!running_matches(PARENT, Some(None), true));
        assert!(running_matches("", Some(None), true)); // Metrics only; no parent inference.
    }
    #[test]
    fn ambiguous_or_unreadable_search_is_not_missing_resume() {
        let root = dir();
        assert_eq!(search(&root, PARENT), Search::Missing);
        assert_eq!(search(&root.join("missing-root"), PARENT), Search::Unknown);
        for project in ["a", "b"] {
            let dir = root.join(project);
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join(format!("{PARENT}.jsonl")), "").unwrap();
        }
        assert_eq!(search(&root, PARENT), Search::Unknown);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn catch_up_does_not_release_success_before_rejection_in_next_chunk_or_partial() {
        let root = dir();
        let path = root.join("catchup.jsonl");
        let mut all = bytes(&line(2, false));
        all.extend(vec![b'x'; 1024 * 1024]);
        all.push(b'\n');
        all.extend(bytes(&line(3, true)));
        std::fs::write(&path, all).unwrap();
        let mut tail = Tail::new(&path, 0);
        let mut gate = Gate::new(0, true);
        for e in tail.read(&path, PARENT).unwrap() {
            gate.state.apply(e);
        }
        assert!(!gate.state.limited);
        assert!(!tail.caught_up());
        gate.keep_holding();
        assert!(gate.hold(|| held("still-held")).is_some());
        for e in tail.read(&path, PARENT).unwrap() {
            gate.state.apply(e);
        }
        assert!(tail.caught_up());
        assert!(gate.is_limited());
        assert!(gate.state.limited);
        let normal = bytes(&line(4, false));
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&normal[..normal.len() - 1]).unwrap();
        assert!(tail.read(&path, PARENT).unwrap().is_empty());
        assert!(!tail.caught_up());
        file.write_all(b"\n").unwrap();
        drop(file);
        for e in tail.read(&path, PARENT).unwrap() {
            gate.state.apply(e);
        }
        assert!(tail.caught_up());
        assert!(!gate.state.limited);
        assert!(gate.is_limited());
        assert_eq!(gate.release().len(), 1);
        assert!(!gate.is_limited());
        assert!(gate.release().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn incremental_partial_duplicate_and_missing_or_replaced_transcript() {
        let root = dir();
        let path = root.join("test.jsonl");
        std::fs::write(&path, bytes(&line(1, true))).unwrap();
        let mut tail = Tail::new(&path, 0);
        let mut state = State::new(0, false);
        for e in tail.read(&path, PARENT).unwrap() {
            state.apply(e);
        }
        assert!(state.limited);
        let normal = bytes(&line(2, false));
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&normal[..normal.len() - 1]).unwrap();
        assert!(tail.read(&path, PARENT).unwrap().is_empty());
        assert!(state.limited);
        file.write_all(b"\n").unwrap();
        drop(file);
        for e in tail.read(&path, PARENT).unwrap() {
            state.apply(e);
        }
        assert!(!state.limited);
        assert!(tail.read(&path, PARENT).unwrap().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(tail.read(&path, PARENT).is_err());
        std::fs::write(&path, bytes(&line(3, true))).unwrap();
        for e in tail.read(&path, PARENT).unwrap() {
            state.apply(e);
        }
        assert!(state.limited);
        std::fs::write(&path, bytes(&line(1, false))).unwrap();
        for e in tail.read(&path, PARENT).unwrap() {
            state.apply(e);
        }
        assert!(state.limited);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resumed_tail_starts_at_pre_spawn_eof() {
        let root = dir();
        let path = root.join("resume.jsonl");
        let old = bytes(&line(1, true));
        std::fs::write(&path, &old).unwrap();
        let mut tail = Tail::new(&path, old.len() as u64);
        let mut appended = old;
        appended.extend(bytes(&line(2, false)));
        std::fs::write(&path, appended).unwrap();
        assert_eq!(
            tail.read(&path, PARENT).unwrap(),
            vec![event(PARENT, &line(2, false)).unwrap()]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn equal_length_replacement_with_shared_suffix_is_read_again() {
        let root = dir();
        let path = root.join("replace.jsonl");
        let mut first = line(2, false);
        let mut second = line(3, true);
        first["padding"] = json!("x".repeat(300));
        second["padding"] = json!("x".repeat(300));
        let difference = bytes(&second).len() - bytes(&first).len();
        first["padding"] = json!("x".repeat(300 + difference));
        assert_eq!(bytes(&first).len(), bytes(&second).len());
        std::fs::write(&path, bytes(&first)).unwrap();
        let mut tail = Tail::new(&path, 0);
        assert_eq!(
            tail.read(&path, PARENT).unwrap(),
            vec![event(PARENT, &first).unwrap()]
        );
        std::fs::write(&path, bytes(&second)).unwrap();
        assert_eq!(
            tail.read(&path, PARENT).unwrap(),
            vec![event(PARENT, &second).unwrap()]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn large_normal_and_oversized_line_skip_cannot_forge_partial_recovery() {
        let mut tail = Tail::new(Path::new("unused"), 0);
        let mut normal = line(2, false);
        normal["message"]["content"] = json!("x".repeat(128 * 1024));
        assert_eq!(tail.feed(PARENT, &bytes(&normal)).len(), 1);
        let mut oversized = vec![b'x'; LINE_MAX + 1];
        oversized.extend(bytes(&line(3, false)));
        assert!(tail.feed(PARENT, &oversized).is_empty());
        assert_eq!(tail.feed(PARENT, &bytes(&line(4, false))).len(), 1);
    }
    #[test]
    fn only_stopped_seats_hold_ordered_deduplicated_posts_across_restart_then_release_once() {
        let root = dir();
        let path = root.join("claude-mailboxes.json");
        let mut a = Gate::new(0, false);
        let mut b = Gate::new(0, false);
        a.state.apply(event(PARENT, &line(1, true)).unwrap());
        let mut boxes = Mailboxes::read(&path);
        boxes.open("a");
        boxes.write(&path).unwrap();
        for id in ["one", "two", "one"] {
            let held = a.hold(|| held(id)).unwrap();
            boxes.push("a", &held);
        }
        assert!(b
            .hold(|| panic!("free seat must deliver normally"))
            .is_none());
        b.state.apply(event(PARENT, &line(1, true)).unwrap());
        boxes.open("b");
        let id = b.hold(|| held("three")).unwrap();
        boxes.push("b", &id);
        boxes.write(&path).unwrap();
        assert_eq!(
            a.held()
                .iter()
                .map(|p| p.message_id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        drop(a);
        drop(b); // New app/launch retains only the file's ids.
        let mut boxes = Mailboxes::read(&path);
        let ids = boxes.get("a").unwrap();
        assert_eq!(ids, ["one", "two"]);
        assert_eq!(boxes.get("b").unwrap(), ["three"]);
        let mut a = Gate::new(0, boxes.get("a").is_some());
        assert!(a.state.limited);
        a.state.apply(event(PARENT, &line(2, false)).unwrap());
        assert!(!a.state.limited);
        // Production holds through the notice, then releases under the lock.
        a.state.limited = true;
        a.hold(|| held("four"));
        let released = a.release();
        let record: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|id| super::super::codex::limit::Recorded {
                message_id: id.into(),
                speaker: "other".into(),
                account: None,
                at: None,
                content: id.into(),
                to: vec!["seat-a".into()],
            })
            .collect();
        let payload = super::super::codex::limit::mailbox(
            &record,
            &super::super::codex::limit::Seat {
                name: "seat-a",
                account_id: "a",
            },
            &ids,
            released.clone(),
        );
        assert_eq!(
            payload
                .iter()
                .map(|p| p.message_id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "four"]
        );
        assert!(super::super::codex::limit::digest(&payload).is_some());
        boxes.close("a");
        boxes.write(&path).unwrap();
        assert_eq!(released[0].message_id, "four");
        assert!(a.release().is_empty());
        assert!(Mailboxes::read(&path).get("a").is_none());
        assert!(Mailboxes::read(&path).get("b").is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restart_reads_the_parents_last_rejection_and_its_reset() {
        let root = dir();
        let path = root.join(format!("{PARENT}.jsonl"));
        let at = timestamp(&line(5, true)["timestamp"]).unwrap();
        let mut rejected = line(5, true);
        rejected["quotaLimits"]["resetsAt"] = json!(at / 1000 + 3600);
        let mut other = line(6, false);
        other["sessionId"] = json!("another-parent");
        other["session_id"] = json!("another-parent");
        let mut child = line(7, false);
        child["isSidechain"] = json!(true);
        let mut body = bytes(&line(1, false));
        body.extend(bytes(&rejected));
        body.extend(bytes(&other));
        body.extend(bytes(&child));
        body.extend(b"{\"type\":\"user\"}\n");
        std::fs::write(&path, &body).unwrap();
        let end = body.len() as u64;
        // The rejection is the parent's last event: its reset comes back.
        assert_eq!(
            last_rejection(&path, PARENT, end),
            Some((at, Some(at / 1000 + 3600)))
        );
        // Another parent's file reading gives nothing of this parent's.
        assert_eq!(last_rejection(&path, "another-parent", end), None);
        // Only bytes before the baseline count.
        let before = bytes(&line(1, false)).len() as u64;
        assert_eq!(last_rejection(&path, PARENT, before), None);
        // A normal response after the rejection means it was answered.
        let mut answered = body.clone();
        answered.extend(bytes(&line(8, false)));
        std::fs::write(&path, &answered).unwrap();
        assert_eq!(last_rejection(&path, PARENT, answered.len() as u64), None);
        assert_eq!(
            last_rejection(&root.join("missing.jsonl"), PARENT, 10),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
