//! The single input handed to a parent after its limit recovery is confirmed.
//! Confirmation remains the caller's Codex/Claude gate (docs/3-accounts.md, #389).
use crate::codex::limit::{digest, Held};

pub const CONTINUE: &str = "利用上限の解除を確認しました。中断していた作業を再開してください。中断していた作業がなければ、そのまま待機してください。";

pub fn input(held: &[Held]) -> String {
    match digest(held) {
        Some(digest) => format!("{CONTINUE}\n\n{digest}"),
        None => CONTINUE.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        claude_limit::{Event, Gate},
        codex::limit::{Answer, Limit},
        prompt_hold::{Item, Queue},
    };

    #[test]
    fn empty_mailbox_still_resumes_and_can_wait_without_new_work() {
        let text = input(&[]);
        assert_eq!(text, CONTINUE);
        assert!(text.contains("中断していた作業を再開してください"));
        assert!(text.contains("作業がなければ、そのまま待機"));
        assert!(!text.contains("read_room_history"));
    }

    #[test]
    fn held_posts_share_one_input_with_the_continuation() {
        let held = [Held {
            message_id: "post-1".into(),
            speaker: "Master".into(),
            at: None,
            content: "continue the same issue".into(),
            addressed: true,
        }];
        let text = input(&held);
        assert!(text.starts_with(CONTINUE));
        assert_eq!(text.matches(CONTINUE).count(), 1);
        assert!(text.contains(&digest(&held).unwrap()));
        assert!(text.contains("post-1"));
        assert!(text.contains("continue the same issue"));
    }

    #[test]
    fn codex_confirmation_releases_once_even_with_no_posts() {
        let mut limit = Limit::default();
        limit.stop(100, Some(200));
        assert!(!limit.answer(201, Answer::Unavailable));
        assert!(limit.is_limited());
        assert!(!limit.answer(
            202,
            Answer::Denied {
                resets_at: Some(200)
            }
        ));
        assert!(limit.is_limited());
        assert!(limit.answer(204, Answer::Allowed));
        assert_eq!(input(&limit.release()), CONTINUE);
        assert!(!limit.answer(203, Answer::Allowed));
    }

    #[test]
    fn claude_confirmed_response_keeps_one_continuation_through_permission_wait() {
        let mut gate = Gate::new(100, false);
        gate.state.apply(Event::Rejected {
            at: 101,
            id: "reject".into(),
            reset: Some(200),
        });
        assert!(gate.is_limited());
        let success = Event::Succeeded {
            at: 201,
            id: "success".into(),
        };
        assert!(gate.state.apply(success.clone()));
        gate.keep_holding();
        assert!(gate.is_limited());
        let mut queue = Queue::new();
        let item = Item {
            message_id: "recovery".into(),
            text: input(&gate.release()),
            post: None,
            launch_bound: true,
            relaunch_text: None,
        };
        assert!(queue.pass(true, item).is_none());
        assert!(!gate.is_limited());
        assert!(!gate.state.apply(success));
        assert!(!queue.begin(true));
        assert!(queue.begin(false));
        assert_eq!(queue.next(false).unwrap().text, CONTINUE);
        assert!(queue.next(false).is_none());
        assert!(!queue.begin(false));
    }

    #[test]
    fn recovery_waiting_on_permission_is_not_carried_to_another_launch() {
        let recovery = Item {
            message_id: "recovery".into(),
            text: input(&[]),
            post: None,
            launch_bound: true,
            relaunch_text: None,
        };
        let ordinary = Item {
            message_id: "post".into(),
            text: "ordinary room post".into(),
            post: None,
            launch_bound: false,
            relaunch_text: None,
        };
        let mut queue = Queue::new();
        queue.pass(true, recovery.clone());
        queue.pass(true, ordinary.clone());
        let mut next_launch = queue.carry();
        assert_eq!(next_launch.len(), 1);
        // An earlier launch's in-flight hand-over can also refuse after carry.
        next_launch.put_first(recovery);
        assert_eq!(next_launch.len(), 1);
        assert!(next_launch.begin(false));
        assert_eq!(next_launch.next(false), Some(ordinary));
        assert!(next_launch.next(false).is_none());
    }

    #[test]
    fn relaunch_keeps_the_existing_digest_and_watermark_without_continuation() {
        let held = [Held {
            message_id: "post-1".into(),
            speaker: "Master".into(),
            at: None,
            content: "same issue".into(),
            addressed: true,
        }];
        let fallback = digest(&held).unwrap();
        let item = Item {
            message_id: "recovery-watermark".into(),
            text: input(&held),
            post: None,
            launch_bound: true,
            relaunch_text: Some(fallback.clone()),
        };
        let mut queue = Queue::new();
        queue.pass(true, item.clone());
        let mut next_launch = queue.carry();
        assert!(next_launch.begin(false));
        let carried = next_launch.next(false).unwrap();
        assert_eq!(carried.message_id, "recovery-watermark");
        assert_eq!(carried.text, fallback);
        assert!(!carried.text.contains(CONTINUE));
        assert!(next_launch.next(false).is_none());
        // A refusal of an item already taken by the old launch has the same fallback.
        next_launch.put_first(item);
        let mut third_launch = next_launch.carry();
        assert!(third_launch.begin(false));
        assert_eq!(third_launch.next(false), Some(carried));
        assert!(third_launch.next(false).is_none());
    }
}
