//! The five panel values of a Codex CLI seat, read off its rollout (#283). See docs/3-accounts.md.
//!
//! Codex has no status-line command. It writes the same facts into the
//! session's rollout, `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`,
//! one JSON object per line (`transcript_checked` finds the file):
//!
//! | value | line |
//! |---|---|
//! | model, effort | `turn_context`: `payload.model`, `payload.effort` |
//! | 5 h, weekly | `event_msg` / `token_count`: `payload.rate_limits.{primary,secondary}.used_percent`, told apart by `window_minutes` (300 / 10080) |
//! | their resets (#306) | the same two windows' `resets_at`, Unix seconds |
//! | context | the same line: `payload.info.last_token_usage.total_tokens` against `payload.info.model_context_window`, by Codex's own formula (`context`) |
//!
//! **The shapes are observed, not published** (Codex CLI 0.160.0, 2026-10-05).
//! So nothing here may fail on a line: one that is not JSON, of a type not
//! listed, cut short, or missing a field changes nothing it cannot read, and
//! a value never read stays `None` — `—` on the panel, as a Claude Code seat's
//! missing field does.
//!
//! **Only appended bytes are read.** `Tail` holds its offset and reads from
//! there on every call; the file is never read again from the top. A resumed
//! seat starts at the length the file had at launch, so the panel says what
//! this run has reported, not what the last one did.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The five values, in the order the panel lists them. Absent until a line names them.
///
/// With them, when each of the two windows resets (#306): what the panel
/// counts down to beside the 5-hour and weekly rows. Read off the same window
/// as its percentage, and replaced with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub five_hour: Option<f64>,
    pub seven_day: Option<f64>,
    pub context: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_resets_at: Option<i64>,
}

/// The two windows by length, not by position: a plan whose `primary` is not
/// the 5-hour window leaves that row `—` rather than mislabelled.
const FIVE_HOUR_MINUTES: u64 = 300;
const SEVEN_DAY_MINUTES: u64 = 10080;

/// The bucket the two windows were observed under. Another bucket's numbers
/// are not this seat's 5 h / weekly and would raise 制限中 on them.
const LIMIT_ID: &str = "codex";

/// The longest line parsed. The lines read are about 2 KiB; tool output lines
/// reach megabytes, and are skipped without being held or parsed.
pub const LINE_MAX: usize = 64 * 1024;

impl Status {
    /// Apply one rollout line. Whether any value changed.
    ///
    /// A `turn_context` is a snapshot of its turn, so both of its values are
    /// replaced, a missing one by `None`. A `token_count` may carry `info` or
    /// `rate_limits` alone; the half it does not carry is left as it stood.
    pub fn apply_line(&mut self, line: &[u8]) -> bool {
        match serde_json::from_slice::<Value>(line) {
            Ok(value) => self.apply(&value),
            Err(_) => false,
        }
    }

    /// [`Status::apply_line`], for a line already parsed.
    pub fn apply(&mut self, value: &Value) -> bool {
        let before = self.clone();
        let payload = &value["payload"];
        match value["type"].as_str() {
            Some("turn_context") if payload.is_object() => {
                self.model = text(&payload["model"]);
                self.effort = text(&payload["effort"]);
            }
            Some("event_msg") if payload["type"] == "token_count" => {
                let info = &payload["info"];
                if info.is_object() {
                    self.context = context(info);
                }
                let limits = &payload["rate_limits"];
                if limits.is_object() && limits["limit_id"].as_str().is_none_or(|id| id == LIMIT_ID)
                {
                    self.five_hour = window(limits, FIVE_HOUR_MINUTES);
                    self.seven_day = window(limits, SEVEN_DAY_MINUTES);
                    self.five_hour_resets_at = window_reset(limits, FIVE_HOUR_MINUTES);
                    self.seven_day_resets_at = window_reset(limits, SEVEN_DAY_MINUTES);
                }
            }
            _ => {}
        }
        *self != before
    }
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

fn window_of(limits: &Value, minutes: u64) -> Option<&Value> {
    ["primary", "secondary"]
        .iter()
        .map(|key| &limits[key])
        .find(|w| w["window_minutes"].as_u64() == Some(minutes))
}

fn window(limits: &Value, minutes: u64) -> Option<f64> {
    window_of(limits, minutes).and_then(|w| finite(&w["used_percent"]))
}

fn window_reset(limits: &Value, minutes: u64) -> Option<i64> {
    window_of(limits, minutes).and_then(|w| crate::epoch_seconds(&w["resets_at"]))
}

/// Tokens Codex counts as always taken (system prompt, tools) and removes from
/// both sides before it computes what is left. Codex-internal, not published,
/// and free to change between versions (openai/codex `codex-rs/protocol/src/protocol.rs`,
/// `percent_of_context_window_remaining`, read 2026-10-05).
const BASELINE_TOKENS: f64 = 12000.0;

/// Used percent as Codex's `/status` shows it: Codex rounds the *remaining*
/// percent to a whole number, and the panel shows `100 -` that, so the two
/// always add up to 100.
fn context(info: &Value) -> Option<f64> {
    let total = finite(&info["last_token_usage"]["total_tokens"])?;
    let window = finite(&info["model_context_window"]).filter(|n| *n > 0.0)?;
    Some(100.0 - remaining_percent(total, window))
}

fn remaining_percent(total: f64, window: f64) -> f64 {
    if window <= BASELINE_TOKENS {
        return 0.0;
    }
    let effective = window - BASELINE_TOKENS;
    let used = (total - BASELINE_TOKENS).max(0.0);
    let remaining = (effective - used).max(0.0);
    (remaining / effective * 100.0).clamp(0.0, 100.0).round()
}

/// One rollout, read forward from an offset a complete line at a time.
#[derive(Debug, Default)]
pub struct Tail {
    offset: u64,
    line: Vec<u8>,
    /// Inside a line longer than `LINE_MAX`: dropped up to its newline.
    skipping: bool,
    pub status: Status,
    /// The last turn end read and not yet taken (#294, `limit::turn_end`).
    turn_end: Option<super::limit::TurnEnd>,
    /// The reset the last `token_count` reported for its fullest window
    /// (#294, `limit::rollout_reset`): what the room is told while the
    /// app-server has not said better.
    pub resets_at: Option<i64>,
}

impl Tail {
    /// Start reading at `offset`: 0 for a file this launch created, the
    /// launch-time length for one it resumed into.
    pub fn new(offset: u64) -> Self {
        Tail {
            offset,
            ..Tail::default()
        }
    }

    /// Bytes appended to the file. Whether any value changed.
    ///
    /// A line is applied when its newline arrives; the part after the last
    /// newline waits for the next call, so a line the CLI is still writing is
    /// never read half-written.
    pub fn feed(&mut self, mut bytes: &[u8]) -> bool {
        let mut changed = false;
        while let Some(end) = bytes.iter().position(|&b| b == b'\n') {
            self.hold(&bytes[..end]);
            if !self.skipping {
                if let Ok(value) = serde_json::from_slice::<Value>(&self.line) {
                    changed |= self.status.apply(&value);
                    if let Some(end) = super::limit::turn_end(&value) {
                        self.turn_end = Some(end);
                    }
                    if let Some(reset) = super::limit::rollout_reset(&value) {
                        self.resets_at = Some(reset);
                    }
                }
            }
            self.line.clear();
            self.skipping = false;
            bytes = &bytes[end + 1..];
        }
        self.hold(bytes);
        changed
    }

    /// The last turn end read since the previous call, if any. One read may
    /// carry several; the last is the seat's state.
    pub fn take_turn_end(&mut self) -> Option<super::limit::TurnEnd> {
        self.turn_end.take()
    }

    fn hold(&mut self, part: &[u8]) {
        if self.skipping {
            return;
        }
        if self.line.len() + part.len() > LINE_MAX {
            self.line.clear();
            self.skipping = true;
        } else {
            self.line.extend_from_slice(part);
        }
    }

    /// Read what was appended to `path` since the last call. Whether any value changed.
    ///
    /// A file shorter than the offset was replaced or cut, and is read again
    /// from its start, holding the values read so far.
    pub fn read(&mut self, path: &Path) -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
            self.line.clear();
            self.skipping = false;
        }
        if len == self.offset {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut changed = false;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            self.offset += read as u64;
            changed |= self.feed(&chunk[..read]);
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cut down from a real 0.160.0 rollout (2026-10-05): the fields read here,
    // with their real names, nesting and values.
    const TURN: &str = r#"{"timestamp":"2026-10-05T11:07:45.538Z","type":"turn_context","payload":{"turn_id":"t","cwd":"C:\\w","model":"gpt-6.1-sol","effort":"high","summary":"none"}}"#;
    const TOKENS: &str = r#"{"timestamp":"2026-10-05T11:08:13.046Z","ordinal":27,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":60336,"total_tokens":60727},"last_token_usage":{"input_tokens":31566,"output_tokens":268,"total_tokens":31834},"model_context_window":258400},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":85,"window_minutes":300,"resets_at":1791207909},"secondary":{"used_percent":93,"window_minutes":10080,"resets_at":1791613577},"credits":null,"plan_type":"plus","rate_limit_reached_type":null}}}"#;

    fn lines(parts: &[&str]) -> String {
        parts.iter().map(|l| format!("{l}\n")).collect()
    }

    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pullcept-rollout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("rollout.jsonl")
    }

    #[test]
    fn reads_all_five_from_the_observed_lines() {
        let mut tail = Tail::new(0);
        assert!(tail.feed(lines(&[TURN, TOKENS]).as_bytes()));
        let s = &tail.status;
        assert_eq!(s.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(s.five_hour, Some(85.0));
        assert_eq!(s.seven_day, Some(93.0));
        assert_eq!(s.five_hour_resets_at, Some(1791207909));
        assert_eq!(s.seven_day_resets_at, Some(1791613577));
        // (258400 - 12000 - (31834 - 12000)) / (258400 - 12000) = 91.95% left -> 92 -> 8 used.
        assert_eq!(s.context, Some(8.0));
    }

    #[test]
    fn context_matches_codex_status() {
        // Observed 2026-10-05: `/status` said `93% left (29.1K used / 258K)`.
        assert_eq!(100.0 - remaining_percent(29100.0, 258000.0), 7.0);
        // The baseline is not counted as used: a fresh session reads 0.
        assert_eq!(remaining_percent(0.0, 258000.0), 100.0);
        assert_eq!(remaining_percent(12000.0, 258000.0), 100.0);
        // Past the window, nothing is left; never below 0.
        assert_eq!(remaining_percent(300000.0, 258000.0), 0.0);
        // A window no larger than the baseline has nothing left at all.
        assert_eq!(remaining_percent(0.0, 12000.0), 0.0);
        assert_eq!(remaining_percent(0.0, 5000.0), 0.0);
        let mut tail = Tail::new(0);
        tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":100},"model_context_window":12000}}}"#]).as_bytes());
        assert_eq!(tail.status.context, Some(100.0));
    }

    #[test]
    fn a_line_is_read_only_once_its_newline_arrives() {
        let mut tail = Tail::new(0);
        let (head, rest) = TURN.split_at(40);
        assert!(!tail.feed(head.as_bytes()));
        assert!(!tail.feed(rest.as_bytes()));
        assert_eq!(tail.status, Status::default());
        assert!(tail.feed(b"\r\n"));
        assert_eq!(tail.status.model.as_deref(), Some("gpt-6.1-sol"));
        // A multi-byte character cut between two reads is held, not broken.
        let mut tail = Tail::new(0);
        let line = TURN.replace("high", "高");
        let bytes = format!("{line}\n").into_bytes();
        let cut = bytes.iter().position(|&b| b >= 0x80).unwrap() + 1;
        tail.feed(&bytes[..cut]);
        tail.feed(&bytes[cut..]);
        assert_eq!(tail.status.effort.as_deref(), Some("高"));
    }

    #[test]
    fn unreadable_lines_change_nothing_and_never_panic() {
        let mut tail = Tail::new(0);
        tail.feed(lines(&[TURN, TOKENS]).as_bytes());
        let known = tail.status.clone();
        let junk: &[&[u8]] = &[
            b"not json",
            b"{\"type\":\"turn_con",
            b"\xff\xfe\x00garbage",
            b"[]",
            b"null",
            b"{}",
            br#"{"type":"response_item","payload":{"type":"message","model":"other"}}"#,
            br#"{"type":"turn_context","payload":"text"}"#,
            br#"{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}"#,
            br#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
            br#"{"type":"event_msg","payload":{"type":"task_started","rate_limits":{"primary":{"used_percent":1,"window_minutes":300}}}}"#,
        ];
        for line in junk {
            let mut bytes = line.to_vec();
            bytes.push(b'\n');
            assert!(!tail.feed(&bytes), "{}", String::from_utf8_lossy(line));
        }
        assert_eq!(tail.status, known);
    }

    #[test]
    fn a_field_that_cannot_be_read_is_absent_not_zero() {
        let mut tail = Tail::new(0);
        tail.feed(lines(&[
            r#"{"type":"turn_context","payload":{"model":"m","effort":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":"many"},"model_context_window":0},"rate_limits":{"primary":{"used_percent":"85","window_minutes":300},"secondary":{"used_percent":4}}}}"#,
        ])
        .as_bytes());
        assert_eq!(tail.status.model.as_deref(), Some("m"));
        assert_eq!(tail.status.effort, None);
        assert_eq!(tail.status.five_hour, None);
        assert_eq!(tail.status.seven_day, None);
        assert_eq!(tail.status.context, None);
        assert_eq!(tail.status.five_hour_resets_at, None);
        assert_eq!(tail.status.seven_day_resets_at, None);
    }

    #[test]
    fn each_reset_goes_with_its_own_window() {
        let mut tail = Tail::new(0);
        tail.feed(lines(&[TOKENS]).as_bytes());
        // Told apart by length, as the percentages are: the weekly window in
        // `primary` puts its reset on the weekly row.
        assert!(tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":40,"window_minutes":10080,"resets_at":1791700000},"secondary":{"used_percent":7,"window_minutes":300,"resets_at":1791210000}}}}"#]).as_bytes()));
        assert_eq!(tail.status.five_hour_resets_at, Some(1791210000));
        assert_eq!(tail.status.seven_day_resets_at, Some(1791700000));
        // A new reset alone is a change the panel is told of.
        assert!(tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":40,"window_minutes":10080,"resets_at":1791700000},"secondary":{"used_percent":7,"window_minutes":300,"resets_at":1791228000}}}}"#]).as_bytes()));
        assert_eq!(tail.status.five_hour_resets_at, Some(1791228000));
        // Another bucket's reset is not this seat's.
        assert!(!tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"other","primary":{"used_percent":1,"window_minutes":300,"resets_at":1}}}}"#]).as_bytes()));
        assert_eq!(tail.status.five_hour_resets_at, Some(1791228000));
        // A window that stops naming its reset leaves none standing.
        tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":41,"window_minutes":10080,"resets_at":"soon"},"secondary":{"used_percent":8,"window_minutes":300}}}}"#]).as_bytes());
        assert_eq!(tail.status.five_hour_resets_at, None);
        assert_eq!(tail.status.seven_day_resets_at, None);
        assert_eq!(tail.status.five_hour, Some(8.0));
    }

    #[test]
    fn windows_are_told_apart_by_length_and_bucket() {
        let mut tail = Tail::new(0);
        tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":40,"window_minutes":10080},"secondary":{"used_percent":7,"window_minutes":300}}}}"#]).as_bytes());
        assert_eq!(tail.status.five_hour, Some(7.0));
        assert_eq!(tail.status.seven_day, Some(40.0));
        // Another bucket does not move this seat's two rows.
        assert!(!tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"other","primary":{"used_percent":100,"window_minutes":300}}}}"#]).as_bytes()));
        assert_eq!(tail.status.five_hour, Some(7.0));
        // A token_count with limits only leaves the context as it stood.
        tail.feed(lines(&[TOKENS]).as_bytes());
        tail.feed(lines(&[r#"{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":86,"window_minutes":300},"secondary":{"used_percent":93,"window_minutes":10080}}}}"#]).as_bytes());
        assert_eq!(tail.status.five_hour, Some(86.0));
        assert!(tail.status.context.is_some());
    }

    #[test]
    fn an_oversized_line_is_skipped_and_the_next_is_read() {
        let mut tail = Tail::new(0);
        let huge = format!(
            r#"{{"type":"turn_context","payload":{{"model":"{}"}}}}"#,
            "x".repeat(LINE_MAX)
        );
        for piece in huge.as_bytes().chunks(4096) {
            tail.feed(piece);
        }
        assert!(tail.line.is_empty());
        tail.feed(b"\n");
        assert_eq!(tail.status.model, None);
        tail.feed(lines(&[TURN]).as_bytes());
        assert_eq!(tail.status.model.as_deref(), Some("gpt-6.1-sol"));
    }

    #[test]
    fn reads_only_what_was_appended_after_the_offset() {
        let path = scratch();
        // A resumed seat: what the file held at launch is the last run's.
        std::fs::write(&path, lines(&[TURN, TOKENS])).unwrap();
        let start = std::fs::metadata(&path).unwrap().len();
        let mut tail = Tail::new(start);
        assert!(!tail.read(&path).unwrap());
        assert_eq!(tail.status, Status::default());

        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(TURN.replace("gpt-6.1-sol", "gpt-6-luna").as_bytes())
            .unwrap();
        assert!(!tail.read(&path).unwrap());
        file.write_all(b"\n").unwrap();
        assert!(tail.read(&path).unwrap());
        assert_eq!(tail.status.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(tail.status.five_hour, None);
        assert!(!tail.read(&path).unwrap());
        drop(file);

        // Replaced by something shorter: read again from its start.
        std::fs::write(&path, lines(&[TOKENS])).unwrap();
        assert!(tail.read(&path).unwrap());
        assert_eq!(tail.status.five_hour, Some(85.0));
        assert_eq!(tail.status.model.as_deref(), Some("gpt-6-luna"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_last_turn_end_and_reset_are_kept_for_the_limit() {
        use super::super::limit::TurnEnd;
        let mut tail = Tail::new(0);
        assert_eq!(tail.take_turn_end(), None);
        tail.feed(lines(&[
            TOKENS,
            r#"{"type":"event_msg","payload":{"type":"task_complete","error":null}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete","error":{"codex_error_info":"usage_limit_exceeded"}}}"#,
        ])
        .as_bytes());
        assert_eq!(tail.resets_at, Some(1791613577), "93 is the fuller window in TOKENS");
        assert_eq!(tail.take_turn_end(), Some(TurnEnd::UsageLimit));
        assert_eq!(tail.take_turn_end(), None);
    }

    #[test]
    fn a_missing_file_is_an_error_to_retry_not_a_panic() {
        let path = scratch();
        let mut tail = Tail::new(0);
        assert!(tail.read(&path).is_err());
        std::fs::write(&path, lines(&[TURN])).unwrap();
        assert!(tail.read(&path).unwrap());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
