//! A stopped seat's observer leaving its thread (#586).
//!
//! The interrupt is answered only once its turn has ended. Sent and followed at
//! once by an unsubscribe and a close, native can lose it, and the turn then
//! runs on with no seat: a loaded thread with an active turn is never unloaded,
//! so the seat's own resume finds it still open. Each step therefore waits for
//! its answer, within a bound, before the next.
use serde_json::{json, Value};
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

/// Request IDs this module sends; none of the observer's own requests use them.
pub const INTERRUPT_ID: u64 = 1_000_001;
pub const UNSUBSCRIBE_ID: u64 = 1_000_002;

/// How the observer left. `None` = not attempted; `Some(false)` = sent, but no
/// answer arrived within the bound.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Left {
    pub turn_ended: Option<bool>,
    pub unsubscribed: Option<bool>,
}

/// Interrupt `turn` if one is running and wait for it to end, then unsubscribe
/// from `thread` and wait for that answer, then close. `each` bounds every wait.
/// The socket's own read timeout paces the waits.
pub fn leave<S: Read + Write>(
    socket: &mut WebSocket<S>,
    thread: &str,
    turn: Option<&str>,
    each: Duration,
) -> Left {
    let mut left = Left::default();
    if let Some(turn) = turn {
        let sent = send(socket, INTERRUPT_ID, "turn/interrupt", json!({"threadId":thread,"turnId":turn}));
        // An error answer (the turn ended on its own meanwhile) also ends the wait.
        left.turn_ended = Some(sent && wait(socket, each, |v| {
            answers(v, INTERRUPT_ID)
                || (v["method"] == "turn/completed" && v["params"]["threadId"].as_str() == Some(thread))
        }));
    }
    let sent = send(socket, UNSUBSCRIBE_ID, "thread/unsubscribe", json!({"threadId":thread}));
    left.unsubscribed = Some(sent && wait(socket, each, |v| answers(v, UNSUBSCRIBE_ID)));
    let _ = socket.close(None);
    let _ = socket.flush();
    left
}

fn answers(value: &Value, id: u64) -> bool {
    value.get("method").is_none() && value["id"].as_u64() == Some(id)
}

fn send<S: Read + Write>(socket: &mut WebSocket<S>, id: u64, method: &str, params: Value) -> bool {
    socket
        .send(Message::text(json!({"id":id,"method":method,"params":params}).to_string()))
        .is_ok()
}

fn wait<S: Read + Write>(socket: &mut WebSocket<S>, within: Duration, done: impl Fn(&Value) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if serde_json::from_str::<Value>(&text).is_ok_and(|v| done(&v)) {
                    return true;
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    /// A native stand-in: records each request with the time it arrived and
    /// answers it after `delay`, or never when `delay` is `None`.
    fn native(
        delay: impl Fn(&str) -> Option<Duration> + Send + 'static,
        extra: Vec<Value>,
    ) -> (WebSocket<TcpStream>, mpsc::Receiver<(String, Instant)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            loop {
                let Ok(Message::Text(text)) = ws.read() else { return };
                let v: Value = serde_json::from_str(&text).unwrap();
                let method = v["method"].as_str().unwrap_or_default().to_string();
                let _ = tx.send((method.clone(), Instant::now()));
                for note in &extra {
                    let _ = ws.send(Message::text(note.to_string()));
                }
                if let Some(after) = delay(&method) {
                    thread::sleep(after);
                    let _ = ws.send(Message::text(json!({"id":v["id"],"result":{}}).to_string()));
                }
            }
        });
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let (ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}"), stream).unwrap();
        (ws, rx)
    }

    #[test]
    fn unsubscribe_is_sent_only_after_the_interrupt_is_answered() {
        let (mut ws, rx) = native(|m| Some(if m == "turn/interrupt" { Duration::from_millis(300) } else { Duration::ZERO }), vec![]);
        let started = Instant::now();
        let left = leave(&mut ws, "T", Some("turn-1"), Duration::from_secs(5));
        assert_eq!(left, Left { turn_ended: Some(true), unsubscribed: Some(true) });
        let (first, _) = rx.recv().unwrap();
        let (second, at) = rx.recv().unwrap();
        assert_eq!((first.as_str(), second.as_str()), ("turn/interrupt", "thread/unsubscribe"));
        assert!(at.duration_since(started) >= Duration::from_millis(300), "unsubscribe must wait for the interrupt's answer");
    }

    #[test]
    fn turn_completed_for_this_thread_ends_the_interrupt_wait() {
        // The answer never comes, but the turn's own end is seen; another thread's is not enough.
        let notes = vec![
            json!({"method":"turn/completed","params":{"threadId":"other","turn":{"id":"x"}}}),
            json!({"method":"turn/completed","params":{"threadId":"T","turn":{"id":"turn-1"}}}),
        ];
        let (mut ws, _rx) = native(|m| (m != "turn/interrupt").then_some(Duration::ZERO), notes);
        let left = leave(&mut ws, "T", Some("turn-1"), Duration::from_secs(5));
        assert_eq!(left, Left { turn_ended: Some(true), unsubscribed: Some(true) });
    }

    #[test]
    fn no_turn_means_only_unsubscribe() {
        let (mut ws, rx) = native(|_| Some(Duration::ZERO), vec![]);
        let left = leave(&mut ws, "T", None, Duration::from_secs(5));
        assert_eq!(left, Left { turn_ended: None, unsubscribed: Some(true) });
        assert_eq!(rx.recv().unwrap().0, "thread/unsubscribe");
    }

    #[test]
    fn a_silent_native_is_left_within_the_bounds() {
        let (mut ws, rx) = native(|_| None, vec![]);
        let started = Instant::now();
        let left = leave(&mut ws, "T", Some("turn-1"), Duration::from_millis(300));
        assert_eq!(left, Left { turn_ended: Some(false), unsubscribed: Some(false) });
        assert!(started.elapsed() < Duration::from_secs(3));
        // Unsubscribe is still sent after an unanswered interrupt.
        assert_eq!(rx.recv().unwrap().0, "turn/interrupt");
        assert_eq!(rx.recv().unwrap().0, "thread/unsubscribe");
    }
}
