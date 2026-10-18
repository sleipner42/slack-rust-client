//! View choices remembered between runs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::activity::ActivityQuery;
use crate::domain::message_search::SearchSort;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// The sidebar lists only conversations that need attention.
    pub unread_only: bool,
    pub activity: ActivityQuery,
    pub search_sort: SearchSort,
}

impl UiSettings {
    pub fn load() -> Self {
        path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let partial = path.with_extension("json.part");
            if std::fs::write(&partial, bytes).is_ok() {
                let _ = std::fs::rename(&partial, &path);
            }
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
            .join("settings.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::activity::ActivityFilter;

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let settings = UiSettings {
            unread_only: true,
            activity: ActivityQuery {
                filter: ActivityFilter::Mentions,
                unread_only: true,
            },
            search_sort: SearchSort::Relevant,
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<UiSettings>(&json).unwrap(), settings);
        let old: UiSettings = serde_json::from_str(r#"{"unread_only": true}"#).unwrap();
        assert!(old.unread_only);
        assert_eq!(old.activity, ActivityQuery::default());
        assert_eq!(old.search_sort, SearchSort::Newest);
    }
}
