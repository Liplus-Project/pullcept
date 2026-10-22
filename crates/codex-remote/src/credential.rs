//! Active-seat bootstrap storage. Native configuration carries only its path.
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
};
pub struct CredentialFile {
    pub path: PathBuf,
    file: Option<File>,
}
impl CredentialFile {
    pub fn create(path: PathBuf, values: &Value) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Windows closes the owning handle even on an app crash. The path can
        // be read while this handle lives but cannot outlive its owner.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x04000000);
        }
        let file = options
            .open(&path)
            .map_err(|_| "席の秘密を準備できません。")?;
        let mut guard = Self {
            path,
            file: Some(file),
        };
        let bytes = serde_json::to_vec(values).map_err(|_| "席の秘密を準備できません。")?;
        guard
            .file
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .map_err(|_| "席の秘密を準備できません。")?;
        Ok(guard)
    }
    pub fn marker(&self) -> PathBuf {
        self.path.with_extension("ready")
    }
}
impl Drop for CredentialFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.marker());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_file_can_be_read_and_owner_drop_revokes_bootstrap() {
        let path = std::env::temp_dir().join(format!(
            "pullcept-credential-fixture-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = CredentialFile::create(
            path.clone(),
            &serde_json::json!({"PULLCEPT_AGENT_NAME":"fixture"}),
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("fixture"));
        std::fs::write(file.marker(), b"ready").unwrap();
        let marker = file.marker();
        drop(file);
        assert!(!path.exists());
        assert!(!marker.exists());
    }
}
