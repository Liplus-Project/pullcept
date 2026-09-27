//! What the room types into a session's terminal (#183).
//!
//! A post from the screen's own person reaches each AI session in its room as
//! input typed into that session's terminal, not as a channel push. What the
//! person at the screen says is what a user says, and a session's terminal is
//! where a user's words go in. Posts from sessions and webhook notices still
//! reach a session through the channel; that path is not touched here.
//!
//! Three things are decided here, each where a test can run it:
//!
//! - what one post reads as once it is typed ([`compose`]);
//! - that nothing inside it presses the submit key — the key is its own write,
//!   sent after a pause ([`SUBMIT`], [`SUBMIT_PAUSE`]);
//! - which terminals a post goes into, and which room connections that covers
//!   ([`targets`]).
//!
//! **Typed the way the terminal pane's own paste reaches the session.** The
//! pane bridges Ctrl+V by writing the clipboard text to the PTY as it is, in
//! one write, and Enter arrives as a separate write of `\r` (`src/main.ts`,
//! `term.onData` and `attachCustomKeyEventHandler`). No bracketed-paste
//! markers are added on that path. A multi-line paste made that way was
//! observed to arrive as one input (Master, 2026-09-27, #183), and it is that
//! path which is reproduced here: the text in one write, then `\r` on its own.

use serde_json::{json, Value};
use std::time::Duration;

/// The key that submits what is in a session's input box.
///
/// Sent as a write of its own, never on the end of the text. A text write and
/// `\r` arriving in one read are one chunk to the CLI, and a chunk longer than
/// one key is taken as text being entered, not as the key.
pub const SUBMIT: &str = "\r";

/// How long after the text the submit key is sent.
///
/// The gap is what puts the key in a read of its own, which is what a person
/// pasting and then pressing Enter produces. The value is not measured: it is
/// chosen to sit under a person's own gap between paste and Enter, and the
/// real-device check after merge is where it is tried (#183).
pub const SUBMIT_PAUSE: Duration = Duration::from_millis(200);

/// What the first line of a typed post starts with.
///
/// The session's instructions name this tag, so the line it opens can be read
/// as the room's own label on the post rather than as part of what was said.
pub const HEADER_TAG: &str = "[pullcept]";

/// One post, as it is typed into a session's terminal.
///
/// The first line is the room's label: [`HEADER_TAG`], then a JSON object with
/// the post's `message_id`, the `user` who said it and, when it was addressed,
/// `to`. The keys are the ones a channel push carries in its `meta`, so a
/// session reads one vocabulary whichever way a post reached it. The
/// `message_id` is what the session declares as `last_seen` on its next
/// `say_to_room` (#47): a post that arrived through the terminal is one the
/// session has seen, and without its id the floor would refuse the next thing
/// it said (Master 判断, 2026-09-27, #183).
///
/// The body follows from the second line, as it was said, except that it can
/// press no key: line endings become `\n` and every other control character is
/// dropped (`body`). Nothing in the returned text is `\r`, so the only submit
/// is the [`SUBMIT`] written after it.
pub fn compose(message_id: &str, speaker: &str, to: Option<&str>, content: &str) -> String {
    let mut label = json!({
        "message_id": message_id,
        "user": speaker,
    });
    if let Some(to) = to {
        label["to"] = Value::String(to.to_string());
    }
    // serde_json escapes every control character in a string, `\r` and `\n`
    // among them, so the label stays one line whatever a name holds.
    format!("{HEADER_TAG} {label}\n{}", body(content))
}

/// What was said, made safe to type.
///
/// `\r\n` and a lone `\r` become `\n`: `\r` is the submit key, and a body that
/// carried one could send half of itself. Tabs and `\n` stay. Every other
/// control character is dropped rather than typed, because a terminal reads
/// one as a key — an escape would start a sequence and a `^C` would interrupt
/// the session — and none of them is text the composer means to hand over.
fn body(content: &str) -> String {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// One terminal a post is typed into, and the room connections it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pty_id: String,
    /// The connections in the room whose session is on this terminal. The
    /// room does not also send them the post as a frame: the post is in front
    /// of the session already, and a second copy through the channel is the
    /// double delivery #183 rules out.
    pub origins: Vec<String>,
}

/// Which terminals a post from the screen goes into.
///
/// `seats` are the room's connections, each with the account it declared at
/// `hello` or `None`. `screen` is the screen's own connection, which is the
/// one that said the post and is never typed into. `pty_of` answers the
/// terminal a running session of that account has in this room, or `None`.
///
/// A connection that declared no account, or whose account has no running
/// terminal in this room, is not a target. It is something this app did not
/// launch, or a session between launch and seat, and the channel still
/// reaches it — it is left out here, not dropped from delivery.
///
/// Two connections declaring one account are one terminal. The app refuses the
/// same account twice in one room (`session::RoomSeats`), so the second one
/// came from outside; typing the post twice into the one terminal would put it
/// in front of that session twice.
pub fn targets<'a>(
    seats: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    screen: &str,
    pty_of: impl Fn(&str) -> Option<String>,
) -> Vec<Target> {
    let mut found: Vec<Target> = Vec::new();
    for (origin, account) in seats {
        if origin == screen {
            continue;
        }
        let Some(pty_id) = account.and_then(&pty_of) else {
            continue;
        };
        match found.iter_mut().find(|target| target.pty_id == pty_id) {
            Some(target) => target.origins.push(origin.to_string()),
            None => found.push(Target {
                pty_id,
                origins: vec![origin.to_string()],
            }),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label line, read back as the JSON it is.
    fn label(typed: &str) -> Value {
        let first = typed.lines().next().expect("a typed post has a first line");
        let json = first
            .strip_prefix(HEADER_TAG)
            .and_then(|rest| rest.strip_prefix(' '))
            .expect("the first line opens with the tag");
        serde_json::from_str(json).expect("the label is JSON")
    }

    #[test]
    fn the_label_carries_the_id_the_session_declares_as_last_seen() {
        let typed = compose("m-1", "Master", Some("Claude Lin"), "こんにちは");
        let label = label(&typed);
        assert_eq!(label["message_id"], "m-1");
        assert_eq!(label["user"], "Master");
        assert_eq!(label["to"], "Claude Lin");
    }

    #[test]
    fn an_unaddressed_post_has_no_to_key() {
        let label = label(&compose("m-1", "Master", None, "hi"));
        assert!(label.get("to").is_none(), "{label}");
    }

    #[test]
    fn the_body_follows_the_label_as_it_was_said() {
        let typed = compose("m-1", "Master", None, "一行目\n二行目");
        assert_eq!(
            typed.lines().skip(1).collect::<Vec<_>>(),
            ["一行目", "二行目"]
        );
    }

    #[test]
    fn nothing_typed_is_the_submit_key() {
        // A CR in the body, in a name, and in an addressee: none of them may
        // reach the terminal as the key, whichever line they sit on.
        let typed = compose("m\r1", "Mas\rter", Some("Lin\r\n"), "a\r\nb\rc\n");
        assert!(!typed.contains(SUBMIT), "{typed:?}");
        assert_eq!(typed.lines().skip(1).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn the_label_is_one_line_whatever_a_name_holds() {
        let typed = compose("m-1", "two\nlines", Some("and\nmore"), "body");
        assert_eq!(typed.lines().count(), 2, "{typed:?}");
        assert_eq!(label(&typed)["user"], "two\nlines");
    }

    #[test]
    fn a_control_character_is_not_typed_but_a_tab_is() {
        let typed = compose("m-1", "Master", None, "a\u{1b}[2Jb\u{3}c\td\u{7f}");
        assert_eq!(typed.lines().nth(1), Some("a[2Jbc\td"));
    }

    fn running<'a>(accounts: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |account| {
            accounts
                .iter()
                .find(|(held, _)| *held == account)
                .map(|(_, pty)| pty.to_string())
        }
    }

    #[test]
    fn the_screen_is_never_typed_into() {
        let found = targets(
            [("screen", Some("lin")), ("s1", Some("lin"))],
            "screen",
            running(&[("lin", "pty-lin")]),
        );
        assert_eq!(
            found,
            [Target {
                pty_id: "pty-lin".into(),
                origins: vec!["s1".into()]
            }]
        );
    }

    #[test]
    fn a_seat_with_no_running_terminal_is_left_to_the_channel() {
        let found = targets(
            [
                // Declared no account: joined from outside this app.
                ("outside", None),
                // An account with no running session in this room.
                ("stale", Some("lay")),
                ("s1", Some("lin")),
            ],
            "screen",
            running(&[("lin", "pty-lin")]),
        );
        let reached: Vec<&str> = found
            .iter()
            .flat_map(|target| target.origins.iter().map(String::as_str))
            .collect();
        assert_eq!(reached, ["s1"]);
    }

    #[test]
    fn one_terminal_is_typed_into_once() {
        let found = targets(
            [
                ("s1", Some("lin")),
                ("s2", Some("lin")),
                ("s3", Some("lay")),
            ],
            "screen",
            running(&[("lin", "pty-lin"), ("lay", "pty-lay")]),
        );
        assert_eq!(
            found,
            [
                Target {
                    pty_id: "pty-lin".into(),
                    origins: vec!["s1".into(), "s2".into()]
                },
                Target {
                    pty_id: "pty-lay".into(),
                    origins: vec!["s3".into()]
                },
            ]
        );
    }
}
