//! Room posts kept for a Claude Code seat while it waits on a permission
//! prompt (#346). See docs/2-screen.md, "許可待ちの席には投稿を打ち込まない".
//!
//! The room types every post into each seat's terminal, the text and then the
//! submit key (`terminal_input::SUBMIT`). A seat stopped on a permission
//! prompt draws that prompt in the same terminal, and the submit key lands on
//! it: the prompt's highlighted choice, by default 「はい」, is taken with no
//! one choosing it. So while the seat's hooks say an agent of it is waiting
//! (`hook_activity::Activity::any_waiting`), what the room would type there is
//! kept here instead, and handed over once the wait has ended — whichever way
//! it ended: a key of the person's, the agent's next call, the allowed tool's
//! end, the turn's end, the seat being launched again.
//!
//! This is the queue and the hand-over, free of tauri so that it is tested.
//! Which seat it belongs to, the lock, and the terminal are the app's
//! (`src-tauri/src/hook_activity.rs`).
//!
//! **Each post is handed over once, in the order it came.** While anything is
//! kept, or a hand-over is under way, a new post queues behind it even when the
//! seat is no longer waiting, so nothing typed later overtakes it. A hand-over
//! stops the moment a wait begins again, and what is left waits for that one
//! to end. An item the terminal refused is put back at the front and the
//! hand-over ends: nothing is dropped.
//!
//! **The usage-limit hold comes first** (#294, #342). A post for a seat stopped
//! on its usage limit is that hold's, as before, and never reaches this queue.
//! A kept post whose hand-over finds the seat stopped on its limit is given to
//! that hold (`deliver`) rather than typed, and goes out with its digest at the
//! recovery. The digest itself, typed at a recovery that finds the seat
//! waiting, is kept here like a post and typed after the wait — it is already
//! the limit hold's release, and is not given back to it. One order is not
//! kept: a post held here before a limit began goes into the limit's mailbox
//! after the posts the limit itself held while this wait lasted.
//!
//! **A wait's end is not a hand-over at once.** The hand-over is begun
//! `RELEASE_GRACE` after the end was heard, and only if no wait has begun in
//! between: a prompt answered in the terminal can be followed at once by the
//! next one (a second call waiting on its own permission), and the request of
//! that one is heard before its prompt is drawn.
use crate::codex::limit::Held;
use std::collections::VecDeque;
use std::time::Duration;

/// How long after a wait's end the hand-over begins, if no wait has begun
/// again by then.
pub const RELEASE_GRACE: Duration = Duration::from_millis(1500);

/// One thing the room would have typed into the seat's terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The post's id, or the watermark a digest is labelled with: for the log
    /// only.
    pub message_id: String,
    /// What is typed, label line and all (`terminal_input::compose`).
    pub text: String,
    /// The post as the usage-limit hold keeps one, or `None` for what is not a
    /// post of the room's — the limit hold's own digest.
    pub post: Option<Held>,
}

/// One seat's kept posts.
#[derive(Debug, Default)]
pub struct Queue {
    items: VecDeque<Item>,
    /// A hand-over has begun and not ended.
    handing: bool,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// One post for the seat. `Some` is the post back, to be typed now: the
    /// seat is not `waiting`, and nothing is kept or being handed over. `None`
    /// is the post kept.
    pub fn pass(&mut self, waiting: bool, item: Item) -> Option<Item> {
        if waiting || self.handing || !self.items.is_empty() {
            self.items.push_back(item);
            return None;
        }
        Some(item)
    }

    /// Whether a hand-over begins now: something is kept, the seat is not
    /// `waiting`, and no hand-over is under way. The caller that gets `true`
    /// takes items with `next` until it gets `None`.
    pub fn begin(&mut self, waiting: bool) -> bool {
        if waiting || self.handing || self.items.is_empty() {
            return false;
        }
        self.handing = true;
        true
    }

    /// The next item of a hand-over, or `None` when it ends: a wait has begun
    /// again (what is left stays kept), or nothing is left.
    pub fn next(&mut self, waiting: bool) -> Option<Item> {
        if !self.handing {
            return None;
        }
        let item = if waiting { None } else { self.items.pop_front() };
        if item.is_none() {
            self.handing = false;
        }
        item
    }

    /// An item the hand-over could not deliver: back at the front, and the
    /// hand-over ends.
    pub fn put_back(&mut self, item: Item) {
        self.items.push_front(item);
        self.handing = false;
    }

    /// An item a hand-over of an earlier launch could not deliver, back at the
    /// front of the queue the launch after it carried (`carry`). A hand-over of
    /// that launch, if one is under way, is not ended by it.
    pub fn put_first(&mut self, item: Item) {
        self.items.push_front(item);
    }

    /// The queue of a seat launched again: what was kept carries over to the
    /// new launch, and a hand-over that was under way for the old one ends.
    pub fn carry(self) -> Queue {
        Queue {
            items: self.items,
            handing: false,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// How one item of a hand-over went.
#[derive(Debug, PartialEq)]
pub enum Delivered {
    /// Given to the usage-limit hold, the seat being stopped on its limit.
    ToLimit,
    /// Handed to the terminal.
    Typed,
}

/// One line of `permission_prompt::LOG_FILE` for a kept post (#346):
/// `[prompt-hold] <kept|typed|to-limit|refused> message=<id> room=<topic id>
/// account=<account id> at=<time>`. `kept` when the room keeps it for the
/// wait, then one of the other three when the hand-over reaches it.
pub fn log_line(action: &str, message_id: &str, topic_id: &str, account_id: &str, at: &str) -> String {
    let token = |text: &str| -> String {
        let cleaned: String = text
            .chars()
            .map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c })
            .take(120)
            .collect();
        if cleaned.is_empty() { "-".to_string() } else { cleaned }
    };
    format!(
        "[prompt-hold] {} message={} room={} account={} at={}",
        token(action),
        token(message_id),
        token(topic_id),
        token(account_id),
        token(at)
    )
}

/// Deliver one item of a hand-over: to the usage-limit hold when `limit` takes
/// its post (the seat is stopped on its limit), else typed. `Err` is the item
/// back when the terminal took nothing, for `Queue::put_back`.
pub fn deliver(
    item: Item,
    limit: impl FnOnce(&Held) -> bool,
    type_in: impl FnOnce(&str) -> bool,
) -> Result<Delivered, Item> {
    if item.post.as_ref().is_some_and(limit) {
        return Ok(Delivered::ToLimit);
    }
    if type_in(&item.text) {
        Ok(Delivered::Typed)
    } else {
        Err(item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_limit::{event, Gate};
    use serde_json::json;

    fn post(id: &str) -> Item {
        Item {
            message_id: id.into(),
            text: format!("[pullcept] {id}"),
            post: Some(Held {
                message_id: id.into(),
                speaker: "Lin".into(),
                at: None,
                content: id.into(),
                addressed: false,
            }),
        }
    }

    fn digest(id: &str) -> Item {
        Item {
            message_id: id.into(),
            text: format!("[pullcept] digest {id}"),
            post: None,
        }
    }

    /// The app's hand-over loop, as `src-tauri/src/hook_activity.rs` runs it,
    /// with `waiting` read again before each item.
    fn hand_over(
        q: &mut Queue,
        waiting: impl Fn() -> bool,
        mut limit: impl FnMut(&Held) -> bool,
        typed: &mut Vec<String>,
    ) {
        if !q.begin(waiting()) {
            return;
        }
        while let Some(item) = q.next(waiting()) {
            match deliver(item, &mut limit, |text| {
                typed.push(text.to_string());
                true
            }) {
                Ok(_) => {}
                Err(item) => {
                    q.put_back(item);
                    return;
                }
            }
        }
    }

    fn ids(typed: &[String]) -> Vec<&str> {
        typed.iter().map(|t| t.trim_start_matches("[pullcept] ")).collect()
    }

    #[test]
    fn a_free_seat_is_typed_into_at_once() {
        let mut q = Queue::new();
        assert_eq!(q.pass(false, post("a")), Some(post("a")));
        assert!(q.is_empty());
        assert!(!q.begin(false));
    }

    #[test]
    fn a_waiting_seat_keeps_posts_and_gets_each_once_in_order_after_the_wait() {
        let mut q = Queue::new();
        for id in ["a", "b", "c"] {
            assert_eq!(q.pass(true, post(id)), None);
        }
        let mut typed = Vec::new();
        // Still waiting: nothing goes.
        hand_over(&mut q, || true, |_| false, &mut typed);
        assert!(typed.is_empty());
        assert_eq!(q.len(), 3);
        // The wait ended.
        hand_over(&mut q, || false, |_| false, &mut typed);
        assert_eq!(ids(&typed), ["a", "b", "c"]);
        assert!(q.is_empty());
        // A second hand-over hands nothing again.
        hand_over(&mut q, || false, |_| false, &mut typed);
        assert_eq!(typed.len(), 3);
        // And the seat is free again.
        assert!(q.pass(false, post("d")).is_some());
    }

    #[test]
    fn a_post_after_the_wait_does_not_overtake_what_was_kept() {
        let mut q = Queue::new();
        q.pass(true, post("a"));
        // The wait has ended but the hand-over has not begun (the grace):
        // a new post queues behind.
        assert_eq!(q.pass(false, post("b")), None);
        assert!(q.begin(false));
        let first = q.next(false).unwrap();
        // Mid hand-over, the last kept item taken but not yet typed: still
        // behind.
        let second = q.next(false).unwrap();
        assert_eq!(q.pass(false, post("c")), None);
        assert_eq!((first.message_id.as_str(), second.message_id.as_str()), ("a", "b"));
        assert_eq!(q.next(false).unwrap().message_id, "c");
        assert_eq!(q.next(false), None);
        // The hand-over over, the seat is free.
        assert!(q.pass(false, post("d")).is_some());
    }

    #[test]
    fn a_wait_beginning_mid_hand_over_stops_it_and_the_rest_waits() {
        let mut q = Queue::new();
        for id in ["a", "b", "c"] {
            q.pass(true, post(id));
        }
        assert!(q.begin(false));
        assert_eq!(q.next(false).unwrap().message_id, "a");
        // The next prompt came.
        assert_eq!(q.next(true), None);
        assert_eq!(q.len(), 2);
        assert!(!q.begin(true));
        assert!(q.begin(false));
        assert_eq!(q.next(false).unwrap().message_id, "b");
        assert_eq!(q.next(false).unwrap().message_id, "c");
        assert_eq!(q.next(false), None);
    }

    #[test]
    fn one_hand_over_at_a_time() {
        let mut q = Queue::new();
        q.pass(true, post("a"));
        assert!(q.begin(false));
        assert!(!q.begin(false));
        // A caller that did not begin gets nothing.
        let mut other = Queue::new();
        other.pass(true, post("x"));
        assert_eq!(other.next(false), None);
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn a_refused_item_goes_back_to_the_front_and_nothing_is_lost() {
        let mut q = Queue::new();
        for id in ["a", "b"] {
            q.pass(true, post(id));
        }
        assert!(q.begin(false));
        let item = q.next(false).unwrap();
        let back = deliver(item, |_| false, |_| false).unwrap_err();
        q.put_back(back);
        assert_eq!(q.len(), 2);
        // A later hand-over starts from it.
        let mut typed = Vec::new();
        hand_over(&mut q, || false, |_| false, &mut typed);
        assert_eq!(ids(&typed), ["a", "b"]);
    }

    #[test]
    fn an_item_of_an_earlier_launch_goes_first_without_ending_a_hand_over() {
        let mut q = Queue::new();
        q.pass(true, post("b"));
        assert!(q.begin(false));
        q.put_first(post("a"));
        assert_eq!(q.next(false).unwrap().message_id, "a");
        assert_eq!(q.next(false).unwrap().message_id, "b");
        assert_eq!(q.next(false), None);
    }

    #[test]
    fn the_log_line_is_one_line_of_key_values() {
        assert_eq!(
            log_line("kept", "m-1", "topic", "acct", "2026-10-09T19:16:06.500+09:00"),
            "[prompt-hold] kept message=m-1 room=topic account=acct at=2026-10-09T19:16:06.500+09:00"
        );
        assert_eq!(log_line("typed", "", "a b", "c\nd", "t"), "[prompt-hold] typed message=- room=a_b account=c_d at=t");
    }

    #[test]
    fn a_relaunch_carries_what_was_kept_and_ends_the_old_hand_over() {
        let mut q = Queue::new();
        for id in ["a", "b"] {
            q.pass(true, post(id));
        }
        assert!(q.begin(false));
        assert_eq!(q.next(false).unwrap().message_id, "a");
        let mut q = q.carry();
        assert_eq!(q.len(), 1);
        assert!(q.begin(false));
        assert_eq!(q.next(false).unwrap().message_id, "b");
    }

    const PARENT: &str = "512c102e-fe28-4e8e-8f23-6cc4226ce2fc";

    fn line(second: u32, rejected: bool) -> serde_json::Value {
        let mut v = json!({"type":"assistant", "sessionId":PARENT, "session_id":PARENT,
            "isSidechain":false, "timestamp":format!("2026-10-09T14:00:{second:02}.000Z"),
            "uuid":format!("event-{second}"), "requestId":format!("request-{second}"),
            "message":{"model":"claude-opus-5-5"}});
        if rejected {
            v["isApiErrorMessage"] = json!(true);
            v["error"] = json!("rate_limit");
            v["apiErrorStatus"] = json!(429);
            v["message"]["model"] = json!("<synthetic>");
        }
        v
    }

    #[test]
    fn a_wait_ending_on_a_limited_seat_gives_its_posts_to_the_limit_hold() {
        // The permission wait and the usage limit at once (#342). The posts
        // kept for the prompt are not typed while the seat is stopped on its
        // limit; they go to the limit hold, and out once with its release.
        let mut gate = Gate::new(0, false);
        let mut q = Queue::new();
        // The prompt first: the seat is free of its limit, so the queue keeps.
        assert!(gate.hold(|| post("a").post.unwrap()).is_none());
        for id in ["a", "b"] {
            assert_eq!(q.pass(true, post(id)), None);
        }
        // The limit begins during the wait (another agent's call): the limit
        // hold is asked before the queue, and takes what comes now.
        gate.state.apply(event(PARENT, &line(1, true)).unwrap());
        assert!(gate.hold(|| post("c").post.unwrap()).is_some());
        // The wait ends while the seat is still stopped.
        let mut typed = Vec::new();
        hand_over(&mut q, || false, |held| gate.hold(|| held.clone()).is_some(), &mut typed);
        assert!(typed.is_empty(), "nothing is typed into a limited seat");
        assert!(q.is_empty(), "the queue let them go to the limit hold");
        // The one order not kept: the limit's own first (module docs).
        assert_eq!(
            gate.held().iter().map(|p| p.message_id.as_str()).collect::<Vec<_>>(),
            ["c", "a", "b"]
        );
        // The recovery releases each one once.
        gate.state.apply(event(PARENT, &line(2, false)).unwrap());
        assert_eq!(gate.release().len(), 3);
        assert!(gate.release().is_empty());
    }

    #[test]
    fn a_limit_on_a_seat_that_then_waits_keeps_its_hold() {
        // The limit first, then a prompt: every post is the limit hold's, the
        // queue keeps nothing, and the wait's end types nothing.
        let mut gate = Gate::new(0, false);
        let q = Queue::new();
        gate.state.apply(event(PARENT, &line(1, true)).unwrap());
        for id in ["a", "b"] {
            assert!(gate.hold(|| post(id).post.unwrap()).is_some());
        }
        assert!(q.is_empty());
        let mut q = q;
        let mut typed = Vec::new();
        hand_over(&mut q, || false, |held| gate.hold(|| held.clone()).is_some(), &mut typed);
        assert!(typed.is_empty());
        assert_eq!(gate.held().len(), 2);
    }

    #[test]
    fn a_recovery_during_a_wait_keeps_its_digest_for_after_the_wait() {
        let mut gate = Gate::new(0, false);
        let mut q = Queue::new();
        gate.state.apply(event(PARENT, &line(1, true)).unwrap());
        gate.hold(|| post("early").post.unwrap());
        // The recovery comes while a prompt is up: its digest is kept here
        // rather than typed onto the prompt.
        gate.state.apply(event(PARENT, &line(2, false)).unwrap());
        assert_eq!(gate.release().len(), 1);
        assert_eq!(q.pass(true, digest("notice")), None);
        // A post after the recovery, still during the wait: the limit hold no
        // longer takes it, the queue does.
        assert!(gate.hold(|| post("b").post.unwrap()).is_none());
        assert_eq!(q.pass(true, post("b")), None);
        let mut typed = Vec::new();
        hand_over(&mut q, || false, |held| gate.hold(|| held.clone()).is_some(), &mut typed);
        assert_eq!(ids(&typed), ["digest notice", "b"]);
        assert!(gate.held().is_empty());
    }

    #[test]
    fn a_digest_is_typed_even_if_a_new_limit_began_during_the_wait() {
        // The digest is the limit hold's release already: it is not given back.
        let mut gate = Gate::new(0, false);
        let mut q = Queue::new();
        q.pass(true, digest("notice"));
        gate.state.apply(event(PARENT, &line(3, true)).unwrap());
        let mut typed = Vec::new();
        hand_over(&mut q, || false, |held| gate.hold(|| held.clone()).is_some(), &mut typed);
        assert_eq!(ids(&typed), ["digest notice"]);
    }
}
