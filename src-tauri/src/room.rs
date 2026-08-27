//! The room socket.
//!
//! The app hosts; the sidecars connect. That direction is forced: the CLI
//! spawns its MCP servers itself, so the app never learns the launch moment or
//! a port chosen on that side. See `docs/0-requirements.md`.
//!
//! Frames on the wire are the room protocol:
//!
//!   sidecar -> room : { type: "hello", protocol, name, hue?, account_id? }
//!   both ways       : { type: "post", message_id, speaker, content, to?, ts,
//!                       last_seen? }
//!   room -> sidecar : { type: "post_result", message_id, delivered, missed }
//!
//! One frame kind carries speech, whoever produced it. A person and a session
//! are both participants; what separates them is a name, not a frame. The
//! earlier protocol had `say` for a person and `reply` for a session, and only
//! `say` was ever fanned out — the asymmetry was not a missing line but the
//! shape of the words, so the words went (#39).
//!
//! `to` is optional and carries the display name of the participant addressed.
//! The room still fans every post out to every participant — narrowing
//! delivery here would make the room hold who heard what, and answering is the
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

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use room_floor::{Admission, Floor, Missed, Post};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
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
pub const PROTOCOL_VERSION: u32 = 5;

/// One post of the room, as the frontend sees it.
///
/// Carries no participant class. What separates two lines is the name on them.
/// `own` is a self/other axis for display, which is a property of the viewer,
/// not of the speaker.
#[derive(Debug, Clone, Serialize)]
pub struct RoomMessage {
    pub message_id: String,
    /// Display name of the speaker.
    pub speaker: String,
    /// The hue the speaker declared, in oklch degrees, or `None` when they
    /// declared none. Carried on the message rather than looked up by name on
    /// the screen: a name is not an identity here, so a lookup by name is the
    /// wrong participant as soon as two answer to one name.
    pub hue: Option<f64>,
    pub content: String,
    pub to: Option<String>,
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

/// A frame on its way out, tagged with the connection that produced it.
///
/// The tag rides beside the frame, never inside it: it is never serialised, so
/// no sender can supply one and no sender can forge one. Suppression is a
/// property of the connection, which is the one identity a name collision
/// cannot blur (#40).
#[derive(Clone)]
struct Fanout {
    origin: String,
    /// Set when this frame is for one connection only — a `post_result` is the
    /// room answering the participant who posted, not something the room says.
    /// `None` is the fan-out proper: everyone but `origin`.
    target: Option<String>,
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
    to: Option<String>,
    ts: Option<String>,
    last_seen: Option<String>,
    protocol: Option<u32>,
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

struct RoomInner {
    port: Option<u16>,
    /// Everyone in the room, people and sessions alike, keyed by the connection
    /// they are in it on. Keyed on the connection rather than the name because
    /// a name is not unique: under a name-keyed roster two participants called
    /// `Claude Code` were one entry, and either of them disconnecting removed
    /// both (#40).
    participants: BTreeMap<String, Seat>,
    /// What has been said, and the order the room put it in. Lives here rather
    /// than beside the lock so the check and the stamp are one critical
    /// section (#47).
    floor: Floor,
}

/// The name whoever is on `origin` answers to, or `None` when no one is seated
/// there yet.
///
/// Takes the seat table rather than a `RoomState`, and the delivery pump in
/// `serve_participant` is the whole reason: `RoomState` carries the fan-out
/// `Sender` beside the `Arc`, not inside it, so holding one to read a name
/// would hold a sender too. Read for logging only. Nothing branches on it, and
/// nothing may: a name is not the identity here (#40), so this is the readable
/// half of a handle whose other half is the connection id.
fn name_on(seats: &Mutex<RoomInner>, origin: &str) -> Option<String> {
    seats
        .lock()
        .participants
        .get(origin)
        .map(|seat| seat.name.clone())
}

/// Shared handle to the room socket. Cloneable; all clones share one room.
#[derive(Clone)]
pub struct RoomState {
    inner: Arc<Mutex<RoomInner>>,
    /// Posts fanned out to every connected participant but their author.
    to_participants: broadcast::Sender<Fanout>,
    /// Bearer token the sidecar must present. Generated per app run, handed to
    /// the sidecar through `.mcp.json` env, never written anywhere else.
    token: String,
    /// The screen's own connection. It has no socket, so it needs an identity
    /// minted here to sit on the same suppression axis as every other one.
    local_origin: String,
}

impl RoomState {
    pub fn new() -> Self {
        let (to_participants, _) = broadcast::channel(256);
        RoomState {
            inner: Arc::new(Mutex::new(RoomInner {
                port: None,
                participants: BTreeMap::new(),
                floor: Floor::new(),
            })),
            to_participants,
            token: Uuid::new_v4().to_string(),
            local_origin: Uuid::new_v4().to_string(),
        }
    }

    pub fn token(&self) -> String {
        self.token.clone()
    }

    pub fn port(&self) -> Option<u16> {
        self.inner.lock().port
    }

    /// The roster, in name order.
    ///
    /// Ordered by name because that is what is read, and tie-broken on the id
    /// so two participants sharing a name hold a stable order rather than
    /// swapping places between emits.
    pub fn participants(&self) -> Vec<Participant> {
        let inner = self.inner.lock();
        let mut roster: Vec<Participant> = inner
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

    /// The hue declared by whoever is on `origin`, for stamping onto a post.
    fn hue_of(&self, origin: &str) -> Option<f64> {
        self.inner.lock().participants.get(origin).and_then(|seat| seat.hue)
    }

    /// The account whoever is on `origin` joined under.
    ///
    /// Read back for the same reason the hue is: `room_post` re-seats under the
    /// name it was given, and passing `None` for what it was not told would
    /// silently withdraw a declaration nobody withdrew.
    fn account_of(&self, origin: &str) -> Option<String> {
        self.inner
            .lock()
            .participants
            .get(origin)
            .and_then(|seat| seat.account.clone())
    }

    fn set_port(&self, port: u16) {
        self.inner.lock().port = Some(port);
    }

    /// Seat a participant on the connection they arrived on.
    ///
    /// One seat per connection, so re-seating replaces rather than adds: a
    /// participant who renames themselves is the same participant.
    ///
    /// Returns true when the roster changed, so a declaration that declares
    /// nothing new does not emit a roster event.
    fn seat(&self, origin: &str, name: &str, hue: Option<f64>, account: Option<&str>) -> bool {
        let mut inner = self.inner.lock();
        let current = inner
            .participants
            .get(origin)
            .map(|seat| (seat.name.clone(), seat.hue, seat.account.clone(), seat.since));
        if let Some((current_name, current_hue, current_account, _)) = &current {
            if current_name == name
                && *current_hue == hue
                && current_account.as_deref() == account
            {
                return false;
            }
        }
        // A seat taken now starts from the floor as it stands: this connection
        // was not there for what came before and was never handed it. A seat
        // being replaced keeps the position it started from — renaming does
        // not make a participant newly arrived.
        let since = match &current {
            Some((_, _, _, since)) => *since,
            None => inner.floor.seq(),
        };
        inner.participants.insert(
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

    fn unseat(&self, origin: &str) {
        self.inner.lock().participants.remove(origin);
    }
}

/// A hue is a position on the colour wheel, so it is taken modulo a turn rather
/// than rejected. `None` for a value that is not a number at all: an undeclared
/// hue and an unusable one are the same state to the screen, which derives one.
fn normalize_hue(hue: Option<f64>) -> Option<f64> {
    hue.filter(|value| value.is_finite())
        .map(|value| value.rem_euclid(360.0))
}

/// Absent is the key omitted, never an empty one: a participant matching `to`
/// against their own name must not have to rule the empty string out first.
fn normalize_to(to: Option<String>) -> Option<String> {
    to.map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
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
/// connection it arrived on: the fan-out skips that connection, and the screen
/// reads it to know whether the line is its own. Two callers reach here — the
/// socket loop and the screen's own command — and neither has a path of its
/// own past this point. The gate is here for that reason, and applies to both:
/// a participant is a participant, and a post from the screen is not a
/// different act (#39).
///
/// `last_seen` is the speaker's own account of the newest post they had seen.
/// When anything on the floor is behind it, nothing is delivered and those
/// posts come back in the outcome. Accepting the post and mentioning the miss
/// afterwards would be detection without the interruption that makes detection
/// worth anything (#47).
fn deliver(
    app: &AppHandle,
    room: &RoomState,
    origin: &str,
    mut post: Post,
    last_seen: Option<&str>,
) -> PostOutcome {
    let message_id = post.message_id.clone();

    // One acquisition, both halves. Concurrent speakers serialise here, so the
    // loser's check runs against a floor the winner has already changed.
    let (admission, hue) = {
        let mut inner = room.inner.lock();
        let (since, hue) = match inner.participants.get(origin) {
            Some(seat) => (seat.since, seat.hue),
            // Unseated: nothing was ever delivered here, so nothing is
            // presumed read. Speaking seats a participant, and the screen's
            // command does that before it reaches this point.
            None => (0, None),
        };
        // Stamped before the floor takes its copy, so the retained post and the
        // live line carry one declaration rather than two readings of it. A
        // refusal hands that copy back, and the screen draws it (#108).
        post.hue = hue;
        let admission = inner.floor.admit(origin, since, last_seen, post.clone());
        (admission, hue)
    };

    if let Admission::Unseen(missed) = admission {
        return PostOutcome {
            delivered: false,
            message_id: None,
            missed,
        };
    }

    let mut frame = serde_json::json!({
        "type": "post",
        "message_id": post.message_id,
        "speaker": post.speaker,
        "content": post.content,
        "ts": post.ts,
    });
    // Omitted rather than null when absent.
    if let Some(name) = &post.to {
        frame["to"] = serde_json::Value::String(name.clone());
    }

    // No subscribers means no session has joined yet. That is not an error —
    // the room accepts what is said in it; a later joiner simply missed it.
    let _ = room.to_participants.send(Fanout {
        origin: origin.to_string(),
        target: None,
        frame: frame.to_string(),
    });

    let _ = app.emit(
        "room-message",
        RoomMessage {
            message_id: post.message_id,
            speaker: post.speaker,
            // Read off the seat on this connection, so the colour of a line and
            // the colour of its author's roster entry are the one declaration.
            // Read inside the critical section above, with the same lock the
            // floor was judged under.
            hue,
            content: post.content,
            to: post.to,
            ts: post.ts,
            own: origin == room.local_origin,
        },
    );

    PostOutcome {
        delivered: true,
        message_id: Some(message_id),
        missed: Vec::new(),
    }
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
                if let Err(err) = serve_participant(app, room, stream).await {
                    eprintln!("[room] participant connection ended: {err}");
                }
            });
        }
    });

    Ok(port)
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
    // The seat table cannot do that: `RoomInner` is a port, a `BTreeMap` of
    // seats and the floor, and no sender lives under it.
    let seats = Arc::clone(&room.inner);
    let pump = tokio::spawn(async move {
        loop {
            let fanout = match from_room.recv().await {
                Ok(fanout) => fanout,
                // No sender left: the room itself is gone, so nothing further
                // will ever arrive. Leaving is all there is to do.
                Err(broadcast::error::RecvError::Closed) => break,
                // This connection read more slowly than the room spoke, and the
                // channel overwrote posts it had not taken yet. The receiver is
                // still live — tokio advances its cursor to the oldest post the
                // channel still holds and the next `recv` returns that one — so
                // a lag is a gap, not an ending, and the pump stays.
                //
                // Leaving here is what made a momentary gap permanent: the
                // socket stayed open and the participant stayed on the roster,
                // so a participant who was never spoken to again looked exactly
                // like one with nothing to hear. Nothing on the screen or in
                // the roster could show the difference, which is why this arm
                // continues and why it logs (#42).
                //
                // The dropped posts are not resent. The room keeps no history
                // to resend from, and giving it one here would put the room in
                // possession of who heard what — the property the fan-out is
                // built to avoid holding. A participant who missed posts still
                // learns of them on their own next post: the floor refuses a
                // post whose `last_seen` is behind and hands the missed ones
                // back (#47). That path is the speaker's, not the room's.
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
                        "[room] \"{who}\" ({own_origin}) fell behind: {dropped} post(s) dropped, delivery continues"
                    );
                    continue;
                }
            };
            match &fanout.target {
                // Addressed to one connection: the room answering whoever
                // posted. Everyone else's socket is not part of that exchange.
                Some(target) if *target != own_origin => continue,
                Some(_) => {}
                // A participant does not receive their own post. Decided on
                // the connection, never on the name: while two participants
                // share a name, a name test drops the other one's posts too
                // (#40).
                None if fanout.origin == own_origin => continue,
                None => {}
            }
            if sink.send(Message::Text(fanout.frame.into())).await.is_err() {
                break;
            }
        }
    });

    // Known once the participant says hello; used to attribute posts and to
    // drop them from the roster.
    let mut joined_as: Option<String> = None;

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
                // Seated on this connection. A second session answering to the
                // same name is a second seat, not the same one — and so is a
                // second session declaring the same account, which this room
                // does not refuse: refusing a duplicate account belongs to the
                // launcher's seat ledger, which knows what it started
                // (`session::RoomSeats`), and a room that enforced it here
                // would be treating the account as the identity.
                room.seat(
                    &origin,
                    &name,
                    normalize_hue(frame.hue),
                    normalize_account(frame.account_id).as_deref(),
                );
                joined_as = Some(name);
                let _ = app.emit("room-participants", room.participants());
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
                let outcome = deliver(
                    &app,
                    &room,
                    &origin,
                    Post {
                        message_id: message_id.clone(),
                        speaker,
                        // Stamped by `deliver` off the seat on this connection.
                        // A sender may name an addressee; it may not name its
                        // own colour any more than its own name.
                        hue: None,
                        content: frame.content.unwrap_or_default(),
                        to: normalize_to(frame.to),
                        ts: frame.ts.unwrap_or_else(now_iso),
                    },
                    frame.last_seen.as_deref(),
                );
                // Answered on the connection that posted, always — a refusal
                // that says nothing is indistinguishable from a delivery, and
                // this answer is the boundary at which a reply needing no
                // other tool finally gets to read what it missed.
                let receipt = serde_json::json!({
                    "type": "post_result",
                    "message_id": message_id,
                    "delivered": outcome.delivered,
                    "missed": outcome.missed,
                });
                let _ = room.to_participants.send(Fanout {
                    origin: origin.clone(),
                    target: Some(origin.clone()),
                    frame: receipt.to_string(),
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
        room.unseat(&origin);
        let _ = app.emit("room-participants", room.participants());
    }
    Ok(())
}

// ── Commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn room_port(state: tauri::State<RoomState>) -> Option<u16> {
    state.port()
}

#[tauri::command]
pub fn room_participants(state: tauri::State<RoomState>) -> Vec<Participant> {
    state.participants()
}

/// Seat this screen's person in the room, under the name and hue they declared.
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
    let local_origin = state.local_origin.clone();
    if state.seat(
        &local_origin,
        &name,
        normalize_hue(hue),
        normalize_account(account_id).as_deref(),
    ) {
        let _ = app.emit("room-participants", state.participants());
    }
    Ok(())
}

/// Post this screen's person's utterance into the room.
///
/// Goes through `deliver` like every other post: same frame, same fan-out,
/// same event, same floor check. The screen does not append locally on send,
/// so the room keeps one ordering authority rather than two.
///
/// `last_seen` is the newest post the screen has drawn. The person at the
/// keyboard is a participant like any other and is refused on the same terms;
/// exempting them would be the room deciding by participant class, which is
/// the distinction the protocol stopped carrying (#39). What differs is only
/// how easily the watermark is known: the screen renders what it is handed, so
/// it always has one.
#[tauri::command]
pub fn room_post(
    app: AppHandle,
    state: tauri::State<RoomState>,
    speaker: String,
    content: String,
    to: Option<String>,
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
    let local_origin = state.local_origin.clone();
    let seated_hue = state.hue_of(&local_origin);
    let seated_account = state.account_of(&local_origin);
    if state.seat(&local_origin, &speaker, seated_hue, seated_account.as_deref()) {
        let _ = app.emit("room-participants", state.participants());
    }

    let message_id = Uuid::new_v4().to_string();
    Ok(deliver(
        &app,
        &state,
        &local_origin,
        Post {
            message_id,
            speaker,
            // Stamped by `deliver` off the seat, which the call above left as
            // it stood. Filling it here would be a second reading of the same
            // declaration.
            hue: None,
            content,
            to: normalize_to(to),
            ts: now_iso(),
        },
        last_seen.as_deref(),
    ))
}
