//! Launch-scoped sidecar admission (#315, docs/3-accounts.md).
use serde_json::Value;

pub const PATH: &str = "/room/sidecar-admission";
pub const REQUIRED_ENV: &str = "PULLCEPT_ROOM_ADMISSION";

pub fn required_for(cli: Option<crate::Cli>) -> bool {
    matches!(cli, Some(crate::Cli::CodexCli))
}

#[derive(Clone)]
pub struct Claim {
    pub launch_id: String,
    pub account_id: String,
    pub room_id: String,
    pub instance_id: String,
}

impl Claim {
    pub fn read(body: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(body).ok()?;
        let field = |key| {
            value
                .get(key)?
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let claim = Self {
            launch_id: field("launch_id")?,
            account_id: field("account_id")?,
            room_id: field("room_id")?,
            instance_id: field("instance_id")?,
        };
        (super::valid_id(&claim.launch_id) && super::valid_id(&claim.instance_id)).then_some(claim)
    }
}

#[derive(Clone, Default)]
pub struct Admission {
    owner: Option<String>,
    origin: Option<String>,
    socket: Option<String>,
}

impl Admission {
    pub fn inherit(starting: Self, previous_launch: &str, running_launch: Option<&str>) -> Self {
        if running_launch == Some(previous_launch) {
            starting
        } else {
            Self::default()
        }
    }

    pub fn admit(
        &mut self,
        claim: &Claim,
        launch: &str,
        account: &str,
        room: &str,
        live: bool,
        origin: &str,
    ) -> bool {
        if !live
            || claim.launch_id != launch
            || claim.account_id != account
            || claim.room_id != room
        {
            return false;
        }
        match &self.owner {
            Some(owner) => owner == &claim.instance_id,
            None => {
                self.owner = Some(claim.instance_id.clone());
                self.origin = Some(origin.to_owned());
                true
            }
        }
    }

    pub fn connect(&mut self, claim: &Claim, socket: &str) -> Option<String> {
        if self.owner.as_deref() != Some(claim.instance_id.as_str()) {
            return None;
        }
        self.socket = Some(socket.to_owned());
        self.origin.clone()
    }

    pub fn current(&self, claim: &Claim, socket: &str) -> bool {
        self.owner.as_deref() == Some(claim.instance_id.as_str())
            && self.socket.as_deref() == Some(socket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const LAUNCH: &str = "00000000-0000-4000-8000-000000000001";
    fn claim(instance: &str) -> Claim {
        Claim {
            launch_id: LAUNCH.into(),
            account_id: "account".into(),
            room_id: "topic".into(),
            instance_id: instance.into(),
        }
    }
    #[test]
    fn registration_window_is_guarded_only_for_codex() {
        assert!(required_for(Some(crate::Cli::CodexCli)));
        assert!(!required_for(Some(crate::Cli::ClaudeCode)));
        assert!(!required_for(None));
    }

    #[test]
    fn first_instance_only_and_same_instance_reconnect() {
        let mut state = Admission::default();
        let parent = claim("parent");
        assert!(state.admit(&parent, LAUNCH, "account", "topic", true, "origin"));
        assert!(!state.admit(&claim("child"), LAUNCH, "account", "topic", true, "other"));
        assert!(state.admit(&parent, LAUNCH, "account", "topic", true, "ignored"));
        assert_eq!(
            state.connect(&parent, "socket-1").as_deref(),
            Some("origin")
        );
        assert_eq!(
            state.connect(&parent, "socket-2").as_deref(),
            Some("origin")
        );
        assert!(!state.current(&parent, "socket-1"));
        assert!(state.current(&parent, "socket-2"));
        assert!(state.connect(&claim("child"), "socket-3").is_none());
        assert!(state.current(&parent, "socket-2"));
    }
    #[test]
    fn mismatch_and_dead_launch_do_not_claim_owner() {
        let parent = claim("parent");
        let mut state = Admission::default();
        for (launch, account, room, live) in [
            ("old", "account", "topic", true),
            (LAUNCH, "other", "topic", true),
            (LAUNCH, "account", "other", true),
            (LAUNCH, "account", "topic", false),
        ] {
            assert!(!state.admit(&parent, launch, account, room, live, "unused"));
        }
        assert!(state.admit(&parent, LAUNCH, "account", "topic", true, "origin"));
    }
    #[test]
    fn separate_launches_topics_and_accounts_have_independent_owners() {
        for (account, topic) in [
            ("account", "topic"),
            ("account", "other"),
            ("other", "topic"),
        ] {
            let mut state = Admission::default();
            let mut parent = claim("parent");
            parent.account_id = account.into();
            parent.room_id = topic.into();
            assert!(state.admit(&parent, LAUNCH, account, topic, true, topic));
        }
        let mut replacement = Admission::default();
        let mut next = claim("next");
        next.launch_id = "next-launch".into();
        assert!(!replacement.admit(
            &claim("parent"),
            "next-launch",
            "account",
            "topic",
            true,
            "unused"
        ));
        assert!(replacement.admit(&next, "next-launch", "account", "topic", true, "new-origin"));
    }
    #[test]
    fn app_server_owner_survives_starting_to_running_but_not_another_launch() {
        let parent = claim("parent");
        let mut starting = Admission::default();
        assert!(starting.admit(&parent, LAUNCH, "account", "topic", true, "server-origin"));
        assert_eq!(
            starting.connect(&parent, "early-socket").as_deref(),
            Some("server-origin")
        );
        let mut running = Admission::inherit(starting.clone(), LAUNCH, Some(LAUNCH));
        assert!(running.current(&parent, "early-socket"));
        assert!(!running.admit(&claim("child"), LAUNCH, "account", "topic", true, "unused"));
        assert_eq!(
            running.connect(&parent, "pty-socket").as_deref(),
            Some("server-origin")
        );
        assert!(!running.admit(&parent, LAUNCH, "account", "topic", false, "unused"));
        let mut next = Admission::inherit(starting, LAUNCH, Some("next-launch"));
        let mut next_claim = claim("next-parent");
        next_claim.launch_id = "next-launch".into();
        assert!(next.admit(
            &next_claim,
            "next-launch",
            "account",
            "topic",
            true,
            "next-origin"
        ));
        assert_eq!(
            next.connect(&next_claim, "next-socket").as_deref(),
            Some("next-origin")
        );
    }

    #[test]
    fn malformed_or_missing_claim_is_rejected() {
        assert!(Claim::read(b"{}").is_none());
        assert!(Claim::read(b"not json").is_none());
        let body = serde_json::json!({"launch_id":LAUNCH,"account_id":"account","room_id":"topic","instance_id":LAUNCH});
        assert!(Claim::read(body.to_string().as_bytes()).is_some());
    }
}
