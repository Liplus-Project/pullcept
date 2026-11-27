//! What a room participant may do to the servers the app runs (#620): read
//! their state, and restart one. There is no stop.
//!
//! The app is each server's one MCP client (`webhook.rs`), so a seat is not
//! connected to a server itself. What reaches a seat is the server's notices,
//! which the app posts into every room an AI session is seated in. Those seats
//! are what a restart interrupts, and the only ones the app can name: a seat
//! that loads its own copy of the server in its CLI runs a process the app does
//! not see, and a restart here does not touch it. They are reported as
//! 「接続中」 (`connected_seats`) and nothing more is claimed of them.
//!
//! A restart names the generation the caller read (`check_generation`), the way
//! `control_account_session` names the PTY it read: the caller has seen which
//! seats are connected before the restart goes ahead, and two seats restarting
//! the same server at once restart it once.
//!
//! What a restart came to is read back from the run after it (`Outcome`), and
//! said into the room as an app notice (`notice_text`). The process and the
//! posting are in the app; this module is the part that can be tested without
//! it.

/// Refuse a restart whose caller read a run that is no longer the current one.
///
/// `current` is 0 for a server this app run has not started, which is the
/// value `mcp_server_status` reports for it.
pub fn check_generation(expected: u64, current: u64) -> Result<(), String> {
    if expected == current {
        Ok(())
    } else {
        Err(format!(
            "Server changed since you read it (expected_generation {expected}, now {current}); \
             read mcp_server_status again and check its connected seats before restarting"
        ))
    }
}

/// The refusal for a name the file does not list. Restarting such a name would
/// only stop it (`app_mcp::restart_mcp_server`), and stopping is not something
/// a room participant is given.
pub fn unlisted(name: &str) -> String {
    format!(
        "Server \"{name}\" is not listed in {}; a restart would only stop it. Ask the person to edit the file",
        crate::FILE_NAME
    )
}

/// What one restart came to, read from the run after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `initialize` was answered.
    Running,
    /// It did not get to running.
    Failed(String),
    /// It got to running and ended again before the read.
    Ended(String),
    /// Still starting when the wait ran out. Not a failure: unconfirmed.
    StillStarting,
    /// Another restart replaced this run before it settled.
    Replaced,
}

impl Outcome {
    /// Whether the server came back: `Some(true)` running, `Some(false)` it did
    /// not, `None` not known yet.
    pub fn came_back(&self) -> Option<bool> {
        match self {
            Outcome::Running => Some(true),
            Outcome::Failed(_) | Outcome::Ended(_) => Some(false),
            Outcome::StillStarting | Outcome::Replaced => None,
        }
    }

    /// The state word the response carries, the same words `mcp_server_status`
    /// uses, with `replaced` for a run that is no longer the current one.
    pub fn state(&self) -> &'static str {
        match self {
            Outcome::Running => "running",
            Outcome::Failed(_) => "failed",
            Outcome::Ended(_) => "ended",
            Outcome::StillStarting => "starting",
            Outcome::Replaced => "replaced",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Outcome::Failed(detail) | Outcome::Ended(detail) => Some(detail),
            _ => None,
        }
    }
}

/// The names of the connected seats, once each, in the order given.
///
/// One account seated in two topics is one name in the notice; the response
/// keeps one entry per seat.
pub fn seat_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for name in names {
        if !seen.iter().any(|known| known == name) {
            seen.push(name.to_string());
        }
    }
    seen
}

/// The app notice a restart is said as: who restarted which server, what it
/// came to, and which seats were connected when it was restarted.
///
/// `server` is the name the server's account goes by, with the entry's name
/// beside it when the two differ. 「（返信不要）」 is added by the app notice
/// itself (`room_floor::app_notice_content`).
pub fn notice_text(requester: &str, server: &str, entry: &str, outcome: &Outcome, connected: &[String]) -> String {
    let label = if server == entry {
        server.to_string()
    } else {
        format!("{server}（{entry}）")
    };
    let result = match outcome {
        Outcome::Running => "成功".to_string(),
        Outcome::Failed(_) | Outcome::Ended(_) => "失敗".to_string(),
        Outcome::StillStarting => "未確認: 起動中のまま".to_string(),
        Outcome::Replaced => "未確認: 別の再起動に置き換わりました".to_string(),
    };
    let mut text = format!("{requester} が MCP サーバー {label} を再起動しました（{result}）");
    if let Some(detail) = outcome.detail() {
        text.push_str(&format!("\n理由: {}", detail.replace(['\r', '\n'], " ")));
    }
    let seats = if connected.is_empty() {
        "なし".to_string()
    } else {
        connected.join("、")
    };
    text.push_str(&format!("\n接続中だった席: {seats}"));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_goes_ahead_only_on_the_generation_read() {
        assert_eq!(check_generation(3, 3), Ok(()));
        assert_eq!(check_generation(0, 0), Ok(()), "a server not started yet is generation 0");
        let refused = check_generation(3, 4).unwrap_err();
        assert!(refused.contains("mcp_server_status"), "{refused}");
        assert!(refused.contains("connected seats"), "{refused}");
    }

    #[test]
    fn an_unlisted_name_is_refused_rather_than_stopped() {
        let refused = unlisted("gone");
        assert!(refused.contains("\"gone\""));
        assert!(refused.contains(crate::FILE_NAME));
        assert!(refused.contains("only stop"));
    }

    #[test]
    fn came_back_is_known_only_once_the_run_settled() {
        assert_eq!(Outcome::Running.came_back(), Some(true));
        assert_eq!(Outcome::Failed("x".into()).came_back(), Some(false));
        assert_eq!(Outcome::Ended("x".into()).came_back(), Some(false));
        assert_eq!(Outcome::StillStarting.came_back(), None);
        assert_eq!(Outcome::Replaced.came_back(), None);
        assert_eq!(Outcome::StillStarting.state(), "starting");
        assert_eq!(Outcome::Replaced.state(), "replaced");
    }

    #[test]
    fn seat_names_keep_one_name_per_account_in_order() {
        assert_eq!(seat_names(["Lin", "Lay", "Lin", "Luna"]), vec!["Lin", "Lay", "Luna"]);
        assert!(seat_names([]).is_empty());
    }

    #[test]
    fn the_notice_says_who_which_server_what_and_the_connected_seats() {
        let seats = vec!["Lin".to_string(), "Luna".to_string()];
        assert_eq!(
            notice_text("Lay", "github-webhook-mcp", "github-webhook-mcp", &Outcome::Running, &seats),
            "Lay が MCP サーバー github-webhook-mcp を再起動しました（成功）\n接続中だった席: Lin、Luna"
        );
        assert_eq!(
            notice_text("Lay", "Webhook", "github-webhook-mcp", &Outcome::Failed("spawn\nfailed".into()), &[]),
            "Lay が MCP サーバー Webhook（github-webhook-mcp） を再起動しました（失敗）\n理由: spawn failed\n接続中だった席: なし"
        );
        assert!(notice_text("Lay", "s", "s", &Outcome::StillStarting, &[]).contains("未確認: 起動中のまま"));
        assert!(notice_text("Lay", "s", "s", &Outcome::Replaced, &[]).contains("未確認"));
    }
}
