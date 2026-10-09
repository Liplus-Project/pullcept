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

pub const APP_NOTICE_SPEAKER: &str = "Pullcept（自動通知）";
pub fn app_notice_content(content: &str) -> String {
    let content = content.trim_end();
    if content.ends_with("（返信不要）") {
        content.to_string()
    } else {
        format!("{content}（返信不要）")
    }
}

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
    /// The account the speaker declared, or `None` when they declared none.
    ///
    /// Stamped by the caller the way `hue` is, and for the same reason: who
    /// said it is attribution, which a sender never supplies about itself. It
    /// is carried and never judged on — the floor keys on the connection, and
    /// an account id is not an identity here (#59). What reads it is the
    /// screen, which draws a post from an `mcp` account folded (#193).
    pub account: Option<String>,
    /// True when the app itself said it rather than anyone in the room: a
    /// notice of its own, such as a Codex seat stopping on its usage limit
    /// (#294). Who said it, on the axis of `speaker` and `account`, and stamped
    /// by the caller off the origin the post came in on, never off the frame —
    /// a session may answer to the app's name, and that does not make its post
    /// the app's. What reads it is the screen, which draws the app's icon in
    /// the circle of such a post (#340).
    pub from_app: bool,
    pub content: String,
    /// The names it was addressed to, in the order they were named, or empty
    /// when it was said to the room (#204). Always passed through
    /// [`addressees`] on the way in, so no entry is blank and none repeats.
    pub to: Vec<String>,
    pub ts: String,
}

/// The names a post is addressed to, as the room keeps them (#204).
///
/// Each name trimmed; a name that is empty once trimmed dropped; a name already
/// on the list dropped, keeping the first place it was named in. Empty is the
/// room as a whole, never a list holding an empty name: a participant matching
/// `to` against its own name must not have to rule the empty string out first,
/// and naming one participant twice says nothing the first naming did not.
///
/// Names, not participants. `to` carries display names, so two participants
/// answering to one name are both addressed by it, and nothing here can tell
/// them apart.
pub fn addressees(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim();
        if !name.is_empty() && !kept.iter().any(|held| held == name) {
            kept.push(name.to_string());
        }
    }
    kept
}

/// Who put a post into the room, as far as its text is read for addressees
/// (#206).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    /// Someone in the room: the person at the screen, or a session. Their
    /// `@名前` address.
    Participant,
    /// A local MCP server's notice (#169). Its text is external content — a
    /// GitHub comment, say — and passes through as it arrived: the app does not
    /// rewrite it, and an outside author cannot address anyone in the room by
    /// writing their name.
    Notice,
}

/// Why a post whose text was nothing but mentions was not taken.
pub const MENTIONS_ONLY: &str = "宛先の @名前 のほかに本文がありません。";

/// Read a post's `@名前` into its addressees, the way the room does as it takes
/// the post (#206).
///
/// From a participant, each `@名前` naming someone in `names` moves out of the
/// text and onto `to` ([`take_mentions`]). A `to` the sender already gave stays
/// first, and a name on it is not added twice. A post that was nothing but
/// mentions is not taken: [`MENTIONS_ONLY`] comes back and the post is left as
/// it was.
///
/// A notice is left as it arrived, text and `to` alike.
pub fn address<'a>(
    post: &mut Post,
    sender: Sender,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), &'static str> {
    if sender == Sender::Notice {
        return Ok(());
    }
    let (content, named) = take_mentions(&post.content, names);
    if !named.is_empty() && content.trim().is_empty() {
        return Err(MENTIONS_ONLY);
    }
    post.content = content;
    post.to = addressees(std::mem::take(&mut post.to).into_iter().chain(named));
    Ok(())
}

/// Take the `@名前` that name someone in the room out of what was said, and
/// hand back the names they named (#206).
///
/// `names` is who is in the room. An `@` — or the full-width `＠` an IME in
/// kana mode types for the same key — followed by one of those names, case
/// aside, and then by whitespace, punctuation or the end of the text is a
/// mention. Where several names match at one `@`, the longest is taken, so
/// `@Claude Lay` is not read as `Claude` followed by ` Lay`. The name handed
/// back is the one the room holds, not the spelling that was typed.
///
/// An `@…` naming no one in the room is left in the text as it was written,
/// and addresses no one. The room reads the text by the names it holds, so a
/// name typed out by hand addresses as a name picked from a list does: the two
/// are the same characters.
///
/// Returns the text with each mention gone and the space it leaves behind
/// closed up, and the names in the order they were first written, each once.
/// Text holding no mention comes back as it was.
pub fn take_mentions<'a>(
    content: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> (String, Vec<String>) {
    let mut names: Vec<&str> = names
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    // Longest first, so the first match at an `@` is the longest one.
    names.sort_by_key(|name| std::cmp::Reverse(name.chars().count()));

    let mut text = String::with_capacity(content.len());
    let mut named: Vec<String> = Vec::new();
    let mut rest = content;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        let hit = if c == '@' || c == '＠' {
            names
                .iter()
                .find_map(|name| mention_len(after, name).map(|len| (*name, len)))
        } else {
            None
        };
        match hit {
            Some((name, len)) => {
                if !named.iter().any(|held| held == name) {
                    named.push(name.to_string());
                }
                rest = &after[len..];
                close_up(&mut text, &mut rest);
            }
            None => {
                text.push(c);
                rest = after;
            }
        }
    }
    (text, named)
}

/// How many bytes of `text` the name takes when `text` opens with it, case
/// aside, and the name ends there; `None` otherwise.
fn mention_len(text: &str, name: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for want in name.chars() {
        let (_, got) = chars.next()?;
        if !got.to_lowercase().eq(want.to_lowercase()) {
            return None;
        }
    }
    let len = chars.next().map_or(text.len(), |(at, _)| at);
    match text[len..].chars().next() {
        None => Some(len),
        Some(next) if ends_name(next) => Some(len),
        Some(_) => None,
    }
}

/// Whether a name may end before `c`: whitespace or punctuation. ASCII
/// punctuation, general punctuation (`…` `“` `—`), the CJK punctuation block
/// (`、` `。` `「`) and the full-width forms of the ASCII punctuation
/// (`！` `？` `，`).
fn ends_name(c: char) -> bool {
    c.is_whitespace()
        || c.is_ascii_punctuation()
        || matches!(
            c,
            '\u{2000}'..='\u{206F}'
                | '\u{3000}'..='\u{303F}'
                | '\u{FF01}'..='\u{FF0F}'
                | '\u{FF1A}'..='\u{FF20}'
                | '\u{FF3B}'..='\u{FF40}'
                | '\u{FF5B}'..='\u{FF65}'
        )
}

/// Spaces within a line: whitespace other than a line break.
fn is_space(c: char) -> bool {
    c.is_whitespace() && c != '\n' && c != '\r'
}

/// Close up what a mention taken out of the text leaves behind.
///
/// `text` is what is kept so far and `rest` what follows the mention. The
/// spaces after a mention were what ended the name, and they go with it: what
/// stood before it keeps its own space, if it had one. Where the mention ended
/// a line, the spaces before it would be left trailing, and they go; a line
/// that held nothing else goes with its break. Punctuation after it stays: it
/// belongs to the sentence.
fn close_up(text: &mut String, rest: &mut &str) {
    *rest = rest.trim_start_matches(is_space);
    if rest.is_empty() || rest.starts_with('\n') {
        let kept = text.trim_end_matches(is_space).len();
        text.truncate(kept);
        if (text.is_empty() || text.ends_with('\n')) && rest.starts_with('\n') {
            *rest = &rest[1..];
        }
    }
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
    /// The account it was said as, carried straight from the post, so a line
    /// drawn from a refusal is drawn as the line it would have been (#193).
    pub account: Option<String>,
    /// Whether the app itself said it, carried straight from the post for the
    /// same reason (#340). Written only when true: a session reads a refusal as
    /// text, and the field says nothing about the posts it is false on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_app: bool,
    pub content: String,
    /// Empty when it was said to the room, as on [`Post`].
    pub to: Vec<String>,
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
                account: entry.post.account.clone(),
                from_app: entry.post.from_app,
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
            account: None,
            from_app: false,
            content: content.to_string(),
            to: Vec::new(),
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
    fn the_refusal_hands_a_post_back_with_the_account_it_was_said_as() {
        let mut floor = Floor::new();
        let mut notice = post("m-1", "github-webhook-mcp", "[issues] opened");
        notice.account = Some("mcp-github-webhook-mcp".to_string());
        floor.admit("notice", 0, None, notice);

        // The screen folds a line by the kind of the account it was said as,
        // and a line drawn from a refusal has to fold the way the live one
        // would have (#193).
        let admission = floor.admit("lin", 0, None, post("m-2", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed[0].account.as_deref(), Some("mcp-github-webhook-mcp"));
    }

    #[test]
    fn the_refusal_hands_a_post_back_as_the_app_s_own_when_it_was() {
        let mut floor = Floor::new();
        let mut notice = post("m-1", APP_NOTICE_SPEAKER, "Codex の席が止まりました");
        notice.from_app = true;
        floor.admit("app", 0, None, notice);
        floor.admit("lay", 0, Some("m-1"), post("m-2", APP_NOTICE_SPEAKER, "参加者が同じ名前を名乗った"));

        // The screen draws the app's icon on the app's own post, and a line
        // drawn from a refusal has to carry it the way the live one did; the
        // posts beside it do not (#340).
        let admission = floor.admit("lin", 0, None, post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert!(missed[0].from_app);
        assert!(!missed[1].from_app);
    }

    #[test]
    fn a_refused_post_carries_from_app_on_the_wire_only_when_it_is_true() {
        let mut floor = Floor::new();
        let mut notice = post("m-1", APP_NOTICE_SPEAKER, "Codex の席が止まりました");
        notice.from_app = true;
        floor.admit("app", 0, None, notice);
        floor.admit("lay", 0, Some("m-1"), post("m-2", "Claude Lay", "了解"));

        // A refusal reaches a session too, which reads it as text: a field that
        // is false on every post but the app's own is left off them (#340).
        let admission = floor.admit("lin", 0, None, post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        let app = serde_json::to_value(&missed[0]).expect("serialize");
        let said = serde_json::to_value(&missed[1]).expect("serialize");
        assert_eq!(app["from_app"], serde_json::Value::Bool(true));
        assert!(said.get("from_app").is_none());
    }

    #[test]
    fn an_addressee_does_not_narrow_the_refusal() {
        let mut floor = Floor::new();
        floor.admit("master", 0, None, post("m-1", "Master", "ハロー"));
        let mut addressed = post("m-2", "Master", "レイだけ答えて");
        addressed.to = vec!["Claude Lay".to_string(), "Master".to_string()];
        floor.admit("master", 0, Some("m-1"), addressed);

        // Addressed to someone else, and it refuses all the same: the room
        // does not judge whether a missed post bears on what Lin would say.
        let admission = floor.admit("lin", 0, Some("m-1"), post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].to, ["Claude Lay", "Master"]);
    }

    #[test]
    fn addressees_are_trimmed_kept_once_and_never_blank() {
        let names = |list: &[&str]| addressees(list.iter().map(|name| name.to_string()));
        assert_eq!(
            names(&[" Claude Lay ", "Claude Lin", "Claude Lay", "", "  "]),
            ["Claude Lay", "Claude Lin"]
        );
        // The order they were named in, not an order of the room's own.
        assert_eq!(names(&["Master", "Claude Lin"]), ["Master", "Claude Lin"]);
        // Nothing left is the room as a whole.
        assert!(names(&["", " "]).is_empty());
        assert!(names(&[]).is_empty());
    }

    const ROOM: [&str; 4] = ["Claude", "Claude Lay", "Claude Lin", "Master"];

    /// `content` said in a room holding [`ROOM`] reads as `text`, addressed to
    /// `names`.
    #[track_caller]
    fn reads(content: &str, text: &str, names: &[&str]) {
        let (kept, named) = take_mentions(content, ROOM);
        assert_eq!(kept, text, "text of {content:?}");
        assert_eq!(named, names, "addressees of {content:?}");
    }

    #[test]
    fn a_mention_of_someone_in_the_room_moves_from_the_text_to_the_addressees() {
        reads("@Claude Lay これ見て", "これ見て", &["Claude Lay"]);
        reads("これ見て @Master", "これ見て", &["Master"]);
        reads("これ @Master 見て", "これ 見て", &["Master"]);
        // No space before the `@` is needed: Japanese runs on without one. The
        // space after the name only ended it, and goes with it.
        reads("これ@Master 見て", "これ見て", &["Master"]);
    }

    #[test]
    fn the_longest_name_that_ends_at_a_boundary_is_taken() {
        // `Claude Lay`, not `Claude` followed by ` Lay`.
        reads("@Claude Lay 頼む", "頼む", &["Claude Lay"]);
        // A longer name that runs on into a word is not a match there, and the
        // shorter one that ends at the space is.
        reads("@Claude Layさん", "Layさん", &["Claude"]);
    }

    #[test]
    fn a_name_ends_at_whitespace_punctuation_or_the_end() {
        reads("@Master、どう？", "、どう？", &["Master"]);
        reads("@Master!", "!", &["Master"]);
        reads("聞いて @Master。", "聞いて 。", &["Master"]);
        reads("見て @Master", "見て", &["Master"]);
        // Running on into a word is not a name ending.
        reads("@Masters 見て", "@Masters 見て", &[]);
    }

    #[test]
    fn the_match_ignores_case_and_hands_back_the_name_the_room_holds() {
        reads("@claude lay 頼む", "頼む", &["Claude Lay"]);
        // The full-width `＠` an IME in kana mode types.
        reads("＠Master 見て", "見て", &["Master"]);
    }

    #[test]
    fn an_at_naming_no_one_in_the_room_stays_as_written() {
        reads("@Nobody 見て", "@Nobody 見て", &[]);
        reads("mail@example.com", "mail@example.com", &[]);
        reads("見て", "見て", &[]);
        // Nobody in the room: nothing is a mention.
        let (kept, named) = take_mentions("@Master 見て", []);
        assert_eq!((kept.as_str(), named.len()), ("@Master 見て", 0));
    }

    #[test]
    fn several_mentions_are_kept_in_order_each_once() {
        reads(
            "@Claude Lin @Claude Lay 二人とも、@claude lin もね",
            "二人とも、もね",
            &["Claude Lin", "Claude Lay"],
        );
    }

    #[test]
    fn the_space_a_mention_leaves_is_closed_up() {
        reads("一行目\n@Master\n二行目", "一行目\n二行目", &["Master"]);
        reads("@Master\n本文", "本文", &["Master"]);
        reads("一行目 @Master\n二行目", "一行目\n二行目", &["Master"]);
        reads("@Master  @Claude Lin  本文", "本文", &["Master", "Claude Lin"]);
        // Nothing but mentions leaves nothing.
        reads("@Master @Claude Lin", "", &["Master", "Claude Lin"]);
        reads(" @Master ", "", &["Master"]);
    }

    #[test]
    fn a_participants_post_is_addressed_by_its_mentions_after_the_to_it_gave() {
        let mut said = post("m-1", "Claude Lin", "@Master @Claude Lay 見て");
        said.to = vec!["Claude Lay".to_string()];
        assert_eq!(address(&mut said, Sender::Participant, ROOM), Ok(()));
        assert_eq!(said.content, "見て");
        assert_eq!(said.to, ["Claude Lay", "Master"]);
    }

    #[test]
    fn a_post_of_nothing_but_mentions_is_not_taken_and_is_left_as_it_was() {
        let mut said = post("m-1", "Master", "@Claude Lay");
        assert_eq!(address(&mut said, Sender::Participant, ROOM), Err(MENTIONS_ONLY));
        assert_eq!(said.content, "@Claude Lay");
        assert!(said.to.is_empty());
    }

    #[test]
    fn a_notice_passes_through_with_its_text_as_it_arrived_and_no_addressee() {
        // External content naming someone in the room: not rewritten, and the
        // outside author addresses no one by it — not even when the name is
        // all it says.
        for text in ["@Master このPRを見て", "@Claude Lay"] {
            let mut notice = post("m-1", "webhook", text);
            assert_eq!(address(&mut notice, Sender::Notice, ROOM), Ok(()));
            assert_eq!(notice.content, text);
            assert!(notice.to.is_empty(), "{:?}", notice.to);
        }
    }

    #[test]
    fn a_missed_post_carries_its_addressees_as_a_list() {
        // What goes out on the wire: a list, one name or several, and an empty
        // one for the room — never a bare string, never null.
        let mut floor = Floor::new();
        let mut addressed = post("m-1", "Master", "二人とも");
        addressed.to = vec!["Claude Lay".to_string(), "Claude Lin".to_string()];
        floor.admit("master", 0, None, addressed);
        floor.admit("master", 0, Some("m-1"), post("m-2", "Master", "全体へ"));
        let admission = floor.admit("lin", 0, None, post("m-3", "Claude Lin", "答えます"));
        let missed = refusal(&admission);
        let wire: Vec<String> = missed
            .iter()
            .map(|one| {
                let value = serde_json::to_value(one).expect("a missed post serialises");
                value["to"].to_string()
            })
            .collect();
        assert_eq!(wire, [r#"["Claude Lay","Claude Lin"]"#, "[]"]);
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

#[cfg(test)]
mod app_notice_tests {
    #[test]
    fn all_app_notices_have_one_reply_unneeded_suffix() {
        assert_eq!(super::APP_NOTICE_SPEAKER, "Pullcept（自動通知）");
        assert_eq!(
            super::app_notice_content("停止しました。"),
            "停止しました。（返信不要）"
        );
        assert_eq!(
            super::app_notice_content("停止しました。（返信不要） "),
            "停止しました。（返信不要）"
        );
    }
}
