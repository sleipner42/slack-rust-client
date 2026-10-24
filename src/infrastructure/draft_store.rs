//! Unsent drafts on disk, so they survive a restart.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Draft text per conversation and thread (`None` for the channel itself).
pub type Drafts = BTreeMap<(String, Option<String>), String>;
/// The text Slack last agreed on, per conversation and thread.
pub type Synced = HashMap<(String, Option<String>), String>;

#[derive(Serialize, Deserialize)]
struct StoredDraft {
    conversation_id: String,
    #[serde(default)]
    thread_ts: Option<String>,
    text: String,
    #[serde(default)]
    synced: Option<String>,
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

    pub fn load(&self) -> (Drafts, Synced) {
        let Some(content) = self
            .path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return Default::default();
        };
        let (mut drafts, mut synced) = (Drafts::new(), Synced::new());
        for draft in serde_json::from_str::<Vec<StoredDraft>>(&content).unwrap_or_default() {
            let key = (draft.conversation_id, draft.thread_ts);
            if let Some(text) = draft.synced {
                synced.insert(key.clone(), text);
            }
            if !draft.text.trim().is_empty() {
                drafts.insert(key, draft.text);
            }
        }
        (drafts, synced)
    }

    pub fn save(&self, drafts: &Drafts, synced: &Synced) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let keys: std::collections::BTreeSet<_> = drafts.keys().chain(synced.keys()).collect();
        let stored: Vec<_> = keys
            .into_iter()
            .map(|key| StoredDraft {
                conversation_id: key.0.clone(),
                thread_ts: key.1.clone(),
                text: drafts.get(key).cloned().unwrap_or_default(),
                synced: synced.get(key).cloned(),
            })
            .collect();
        super::private_file::write(path, &serde_json::to_vec_pretty(&stored)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_round_trip_and_empty_ones_are_dropped() {
        let directory = std::env::temp_dir().join(format!("drafts-{}", uuid::Uuid::new_v4()));
        let store = DraftStore::new(Some(directory.join("drafts.json")));
        assert!(store.load().0.is_empty());
        let drafts = Drafts::from([
            (("C1".into(), None), "hello *there*".into()),
            (("C1".into(), Some("1.0".into())), "thread reply".into()),
            (("C2".into(), None), "   ".into()),
        ]);
        let synced = Synced::from([(("C3".into(), None), "sent elsewhere?".into())]);
        store.save(&drafts, &synced).unwrap();
        let (loaded, loaded_synced) = store.load();
        assert_eq!(loaded_synced, synced);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&("C1".into(), None)], "hello *there*");
        assert_eq!(loaded[&("C1".into(), Some("1.0".into()))], "thread reply");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
