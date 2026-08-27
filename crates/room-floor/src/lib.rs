//! The floor of the room.
//!
//! What has been said, in the order the room put it in, and how much of it a
//! participant had seen at the moment they tried to speak.
//!
//! A participant composing a reply cannot see the floor. Posts that arrive
//! while they compose are delivered to their sidecar, but whether they enter
//! the participant's context is decided by where the next tool-result boundary
//! falls — and a reply that needs no tool has no boundary before its own send.
//! Delivery is therefore not reading, and the room cannot tell the two apart
//! from its own side. Only the speaker knows what they actually saw, so the
//! speaker declares it, as `last_seen` (#47).
//!
//! [`Floor::admit`] is one operation: it reads the watermark and appends the
//! post under the caller's single lock acquisition. That is the whole point of
//! the type. Two participants speaking at once are serialised by that lock, so
//! the first one's post is on the floor before the second one's check reads
//! it, and the second is refused rather than delivered blind. A design where
//! the check and the append can interleave gives both of them an empty floor,
//! which is the case this exists to close.
//!
//! The floor holds no opinion about content. Whether a missed post bears on
//! what the speaker was going to say is the speaker's judgment; one missed
//! post refuses the attempt, whoever it was addressed to.

use std::collections::VecDeque;

use serde::Serialize;

/// How many posts the floor keeps.
///
/// Bounded because a room runs for as long as the app does. The bound is what
/// makes a watermark resolvable or not: an id older than this has been
/// dropped, and [`Floor::admit`] then reads the speaker as having seen nothing
/// rather than guessing (see that method's watermark resolution).
pub const DEFAULT_CAPACITY: usize = 512;

/// One utterance, as it was said.
///
/// Not `Eq`: `hue` is a float. Nothing compares posts for identity — the
/// `message_id` is the identity — so the weaker bound costs nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub message_id: String,
    pub speaker: String,
    /// The hue the speaker had declared when they said it, in oklch degrees,
    /// or `None` when they declared none.
    ///
    /// Stamped by the caller from the connection the post arrived on, in the
    /// same critical section this is admitted under, and never read off the
    /// frame — a sender may name an addressee and may not name itself, and the
    /// colour is part of that attribution (#40).
    ///
    /// On the post rather than looked up when it is handed back: a name is not
    /// an identity here, so a lookup by name is the wrong participant as soon
    /// as two answer to one name, and the speaker may have left the room by
    /// then. It is what lets a refusal hand a missed post back in the colour it
    /// was said in (#108).
    pub hue: Option<f64>,
    pub content: String,
    pub to: Option<String>,
    pub ts: String,
}

/// A post the speaker had not seen, handed back in place of their own.
///
/// Carries what a participant needs in order to decide again: who said it,
/// what they said, who they said it to, and the `message_id` to declare as
/// `last_seen` on the next attempt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Missed {
    pub message_id: String,
    pub speaker: String,
    /// The hue it was said in, carried straight from the post. What lets a
    /// screen draw this post as the line it would have been, rather than as a
    /// line in some other colour (#108).
    pub hue: Option<f64>,
    pub content: String,
    pub to: Option<String>,
    pub ts: String,
}

/// What the floor did with an attempt to speak.
///
/// An enum rather than a bool, so the refusal can carry its reason. Widening
/// [`Admission::Admitted`] later — to hand back a position in a queue rather
/// than only the position taken — adds a field here and changes no caller's
/// signature (#47 constraint: leave no step up to turn assignment).
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// On the floor, at this position.
    Admitted { seq: u64 },
    /// Not on the floor. These are the posts the speaker had not seen, oldest
    /// first. Nothing was delivered in their place.
    Unseen(Vec<Missed>),
}

#[derive(Debug, Clone)]
struct Entry {
    seq: u64,
    /// The connection the post arrived on. Never a name: two participants may
    /// answer to one name, and a name test would hold one of them responsible
    /// for the other's post.
    origin: String,
    post: Post,
}

/// The room's ordering authority.
#[derive(Debug)]
pub struct Floor {
    seq: u64,
    log: VecDeque<Entry>,
    capacity: usize,
}

impl Default for Floor {
    fn default() -> Self {
        Floor::new()
    }
}

impl Floor {
    pub fn new() -> Self {
        Floor::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Floor {
            seq: 0,
            log: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// The position of the newest post, or 0 when nothing has been said.
    ///
    /// Read when a participant takes a seat: it is the floor they start from,
    /// since what predates their connection was never delivered to them.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Empty the floor and start its numbering over.
    ///
    /// For a room changing topic: the posts of the topic being left are not the
    /// floor of the one being entered, and leaving them would refuse the first
    /// thing said in the new one and hand back the old one's contents as
    /// "missed" (`room.rs`).
    ///
    /// The caller has seats holding `since` positions taken against the old
    /// numbering. Every one of them is now past the end of this floor, which
    /// reads as having seen everything rather than nothing — so the caller puts
    /// them back to [`Floor::seq`] itself. That is the room's to do: this type
    /// holds no seats.
    pub fn reset(&mut self) {
        self.seq = 0;
        self.log.clear();
    }

    /// Check the speaker against the floor and, if they are clear, put their
    /// post on it.
    ///
    /// One operation on purpose. The caller holds one lock across both halves,
    /// so concurrent speakers get an order instead of both reading the floor
    /// as it stood before either of them spoke.
    ///
    /// `since` is the position at which the speaker's seat was taken, used
    /// when they declare no watermark: a participant who joined mid
    /// conversation has seen nothing, and is owed nothing for what predates
    /// their seat either.
    ///
    /// Watermark resolution:
    ///
    /// - `last_seen` naming a post still on the floor: that post's position.
    /// - `last_seen` naming anything else — an id from before the retained
    ///   window, or a value the room never issued: position 0, which is the
    ///   whole retained floor. A value the room cannot resolve is read as
    ///   having seen nothing, erring toward refusing (#47 constraint). It
    ///   costs the speaker one round trip and cannot cost anyone a missed
    ///   post.
    /// - no `last_seen`: `since`.
    ///
    /// The speaker's own posts are never counted against them. They are not
    /// delivered back to their author, so there was nothing there to read.
    pub fn admit(
        &mut self,
        origin: &str,
        since: u64,
        last_seen: Option<&str>,
        post: Post,
    ) -> Admission {
        let watermark = self.watermark(since, last_seen);
        let missed: Vec<Missed> = self
            .log
            .iter()
            .filter(|entry| entry.seq > watermark && entry.origin != origin)
            .map(|entry| Missed {
                message_id: entry.post.message_id.clone(),
                speaker: entry.post.speaker.clone(),
                hue: entry.post.hue,
                content: entry.post.content.clone(),
                to: entry.post.to.clone(),
                ts: entry.post.ts.clone(),
            })
            .collect();

        if !missed.is_empty() {
            return Admission::Unseen(missed);
        }

        self.seq += 1;
        let seq = self.seq;
        self.log.push_back(Entry {
            seq,
            origin: origin.to_string(),
            post,
        });
        while self.log.len() > self.capacity {
            self.log.pop_front();
        }
        Admission::Admitted { seq }
    }

    fn watermark(&self, since: u64, last_seen: Option<&str>) -> u64 {
        match last_seen {
            None => since,
            Some(id) => self
                .log
                .iter()
                .find(|entry| entry.post.message_id == id)
                .map(|entry| entry.seq)
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    fn post(message_id: &str, speaker: &str, content: &str) -> Post {
        Post {
            message_id: message_id.to_string(),
            speaker: speaker.to_string(),
            hue: None,
            content: content.to_string(),
            to: None,
            ts: "2026-08-23T00:00:00.000Z".to_string(),
        }
    }

    fn admitted_seq(admission: &Admission) -> u64 {
        match admission {
            Admission::Admitted { seq } => *seq,
            Admission::Unseen(missed) => panic!("expected admission, refused with {missed:?}"),
        }
    }

    fn refusal(admission: &Admission) -> &[Missed] {
        match admission {
            Admission::Unseen(missed) => missed,
            Admission::Admitted { seq } => panic!("expected refusal, admitted at {seq}"),
        }
    }

    #[test]
    fn a_reset_floor_admits_a_speaker_carrying_a_watermark_from_before_it() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "前のトピック"));
        floor.admit("claude", 0, Some("m-1"), post("c-1", "Claude", "はい"));

        floor.reset();
        assert_eq!(floor.seq(), 0);

        // The watermark names a post the floor no longer holds, which resolves
        // to position 0. An empty floor has nothing past 0, so the speaker is
        // admitted rather than handed back a topic they have already read.
        let admission = floor.admit("master", 0, Some("c-1"), post("m-2", "Master", "続き"));
        assert_eq!(admitted_seq(&admission), 1);
    }

    #[test]
    fn an_empty_floor_admits_the_first_speaker() {
        let mut floor = Floor::new();
        let admission = floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        assert_eq!(admitted_seq(&admission), 1);
    }

    #[test]
    fn a_speaker_who_has_seen_the_floor_is_admitted() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        let admission = floor.admit("lin", 0, Some("m-1"), post("m-2", "Claude Lin", "はい"));
        assert_eq!(admitted_seq(&admission), 2);
    }

    #[test]
    fn an_unseen_post_refuses_the_speaker_and_is_handed_back() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        floor.admit("lay", 0, Some("m-1"), post("m-2", "Claude Lay", "答えます"));

        // Lin is still declaring m-1: it began composing before m-2 landed.
        let admission = floor.admit("lin", 0, Some("m-1"), post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].message_id, "m-2");
        assert_eq!(missed[0].speaker, "Claude Lay");
        assert_eq!(missed[0].content, "答えます");

        // Refused means not delivered: the floor did not take the post.
        assert_eq!(floor.seq(), 2);
    }

    #[test]
    fn the_refusal_carries_every_missed_post_oldest_first() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        floor.admit("lay", 0, Some("m-1"), post("m-2", "Claude Lay", "ひとつめ"));
        floor.admit("master", 0, Some("m-2"), post("m-3", "Master", "ふたつめ"));

        let admission = floor.admit("lin", 0, Some("m-1"), post("m-4", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        let ids: Vec<&str> = missed.iter().map(|one| one.message_id.as_str()).collect();
        assert_eq!(ids, ["m-2", "m-3"]);
    }

    #[test]
    fn a_speakers_own_posts_are_not_held_against_them() {
        let mut floor = Floor::new();
        floor.admit("lin", 0, None, post("m-1", "Claude Lin", "ひとつめ"));
        // Nothing arrived in between, so Lin has no id but its own to declare.
        // A reply that needs no tool is exactly this shape.
        let admission = floor.admit("lin", 0, None, post("m-2", "Claude Lin", "ふたつめ"));
        assert_eq!(admitted_seq(&admission), 2);
    }

    #[test]
    fn an_unresolvable_watermark_is_read_as_having_seen_nothing() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));

        // `since` alone would have cleared this speaker; a declared value the
        // room cannot resolve must not be softened into it.
        let seated_at = floor.seq();
        let admission = floor.admit(
            "lin",
            seated_at,
            Some("m-nonexistent"),
            post("m-2", "Claude Lin", "答えます"),
        );
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].message_id, "m-1");
    }

    #[test]
    fn a_watermark_dropped_from_the_window_falls_back_to_the_whole_floor() {
        let mut floor = Floor::with_capacity(2);
        floor.admit("master", 0, None, post("m-1", "Master", "ひとつめ"));
        floor.admit("master", 0, Some("m-1"), post("m-2", "Master", "ふたつめ"));
        floor.admit("master", 0, Some("m-2"), post("m-3", "Master", "みっつめ"));

        // m-1 has been dropped. Declaring it resolves to nothing, so the
        // retained floor is handed back rather than assumed read.
        let admission = floor.admit("lin", 0, Some("m-1"), post("m-4", "Claude Lin", "答えます"));
        let ids: Vec<&str> = refusal(&admission)
            .iter()
            .map(|one| one.message_id.as_str())
            .collect();
        assert_eq!(ids, ["m-2", "m-3"]);
    }

    #[test]
    fn a_participant_is_not_shown_what_predates_their_seat() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));

        // Lin connects here and declares nothing: it has seen nothing, and
        // m-1 was never delivered to it either.
        let since = floor.seq();
        let admission = floor.admit("lin", since, None, post("m-2", "Claude Lin", "参加しました"));
        assert_eq!(admitted_seq(&admission), 2);
    }

    #[test]
    fn what_arrives_after_a_seat_is_taken_still_refuses() {
        let mut floor = Floor::new();
        let since = floor.seq();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));

        let admission = floor.admit("lin", since, None, post("m-2", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].message_id, "m-1");
    }

    #[test]
    fn the_refusal_hands_a_post_back_in_the_colour_it_was_said_in() {
        let mut floor = Floor::new();
        let mut declared = post("m-1", "Claude Lay", "ハロー");
        declared.hue = Some(275.0);
        floor.admit("lay", 0, None, declared);

        // The screen draws this line from the refusal, and it has to be the
        // line it would have been. Deriving the colour from the name on the way
        // out would repaint it (#108).
        let admission = floor.admit("lin", 0, None, post("m-2", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].hue, Some(275.0));
    }

    #[test]
    fn an_addressee_does_not_narrow_the_refusal() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        let mut addressed = post("m-2", "Master", "レイだけ答えて");
        addressed.to = Some("Claude Lay".to_string());
        floor.admit("master", 0, Some("m-1"), addressed);

        // Addressed to someone else, and it refuses all the same: the room
        // does not judge whether a missed post bears on what Lin would say.
        let admission = floor.admit("lin", 0, Some("m-1"), post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].to.as_deref(), Some("Claude Lay"));
    }

    #[test]
    fn two_speakers_at_once_get_an_order() {
        // The case the type exists for. Both threads hold the same watermark —
        // they composed from the same floor — and both try to speak. The lock
        // serialises them, so the second one's check runs against a floor the
        // first has already changed.
        let floor = Arc::new(Mutex::new(Floor::new()));
        floor
            .lock()
            .unwrap()
            .admit("master", 0, None, post("m-1", "Master", "ハロー"));

        let start = Arc::new(Barrier::new(2));
        let speakers = [
            ("lin", "m-lin", "Claude Lin"),
            ("lay", "m-lay", "Claude Lay"),
        ];
        let handles: Vec<_> = speakers
            .into_iter()
            .map(|(origin, id, speaker)| {
                let floor = Arc::clone(&floor);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    let mut floor = floor.lock().unwrap();
                    floor.admit(origin, 0, Some("m-1"), post(id, speaker, "答えます"))
                })
            })
            .collect();

        let results: Vec<Admission> = handles.into_iter().map(|one| one.join().unwrap()).collect();

        let admitted: Vec<&Admission> = results
            .iter()
            .filter(|one| matches!(one, Admission::Admitted { .. }))
            .collect();
        assert_eq!(
            admitted.len(),
            1,
            "exactly one of two simultaneous speakers may take the floor, got {results:?}"
        );
        assert_eq!(admitted_seq(admitted[0]), 2);

        let refused = results
            .iter()
            .find(|one| matches!(one, Admission::Unseen(_)))
            .expect("the other speaker must be refused");
        let missed = refusal(refused);
        assert_eq!(
            missed.len(),
            1,
            "the refused speaker is handed the post that beat them"
        );
        assert!(
            missed[0].message_id == "m-lin" || missed[0].message_id == "m-lay",
            "the missed post is the one that won the floor, got {:?}",
            missed[0]
        );

        // One post went on, not two: the refusal is a refusal, not a notice
        // attached to a delivery.
        assert_eq!(floor.lock().unwrap().seq(), 2);
    }
}
