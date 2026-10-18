//! Strict, atomic per-topic work journals. Malformed files are never reset implicitly.
use std::{io::Write, path::{Path, PathBuf}};
use serde_json::Value;

pub fn path(dir: &Path, topic: &str) -> Result<PathBuf, String> {
    if !super::names_one_file(topic) { return Err("Invalid topic id".into()); }
    Ok(dir.join("work-status").join(format!("{topic}.json")))
}
pub fn read(dir: &Path, topic: &str) -> Result<Value, String> {
    match std::fs::read(path(dir, topic)?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("Invalid work journal: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({"records":{},"receipts":{}})),
        Err(e) => Err(e.to_string()),
    }
}
pub fn save(dir: &Path, topic: &str, value: &Value) -> Result<(), String> {
    let path = path(dir, topic)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
/// An ID must be unique in this topic's persisted log and carry delivery-time admin evidence.
pub fn human_answer(dir: &Path, topic: &str, id: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(super::topic_path(dir, topic)).map_err(|e| e.to_string())?;
    let mut found = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let post: super::LoggedPost = serde_json::from_str(line).map_err(|e| format!("Invalid topic log: {e}"))?;
        if post.message_id == id {
            if found.is_some() { return Err("Ambiguous message id".into()); }
            found = Some(post);
        }
    }
    let post = found.filter(|p| p.role.as_deref() == Some("admin")).ok_or("No verified human message in this topic")?;
    Ok(serde_json::json!({"message_id":post.message_id,"content":post.content,"account":post.account,"name":post.speaker,"answered_at":post.ts}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn unknown_cross_topic_duplicate_and_malformed_evidence_fail() {
        let dir = std::env::temp_dir().join(format!("work-evidence-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = serde_json::json!({"message_id":"m","speaker":"human","content":"yes","ts":"now"});
        std::fs::write(super::super::topic_path(&dir,"t"), format!("{p}\n")).unwrap();
        assert!(human_answer(&dir,"t","m").is_err());
        let mut p = p; p["role"] = "admin".into();
        std::fs::write(super::super::topic_path(&dir,"t"), format!("{p}\n")).unwrap();
        assert_eq!(human_answer(&dir,"t","m").unwrap()["content"],"yes");
        assert!(human_answer(&dir,"other","m").is_err());
        std::fs::write(super::super::topic_path(&dir,"t"), format!("{p}\n{p}\n")).unwrap();
        assert!(human_answer(&dir,"t","m").is_err());
        std::fs::write(super::super::topic_path(&dir,"t"), format!("{p}\nbroken\n")).unwrap();
        assert!(human_answer(&dir,"t","m").is_err());
        save(&dir,"t",&serde_json::json!({"records":{}})).unwrap();
        assert!(read(&dir,"t").is_ok());
        std::fs::write(path(&dir,"t").unwrap(), "broken").unwrap();
        assert!(read(&dir,"t").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
