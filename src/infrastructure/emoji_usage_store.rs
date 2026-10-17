//! Emoji picked in this app, kept on disk so the frequently used list
//! survives a restart. Slack's own history is read from its preferences.

use std::path::PathBuf;

use crate::domain::emoji_usage::EmojiUsage;

pub fn load() -> EmojiUsage {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

pub fn save(usage: &EmojiUsage) {
    let Some(path) = path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(usage) {
        let partial = path.with_extension("json.part");
        if std::fs::write(&partial, bytes).is_ok() {
            let _ = std::fs::rename(&partial, &path);
        }
    }
}

fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        dirs::config_dir()?
            .join("better-slack")
            .join("emoji_use.json"),
    )
}
