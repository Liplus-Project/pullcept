//! The topics of one room: a directory of topic files, and the index that
//! annotates it.
//!
//! ```text
//! {dir}/index.json      one entry per topic: name, when it was made,
//!                       and the session each account was in
//! {dir}/{topic}.jsonl   one topic's posts, append-only
//! ```
//!
//! **The directory is what exists; the index annotates it.** A topic's posts
//! are the file, and [`read`] adopts any `.jsonl` it finds without an entry.
//! That is what makes lazy creation safe — a launch mints a topic and writes
//! nothing, so a run where nothing was said leaves no row in the list, and a
//! crash between the post reaching the file and the entry reaching the index
//! closes because the file alone is enough to rebuild the entry (#115).
//!
//! It is also what fixes the shape of a delete. The entry is an annotation, so
//! removing it removes an annotation: the file is still there, and the next
//! read adopts it back under a rebuilt entry. A topic is deleted by deleting
//! the file, and the entry goes with it (#119, decision 1). [`delete`] does
//! both in that order, and the test that matters here is the one that runs the
//! other order and watches the topic come back.
//!
//! Everything in this crate takes the room's directory as an argument. The path
//! to it is resolved by the app (`src-tauri/src/room_log.rs`), which is also
//! where the lock over read-modify-write of the index lives, and where the
//! screen is told the list has changed. What is here is the part that had to be
//! reachable by `cargo test`: `src-tauri` is compiled but not tested, for the
//! reason docs/0-requirements.md gives under テストの配置.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The index, as it is named inside the room's directory.
pub const INDEX_FILE: &str = "index.json";

/// The extension a topic's posts are kept under.
pub const TOPIC_EXTENSION: &str = "jsonl";

/// How long an auto-generated title is allowed to be, in characters.
///
/// Characters rather than bytes: the titles are Japanese more often than not,
/// and a byte cut would land inside one.
const TITLE_CHARS: usize = 40;

/// One post, as the log holds it.
///
/// The same five fields going in and coming out. `hue` and `own` are not among
/// them, and why each is absent is written down where the mapping from a post
/// is made (`src-tauri/src/room_log.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedPost {
    pub message_id: String,
    pub speaker: String,
    pub content: String,
    /// The participant this was addressed to, or absent when it was said to the
    /// room. Omitted rather than written as null, so the two states are the
    /// field's presence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub ts: String,
}

/// One topic, as the index holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    /// Opaque, minted once, and the file name of this topic's posts. Never a
    /// title: a title is edited, and a file whose name moved with it would
    /// leave the posts behind.
    pub topic_id: String,
    /// What the list shows. Generated from the opening of the first post said
    /// in it and editable afterwards (#115, decision 9). Empty until that first
    /// post lands, which is a state the screen draws rather than a value
    /// missing.
    pub title: String,
    /// When it was made, RFC 3339. The room's clock, or the first post's own
    /// stamp for a topic adopted from a file.
    pub created_at: String,
    /// The session each account was in while this topic was open, keyed by
    /// account id.
    ///
    /// This is what makes a topic a vessel rather than a transcript: reopening
    /// it hands these back to the launch, and the participant returns carrying
    /// its own context instead of being read a summary of it (#115, decisions 3
    /// and 4). Deleting a topic throws them away with it — the conversation and
    /// the way back to whoever was in it are one thing, and #119 decision 1 is
    /// the decision to treat them as one.
    #[serde(default)]
    pub sessions: BTreeMap<String, String>,
}

/// The topics, as the file holds them.
///
/// Oldest first, by `created_at`. The screen reverses it — a list is read
/// newest first — and the file keeps the order the conversation happened in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopicIndex {
    #[serde(default)]
    pub topics: Vec<Topic>,
}

impl TopicIndex {
    pub fn find(&self, topic_id: &str) -> Option<&Topic> {
        self.topics.iter().find(|one| one.topic_id == topic_id)
    }

    pub fn find_mut(&mut self, topic_id: &str) -> Option<&mut Topic> {
        self.topics.iter_mut().find(|one| one.topic_id == topic_id)
    }

    /// Make sure `topic_id` has an entry, and hand back a mutable hold on it.
    ///
    /// The one place a topic reaches the index. Every caller is a deliberate act
    /// that has to survive the app closing — the first post, a session id, a
    /// rename — and none of them is "the app was opened".
    pub fn realize(&mut self, topic_id: &str, created_at: &str) -> &mut Topic {
        if self.find(topic_id).is_none() {
            self.topics.push(Topic {
                topic_id: topic_id.to_string(),
                title: String::new(),
                created_at: created_at.to_string(),
                sessions: BTreeMap::new(),
            });
        }
        self.find_mut(topic_id).expect("just inserted when absent")
    }

    /// Take one account's session id off a topic, when the topic still holds
    /// exactly that id. Answers whether anything was taken.
    ///
    /// The id is named rather than assumed, so this removes the record it was
    /// told about and never a later one. A resume that could not go back and a
    /// fresh launch that recorded a new id are the same account in the same
    /// topic; matching on the id is what keeps the first from reaching into the
    /// second (#127).
    ///
    /// A topic this index does not name, an account with nothing on record, and
    /// a record holding some other id all answer false. None of the three is an
    /// error: the caller is undoing a record it may already have lost the race
    /// for, and there being nothing to undo is a legitimate outcome of that.
    ///
    /// The entry itself stays. What is dropped is the way back into one
    /// session; the topic is the conversation, and the conversation did happen.
    pub fn forget_session(&mut self, topic_id: &str, account_id: &str, session_id: &str) -> bool {
        let Some(topic) = self.find_mut(topic_id) else {
            return false;
        };
        if topic.sessions.get(account_id).map(String::as_str) != Some(session_id) {
            return false;
        }
        topic.sessions.remove(account_id).is_some()
    }

    /// Take one topic's entry out. Answers whether there was one.
    ///
    /// Private, and a delete path cannot be built out of it by accident: the
    /// entry is an annotation of a file that is still there, so a read after
    /// this adopts the file back (see [`delete`]).
    fn forget(&mut self, topic_id: &str) -> bool {
        let before = self.topics.len();
        self.topics.retain(|one| one.topic_id != topic_id);
        self.topics.len() != before
    }
}

/// Where the index lives inside the room's directory.
pub fn index_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_FILE)
}

/// Where one topic's posts live.
///
/// The id is a UUID minted by the app, so nothing a person types reaches a
/// path. A title with a slash in it would otherwise be a directory.
pub fn topic_path(dir: &Path, topic_id: &str) -> PathBuf {
    dir.join(format!("{topic_id}.{TOPIC_EXTENSION}"))
}

/// Whether `topic_id` names one file in the room's directory and nothing else.
///
/// The ids this app mints are UUIDs, and the ids it adopts are file stems from
/// one non-recursive scan of that directory, so nothing reachable through the
/// app fails this. It is checked anyway on the one operation that removes a
/// file: an id carrying a separator makes [`topic_path`] a path out of the
/// directory, and being wrong there costs somebody else's file rather than a
/// failed read.
fn names_one_file(topic_id: &str) -> bool {
    !topic_id.is_empty()
        && !topic_id.contains(['/', '\\', ':'])
        && topic_id != "."
        && topic_id != ".."
}

/// A title from the opening of the first thing said in the topic.
///
/// The first line, whitespace collapsed, cut at `TITLE_CHARS`. A list of
/// timestamps is a list nobody can read (#115, decision 9), and the opening of
/// the first post is what a person would have written there anyway.
pub fn title_from(content: &str) -> String {
    let flat = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return String::new();
    }
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(TITLE_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Whether anything has been said in `topic_id` yet, without reading it.
///
/// The file's size and nothing else, which is the same question [`read_posts`]
/// answers and a different cost: a launch asks this to decide one sentence of
/// the manners it hands the session, and paying the length of the topic for
/// that sentence would put the cost #115 kept off every launch back on it. A
/// count would need the read; that is why no count is handed to the session
/// (#133).
///
/// A missing file and an empty one are the same state here, as they are for the
/// first-post check the append makes. So is a file of nothing but torn lines:
/// it answers true, and the pull that follows says the topic is empty. Over-
/// answering costs one call nobody had to make; under-answering costs the
/// session the fact that it is missing something.
pub fn has_posts(dir: &Path, topic_id: &str) -> bool {
    std::fs::metadata(topic_path(dir, topic_id))
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

/// One topic's posts, oldest first.
///
/// The tuple's second half is how many lines did not parse. A line that does
/// not parse is skipped rather than failing the read: the case it covers is a
/// torn tail from a run that ended mid-write, and refusing the whole topic over
/// the last line of it would lose everything to protect nothing. It is not
/// skipped quietly — the caller says how many, on the same surface a failed
/// append is said on.
pub fn read_posts(path: &Path) -> (Vec<LoggedPost>, usize) {
    // No file is no history, not a failure. It is what a topic nobody has
    // spoken in yet looks like.
    let Ok(content) = std::fs::read_to_string(path) else {
        return (Vec::new(), 0);
    };

    let mut posts = Vec::new();
    let mut skipped = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<LoggedPost>(line) {
            Ok(post) => posts.push(post),
            Err(_) => skipped += 1,
        }
    }
    (posts, skipped)
}

/// Read the index off disk and reconcile it with the directory.
///
/// The reconciliation is not a repair path bolted on: it is what lets a topic
/// exist before any entry is written for it. An entry with no file stays — a
/// topic whose session was launched but which nobody has spoken in yet is that
/// case, and its session id is the whole reason to keep it.
///
/// `fallback_created_at` dates an adopted file with nothing readable in it. A
/// topic made through the app carries its own stamp and never reaches that
/// branch.
///
/// The second half of the answer is whether the caller should write the index
/// back: something was adopted, or there was no index file to begin with.
pub fn read(dir: &Path, fallback_created_at: &str) -> Result<(TopicIndex, bool), String> {
    let path = index_path(dir);
    let existed = path.exists();
    let mut index = if existed {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read the topic index: {e}"))?;
        serde_json::from_str::<TopicIndex>(&content)
            .map_err(|e| format!("Failed to parse the topic index: {e}"))?
    } else {
        TopicIndex::default()
    };

    let adopted = adopt_orphans(dir, &mut index, fallback_created_at);
    index
        .topics
        .sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.topic_id.cmp(&b.topic_id)));
    Ok((index, adopted || !existed))
}

/// Give an entry to every topic file the index does not name.
///
/// Answers true when it changed anything.
fn adopt_orphans(dir: &Path, index: &mut TopicIndex, fallback_created_at: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // No directory is no topics, not a failure. It is the first run.
        return false;
    };
    let mut adopted = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(TOPIC_EXTENSION) {
            continue;
        }
        let Some(topic_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if index.find(topic_id).is_some() {
            continue;
        }
        let (posts, _) = read_posts(&path);
        let first = posts.first();
        index.topics.push(Topic {
            topic_id: topic_id.to_string(),
            title: first.map(|post| title_from(&post.content)).unwrap_or_default(),
            // The first thing said in it, which is the closest thing a file
            // carries to when it began.
            created_at: first
                .map(|post| post.ts.clone())
                .unwrap_or_else(|| fallback_created_at.to_string()),
            sessions: BTreeMap::new(),
        });
        adopted = true;
    }
    adopted
}

/// Put the index back on disk.
pub fn write(dir: &Path, index: &TopicIndex) -> Result<(), String> {
    let path = index_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create the room log dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize the topic index: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Failed to write the topic index: {e}"))
}

/// Delete one topic: its posts, and the entry that annotated them.
///
/// **Both, and the file first.** The entry is an annotation of the file, so an
/// index with the entry taken out and the file left behind is not a deleted
/// topic — it is a topic with no annotation, and [`read`] adopts it back under
/// a rebuilt entry the next time anything reads the list (#119, decision 1).
/// The order follows from that: the file is the half that has to go, so it goes
/// first, and a failure between the two leaves an entry whose file is gone.
/// That state is visible in the list and is the one #117 already describes; the
/// other order fails by putting the topic back, and reports nothing while it
/// does.
///
/// A missing file is not a failure. It is #117's own state — an entry the index
/// carries for a file that never existed — and taking the entry out is exactly
/// what is wanted for it.
///
/// The caller writes the index back. It is holding the lock over the whole
/// read-modify-write, and this is one modify inside it.
pub fn delete(dir: &Path, index: &mut TopicIndex, topic_id: &str) -> Result<(), String> {
    if !names_one_file(topic_id) {
        return Err(format!(
            "Refusing to delete a topic whose id is not one file name: {topic_id}"
        ));
    }
    match std::fs::remove_file(topic_path(dir, topic_id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Failed to delete the topic's log: {e}")),
    }
    index.forget(topic_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("pullcept-topic-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const NOW: &str = "2026-08-28T00:00:00+09:00";

    /// One topic file with one post in it.
    fn put_topic(dir: &Path, topic_id: &str, content: &str, ts: &str) {
        let post = LoggedPost {
            message_id: Uuid::new_v4().to_string(),
            speaker: "Lin".to_string(),
            content: content.to_string(),
            to: None,
            ts: ts.to_string(),
        };
        let mut line = serde_json::to_string(&post).expect("serialize");
        line.push('\n');
        std::fs::write(topic_path(dir, topic_id), line).expect("write topic");
    }

    fn read_now(dir: &Path) -> TopicIndex {
        let (index, _) = read(dir, NOW).expect("read");
        index
    }

    #[test]
    fn adopts_a_topic_file_the_index_does_not_name() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "alpha", "最初の一言", "2026-08-27T10:00:00+09:00");

        let (index, needs_write) = read(scratch.path(), NOW).expect("read");

        assert!(needs_write, "an adoption is a change the caller has to write");
        let topic = index.find("alpha").expect("adopted");
        assert_eq!(topic.title, "最初の一言");
        assert_eq!(topic.created_at, "2026-08-27T10:00:00+09:00");
    }

    #[test]
    fn a_topic_with_posts_is_told_from_one_without_them() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "spoken", "先に言われたこと", "2026-08-27T10:00:00+09:00");
        std::fs::write(topic_path(scratch.path(), "opened"), "").expect("write empty topic");

        assert!(
            has_posts(scratch.path(), "spoken"),
            "a topic that was spoken in has posts"
        );
        assert!(
            !has_posts(scratch.path(), "opened"),
            "a topic realised by a launch and never spoken in has none"
        );
        assert!(
            !has_posts(scratch.path(), "never-made"),
            "a topic with no file at all has none"
        );
    }

    #[test]
    fn deleting_takes_the_file_and_the_entry_together() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "alpha", "消す方", "2026-08-27T10:00:00+09:00");
        let mut index = read_now(scratch.path());
        assert!(index.find("alpha").is_some());

        delete(scratch.path(), &mut index, "alpha").expect("delete");
        write(scratch.path(), &index).expect("write");

        assert!(!topic_path(scratch.path(), "alpha").exists());
        assert!(
            read_now(scratch.path()).find("alpha").is_none(),
            "a deleted topic does not come back on the next read"
        );
    }

    /// The reason [`delete`] removes the file.
    ///
    /// This is the index-only delete decision 1 refused, run as itself: the
    /// entry goes, the file stays, and the very next read adopts the file back
    /// under a rebuilt entry. Nothing reports a failure along the way, which is
    /// why the shape had to be settled in the code rather than left to whoever
    /// writes the next delete path.
    #[test]
    fn removing_only_the_entry_lets_the_next_read_resurrect_the_topic() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "alpha", "消えない方", "2026-08-27T10:00:00+09:00");
        let mut index = read_now(scratch.path());

        index.forget("alpha");
        write(scratch.path(), &index).expect("write");
        assert!(
            read_now(scratch.path()).find("alpha").is_some(),
            "the file is still there, so the read adopts it back"
        );

        // The same topic, deleted the way `delete` does it, stays deleted.
        let mut index = read_now(scratch.path());
        delete(scratch.path(), &mut index, "alpha").expect("delete");
        write(scratch.path(), &index).expect("write");
        assert!(read_now(scratch.path()).find("alpha").is_none());
    }

    #[test]
    fn deleting_one_topic_leaves_the_others_where_they_were() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "alpha", "残る方", "2026-08-27T10:00:00+09:00");
        put_topic(scratch.path(), "beta", "消す方", "2026-08-27T11:00:00+09:00");
        put_topic(scratch.path(), "gamma", "残る方も", "2026-08-27T12:00:00+09:00");
        let mut index = read_now(scratch.path());

        delete(scratch.path(), &mut index, "beta").expect("delete");
        write(scratch.path(), &index).expect("write");

        let index = read_now(scratch.path());
        let ids: Vec<&str> = index.topics.iter().map(|one| one.topic_id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "gamma"]);
        assert!(topic_path(scratch.path(), "alpha").exists());
        assert!(topic_path(scratch.path(), "gamma").exists());
    }

    /// #117's own state: an entry the index carries for a file that is not
    /// there. Nothing else reaches it, and this is the way out.
    #[test]
    fn deleting_an_entry_whose_file_is_missing_is_not_a_failure() {
        let scratch = Scratch::new();
        let mut index = TopicIndex::default();
        index.realize("orphaned", NOW);
        write(scratch.path(), &index).expect("write");

        let mut index = read_now(scratch.path());
        assert!(index.find("orphaned").is_some(), "an entry with no file stays");

        delete(scratch.path(), &mut index, "orphaned").expect("delete");
        write(scratch.path(), &index).expect("write");

        assert!(read_now(scratch.path()).find("orphaned").is_none());
    }

    /// The way back out of a session id that no conversation stands behind.
    ///
    /// The record is written at spawn, which is earlier than the moment a
    /// conversation exists, so a launch that ended before the CLI made one
    /// leaves an id that every later resume fails on (#127). Taking it off is
    /// what puts the next launch back on the normal line.
    #[test]
    fn forgetting_a_session_leaves_the_topic_and_the_other_accounts() {
        let mut index = TopicIndex::default();
        let topic = index.realize("alpha", NOW);
        topic.sessions.insert("lay".to_string(), "dead".to_string());
        topic.sessions.insert("lin".to_string(), "alive".to_string());

        assert!(index.forget_session("alpha", "lay", "dead"));

        let topic = index.find("alpha").expect("the topic itself stays");
        assert_eq!(topic.sessions.get("lay"), None);
        assert_eq!(
            topic.sessions.get("lin").map(String::as_str),
            Some("alive"),
            "one account's dead id is not another account's"
        );
    }

    /// The guard that keeps a late undo from reaching a live record.
    ///
    /// The account is seatless the moment its session ends, so it can be
    /// launched again before the exit is acted on. That launch records a new
    /// id under the same topic and the same account, and it is the id — not the
    /// pair — that says which of the two this is.
    #[test]
    fn forgetting_takes_only_the_id_it_was_told_about() {
        let mut index = TopicIndex::default();
        index
            .realize("alpha", NOW)
            .sessions
            .insert("lay".to_string(), "fresh".to_string());

        assert!(!index.forget_session("alpha", "lay", "dead"));
        assert_eq!(
            index.find("alpha").expect("topic").sessions.get("lay").map(String::as_str),
            Some("fresh"),
            "a record that has moved on is not this caller's to undo"
        );

        assert!(!index.forget_session("alpha", "nobody", "dead"));
        assert!(!index.forget_session("missing", "lay", "fresh"));
    }

    #[test]
    fn refuses_an_id_that_is_not_one_file_name() {
        let scratch = Scratch::new();
        let outsider = scratch.path().join("outsider.jsonl");
        std::fs::write(&outsider, "").expect("write outsider");
        let inner = scratch.path().join("room");
        std::fs::create_dir_all(&inner).expect("inner dir");
        let mut index = TopicIndex::default();

        for id in ["../outsider", "..\\outsider", "..", ".", ""] {
            let refused = delete(&inner, &mut index, id);
            assert!(refused.is_err(), "id {id:?} names more than one file");
        }
        assert!(
            outsider.exists(),
            "nothing outside the room's directory was touched"
        );
    }

    #[test]
    fn a_title_is_the_first_line_collapsed_and_cut() {
        assert_eq!(title_from("  こんにちは   部屋  "), "こんにちは 部屋");
        assert_eq!(title_from("一行目\n二行目"), "一行目 二行目");
        assert_eq!(title_from("   "), "");

        let long = "あ".repeat(TITLE_CHARS + 5);
        let cut = title_from(&long);
        assert_eq!(cut.chars().count(), TITLE_CHARS + 1, "the cut plus its mark");
        assert!(cut.ends_with('…'));

        let exact = "い".repeat(TITLE_CHARS);
        assert_eq!(title_from(&exact), exact, "no mark when nothing was cut");
    }

    #[test]
    fn a_torn_line_is_skipped_and_counted_rather_than_failing_the_read() {
        let scratch = Scratch::new();
        put_topic(scratch.path(), "alpha", "読める行", "2026-08-27T10:00:00+09:00");
        let path = topic_path(scratch.path(), "alpha");
        let mut content = std::fs::read_to_string(&path).expect("read");
        content.push_str("{\"message_id\":\"torn\",\"spea\n");
        std::fs::write(&path, content).expect("write");

        let (posts, skipped) = read_posts(&path);
        assert_eq!(posts.len(), 1);
        assert_eq!(skipped, 1);
    }
}
