//! App-owned, retry-safe session operation records (#415).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CAPABILITY_ENV: &str = "PULLCEPT_CONTROL_CAPABILITY";
pub fn capability_matches(
    supplied: Option<&str>,
    expected: Option<&str>,
    live_or_starting: bool,
) -> bool {
    live_or_starting && supplied.is_some_and(|token| !token.is_empty() && expected == Some(token))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation_id: String,
    pub action: Action,
    pub topic_id: String,
    pub account_id: String,
    pub expected_pty_id: Option<String>,
    pub mode: Option<Mode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Start,
    Stop,
    Restart,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Fresh,
    Resume,
}

#[derive(Clone)]
pub struct Connection {
    pub socket: String,
    pub topic: String,
    pub account: String,
    pub generation: Option<String>,
    pub launch: Option<String>,
}
impl Connection {
    pub fn matches_live(&self, live: bool, generation: Option<&str>, launch: Option<&str>) -> bool {
        live && match &self.launch {
            Some(expected) => launch == Some(expected.as_str()),
            None => self
                .generation
                .as_deref()
                .is_some_and(|expected| generation == Some(expected)),
        }
    }
    pub fn authorize(
        &self,
        socket: &str,
        live: bool,
        generation: Option<&str>,
        launch: Option<&str>,
    ) -> Result<(), String> {
        if self.socket != socket || !self.matches_live(live, generation, launch) {
            return Err("Requester's socket or session changed".into());
        }
        Ok(())
    }
}

pub struct Current {
    pub pty_id: String,
    pub native_id: Option<String>,
    pub resumed_from: Option<String>,
}
/// The app holds its lifecycle lock for this whole sequence. Drivers resolve
/// the exact configured account/topic; they never accept a command from MCP.
pub trait Driver {
    fn current(&mut self, request: &Request) -> Result<Option<Current>, String>;
    fn resume(&mut self, request: &Request) -> Result<String, String>;
    fn stop(&mut self, request: &Request) -> Result<(), String>;
    fn start(
        &mut self,
        request: &Request,
        mode: Mode,
        resume_id: Option<&str>,
    ) -> Result<String, String>;
}

pub fn execute(request: &Request, driver: &mut impl Driver) -> Result<Option<String>, String> {
    request.validate()?;
    let current = driver.current(request)?;
    request.check_target(current.as_ref().map(|c| c.pty_id.as_str()))?;
    let mode = if request.action == Action::Restart {
        Mode::Resume
    } else {
        request.mode.unwrap_or(Mode::Fresh)
    };
    let resume = if mode == Mode::Resume {
        let id = driver.resume(request)?;
        if request.action == Action::Restart
            && current.as_ref().is_none_or(|c| {
                c.native_id.as_deref() != Some(&id) && c.resumed_from.as_deref() != Some(&id)
            })
        {
            return Err(
                "Current session has no matching captured resume ID yet; it was left running"
                    .into(),
            );
        }
        Some(id)
    } else {
        None
    };
    if matches!(request.action, Action::Stop | Action::Restart) {
        driver.stop(request)?;
    }
    if request.action == Action::Stop {
        return Ok(None);
    }
    driver.start(request, mode, resume.as_deref()).map(Some)
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        for id in [&self.operation_id, &self.topic_id, &self.account_id] {
            if id.trim().is_empty() || id.len() > 200 {
                return Err("IDs must contain 1..200 bytes".into());
            }
        }
        match self.action {
            Action::Start if self.expected_pty_id.is_none() && self.mode.is_some() => Ok(()),
            Action::Stop | Action::Restart if self.expected_pty_id.as_deref().is_some_and(|id| !id.is_empty()) && self.mode.is_none() => Ok(()),
            _ => Err("start requires explicit fresh/resume mode; stop/restart require expected_pty_id and no mode".into()),
        }
    }
    pub fn check_target(&self, current: Option<&str>) -> Result<(), String> {
        match self.action {
            Action::Start if current.is_none() => Ok(()),
            Action::Stop | Action::Restart
                if current.is_some() && current == self.expected_pty_id.as_deref() =>
            {
                Ok(())
            }
            _ => Err("target seat changed, is starting, or is not in the required state".into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Operation {
    pub request: Request,
    pub requester_account: String,
    pub requester_topic: String,
    pub requester_name: String,
    pub status: String,
    pub accepted_at: String,
    pub updated_at: String,
    pub new_pty_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Book {
    pub operations: BTreeMap<String, Operation>,
}
impl Book {
    pub fn accept_durable(
        &mut self,
        request: Request,
        account: &str,
        topic: &str,
        name: &str,
        at: &str,
        persist: impl FnOnce(&Book) -> Result<(), String>,
    ) -> Result<bool, String> {
        let previous = self.clone();
        let accepted = self.accept(request, account, topic, name, at)?;
        if accepted {
            if let Err(error) = persist(self) {
                *self = previous;
                return Err(error);
            }
        }
        Ok(accepted)
    }
    /// Return false for an identical retry. IDs cannot be reused by another caller.
    pub fn accept(
        &mut self,
        request: Request,
        account: &str,
        topic: &str,
        name: &str,
        at: &str,
    ) -> Result<bool, String> {
        request.validate()?;
        if let Some(old) = self.operations.get(&request.operation_id) {
            return if old.request == request
                && old.requester_account == account
                && old.requester_topic == topic
            {
                Ok(false)
            } else {
                Err("operation_id already belongs to a different request or requester".into())
            };
        }
        // Keep requests until explicit app-data removal: forgetting an ID permits replay.
        self.operations.insert(
            request.operation_id.clone(),
            Operation {
                request,
                requester_account: account.into(),
                requester_topic: topic.into(),
                requester_name: name.into(),
                status: "accepted".into(),
                accepted_at: at.into(),
                updated_at: at.into(),
                new_pty_id: None,
                error: None,
            },
        );
        Ok(true)
    }
    pub fn interrupt(&mut self, at: &str) {
        for op in self
            .operations
            .values_mut()
            .filter(|op| matches!(op.status.as_str(), "accepted" | "running"))
        {
            op.status = "interrupted".into();
            op.updated_at = at.into();
            op.error = Some("Pullcept exited before completion; operation was not replayed".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(action: Action) -> Request {
        Request {
            operation_id: "op".into(),
            action,
            topic_id: "room".into(),
            account_id: "target".into(),
            expected_pty_id: (action != Action::Start).then(|| "old".into()),
            mode: (action == Action::Start).then_some(Mode::Fresh),
        }
    }
    #[test]
    fn caller_authority_is_live_launch_and_current_socket_bound() {
        // A bearer-token client cannot gain control by naming a live account.
        assert!(!capability_matches(None, Some("secret"), true));
        assert!(!capability_matches(
            Some("other-secret"),
            Some("secret"),
            true
        ));
        assert!(!capability_matches(
            Some("old-secret"),
            Some("new-secret"),
            true
        ));
        assert!(!capability_matches(Some("secret"), Some("secret"), false));
        assert!(capability_matches(Some("secret"), Some("secret"), true));
        let mut caller = Connection {
            socket: "socket".into(),
            topic: "room".into(),
            account: "caller".into(),
            generation: Some("old".into()),
            launch: None,
        };
        assert!(caller.authorize("socket", true, Some("old"), None).is_ok());
        assert!(caller
            .authorize("replaced-socket", true, Some("old"), None)
            .is_err());
        assert!(caller
            .authorize("socket", false, Some("old"), None)
            .is_err());
        assert!(caller.authorize("socket", true, Some("new"), None).is_err());
        assert!(caller.authorize("socket", true, None, None).is_err());
        caller.launch = Some("launch".into());
        assert!(caller
            .authorize("socket", true, Some("old"), Some("launch"))
            .is_ok());
        assert!(caller
            .authorize("socket", true, Some("old"), Some("other-launch"))
            .is_err());
    }

    #[test]
    fn concurrent_identical_requests_are_accepted_once() {
        let book = std::sync::Arc::new(std::sync::Mutex::new(Book::default()));
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let book = book.clone();
                std::thread::spawn(move || {
                    book.lock()
                        .unwrap()
                        .accept(request(Action::Start), "caller", "room", "Caller", "now")
                        .unwrap()
                })
            })
            .collect();
        let accepted = threads
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(accepted, 1);
        assert_eq!(book.lock().unwrap().operations.len(), 1);
    }

    #[test]
    fn failed_journal_write_does_not_accept_or_schedule_an_operation() {
        let mut book = Book::default();
        assert!(book
            .accept_durable(
                request(Action::Restart),
                "caller",
                "room",
                "Caller",
                "now",
                |_| Err("disk unavailable".into())
            )
            .is_err());
        assert!(book.operations.is_empty());
        assert!(book
            .accept_durable(
                request(Action::Restart),
                "caller",
                "room",
                "Caller",
                "later",
                |_| Ok(())
            )
            .unwrap());
        assert!(!book
            .accept_durable(
                request(Action::Restart),
                "caller",
                "room",
                "Caller",
                "later",
                |_| panic!("identical retry must not write or schedule")
            )
            .unwrap());
    }
    #[test]
    fn retries_and_requester_identity() {
        let mut book = Book::default();
        let req = request(Action::Restart);
        assert!(book
            .accept(req.clone(), "caller", "room", "Caller", "now")
            .unwrap());
        assert!(!book
            .accept(req.clone(), "caller", "room", "Renamed", "later")
            .unwrap());
        assert!(book
            .accept(req.clone(), "other", "room", "Other", "later")
            .is_err());
        let mut changed = req;
        changed.account_id = "another".into();
        assert!(book
            .accept(changed, "caller", "room", "Caller", "later")
            .is_err());
    }
    #[test]
    fn stale_target_does_not_stop_a_new_launch() {
        let req = request(Action::Stop);
        assert!(req.check_target(Some("old")).is_ok());
        assert!(req.check_target(Some("new")).is_err());
        assert!(req.check_target(None).is_err());
        assert!(request(Action::Start)
            .check_target(Some("starting"))
            .is_err());
    }
    struct Fake {
        topic: &'static str,
        pty: &'static str,
        resume: Result<String, String>,
        calls: Vec<String>,
        fail_start: bool,
    }
    impl Driver for Fake {
        fn current(&mut self, req: &Request) -> Result<Option<Current>, String> {
            if req.topic_id != self.topic || req.account_id != "target" {
                return Err("Wrong topic/account".into());
            }
            Ok(Some(Current {
                pty_id: self.pty.into(),
                native_id: Some("native".into()),
                resumed_from: None,
            }))
        }
        fn resume(&mut self, _: &Request) -> Result<String, String> {
            self.resume.clone()
        }
        fn stop(&mut self, req: &Request) -> Result<(), String> {
            assert_eq!(req.expected_pty_id.as_deref(), Some(self.pty));
            self.calls.push("stop".into());
            Ok(())
        }
        fn start(&mut self, _: &Request, mode: Mode, id: Option<&str>) -> Result<String, String> {
            assert_eq!(mode, Mode::Resume);
            assert_eq!(id, Some("native"));
            self.calls.push("resume native".into());
            if self.fail_start {
                Err("Spawn failed".into())
            } else {
                Ok("new".into())
            }
        }
    }
    fn fake() -> Fake {
        Fake {
            topic: "room",
            pty: "old",
            resume: Ok("native".into()),
            calls: vec![],
            fail_start: false,
        }
    }
    #[test]
    fn worker_survives_caller_disconnect_and_preserves_resume() {
        let (caller, received) = std::sync::mpsc::channel();
        caller.send(request(Action::Restart)).unwrap();
        drop(caller);
        let worker = std::thread::spawn(move || {
            let req = received.recv().unwrap();
            let mut driver = fake();
            let result = execute(&req, &mut driver);
            (result, driver.calls)
        });
        let (result, calls) = worker.join().unwrap();
        assert_eq!(result.unwrap(), Some("new".into()));
        assert_eq!(calls, ["stop", "resume native"]);
    }
    #[test]
    fn wrong_target_missing_resume_and_launch_failure_are_observable() {
        let mut req = request(Action::Restart);
        let mut driver = fake();
        req.topic_id = "other".into();
        assert!(execute(&req, &mut driver).is_err());
        assert!(driver.calls.is_empty());
        req.topic_id = "room".into();
        driver.pty = "new";
        assert!(execute(&req, &mut driver).is_err());
        assert!(driver.calls.is_empty());
        driver = fake();
        driver.resume = Err("Transcript missing".into());
        assert!(execute(&req, &mut driver).is_err());
        assert!(driver.calls.is_empty());
        driver = fake();
        driver.resume = Ok("older-native".into());
        assert!(execute(&req, &mut driver).is_err());
        assert!(driver.calls.is_empty());
        driver = fake();
        driver.fail_start = true;
        assert_eq!(execute(&req, &mut driver).unwrap_err(), "Spawn failed");
        assert_eq!(driver.calls, ["stop", "resume native"]);
    }
    #[test]
    fn modes_are_explicit_and_restart_cannot_request_fresh() {
        let mut req = request(Action::Start);
        req.mode = None;
        assert!(req.validate().is_err());
        req = request(Action::Restart);
        req.mode = Some(Mode::Fresh);
        assert!(req.validate().is_err());
    }
    #[test]
    fn self_restart_record_outlives_caller_and_survives_restart_without_replay() {
        let mut book = Book::default();
        {
            let caller = String::from("target");
            book.accept(request(Action::Restart), &caller, "room", "Self", "now")
                .unwrap();
        }
        let mut restored: Book =
            serde_json::from_slice(&serde_json::to_vec(&book).unwrap()).unwrap();
        restored.interrupt("restart");
        assert_eq!(restored.operations["op"].status, "interrupted");
        assert!(!restored
            .accept(request(Action::Restart), "target", "room", "Self", "later")
            .unwrap());
    }
}
