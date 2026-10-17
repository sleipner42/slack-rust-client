//! Unsent drafts on disk, so they survive a restart.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Draft text per conversation and thread (`None` for the channel itself).
pub type Drafts = BTreeMap<(String, Option<String>), String>;

#[derive(Serialize, Deserialize)]
struct StoredDraft {
    conversation_id: String,
    #[serde(default)]
    thread_ts: Option<String>,
    text: String,
}

pub struct DraftStore {
    path: Option<PathBuf>,
}

impl DraftStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    /// The app's drafts file; none in tests.
    pub fn default_path() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        Some(dirs::config_dir()?.join("better-slack").join("drafts.json"))
    }

    pub fn load(&self) -> Drafts {
        let Some(content) = self
            .path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return Drafts::new();
        };
        serde_json::from_str::<Vec<StoredDraft>>(&content)
            .unwrap_or_default()
            .into_iter()
            .filter(|draft| !draft.text.trim().is_empty())
            .map(|draft| ((draft.conversation_id, draft.thread_ts), draft.text))
            .collect()
    }

    pub fn save(&self, drafts: &Drafts) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let stored: Vec<_> = drafts
            .iter()
            .map(|((conversation_id, thread_ts), text)| StoredDraft {
                conversation_id: conversation_id.clone(),
                thread_ts: thread_ts.clone(),
                text: text.clone(),
            })
            .collect();
        // Write then rename, so a crash never leaves a half-written file.
        let partial = path.with_extension("json.part");
        std::fs::write(&partial, serde_json::to_vec_pretty(&stored)?)?;
        std::fs::rename(&partial, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_round_trip_and_empty_ones_are_dropped() {
        let directory = std::env::temp_dir().join(format!("drafts-{}", uuid::Uuid::new_v4()));
        let store = DraftStore::new(Some(directory.join("drafts.json")));
        assert!(store.load().is_empty());
        let drafts = Drafts::from([
            (("C1".into(), None), "hello *there*".into()),
            (("C1".into(), Some("1.0".into())), "thread reply".into()),
            (("C2".into(), None), "   ".into()),
        ]);
        store.save(&drafts).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&("C1".into(), None)], "hello *there*");
        assert_eq!(loaded[&("C1".into(), Some("1.0".into()))], "thread reply");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
