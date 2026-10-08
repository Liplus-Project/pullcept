//! The room socket.
//!
//! The app hosts; the sidecars connect. That direction is forced: the CLI
//! spawns its MCP servers itself, so the app never learns the launch moment or
//! a port chosen on that side. See `docs/1-room.md`.
//!
//! Frames on the wire are the room protocol:
//!
//!   sidecar -> room : { type: "hello", protocol, name, room, hue?, account_id? }
//!   sidecar -> room : { type: "post", message_id, content, to?: [name], ts, last_seen? }
//!   room -> sidecar : { type: "post_result", message_id, delivered, missed }
//!
//! One frame kind carries speech into the room, whoever produced it. A person
//! and a session are both participants; what separates them is a name, not a
//! frame. The earlier protocol had `say` for a person and `reply` for a
//! session, and only `say` was ever fanned out — the asymmetry was not a
//! missing line but the shape of the words, so the words went (#39).
//!
//! **Nothing said in the room goes back out on the socket (#195).** Every post
//! reaches a session by being typed into its terminal, with the room's label
//! on its first line — the person's, another session's and a notice alike. The
//! socket carries a session's own speech in and the room's answers to that one
//! connection back; it used to carry every post out as well, for the sidecar
//! to push onto the channel, and that path is gone with the channel.
//!
//! `to` is optional and carries the display names of the participants
//! addressed, as a list — one name or several (#204). An `@名前` in `content`
//! that names someone in the room is moved into it, out of the text (#206) —
//! for a participant's post; a notice is passed through as it arrived. The
//! room still delivers every post to every participant — narrowing delivery
//! here would make the room hold who heard what, and answering is the
//! participant's judgment, not the room's.
//!
//! `speaker` is stamped by the room from the connection the frame arrived on,
//! never read off the frame. A sender cannot claim to be someone else, and the
//! roster and the attribution cannot disagree.
//!
//! `hello` carries who this participant is in the room: the name they answer to
//! and, optionally, the hue they chose to be drawn in. Both are declarations
//! made at the moment of joining, which is the one moment a participant has to
//! make them — the same moment, and the same pair, the screen's own person
//! declares through `room_join`.
//!
//! Roster identity is the connection, not the name. Two participants may answer
//! to one name; they are still two, and one of them leaving must not take the
//! other off the roster (#40).
//!
//! `account_id` is optional on `hello` and on `room_join`, and the room does no
//! more with it than put it on the seat and hand it back on the roster. It is
//! not an identity here and does not become one by being more stable than a
//! name: the roster is keyed on the connection, self-suppression is decided on
//! the connection, and `speaker` is stamped from the connection — all three
//! unchanged, and all three are what would break if any of them started reading
//! this field (#39 / #40 / #47). What it buys is on the screen: its own list of
//! accounts can be joined against the roster by id rather than by name, which
//! is the match #40 and #53 ruled out and the reason the panel had split into
//! two lists (#57, #59).
//!
//! A connection may carry none. The room does not presume an account exists
//! behind a participant — a person or a session that has one is not a different
//! kind of participant from one that has not.
//!
//! `last_seen` on a post is the speaker's watermark: the `message_id` of the
//! newest post they had actually seen when they composed. The room checks it
//! against the floor (`room_floor::Floor`) and refuses the post outright when
//! anything is behind it, handing those posts back as `missed` instead of
//! delivering. The room knows what it handed to each connection, but delivery
//! is not reading — whether a post entered a participant's context depends on
//! where their next tool-result boundary fell, which the room cannot see. Only
//! the speaker knows, so the speaker declares (#47).
//!
//! The check and the stamp share one acquisition of the room's lock. That is
//! what gives two participants speaking at once an order: the first one's post
//! is on the floor before the second one's check reads it. Splitting them —
//! checking, then delivering — hands both of them the floor as it stood before
//! either spoke, which is the case the whole mechanism exists to close.
//!
//! `post_result` is the answer, and it goes back only to the connection that
//! posted. It is the reason a reply needing no other tool no longer misses
//! what arrived while it was composed: the call is itself the boundary, and
//! the refusal arrives on it.
//!
//! Everything the frontend needs arrives as a `room-message` event. The room
//! never reads a CLI's terminal output; that is not a message source.
//!
//! **The socket answers one thing that is not the protocol.** A session posts
//! its status line to `/hooks/status` on this same port, from a Claude Code
//! `statusLine` command the launch put on its line (#155). It is not a frame
//! and not a participant: no seat, no floor, no log — the app emits
//! `session-stats` for the named seat and answers. The two callers are told
//! apart by their first bytes, since a WebSocket upgrade is a `GET`.
//!
//! A status POST path, where #149 had put a second for a `StopFailure` hook. That
//! path went out with the hook (#161): 制限中 is read off the two rate-limit
//! percentages this same report already carries, so nothing is left for a
//! second POST to say. The first-byte split above is unchanged — it divides
//! the protocol from a POST, and what has gone is a second kind of POST.
//! #272 adds `/hooks/codex-session` on this authenticated listener for a guarded
//! native SessionStart ID. It records a session, without posting room speech.
//! A Codex seat's five values do not come in on this socket: the app reads
//! them off the session's rollout itself (#283, `codex_status`) and emits the
//! same `session-stats`.
//!
//! **There are several rooms, one per topic (#141, decision 3).** A room is a
//! topic's floor and the participants in it, and a topic that is not on the
//! screen is still a room: a session started in it keeps running, keeps being
//! spoken to by whoever else is in that room, and keeps speaking into that
//! topic's log, while the screen shows another one (decision 2). Switching
//! topics changes which room the screen is looking at, and nothing about any
//! room.
//!
//! One socket serves them all. The address is the app's and shared by every
//! room of the run, so `hello` names the room the connection is for, and the
//! connection stays in that room for as long as it lives. Delivery, the floor,
//! the log and the pull are all that room's own; nothing crosses from one room
//! to another. A `hello` naming no room, or a room this app is not holding, is
//! not seated anywhere — there is no room it could be put in, and choosing one
//! for it would put a session in a conversation it was not started into.
//!
//! What is admitted is also written to the room's log, inside the same
//! acquisition it was judged under, so the file's order is the floor's order
//! (`room_log`). A post is never held back on account of that write: it is in
//! the room before the disk is touched, and a failure there costs the record,
//! not the utterance (#48).
//!
//! **One post comes from no connection: a notice from a local MCP server
//! (#169).** The app receives it itself (`webhook`) and puts it into every room
//! an AI session is seated in, through the same `deliver` as everything else —
//! so it is logged and typed into the sessions like any post. It is said as
//! the server's `mcp` account (#193): the account's name, its colour and its
//! id are on the post. It takes no seat and is spoken from an origin no connection holds, so
//! nobody is skipped and nobody can address it.
//!
//! **Every post reaches a session through its terminal (#183, #195).** It
//! goes through `deliver` — the same floor, the same log, the same
//! `room-message` — and then each session in the room that this app launched,
//! the speaker's own aside, has the post typed into its terminal. The first
//! line is the room's label, carrying the post's `message_id` and the `role`
//! the app gives it: `admin` for the person at the screen, and for nothing
//! else — the socket never makes one, whatever account it names
//! (`terminal_input::role`). That is what lets a session tell its user's words
//! from a post that only informs it, on one path and in one order. A
//! connection with no terminal this app can type into is handed nothing live;
//! it reads the topic through the pull. The room writes to a terminal here; it
//! still reads nothing from one.

use crate::pty::PtyState;
use crate::room_log::{self, TopicRef};
use crate::session::RoomSeats;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use room_floor::{Admission, Floor, Missed, Post};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

/// Bumped when a frame's shape changes in a way a sidecar must notice.
///
/// 2: `say` and `reply` collapsed into one `post` frame; `agent` became `name`.
/// 3: `hello` carries the declared `hue` beside the name.
/// 4: `post` carries the speaker's `last_seen` watermark, and the room answers
///    every post with `post_result` (#47).
/// 5: `hello` may carry the `account_id` the session was launched as, and the
///    roster hands it back. Carried only — identity stays on the connection
///    (#59).
/// 6: `history` / `history_result` — a participant may pull what was said in
///    the current topic before it arrived. Pull only: the room still pushes
///    nothing it did not fan out live (#115, decision 4C).
/// 7: `hello` names the `room` the connection is for. Rooms are plural and
///    share one socket, so without it a connection has nowhere to be (#141).
/// 8: the room sends no `post` frames. Every post reaches a session through
///    its terminal, and the channel the sidecar pushed them onto is gone
///    (#195).
pub const PROTOCOL_VERSION: u32 = 9;

/// One post of the room, as the frontend sees it.
///
/// Carries no participant class. What separates two lines is the name on them.
/// `own` is a self/other axis for display, which is a property of the viewer,
/// not of the speaker.
#[derive(Debug, Clone, Serialize)]
pub struct RoomMessage {
    /// The room it was said in, which is the topic's id.
    ///
    /// The screen draws one room at a time and every room keeps talking, so a
    /// post arrives whether or not its room is the one on the glass. This is
    /// what the screen reads to leave it in its own topic rather than draw it
    /// into the conversation being looked at (#141).
    pub topic_id: String,
    pub message_id: String,
    /// Display name of the speaker.
    pub speaker: String,
    /// The hue the speaker declared, in oklch degrees, or `None` when they
    /// declared none. Carried on the message rather than looked up by name on
    /// the screen: a name is not an identity here, so a lookup by name is the
    /// wrong participant as soon as two answer to one name.
    pub hue: Option<f64>,
    /// The account the speaker declared, or `None` when they declared none.
    /// Stamped from the seat the way `hue` is; for a notice, the account of the
    /// server that pushed it (#193). The screen folds a line said as an `mcp`
    /// account, and reads the kind off the account this names.
    pub account: Option<String>,
    pub content: String,
    /// The names it was addressed to, or empty when it was said to the room.
    pub to: Vec<String>,
    pub ts: String,
    /// True when this screen's own participant produced it.
    pub own: bool,
}

/// One participant of the room, as the roster shows them.
///
/// `id` is the connection, and it is what the roster is keyed on. `name` is
/// what they are called and what a post can be addressed to — a display and
/// addressing attribute, never the identity.
#[derive(Debug, Clone, Serialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    /// Declared at join; `None` when this participant declared none, which the
    /// screen answers by deriving one from the name.
    pub hue: Option<f64>,
    /// The account this participant was launched as, or `None` when they
    /// declared none.
    ///
    /// Handed back so the screen can join its own account list against this
    /// roster by id. It is not the identity and is not what this entry is keyed
    /// on — `id` above is both, and stays both however much more stable an
    /// account id looks (#39 / #40 / #59). `None` is a participant like any
    /// other, not a participant the screen may leave out.
    pub account: Option<String>,
    /// True for this screen's own person. Viewer-relative, like a message's
    /// `own`, and there is one screen.
    pub own: bool,
}

/// One room's roster, as the `room-participants` event carries it.
///
/// Named by its room, for the reason a `RoomMessage` is: a roster changes in a
/// room the screen is not looking at — a session joining the topic it was
/// started in — and the screen keeps it for when that topic is opened (#141).
#[derive(Debug, Clone, Serialize)]
pub struct Roster {
    pub topic_id: String,
    pub participants: Vec<Participant>,
}

/// What the room did with a post, as the participant who submitted it sees it.
///
/// One shape for both callers, because there is one path. Refused is not an
/// error: the participant asked to speak and was told what they had not seen,
/// and deciding not to speak after reading it is a legitimate answer.
///
/// Widening this to carry a place in a queue as well as the posts that were
/// missed adds a field here and changes no signature (#47).
#[derive(Debug, Clone, Serialize)]
pub struct PostOutcome {
    /// True when the post went into the room.
    pub delivered: bool,
    /// The id the post is filed under, or `None` when it was refused. A
    /// refused post has no id in the room because it is not in the room.
    pub message_id: Option<String>,
    /// What the speaker had not seen, oldest first. Empty when `delivered`.
    pub missed: Vec<Missed>,
}

/// A frame on its way out to one connection: the room answering a participant
/// who posted or pulled.
///
/// The target rides beside the frame, never inside it: it is never serialised,
/// so no sender can supply one and no sender can forge one. It is the
/// connection, which is the one identity a name collision cannot blur (#40).
///
/// Only answers go out. What was said in the room reaches a session through
/// its terminal, not through here (#195).
#[derive(Clone)]
struct Fanout {
    target: String,
    frame: String,
}

#[derive(Debug, Deserialize)]
struct IncomingFrame {
    #[serde(rename = "type")]
    kind: String,
    message_id: Option<String>,
    name: Option<String>,
    hue: Option<f64>,
    account_id: Option<String>,
    content: Option<String>,
    /// A list of names, since protocol 9 (#204). The room keeps no reading of
    /// the single name protocol 8 sent: the sidecar is the one this build ships.
    to: Option<Vec<String>>,
    ts: Option<String>,
    last_seen: Option<String>,
    protocol: Option<u32>,
    /// The room a `hello` is for (#141).
    room: Option<String>,
    /// Correlates a `history` request with the `history_result` that answers
    /// it, the way `message_id` correlates a post with its receipt. Minted by
    /// the asker: two pulls may be in flight, and settling the wrong one would
    /// hand back another request's posts.
    request_id: Option<String>,
    limit: Option<usize>,
    before: Option<String>,
}

/// What the room holds about one seated participant.
#[derive(Debug, Clone)]
struct Seat {
    name: String,
    hue: Option<f64>,
    /// The account declared at join, or `None`. Held so the roster can hand it
    /// back; nothing in this file branches on it.
    account: Option<String>,
    /// Where the floor stood when this connection took its seat. It is the
    /// watermark of a participant who declares none: what predates the seat
    /// was never delivered to them, so it is not theirs to have missed.
    /// Preserved across a rename — that is the same participant, still having
    /// seen what they saw.
    since: u64,
}

/// One room: a topic, the floor of what has been said in it this run, and who
/// is in it.
struct Room {
    /// Everyone in the room, people and sessions alike, keyed by the connection
    /// they are in it on. Keyed on the connection rather than the name because
    /// a name is not unique: under a name-keyed roster two participants called
    /// `Claude Code` were one entry, and either of them disconnecting removed
    /// both (#40).
    participants: BTreeMap<String, Seat>,
    /// What has been said, and the order the room put it in. Lives under the
    /// same lock as the seats so the check and the stamp are one critical
    /// section (#47).
    ///
    /// **Its own, and never emptied by the screen moving (#141).** One floor for
    /// the app used to be emptied whenever the screen opened another topic,
    /// which was harmless while every session followed the screen and is not
    /// once they do not: a session still speaking in the topic being left would
    /// have its floor pulled out from under it, and a post it composed against
    /// what it had seen would be judged against nothing.
    floor: Floor,
    /// Which topic this room is: where a post said in it is written down, and
    /// what a pull from it reads back.
    ///
    /// It exists before anything is written down. A launch opens a new topic
    /// (#115, Master 判断5) and most runs of the app say nothing, so the index
    /// entry waits for the first post rather than the app being opened
    /// (`room_log::TopicRef`).
    topic: TopicRef,
}

impl Room {
    fn new(topic: TopicRef) -> Self {
        Room {
            participants: BTreeMap::new(),
            floor: Floor::new(),
            topic,
        }
    }
}

/// What the screen's own person declared at `room_join`.
///
/// Held apart from any one seat because the person is at the screen, not in a
/// room: every room the screen opens seats them under this, so opening a topic
/// is enough to be in it and nobody has to join again (#141).
#[derive(Debug, Clone)]
struct Declaration {
    name: String,
    hue: Option<f64>,
    account: Option<String>,
}

struct RoomsInner {
    port: Option<u16>,
    /// Every room this run holds, keyed on its topic id.
    ///
    /// A room is made when its topic is opened — by a launch of the app, by 新規,
    /// or by picking one from the list — and goes when the topic is deleted. It
    /// is not dropped for going quiet: a session may be running in it, and a
    /// room that vanished under a running session would leave it no floor to
    /// speak onto.
    rooms: BTreeMap<String, Room>,
    /// The room the screen has open: the one 新規 and the list move between.
    current: String,
    /// The person at the screen, once they have joined.
    local: Option<Declaration>,
}

/// The name whoever is on `origin` answers to, or `None` when no one is seated
/// there yet.
///
/// Takes the seat table rather than a `RoomState`, and the delivery pump in
/// `serve_participant` is the whole reason: `RoomState` carries its
/// `Sender` beside the `Arc`, not inside it, so holding one to read a name
/// would hold a sender too. Read for logging only. Nothing branches on it, and
/// nothing may: a name is not the identity here (#40), so this is the readable
/// half of a handle whose other half is the connection id.
fn name_on(seats: &Mutex<RoomsInner>, origin: &str) -> Option<String> {
    seats
        .lock()
        .rooms
        .values()
        .find_map(|room| room.participants.get(origin))
        .map(|seat| seat.name.clone())
}

/// Shared handle to the room socket. Cloneable; all clones share every room.
#[derive(Clone)]
pub struct RoomState {
    inner: Arc<Mutex<RoomsInner>>,
    /// The room's answers, each to the one connection it is for.
    to_participants: broadcast::Sender<Fanout>,
    /// Bearer token the sidecar must present. Generated per app run, handed to
    /// the sidecar through `.mcp.json` env, never written anywhere else.
    token: String,
    /// The screen's own connection. It has no socket, so it needs an identity
    /// minted here to sit on the same suppression axis as every other one.
    ///
    /// One for every room: it is one person at one screen, seated once in each
    /// room the screen has opened, and each of those seats is in its own
    /// room's roster.
    local_origin: String,
    /// The origin a notice from a local MCP server is posted from (#169).
    ///
    /// Minted here like the screen's, and belonging to no connection, so the
    /// typing skips nobody: every session in the room has the notice typed in
    /// under the role `mcp`, and the screen draws it. It takes no seat. The
    /// server has an account and the notice is said as it (#193), but the
    /// server does not sit in the room: it is on no roster and no one can
    /// address it — it has nothing to answer with. One origin for every
    /// server, because nothing on this axis tells one from another: the
    /// account on the post does.
    notice_origin: String,
    /// The origin the app's own notices are posted from (#294): a Codex seat
    /// stopping on its usage limit, and its recovery. Minted like the two
    /// above and held by no connection; handled as a notice in every respect
    /// but its role (`app`) and its speaker, which is the app (`APP_SPEAKER`)
    /// with no account.
    app_origin: String,
}

/// The name the app's own notices are said under (#294).
pub const APP_SPEAKER: &str = "Pullcept";

impl RoomState {
    pub fn new() -> Self {
        let (to_participants, _) = broadcast::channel(256);
        // A new one, every launch. Nothing of the previous run is reopened by
        // starting the app: the room begins empty and a past topic is opened by
        // being picked (#48 / #115).
        let topic = TopicRef::new(now_iso());
        let current = topic.topic_id.clone();
        let mut rooms = BTreeMap::new();
        rooms.insert(current.clone(), Room::new(topic));
        RoomState {
            inner: Arc::new(Mutex::new(RoomsInner {
                port: None,
                rooms,
                current,
                local: None,
            })),
            to_participants,
            token: Uuid::new_v4().to_string(),
            local_origin: Uuid::new_v4().to_string(),
            notice_origin: Uuid::new_v4().to_string(),
            app_origin: Uuid::new_v4().to_string(),
        }
    }

    /// The rooms a notice goes into now: every room an AI session is seated in
    /// (`webhook_bridge::holds_session`). A server's account takes no seat, so
    /// it is never what makes a room hold one.
    fn rooms_in_session(&self) -> Vec<String> {
        self.inner
            .lock()
            .rooms
            .iter()
            .filter(|(_, room)| {
                webhook_bridge::holds_session(
                    room.participants.keys().map(String::as_str),
                    &self.local_origin,
                )
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn token(&self) -> String {
        self.token.clone()
    }

    pub fn port(&self) -> Option<u16> {
        self.inner.lock().port
    }

    /// One room's roster, in name order — empty for a room this app does not
    /// hold.
    ///
    /// Ordered by name because that is what is read, and tie-broken on the id
    /// so two participants sharing a name hold a stable order rather than
    /// swapping places between emits.
    pub fn participants(&self, room_id: &str) -> Vec<Participant> {
        let inner = self.inner.lock();
        let Some(room) = inner.rooms.get(room_id) else {
            return Vec::new();
        };
        let mut roster: Vec<Participant> = room
            .participants
            .iter()
            .map(|(id, seat)| Participant {
                id: id.clone(),
                name: seat.name.clone(),
                hue: seat.hue,
                account: seat.account.clone(),
                // Decided on the connection, as it has to be: this is the one
                // identity a shared name — or a shared account id arriving from
                // somewhere this app did not launch — cannot blur (#40).
                own: *id == self.local_origin,
            })
            .collect();
        roster.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        roster
    }

    /// One room's roster, named by its room, for the `room-participants` event.
    fn roster(&self, room_id: &str) -> Roster {
        Roster {
            topic_id: room_id.to_string(),
            participants: self.participants(room_id),
        }
    }

    fn set_port(&self, port: u16) {
        self.inner.lock().port = Some(port);
    }

    /// Seat a participant in one room, on the connection they arrived on.
    ///
    /// One seat per connection per room, so re-seating replaces rather than
    /// adds: a participant who renames themselves is the same participant.
    ///
    /// Returns true when the roster changed, so a declaration that declares
    /// nothing new does not emit a roster event. A room this app does not hold
    /// has no roster to change, and answers false.
    fn seat(
        &self,
        room_id: &str,
        origin: &str,
        name: &str,
        hue: Option<f64>,
        account: Option<&str>,
    ) -> bool {
        let mut inner = self.inner.lock();
        match inner.rooms.get_mut(room_id) {
            Some(room) => seat_in(room, origin, name, hue, account),
            None => false,
        }
    }

    fn unseat(&self, room_id: &str, origin: &str) {
        if let Some(room) = self.inner.lock().rooms.get_mut(room_id) {
            room.participants.remove(origin);
        }
    }

    /// Whether this run holds the room.
    fn holds(&self, room_id: &str) -> bool {
        self.inner.lock().rooms.contains_key(room_id)
    }

    /// The topic a room is, or `None` for a room this run does not hold.
    pub fn topic_of(&self, room_id: &str) -> Option<TopicRef> {
        self.inner
            .lock()
            .rooms
            .get(room_id)
            .map(|room| room.topic.clone())
    }

    /// The topic the screen has open.
    pub fn topic(&self) -> TopicRef {
        let inner = self.inner.lock();
        inner
            .rooms
            .get(&inner.current)
            .map(|room| room.topic.clone())
            // Every path that moves `current` puts a room there first, and the
            // one that takes a room away moves `current` off it in the same
            // acquisition (`remove`).
            .expect("the current room is always held")
    }

    /// Open a topic on the screen, making its room if this run has none yet.
    ///
    /// **Nothing in any room moves (#141).** A room already held keeps its floor
    /// and its seats exactly as they were: a session speaking in it has not gone
    /// anywhere because the screen looked elsewhere and came back. This used to
    /// empty the floor and put every seat back to its start, which was right
    /// while one room was carried from topic to topic and is wrong for a room
    /// that goes on without being looked at.
    ///
    /// A room made now starts empty, and the screen's person is seated in it
    /// once they have joined: they are at the screen, and opening a topic is
    /// being there. Answers whether that seating changed the room's roster.
    pub fn enter_topic(&self, topic: TopicRef) -> bool {
        let mut inner = self.inner.lock();
        let room_id = topic.topic_id.clone();
        let mut seated = false;
        if !inner.rooms.contains_key(&room_id) {
            let mut room = Room::new(topic);
            if let Some(person) = inner.local.clone() {
                seated = seat_in(
                    &mut room,
                    &self.local_origin,
                    &person.name,
                    person.hue,
                    person.account.as_deref(),
                );
            }
            inner.rooms.insert(room_id.clone(), room);
        }
        inner.current = room_id;
        seated
    }

    /// Take a room out of this run, for its topic being deleted, and move the
    /// screen to a new room when it had that one open.
    ///
    /// Answers the topic the screen moved to, or `None` when it was somewhere
    /// else. One acquisition, so no reader of the current topic can find it
    /// naming a room that is gone.
    ///
    /// A connection still in the removed room stays connected and seated
    /// nowhere: a post or a pull from it is answered with an error rather than
    /// written into a topic that no longer exists. The sessions that were in it
    /// are ended before this is reached (`room_delete_topic`), so what is left
    /// is a launch still in flight — the window the accepted tradeoff names.
    fn remove(&self, room_id: &str) -> Option<TopicRef> {
        let mut inner = self.inner.lock();
        inner.rooms.remove(room_id);
        if inner.current != room_id {
            return None;
        }
        let fresh = TopicRef::new(now_iso());
        let mut room = Room::new(fresh.clone());
        if let Some(person) = inner.local.clone() {
            seat_in(
                &mut room,
                &self.local_origin,
                &person.name,
                person.hue,
                person.account.as_deref(),
            );
        }
        inner.current = fresh.topic_id.clone();
        inner.rooms.insert(fresh.topic_id.clone(), room);
        Some(fresh)
    }

    /// The hue and account the screen's person declared, or neither before
    /// they have joined.
    fn local_hue_and_account(&self) -> (Option<f64>, Option<String>) {
        match &self.inner.lock().local {
            Some(person) => (person.hue, person.account.clone()),
            None => (None, None),
        }
    }

    /// Record what the screen's person declared, and seat them under it in
    /// every room the screen has opened.
    ///
    /// Every room rather than the open one: a rename is the same person
    /// everywhere, and a roster still carrying the old name in a topic not on
    /// the glass would be addressed under a name nobody answers to. Answers the
    /// rooms whose roster changed.
    fn declare_local(&self, name: &str, hue: Option<f64>, account: Option<&str>) -> Vec<String> {
        let mut inner = self.inner.lock();
        inner.local = Some(Declaration {
            name: name.to_string(),
            hue,
            account: account.map(str::to_string),
        });
        let origin = self.local_origin.clone();
        inner
            .rooms
            .iter_mut()
            .filter_map(|(id, room)| {
                seat_in(room, &origin, name, hue, account).then(|| id.clone())
            })
            .collect()
    }
}

/// Seat `origin` in `room`, under the lock the caller already holds.
///
/// Answers true when the roster changed.
fn seat_in(
    room: &mut Room,
    origin: &str,
    name: &str,
    hue: Option<f64>,
    account: Option<&str>,
) -> bool {
    let current = room
        .participants
        .get(origin)
        .map(|seat| (seat.name.clone(), seat.hue, seat.account.clone(), seat.since));
    if let Some((current_name, current_hue, current_account, _)) = &current {
        if current_name == name && *current_hue == hue && current_account.as_deref() == account {
            return false;
        }
    }
    // A seat taken now starts from the floor as it stands: this connection was
    // not there for what came before and was never handed it. A seat being
    // replaced keeps the position it started from — renaming does not make a
    // participant newly arrived.
    let since = match &current {
        Some((_, _, _, since)) => *since,
        None => room.floor.seq(),
    };
    room.participants.insert(
        origin.to_string(),
        Seat {
            name: name.to_string(),
            hue,
            account: account.map(str::to_string),
            since,
        },
    );
    true
}

/// A hue is a position on the colour wheel, so it is taken modulo a turn rather
/// than rejected. `None` for a value that is not a number at all: an undeclared
/// hue and an unusable one are the same state to the screen, which derives one.
fn normalize_hue(hue: Option<f64>) -> Option<f64> {
    hue.filter(|value| value.is_finite())
        .map(|value| value.rem_euclid(360.0))
}

/// The names a post is addressed to, as the room keeps them: trimmed, none
/// blank, none twice, and empty for the room as a whole
/// (`room_floor::addressees`, #204).
fn normalize_to(to: Option<Vec<String>>) -> Vec<String> {
    room_floor::addressees(to.unwrap_or_default())
}

/// Absent is the key omitted, never an empty one, for the same reason `to` is:
/// the screen matches this against its own account ids, and an empty string
/// would be an id no account has while looking like a declared one.
fn normalize_account(account_id: Option<String>) -> Option<String> {
    account_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// The room's clock.
///
/// One clock for everything the screen puts a time on. A session's start time
/// is read against the posts around it, so a second implementation elsewhere
/// would be a second clock to keep in step.
pub fn now_iso() -> String {
    // Tauri already pulls chrono-free time handling in; a plain RFC3339-ish
    // stamp from SystemTime keeps the dependency list unchanged.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // Days-since-epoch to calendar date, civil-from-days (Howard Hinnant).
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60,
        millis
    )
}

/// Put one post into the room, if the speaker has seen the floor.
///
/// The single path every utterance takes, whoever spoke. `origin` is the
/// connection it arrived on: the typing skips that connection's terminal, the
/// label's `role` is read off it, and the screen reads it to know whether the
/// line is its own. Two callers reach here — the socket loop and the screen's
/// own command — and neither has a path of its own past this point. The gate
/// is here for that reason, and applies to both:
/// a participant is a participant, and a post from the screen is not a
/// different act (#39).
///
/// `last_seen` is the speaker's own account of the newest post they had seen.
/// When anything on the floor is behind it, nothing is delivered and those
/// posts come back in the outcome. Accepting the post and mentioning the miss
/// afterwards would be detection without the interruption that makes detection
/// worth anything (#47).
///
/// `room_id` is the room it is said in. The floor that judges it, the log that
/// records it and the sessions it is typed into are all that room's, and none
/// of another's (#141). A room this run does not hold refuses nothing and
/// admits nothing — it answers with an error, because there is no floor there to
/// have seen or not seen.
fn deliver(
    app: &AppHandle,
    room: &RoomState,
    room_id: &str,
    origin: &str,
    post: Post,
    last_seen: Option<&str>,
) -> Result<PostOutcome, String> {
    deliver_except(app, room, room_id, origin, post, last_seen, None)
}

/// [`deliver`], leaving one terminal untyped: the seat an app notice is about
/// (#294). A Codex seat stopped on its limit is not typed the notice of its
/// own stop or recovery: that would be a turn it cannot run, or a post it is
/// told about twice.
fn deliver_except(
    app: &AppHandle,
    room: &RoomState,
    room_id: &str,
    origin: &str,
    mut post: Post,
    last_seen: Option<&str>,
    except_pty: Option<&str>,
) -> Result<PostOutcome, String> {
    let message_id = post.message_id.clone();

    // One acquisition, both halves. Concurrent speakers serialise here, so the
    // loser's check runs against a floor the winner has already changed.
    let (admission, hue, logged, topic, seats) = {
        let mut guard = room.inner.lock();
        let Some(inner) = guard.rooms.get_mut(room_id) else {
            return Err(format!(
                "トピック {room_id} はこのアプリで開かれていません。削除されたトピックかもしれません。"
            ));
        };
        let (since, hue, account) = match inner.participants.get(origin) {
            Some(seat) => (seat.since, seat.hue, seat.account.clone()),
            // A notice (#169). It was not composed against anything said here,
            // so there is nothing it could have missed: it stands where a
            // participant taking a seat this instant would stand, and the floor
            // holds nothing against it. It has no seat to be stamped from, so
            // the colour and the account are the ones `post_notice` read off
            // the server's account (#193) — the app is the one saying it, and
            // it is not a sender that could name itself.
            None if origin == room.notice_origin || origin == room.app_origin => {
                (inner.floor.seq(), post.hue, post.account.clone())
            }
            // Unseated: nothing was ever delivered here, so nothing is
            // presumed read. Speaking seats a participant, and the screen's
            // command does that before it reaches this point.
            None => (0, None, None),
        };
        // The `@名前` that name someone in this room address them, and leave
        // the text (#206). Read here because this is the one path: the screen's
        // post and a session's `say_to_room` get the same reading, against the
        // names seated under this same acquisition. A notice is external
        // content and passes through as it arrived (`room_floor::Sender`).
        let sender = if origin == room.notice_origin || origin == room.app_origin {
            room_floor::Sender::Notice
        } else {
            room_floor::Sender::Participant
        };
        room_floor::address(
            &mut post,
            sender,
            inner.participants.values().map(|seat| seat.name.as_str()),
        )
        .map_err(str::to_string)?;
        // Stamped before the floor takes its copy, so the retained post and the
        // live line carry one declaration rather than two readings of it. A
        // refusal hands that copy back, and the screen draws it (#108).
        post.hue = hue;
        post.account = account;
        let admission = inner.floor.admit(origin, since, last_seen, post.clone());
        // Written here, inside the acquisition the floor was judged under, so
        // the file's order is the floor's order. Appending after the lock is
        // dropped would let two speakers the floor has already ordered reach
        // the disk the other way round, and the conversation would then have
        // two orderings — which is the one thing the room is the authority on
        // (#48).
        //
        // Only what was admitted. A refused post is not in the room, so there
        // is nothing about it for the room to have recorded.
        //
        // The failure is carried out rather than reported here: the room is
        // stopped while this lock is held, and telling the screen is not work
        // to do with everyone waiting.
        //
        // The topic is the room's own, read under this same acquisition: a
        // room is one topic for as long as it exists, so the post reaches the
        // file of the floor that admitted it.
        let topic = inner.topic.clone();
        let logged = match &admission {
            Admission::Admitted { .. } => room_log::append(app, &topic.topic_id, &post),
            Admission::Unseen(_) => Ok(false),
        };
        // Who is in the room as this post is admitted, with the account each
        // declared: every post is typed into the terminals of the sessions in
        // it (#183, #195). Taken under this acquisition so the seats reached
        // are the ones the floor judged against; the typing itself waits until
        // the lock is dropped.
        //
        // The names ride along for one reading: whether a post held back from
        // a stopped Codex seat was addressed to it (#294).
        let seats: Vec<(String, Option<String>, String)> = inner
            .participants
            .iter()
            .map(|(id, seat)| (id.clone(), seat.account.clone(), seat.name.clone()))
            .collect();
        (admission, hue, logged, topic, seats)
    };

    if let Admission::Unseen(missed) = admission {
        return Ok(PostOutcome {
            delivered: false,
            message_id: None,
            missed,
        });
    }

    // The post is in the room either way. A log that cannot be written loses
    // the record, never the utterance — so nothing below is conditional on
    // this, and the one thing that must not happen is it passing unnoticed
    // (#48).
    match logged {
        Err(err) => room_log::report(app, err),
        // The first thing said in this topic. The topic gets its entry and its
        // name from it, out here rather than under the lock: the index is a
        // second file read and rewritten whole, and nothing about a post waits
        // on it (#115, decision 9).
        Ok(true) => room_log::realize_from_first_post(app, &topic, &post.content),
        Ok(false) => {}
    }

    // No session in the room is not an error — the room accepts what is said
    // in it; a later joiner simply missed it, and can pull it.
    let role = role_of(app, room, origin, post.account.as_deref());
    type_into_sessions(app, room_id, &seats, origin, &role, &post, except_pty);

    let _ = app.emit(
        "room-message",
        RoomMessage {
            topic_id: room_id.to_string(),
            message_id: post.message_id,
            speaker: post.speaker,
            // Read off the seat on this connection, so the colour of a line and
            // the colour of its author's roster entry are the one declaration.
            // Read inside the critical section above, with the same lock the
            // floor was judged under.
            hue,
            account: post.account,
            content: post.content,
            to: post.to,
            ts: post.ts,
            own: origin == room.local_origin,
        },
    );

    Ok(PostOutcome {
        delivered: true,
        message_id: Some(message_id),
        missed: Vec::new(),
    })
}

/// The role a post is labelled with, from where it came into the room (#195).
///
/// The screen's own command and the app's notices are told apart by their
/// origin, which no connection holds. Everything else came in on the socket,
/// and carries the kind of the account its seat declared — read off the
/// config, since the room holds only the id. `terminal_input::role` is where
/// the rule is: the socket never makes `admin`, whatever it names.
fn role_of(app: &AppHandle, room: &RoomState, origin: &str, account: Option<&str>) -> String {
    let kind;
    let source = if origin == room.local_origin {
        terminal_input::Source::Screen
    } else if origin == room.notice_origin {
        terminal_input::Source::Notice
    } else if origin == room.app_origin {
        terminal_input::Source::App
    } else {
        kind = account.and_then(|id| crate::config::account_kind_name(app, id));
        terminal_input::Source::Socket(kind.as_deref())
    };
    terminal_input::role(source).to_string()
}

/// Type a post into the terminal of each session in its room but the
/// speaker's own (#183, #195).
///
/// Every post, whoever said it: the person at the screen, a session, a notice.
/// The label on its first line carries the `role` (`role_of`), which is how a
/// session tells its user's words from everything else it is handed on this
/// one path, and the time the post was said, in this PC's local time
/// (`terminal_input::at`, #219).
///
/// Which terminal belongs to a seat is read off the launcher's ledger by the
/// room and the account the seat declared (`session::RoomSeats`). A seat that
/// maps to no running terminal is not typed into; there is no other way left
/// to push to it, and it reads the topic through the pull
/// (`terminal_input::targets`).
///
/// **A Codex seat stopped on its usage limit is not typed into** (#294): the
/// post is kept for it (`codex_limit::CodexLimits`), its id written to the
/// seat's mailbox (#312), and handed over as one line when the recovery is
/// confirmed. Every other seat, every Claude Code
/// seat among them, is typed into as before.
fn type_into_sessions(
    app: &AppHandle,
    room_id: &str,
    seats: &[(String, Option<String>, String)],
    speaker: &str,
    role: &str,
    post: &Post,
    except_pty: Option<&str>,
) {
    let ptys = app.state::<PtyState>();
    let running = app.state::<RoomSeats>().seated(&ptys);
    let pty_of = |account: &str| {
        running
            .iter()
            .find(|seat| seat.topic_id == room_id && seat.account_id == account)
            .and_then(|seat| seat.session.as_ref())
            .map(|session| session.pty_id.clone())
    };
    let targets = terminal_input::targets(
        seats
            .iter()
            .map(|(origin, account, _)| (origin.as_str(), account.as_deref())),
        speaker,
        pty_of,
    );
    if targets.is_empty() {
        return;
    }
    let at = terminal_input::at(&post.ts);
    let text = terminal_input::compose(
        &post.message_id,
        &post.speaker,
        role,
        at.as_deref(),
        &post.to,
        &post.content,
    );
    let limits = app.state::<crate::codex_limit::CodexLimits>();
    for target in targets {
        if except_pty == Some(target.pty_id.as_str()) {
            continue;
        }
        let held = limits.hold(&target.pty_id, || mcp_config::codex::limit::Held {
            message_id: post.message_id.clone(),
            speaker: post.speaker.clone(),
            at: at.clone(),
            content: post.content.clone(),
            addressed: seats
                .iter()
                .filter(|(origin, _, _)| target.origins.contains(origin))
                .any(|(_, _, name)| post.to.contains(name)),
        });
        if held {
            continue;
        }
        ptys.type_in(&target.pty_id, text.clone());
    }
}

/// Say one notice of the app's own into one room (#294), typed into every
/// session in it but `except_pty`'s. Said as [`APP_SPEAKER`] with no account,
/// under the role `app`, through `deliver` like every other post: the same
/// floor, log and `room-message`. Answers the id it was filed under, or `None`
/// when the room is gone.
pub fn post_app_notice(
    app: &AppHandle,
    room: &RoomState,
    room_id: &str,
    content: &str,
    except_pty: Option<&str>,
) -> Option<String> {
    let outcome = deliver_except(
        app,
        room,
        room_id,
        &room.app_origin,
        Post {
            message_id: Uuid::new_v4().to_string(),
            speaker: APP_SPEAKER.to_string(),
            hue: None,
            account: None,
            content: content.to_string(),
            to: Vec::new(),
            ts: now_iso(),
        },
        None,
        except_pty,
    );
    match outcome {
        Ok(outcome) if outcome.delivered => outcome.message_id,
        // The floor holds nothing against a notice; said rather than assumed.
        Ok(_) => {
            eprintln!("[codex-limit] the floor of room {room_id} refused a notice");
            None
        }
        Err(err) => {
            eprintln!("[codex-limit] {err}");
            None
        }
    }
}

/// Put one notice from a local MCP server into every room an AI session is
/// seated in, and answer how many it went into (#169).
///
/// One post per room, each through `deliver` like any other: the same floor,
/// the same log, the same `room-message`, and typed into each session's
/// terminal like any other, labelled `mcp` (#195). Nothing about who the event
/// concerns is read — a notice is handed round, not sorted (#32).
/// Each room gets its own `message_id`, because each is a post of its own room.
///
/// Said as the server's account (#193): its name is the speaker, and its colour
/// and id ride on the post the way a seat's do.
///
/// **No room, no post (Master 判断, 2026-09-27).** A room with only the screen
/// in it is not written to, and the caller leaves the event unmarked, so it
/// stays pending on the worker. A session that sits down afterwards is not
/// handed it: the room does not push its past to a participant, and a notice
/// is no exception.
pub fn post_notice(
    app: &AppHandle,
    room: &RoomState,
    speaker: &crate::config::McpSpeaker,
    content: &str,
) -> usize {
    let mut delivered = 0;
    for room_id in room.rooms_in_session() {
        let outcome = deliver(
            app,
            room,
            &room_id,
            &room.notice_origin,
            Post {
                message_id: Uuid::new_v4().to_string(),
                speaker: speaker.name.clone(),
                hue: speaker.hue,
                account: Some(speaker.account_id.clone()),
                content: content.to_string(),
                to: Vec::new(),
                ts: now_iso(),
            },
            None,
        );
        match outcome {
            Ok(outcome) if outcome.delivered => delivered += 1,
            // The floor holds nothing against a notice, so this is not reached.
            // Said rather than assumed, in case that ever changes.
            Ok(_) => eprintln!("[webhook] the floor of room {room_id} refused a notice"),
            // Deleted between the list and the post.
            Err(err) => eprintln!("[webhook] {err}"),
        }
    }
    delivered
}

/// How many posts one pull answers with when the asker names no number.
const HISTORY_PAGE: usize = 50;

/// The most one pull answers with, whatever the asker names.
///
/// A topic has no ceiling, and a participant asking for all of one would be
/// handed a context's worth of text in a single tool result. The page and the
/// `before` cursor together are what make a long topic readable in the
/// direction it is actually read — backwards from the end.
const HISTORY_MAX: usize = 200;

/// One page of a topic, oldest first, ending at `before`.
///
/// The tail of the eligible window rather than its head: what a participant
/// joining late needs first is what was just said, and paging further back is
/// what `before` is for. `has_more` says whether there is anything older, so
/// the asker knows whether the top of the page is the top of the topic.
///
/// A `before` the topic does not contain is read as naming its end. Erring the
/// other way — answering with nothing — would be indistinguishable from an
/// empty topic, and the asker would stop.
fn history_answer(
    request_id: &str,
    posts: Vec<room_log::LoggedPost>,
    before: Option<&str>,
    limit: Option<usize>,
) -> serde_json::Value {
    let end = before
        .and_then(|id| posts.iter().position(|post| post.message_id == id))
        .unwrap_or(posts.len());
    let window = &posts[..end];
    let limit = limit.unwrap_or(HISTORY_PAGE).clamp(1, HISTORY_MAX);
    let start = window.len().saturating_sub(limit);
    serde_json::json!({
        "type": "history_result",
        "request_id": request_id,
        "posts": &window[start..],
        "has_more": start > 0,
    })
}

/// Bind the room socket and start accepting sidecars.
///
/// Port 0: the OS picks. The port is handed to sidecars through `.mcp.json`,
/// so nothing needs it to be stable across runs.
pub async fn start(app: AppHandle, room: RoomState) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind room socket: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read room socket address: {e}"))?
        .port();
    room.set_port(port);

    // The frontend loads before this bind completes, so a poll at load time
    // reads "not listening" and reports a failure that is only a race. The
    // event is the authority; `room_port` remains for a late reader.
    let _ = app.emit("room-ready", port);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let app = app.clone();
            let room = room.clone();
            tokio::spawn(async move {
                // Two kinds of caller on one address. A sidecar opens a
                // WebSocket, which is a `GET` upgrade; a session's status line
                // posts (#155). One listener because the port is what a launch
                // already carries — a second one would be a second address to
                // hand out, hold and hand back on every launch.
                let mut head = [0u8; 4];
                let posted = matches!(stream.peek(&mut head).await, Ok(n) if head[..n].starts_with(b"POST"));
                let ended = if posted {
                    serve_hook(app, room, stream).await
                } else {
                    serve_participant(app, room, stream).await
                };
                if let Err(err) = ended {
                    eprintln!("[room] participant connection ended: {err}");
                }
            });
        }
    });

    Ok(port)
}

/// The event the screen reads one session's own account of itself off (#155).
///
/// The seat, and the five values the panel shows. Every one of them is
/// `Option`, because every one of them is a field the CLI may not send: the
/// rate limits are absent off a claude.ai plan and until the session's first
/// API answer, the effort is absent on a model with no such parameter, and the
/// context percentage is null early in a session (Claude Code docs,
/// `statusline`, read 2026-09-17; not measured on a live CLI). Absent reaches
/// the screen as absent, so a row reads `—` rather than `0%`.
///
/// **This is where the app reads what a CLI sent, and one of two places.**
/// The values are what was asked for, so the field names below are the CLI's
/// and are a thing to keep in step with it. The terminal's own output is still
/// not read (#82); what is read is a structured report the CLI hands out for
/// this. The other place is a Codex CLI seat's rollout (#283,
/// `mcp_config::codex::status`): Codex has no status-line command, so the app
/// reads the same five out of the file the CLI writes them into, and hands
/// them to the screen as this same event (`from_codex`, `emit`).
///
/// **Two of the five are also where 制限中 comes from** (#161). The screen reads
/// the word off `five_hour` and `seven_day` rather than off a signal of its own:
/// a turn that stopped on the limit used to arrive as a `StopFailure` hook on a
/// second path (#149), and that path is gone. Nothing is added here for it —
/// the percentages were already being sent, and the reading is the screen's
/// (`main.ts`, `activityNote`).
#[derive(Debug, Clone, Serialize)]
pub struct SessionStats {
    pub topic_id: String,
    pub account_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub context: Option<f64>,
    /// 制限中, said by the app rather than read off the percentages (#294).
    /// A Codex seat carries it: the app watches that seat's stop and recovery
    /// (`codex_limit`), and the rollout's percentages were seen stuck at 99
    /// for a seat that had stopped. `None` for every other seat, whose screen
    /// still reads the word off `five_hour` and `seven_day` (#161).
    pub limited: Option<bool>,
    /// When the 5-hour and weekly windows reset, as Unix seconds (#306): what
    /// the panel counts down to beside those two rows. Read off the same
    /// window as its percentage (`mcp_config::epoch_seconds`). The screen stops
    /// showing one once it has passed; Claude Code drops a window at its reset
    /// and runs the status line again (Claude Code docs, `statusline`, read
    /// 2026-10-07), so a passed reset is not waited on here.
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_resets_at: Option<i64>,
}

impl SessionStats {
    /// Read one status-line report, or `None` when it is not JSON at all.
    ///
    /// A field this does not find is left absent rather than defaulted. The
    /// two model fields are one value on the screen — the display name is what
    /// a person reads, and the id is what is there when a CLI sends no display
    /// name.
    fn read(topic_id: String, account_id: String, body: &[u8]) -> Option<Self> {
        let data: serde_json::Value = serde_json::from_slice(body).ok()?;
        let text = |value: &serde_json::Value| {
            value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
        };
        Some(SessionStats {
            topic_id,
            account_id,
            model: text(&data["model"]["display_name"]).or_else(|| text(&data["model"]["id"])),
            effort: text(&data["effort"]["level"]),
            five_hour: data["rate_limits"]["five_hour"]["used_percentage"].as_f64(),
            seven_day: data["rate_limits"]["seven_day"]["used_percentage"].as_f64(),
            context: data["context_window"]["used_percentage"].as_f64(),
            limited: None,
            five_hour_resets_at: mcp_config::epoch_seconds(
                &data["rate_limits"]["five_hour"]["resets_at"],
            ),
            seven_day_resets_at: mcp_config::epoch_seconds(
                &data["rate_limits"]["seven_day"]["resets_at"],
            ),
        })
    }

    /// One Codex CLI seat's values as its rollout has reported them so far (#283).
    pub fn from_codex(
        topic_id: String,
        account_id: String,
        status: &mcp_config::codex::status::Status,
        limited: bool,
    ) -> Self {
        SessionStats {
            topic_id,
            account_id,
            model: status.model.clone(),
            effort: status.effort.clone(),
            five_hour: status.five_hour,
            seven_day: status.seven_day,
            context: status.context,
            limited: Some(limited),
            five_hour_resets_at: status.five_hour_resets_at,
            seven_day_resets_at: status.seven_day_resets_at,
        }
    }

    /// Hand the report to the screen. The one event both paths end in, so the
    /// panel draws a Codex seat and a Claude Code seat the same way.
    pub fn emit(self, app: &AppHandle) {
        let _ = app.emit("session-stats", self);
    }
}

/// The most of a hook request this reads before giving up on it.
///
/// The head is a few hundred bytes. The body is the CLI's own input, read and
/// parsed (`SessionStats`).
const HOOK_HEAD_MAX: usize = 16 * 1024;
const HOOK_BODY_MAX: usize = 4 * 1024 * 1024;

/// How long one hook request may take before the connection is dropped.
const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Answer one POST from a session: its status-line report (#155, decision 2).
///
/// **The path names the seat**, and the header carries the room's token. The
/// body is read, because its values are what was asked for (`SessionStats`) —
/// which is what makes this the one path (#161): the usage limit is two of
/// those values, so there is nothing left for a second POST to say.
///
/// The activity path sits beside it: `/hooks/activity/<event>/…`, a Claude Code
/// seat's hooks (`mcp_config::activity_hook_settings`), whose body is read for
/// a few fields only and handed to that seat's state (`hook_activity`, #331).
async fn serve_hook(
    app: AppHandle,
    room: RoomState,
    stream: tokio::net::TcpStream,
) -> Result<(), String> {
    match tokio::time::timeout(HOOK_TIMEOUT, read_hook(&app, &room, stream)).await {
        Ok(result) => result,
        Err(_) => Err("a hook request did not finish within its window".to_string()),
    }
}

async fn read_hook(
    app: &AppHandle,
    room: &RoomState,
    stream: tokio::net::TcpStream,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("hook request could not be read: {e}"))?;
        if read == 0 {
            return Err("hook request ended before its head did".to_string());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
        if head.len() > HOOK_HEAD_MAX {
            return Err("hook request head is longer than this app reads".to_string());
        }
    }

    let mut lines = head.lines();
    let target = lines
        .next()
        .and_then(|request| request.split(' ').nth(1))
        .unwrap_or("")
        .to_string();
    let mut authorization = None;
    let mut length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => authorization = Some(value.to_string()),
            "content-length" => length = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    // Read before answering, so the client has a finished request to close on
    // rather than a reset in the middle of sending one.
    let mut body = Vec::new();
    if length > 0 {
        reader
            .take(length.min(HOOK_BODY_MAX) as u64)
            .read_to_end(&mut body)
            .await
            .map_err(|e| format!("hook request body could not be read: {e}"))?;
    }

    // The same token the sidecars present. The listener is on loopback, and
    // any local process can reach loopback — without this, anything on the
    // machine could put any five values it liked on a row, 制限中 among them
    // (#161).
    let authorized = authorization.as_deref() == Some(&format!("Bearer {}", room.token()));
    let reported = mcp_config::parse_status_hook_target(&target);
    let activity = mcp_config::parse_activity_hook_target(&target);
    let native = target == mcp_config::codex::NATIVE_PATH;
    let captured = authorized
        && native
        && app.state::<crate::session::RoomSeats>().capture_native(
            app,
            &app.state::<crate::pty::PtyState>(),
            room,
            &body,
        );
    let status = match (authorized, reported.is_some() || activity.is_some() || native) {
        (false, _) => "401 Unauthorized",
        (true, false) => "404 Not Found",
        (true, true) if native && !captured => "409 Conflict",
        (true, true) => "200 OK",
    };
    if authorized {
        // Emitted whether or not this app holds the room. The screen keys its
        // terminals on the pair, and a seat it does not have is a payload it
        // drops — the same as a post arriving for a topic it is not drawing.
        //
        // A body this cannot read emits nothing at all. Leaving the panel on
        // its last values is what decision 4 already says happens while a
        // session is quiet, and it is better than five rows going blank
        // because one report arrived malformed.
        if let Some((room_id, account_id)) = reported {
            if let Some(stats) = SessionStats::read(room_id, account_id, &body) {
                stats.emit(app);
            }
        }
        // A Claude Code seat's activity hook (#331). The event is the path's;
        // the body — cut at `HOOK_BODY_MAX`, so possibly not whole — is read
        // for a few fields only (`mcp_config::hook_activity`). Handled before
        // the answer goes out, which takes no longer than parsing the body:
        // the CLI sends a tool's end only after its start's answer, so
        // the two are kept in the order they happened.
        if let Some((event, room_id, account_id)) = &activity {
            app.state::<crate::hook_activity::HookSeats>()
                .hear(app, event, room_id, account_id, &body);
        }
    }

    // A JSON body, because the CLI reads a hook's answer as its JSON output.
    // An empty object decides nothing, which is what this is for: it observes.
    // The status line draws what its own script printed, not this.
    let answer = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    writer
        .write_all(answer.as_bytes())
        .await
        .map_err(|e| format!("hook answer could not be sent: {e}"))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("hook answer could not be flushed: {e}"))?;
    Ok(())
}

async fn serve_participant(
    app: AppHandle,
    room: RoomState,
    stream: tokio::net::TcpStream,
) -> Result<(), String> {
    let expected = format!("Bearer {}", room.token());
    // The listener is on loopback, but any local process can reach loopback.
    // The token is what makes this room, and not merely this machine.
    let check = |req: &Request, res: Response| -> Result<Response, ErrorResponse> {
        let ok = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v == expected)
            .unwrap_or(false);
        if ok {
            Ok(res)
        } else {
            let mut deny = ErrorResponse::new(Some("unauthorized".to_string()));
            *deny.status_mut() = StatusCode::UNAUTHORIZED;
            Err(deny)
        }
    };

    let ws = tokio_tungstenite::accept_hdr_async(stream, check)
        .await
        .map_err(|e| format!("handshake failed: {e}"))?;
    let (mut sink, mut source) = ws.split();

    // This connection's identity, minted here. Nothing the far side sends can
    // set it or read it, so nothing the far side sends can wear another
    // participant's suppression or shed its own.
    let origin = Uuid::new_v4().to_string();

    // The room this connection is in, once its `hello` has named one. Set once
    // and never moved: a connection is started into one topic, and a second
    // `hello` naming another would be a session changing conversations under a
    // registration that says otherwise (#141).
    let joined_room: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

    let mut from_room = room.to_participants.subscribe();
    let own_origin = origin.clone();
    // The seat table alone, for one purpose: naming this connection in the lag
    // log below.
    //
    // Deliberately not `room.clone()`. `RoomState` holds `to_participants` as
    // a plain field beside `inner`, not inside the `Arc`, so cloning the room
    // clones the `Sender` — and a pump that owns a sender can never see its own
    // channel close. `RecvError::Closed` means every sender is gone, so the
    // `Closed` arm below would be unreachable and the pump would sit in `recv`
    // forever after the room is torn down, holding the socket task with it.
    // The seat table cannot do that: `RoomsInner` is a port, the rooms with
    // their seats and floors, and the screen's declaration — no sender lives
    // under it.
    let seats = Arc::clone(&room.inner);
    let pump = tokio::spawn(async move {
        loop {
            let fanout = match from_room.recv().await {
                Ok(fanout) => fanout,
                // No sender left: the room itself is gone, so nothing further
                // will ever arrive. Leaving is all there is to do.
                Err(broadcast::error::RecvError::Closed) => break,
                // This connection read more slowly than the room answered, and
                // the channel overwrote frames it had not taken yet. The
                // receiver is still live — tokio advances its cursor to the
                // oldest frame the channel still holds and the next `recv`
                // returns that one — so a lag is a gap, not an ending, and the
                // pump stays.
                //
                // Leaving here is what made a momentary gap permanent: the
                // socket stayed open and the participant stayed on the roster,
                // so a participant who was never spoken to again looked exactly
                // like one with nothing to hear. Nothing on the screen or in
                // the roster could show the difference, which is why this arm
                // continues and why it logs (#42).
                //
                // The dropped frames are not resent. Every frame here is an
                // answer to another connection or to this one, and one of this
                // connection's own that went missing is a tool call the sidecar
                // reports as unconfirmed when its wait runs out.
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    // Named by both halves on purpose. The name is what a
                    // reader recognises and what every other line in this file
                    // logs; the connection id is what tells two participants
                    // sharing one name apart, which is the case that made the
                    // name unusable as an identity in the first place (#40).
                    // Before `hello` there is no name, and the id alone still
                    // identifies the connection.
                    let seated = name_on(&seats, &own_origin);
                    let who = seated.as_deref().unwrap_or("(not yet seated)");
                    eprintln!(
                        "[room] \"{who}\" ({own_origin}) fell behind: {dropped} frame(s) dropped, delivery continues"
                    );
                    continue;
                }
            };
            // Addressed to one connection: the room answering whoever posted
            // or pulled. Everyone else's socket is not part of that exchange.
            if fanout.target != own_origin {
                continue;
            }
            if sink.send(Message::Text(fanout.frame.into())).await.is_err() {
                break;
            }
        }
    });

    // Known once the participant says hello; used to attribute posts and to
    // drop them from the roster.
    let mut joined_as: Option<String> = None;

    // What a post or a pull from a connection in no room is answered with. Not
    // a refusal and not an empty page: either would read as the room having
    // judged the request, and there is no room here to have judged it.
    let no_room = |joined: Option<&String>| match joined {
        Some(id) => format!(
            "the topic this session was started in ({id}) is no longer open in the app; it may have been deleted"
        ),
        None => "this session is not in a room: its hello named none this app holds".to_string(),
    };

    while let Some(Ok(msg)) = source.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(frame) = serde_json::from_str::<IncomingFrame>(&text) else {
            continue;
        };

        match frame.kind.as_str() {
            "hello" => {
                let name = frame.name.unwrap_or_else(|| "session".to_string());
                if frame.protocol != Some(PROTOCOL_VERSION) {
                    // Legible mismatch beats a silent half-working room.
                    eprintln!(
                        "[room] \"{name}\" speaks protocol {:?}, room speaks {PROTOCOL_VERSION}",
                        frame.protocol
                    );
                }
                // Which room. The first `hello` decides it for the life of the
                // connection; a room this app does not hold is no room at all,
                // and nothing is seated (#141).
                let asked = frame
                    .room
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty());
                let room_id = match (joined_room.get(), asked) {
                    (Some(joined), Some(asked)) if *joined != asked => {
                        eprintln!(
                            "[room] \"{name}\" asked to move from room {joined} to {asked}; a connection stays in the room it joined"
                        );
                        joined.clone()
                    }
                    (Some(joined), _) => joined.clone(),
                    (None, Some(asked)) if room.holds(&asked) => {
                        let _ = joined_room.set(asked.clone());
                        asked
                    }
                    (None, asked) => {
                        // Legible, like the protocol mismatch above: a session
                        // sitting in the app's socket and in no roster is
                        // otherwise indistinguishable from one that never
                        // connected.
                        eprintln!(
                            "[room] \"{name}\" named room {asked:?}, which this app is not holding; not seated"
                        );
                        continue;
                    }
                };
                // Seated on this connection. A second session answering to the
                // same name is a second seat, not the same one — and so is a
                // second session declaring the same account, which this room
                // does not refuse: refusing a duplicate account belongs to the
                // launcher's seat ledger, which knows what it started
                // (`session::RoomSeats`), and a room that enforced it here
                // would be treating the account as the identity.
                room.seat(
                    &room_id,
                    &origin,
                    &name,
                    normalize_hue(frame.hue),
                    normalize_account(frame.account_id).as_deref(),
                );
                joined_as = Some(name);
                let _ = app.emit("room-participants", room.roster(&room_id));
            }
            "post" => {
                // Attribution comes from the connection, not from the frame. A
                // sender may name an addressee; it may not name itself.
                let speaker = joined_as.clone().unwrap_or_else(|| "session".to_string());
                let message_id = frame
                    .message_id
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                // `last_seen` is not a claim about identity, so nothing is
                // verified here. A participant who declares a false watermark
                // spends its own round trips; nobody else's post moves.
                let outcome = match joined_room.get() {
                    Some(room_id) if room.holds(room_id) => deliver(
                        &app,
                        &room,
                        room_id,
                        &origin,
                        Post {
                            message_id: message_id.clone(),
                            speaker,
                            // Stamped by `deliver` off the seat on this connection.
                            // A sender may name an addressee; it may not name its
                            // own colour any more than its own name.
                            hue: None,
                            // Stamped by `deliver` too, for the same reason.
                            account: None,
                            content: frame.content.unwrap_or_default(),
                            to: normalize_to(frame.to),
                            ts: frame.ts.unwrap_or_else(now_iso),
                        },
                        frame.last_seen.as_deref(),
                    ),
                    _ => Err(no_room(joined_room.get())),
                };
                // Answered on the connection that posted, always — a refusal
                // that says nothing is indistinguishable from a delivery, and
                // this answer is the boundary at which a reply needing no
                // other tool finally gets to read what it missed.
                //
                // A post that did not go in is answered with the reason,
                // beside `delivered: false`: no room to go into, or nothing
                // left to say once its `@名前` became addressees (#206).
                // Without it the answer would read as a refusal with nothing
                // missed, which invites the same post again.
                let receipt = match outcome {
                    Ok(outcome) => serde_json::json!({
                        "type": "post_result",
                        "message_id": message_id,
                        "delivered": outcome.delivered,
                        "missed": outcome.missed,
                    }),
                    Err(error) => serde_json::json!({
                        "type": "post_result",
                        "message_id": message_id,
                        "delivered": false,
                        "missed": [],
                        "error": error,
                    }),
                };
                let _ = room.to_participants.send(Fanout {
                    target: origin.clone(),
                    frame: receipt.to_string(),
                });
            }
            // The read-out (#115, decision 4C). A participant asks for what
            // was said in its room's topic before it got here; the room
            // answers on this connection alone.
            //
            // **Pull, and only pull.** The room still fans out nothing it did
            // not deliver live — a later joiner missed what predates its seat,
            // and that is unchanged. What changes is that the participant can
            // now go and get it, which is a different thing from the room
            // holding who has heard what (#31 / #39). Nothing here is recorded
            // against the asker, and asking twice is the same as asking once.
            //
            // The topic is read and the lock released before the file is
            // touched: this is the room's own lock, and a read of a log with
            // no ceiling is not work to do with everyone waiting.
            //
            // The topic is the connection's own room, not the one on the
            // screen. Reading the screen's was right while every session was
            // in it, and handed a session in any other topic the conversation
            // it was not in (#141).
            "history" => {
                let request_id = frame.request_id.unwrap_or_default();
                let topic = joined_room.get().and_then(|id| room.topic_of(id));
                let read = match &topic {
                    Some(topic) => room_log::topic_posts(&app, &topic.topic_id),
                    None => Err(no_room(joined_room.get())),
                };
                let answer = match read {
                    Ok(posts) => history_answer(&request_id, posts, frame.before.as_deref(), frame.limit),
                    Err(err) if topic.is_none() => serde_json::json!({
                        "type": "history_result",
                        "request_id": request_id,
                        "error": err,
                    }),
                    Err(err) => {
                        room_log::report(&app, err.clone());
                        // Said rather than answered with an empty page: a
                        // participant told "nothing was said" would go on to
                        // act on that.
                        serde_json::json!({
                            "type": "history_result",
                            "request_id": request_id,
                            "error": err,
                        })
                    }
                };
                let _ = room.to_participants.send(Fanout {
                    target: origin.clone(),
                    frame: answer.to_string(),
                });
            }
            _ => {}
        }
    }

    pump.abort();
    if joined_as.is_some() {
        // By connection. Removing by name took every participant answering to
        // that name off the roster, so one session ending emptied the other's
        // seat too (#40).
        // A room deleted while this connection was in it has no roster left to
        // announce, and an empty one sent under its id would hand the screen a
        // topic that no longer exists.
        if let Some(room_id) = joined_room.get().filter(|id| room.holds(id)) {
            room.unseat(room_id, &origin);
            let _ = app.emit("room-participants", room.roster(room_id));
        }
    }
    Ok(())
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn room_port(state: tauri::State<RoomState>) -> Option<u16> {
    state.port()
}

/// One room's roster, named by its topic.
///
/// Named rather than read off the open room, because the screen asks for it
/// right after opening a topic, and a roster read from whichever room the app
/// thinks is open would be the other one whenever the two had not caught up.
#[tauri::command]
pub fn room_participants(state: tauri::State<RoomState>, topic_id: String) -> Vec<Participant> {
    state.participants(&topic_id)
}

/// The topic the screen has open.
///
/// It may not be in the index yet — a launch opens a new one and nothing is
/// written until something is said in it (#115) — so the screen draws this
/// beside the list rather than looking for it inside the list.
#[tauri::command]
pub fn room_current_topic(state: tauri::State<RoomState>) -> TopicRef {
    state.topic()
}

/// Cut here: a new topic, open on the screen from now on.
///
/// The 新規 button, and the whole of what a topic boundary is — drawn by hand,
/// independent of when the app was started (#115, decision 1). Nothing is
/// written down: the index entry waits for the first post, so a topic opened
/// and left unspoken leaves no row behind.
///
/// A new room, and every other room left as it was: the sessions of the topic
/// being left keep running in it (#141, decision 2).
#[tauri::command]
pub fn room_new_topic(app: AppHandle, state: tauri::State<RoomState>) -> TopicRef {
    let topic = TopicRef::new(now_iso());
    if state.enter_topic(topic.clone()) {
        let _ = app.emit("room-participants", state.roster(&topic.topic_id));
    }
    topic
}

/// Open an existing topic on the screen.
///
/// Selecting one from the list. What comes back with it is its posts, which the
/// screen reads for itself, and the session each account was in, which a launch
/// reads when a seat is started (`session.rs`). Nothing is started here: the
/// topic opens whether or not anything can be resumed into it (#115, decision
/// 6).
///
/// A topic whose room this run already holds is opened as it stands — its
/// floor, its seats and the sessions in it untouched (#141). One the run does
/// not hold yet has to be in the index, which is what a list row is.
#[tauri::command]
pub fn room_select_topic(
    app: AppHandle,
    state: tauri::State<RoomState>,
    topic_id: String,
    created_at: String,
) -> Result<(), String> {
    let topic = match state.topic_of(&topic_id) {
        Some(held) => held,
        None if room_log::topic_exists(&app, &topic_id) => TopicRef {
            topic_id,
            created_at,
        },
        None => return Err(format!("トピック {topic_id} は見つかりません。")),
    };
    let room_id = topic.topic_id.clone();
    if state.enter_topic(topic) {
        let _ = app.emit("room-participants", state.roster(&room_id));
    }
    Ok(())
}

/// Delete a topic: its posts, its entry, and the sessions that were in it.
///
/// Three acts, and the order between them is the whole of what this function
/// decides. Each half is done by whoever owns it — the seats end the sessions,
/// the log deletes the store, the room goes — and none of the three would be
/// right on its own.
///
/// **The sessions first (#119, decision 4).** A topic holds the way back into
/// the sessions that were in it, so deleting it while they run leaves sessions
/// belonging to no topic — running, seated, and unreachable by any resume. Only
/// the seats started into *this* topic are ended: which topic a session belongs
/// to is a fact about its launch, and the seats carry it
/// (`RoomSeats::running_in_topic`). Running is not a reason to refuse the
/// delete (#119, decision 3).
///
/// **The store second.** If it fails, the sessions are already gone and the
/// topic is still listed — visible, and the person can start a session again or
/// press delete again. The other order fails the way decision 4 forbids: the
/// topic gone and the sessions still running.
///
/// **The room last.** It goes whether or not the screen had it open (#141): a
/// room is a topic, and a topic that no longer exists has nowhere for a post to
/// be written. When the screen had it open, the screen moves to a new topic,
/// which is the state a launch already produces — an empty room, in a topic not
/// yet in the index (#119, decision 6). Moving to the next topic in the list
/// would open a conversation nobody asked for, and refusing to delete the open
/// one would ask the person to leave a topic before deciding they do not want
/// it.
///
/// Answers the topic the screen moved to, or `None` when it had another one
/// open and did not move. The screen needs it: it holds its own current topic,
/// and a screen still pointing at a deleted one would draw one conversation
/// while the next post was recorded in another.
#[tauri::command]
pub fn room_delete_topic(
    app: AppHandle,
    state: tauri::State<RoomState>,
    pty_state: tauri::State<PtyState>,
    seats: tauri::State<RoomSeats>,
    topic_id: String,
) -> Result<Option<TopicRef>, String> {
    pty_state.kill_each(&seats.running_in_topic(&topic_id, &pty_state));

    room_log::delete_topic(&app, &topic_id)?;

    let moved = state.remove(&topic_id);
    if let Some(fresh) = &moved {
        let _ = app.emit("room-participants", state.roster(&fresh.topic_id));
    }
    // After the move, so a list arriving on this event finds the screen already
    // somewhere the deleted topic is not.
    room_log::announce(&app);
    Ok(moved)
}

/// Seat this screen's person in the rooms, under the name and hue they declared.
///
/// A person is in the room by being there, not by speaking: without this the
/// roster would list only sessions until the first utterance, and nobody could
/// address someone who had not spoken yet.
///
/// `hue` and `account_id` are optional and are the same declarations a session
/// makes in its `hello`. The screen's person and a session take one seat of the
/// same kind, and there is one path to it — which is why the account rides here
/// too: the person at the keyboard is an account of this app like any other
/// (#59), and a join path that could not say so would put them back outside the
/// one list this exists to make possible.
///
/// Every room the screen has opened, and every room it opens after this: the
/// declaration is kept, not only applied (`RoomState::declare_local`, #141).
#[tauri::command]
pub fn room_join(
    app: AppHandle,
    state: tauri::State<RoomState>,
    name: String,
    hue: Option<f64>,
    account_id: Option<String>,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("name is empty".to_string());
    }
    let changed = state.declare_local(
        &name,
        normalize_hue(hue),
        normalize_account(account_id).as_deref(),
    );
    for room_id in changed {
        let _ = app.emit("room-participants", state.roster(&room_id));
    }
    Ok(())
}

/// Post this screen's person's utterance into one room.
///
/// Goes through `deliver` like every other post: same event, same floor check,
/// same log. The screen does not append locally on send, so the room keeps one
/// ordering authority rather than two. It reaches the sessions typed into
/// their terminals, like every post, and this is the one path whose label says
/// `admin`: the webview is the only caller, and no session can reach it
/// (#183, #195, `type_into_sessions`).
///
/// `last_seen` is the newest post the screen has drawn. The person at the
/// keyboard is a participant like any other and is refused on the same terms;
/// exempting them would be the room deciding by participant class, which is
/// the distinction the protocol stopped carrying (#39). What differs is only
/// how easily the watermark is known: the screen renders what it is handed, so
/// it always has one.
///
/// `topic_id` names the room, rather than the post going to whichever one the
/// app has open: the watermark is a post of the conversation that was on the
/// glass when this was typed, and judged against another room's floor it would
/// be a watermark from somewhere else (#141).
///
/// There is no `to`. The screen addresses in the text, as `@名前`, and
/// `deliver` reads those out of it against the room's names, the same way it
/// reads a session's (#206).
#[tauri::command]
pub fn room_post(
    app: AppHandle,
    state: tauri::State<RoomState>,
    topic_id: String,
    speaker: String,
    content: String,
    last_seen: Option<String>,
) -> Result<PostOutcome, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("content is empty".to_string());
    }

    let speaker = speaker.trim().to_string();
    if speaker.is_empty() {
        return Err("speaker is empty".to_string());
    }
    // Speaking is being present. A post under a name the roster has not seen
    // seats it, so the two cannot disagree. The hue and the account are left as
    // they stand: the composer declares neither, and passing `None` here would
    // silently withdraw a declaration made at the join.
    let (hue, account) = state.local_hue_and_account();
    for room_id in state.declare_local(&speaker, hue, account.as_deref()) {
        let _ = app.emit("room-participants", state.roster(&room_id));
    }

    let local_origin = state.local_origin.clone();
    let message_id = Uuid::new_v4().to_string();
    deliver(
        &app,
        &state,
        &topic_id,
        &local_origin,
        Post {
            message_id,
            speaker,
            // Stamped by `deliver` off the seat, which the call above left as
            // it stood. Filling it here would be a second reading of the same
            // declaration.
            hue: None,
            account: None,
            content,
            to: Vec::new(),
            ts: now_iso(),
        },
        last_seen.as_deref(),
    )
}
