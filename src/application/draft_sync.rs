//! Keeps composer drafts and Slack's drafts in step, so a draft follows the
//! user to other devices and back. Each side's changes are compared with the
//! text both last agreed on: whichever side changed wins, Slack on a tie.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::domain::model::RemoteDraft;

/// A conversation, or a thread in it.
pub type Key = (String, Option<String>);
/// Composer text per conversation or thread.
pub type Drafts = BTreeMap<Key, String>;

#[derive(Clone, Debug, PartialEq)]
pub enum Push {
    Save {
        key: Key,
        existing: Option<RemoteDraft>,
        text: String,
    },
    Delete {
        key: Key,
        id: String,
    },
}

#[derive(Default)]
pub struct DraftSync {
    /// Slack's drafts, once listed.
    remote: HashMap<Key, RemoteDraft>,
    /// The text Slack and the composer last agreed on.
    synced: HashMap<Key, String>,
    /// The composer text last seen, and when it last changed (Unix seconds).
    seen: HashMap<Key, (String, f64)>,
    in_flight: HashSet<Key>,
    retry_after: HashMap<Key, f64>,
    loaded: bool,
}

impl DraftSync {
    pub fn new(synced: HashMap<Key, String>) -> Self {
        Self {
            synced,
            ..Self::default()
        }
    }

    pub fn synced(&self) -> &HashMap<Key, String> {
        &self.synced
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Merge Slack's drafts with the composer's. Returns composer texts to
    /// set, an empty text to clear one.
    pub fn load(&mut self, remote: Vec<RemoteDraft>, local: &Drafts) -> Vec<(Key, String)> {
        self.loaded = true;
        self.remote = remote
            .into_iter()
            .map(|draft| (key_of(&draft), draft))
            .collect();
        let keys: HashSet<Key> = self
            .remote
            .keys()
            .chain(local.keys())
            .chain(self.synced.keys())
            .cloned()
            .collect();
        let mut apply = Vec::new();
        for key in keys {
            let mine = local.get(&key).map_or("", String::as_str);
            let agreed = self.synced.get(&key).cloned();
            match self.remote.get(&key).map(|draft| draft.text.clone()) {
                Some(theirs) => {
                    let local_changed = agreed.as_deref().unwrap_or("") != mine;
                    let remote_changed = agreed.as_deref() != Some(theirs.as_str());
                    if mine == theirs || !local_changed || remote_changed {
                        if mine != theirs {
                            apply.push((key.clone(), theirs.clone()));
                        }
                        self.synced.insert(key, theirs);
                    }
                    // Otherwise only the composer changed; `due` sends it.
                }
                None => {
                    if !mine.trim().is_empty() && agreed.as_deref() == Some(mine) {
                        // Sent or discarded elsewhere since it last synced.
                        apply.push((key.clone(), String::new()));
                    }
                    self.synced.remove(&key);
                }
            }
        }
        apply
    }

    /// Note composer changes; call regularly with the current texts.
    pub fn observe(&mut self, local: &Drafts, now: f64) {
        let keys: HashSet<Key> = local.keys().chain(self.seen.keys()).cloned().collect();
        for key in keys {
            let text = local.get(&key).cloned().unwrap_or_default();
            if self.seen.get(&key).is_none_or(|(seen, _)| *seen != text) {
                self.seen.insert(key, (text, now));
            }
        }
    }

    /// Changes to send to Slack, once typing has paused.
    pub fn due(&mut self, local: &Drafts, now: f64) -> Vec<Push> {
        if !self.loaded {
            return Vec::new();
        }
        let keys: HashSet<Key> = local
            .keys()
            .chain(self.synced.keys())
            .chain(self.remote.keys())
            .cloned()
            .collect();
        let mut pushes = Vec::new();
        for key in keys {
            let mine = local.get(&key).map_or("", String::as_str);
            let agreed = self.synced.get(&key).map_or("", String::as_str);
            let changed_at = self.seen.get(&key).map_or(0.0, |(_, at)| *at);
            if mine == agreed
                || self.in_flight.contains(&key)
                || now - changed_at < PUSH_DELAY
                || self.retry_after.get(&key).is_some_and(|at| now < *at)
            {
                continue;
            }
            if mine.trim().is_empty() {
                match self.remote.get(&key) {
                    Some(draft) => pushes.push(Push::Delete {
                        key: key.clone(),
                        id: draft.id.clone(),
                    }),
                    None => {
                        self.synced.remove(&key);
                        continue;
                    }
                }
            } else {
                pushes.push(Push::Save {
                    key: key.clone(),
                    existing: self.remote.get(&key).cloned(),
                    text: mine.to_owned(),
                });
            }
            self.in_flight.insert(key);
        }
        pushes
    }

    /// The outcome of a push: the saved draft, `None` after a delete.
    pub fn pushed(
        &mut self,
        key: &Key,
        text: &str,
        result: Result<Option<RemoteDraft>, ()>,
        now: f64,
    ) {
        self.in_flight.remove(key);
        match result {
            Ok(Some(draft)) => {
                self.remote.insert(key.clone(), draft);
                self.synced.insert(key.clone(), text.to_owned());
                self.retry_after.remove(key);
            }
            Ok(None) => {
                self.remote.remove(key);
                self.synced.remove(key);
                self.retry_after.remove(key);
            }
            Err(()) => {
                self.retry_after.insert(key.clone(), now + RETRY_DELAY);
            }
        }
    }

    /// A draft saved in Slack, possibly by another device. Returns the text
    /// to put in the composer, unless the composer has its own unsent change.
    pub fn remote_saved(&mut self, draft: RemoteDraft, local: &str) -> Option<String> {
        let key = key_of(&draft);
        if let Some(known) = self.remote.get(&key)
            && known.updated > draft.updated
        {
            return None;
        }
        let text = draft.text.clone();
        self.remote.insert(key.clone(), draft);
        if self.in_flight.contains(&key) {
            return None;
        }
        let agreed = self.synced.get(&key).map_or("", String::as_str);
        if local == text {
            self.synced.insert(key, text);
            None
        } else if local == agreed {
            self.synced.insert(key, text.clone());
            Some(text)
        } else {
            None
        }
    }

    /// A draft sent or discarded in Slack. Returns the conversation whose
    /// composer to clear, unless it holds an unsent change.
    pub fn remote_deleted(&mut self, id: &str, local: &Drafts) -> Option<Key> {
        let key = self
            .remote
            .iter()
            .find(|(_, draft)| draft.id == id)
            .map(|(key, _)| key.clone())?;
        self.remote.remove(&key);
        if self.in_flight.contains(&key) {
            return None;
        }
        let mine = local.get(&key).map_or("", String::as_str);
        let agreed = self.synced.remove(&key);
        (agreed.as_deref() == Some(mine) && !mine.is_empty()).then_some(key)
    }
}

fn key_of(draft: &RemoteDraft) -> Key {
    (draft.conversation_id.clone(), draft.thread_ts.clone())
}

/// Seconds without typing before a draft goes to Slack.
const PUSH_DELAY: f64 = 1.5;
const RETRY_DELAY: f64 = 30.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn key(conversation: &str) -> Key {
        (conversation.into(), None)
    }

    fn remote(conversation: &str, id: &str, text: &str, updated: f64) -> RemoteDraft {
        RemoteDraft {
            id: id.into(),
            conversation_id: conversation.into(),
            thread_ts: None,
            text: text.into(),
            file_ids: Vec::new(),
            updated,
        }
    }

    fn drafts(entries: &[(&str, &str)]) -> Drafts {
        entries
            .iter()
            .map(|(conversation, text)| (key(conversation), text.to_string()))
            .collect()
    }

    #[test]
    fn loading_merges_by_which_side_changed_since_the_last_sync() {
        let mut sync = DraftSync::new(HashMap::from([
            (key("EDITED_HERE"), "old".to_string()),
            (key("EDITED_THERE"), "old".to_string()),
            (key("SENT_ELSEWHERE"), "gone".to_string()),
        ]));
        let local = drafts(&[
            ("EDITED_HERE", "mine"),
            ("EDITED_THERE", "old"),
            ("SENT_ELSEWHERE", "gone"),
            ("NEW_HERE", "fresh"),
        ]);
        let mut apply = sync.load(
            vec![
                remote("EDITED_HERE", "D1", "old", 1.0),
                remote("EDITED_THERE", "D2", "theirs", 1.0),
                remote("NEW_THERE", "D3", "from phone", 1.0),
            ],
            &local,
        );
        apply.sort();
        assert_eq!(
            apply,
            vec![
                (key("EDITED_THERE"), "theirs".to_string()),
                (key("NEW_THERE"), "from phone".to_string()),
                (key("SENT_ELSEWHERE"), String::new()),
            ]
        );
        let mut local = local;
        for (key, text) in apply {
            local.insert(key, text);
        }
        let mut pushes = sync.due(&local, 100.0);
        pushes.sort_by_key(|push| format!("{push:?}"));
        assert_eq!(
            pushes,
            vec![
                Push::Save {
                    key: key("EDITED_HERE"),
                    existing: Some(remote("EDITED_HERE", "D1", "old", 1.0)),
                    text: "mine".into(),
                },
                Push::Save {
                    key: key("NEW_HERE"),
                    existing: None,
                    text: "fresh".into(),
                },
            ]
        );
    }

    #[test]
    fn typing_pushes_after_a_pause_and_clearing_deletes() {
        let mut sync = DraftSync::default();
        sync.load(Vec::new(), &Drafts::new());
        let typed = drafts(&[("C1", "hel")]);
        sync.observe(&typed, 10.0);
        assert!(sync.due(&typed, 10.5).is_empty(), "still typing");
        let pushes = sync.due(&typed, 12.0);
        assert_eq!(pushes.len(), 1);
        assert!(sync.due(&typed, 13.0).is_empty(), "one push at a time");
        sync.pushed(
            &key("C1"),
            "hel",
            Ok(Some(remote("C1", "D1", "hel", 12.0))),
            12.5,
        );
        assert!(sync.due(&typed, 14.0).is_empty(), "in step");
        // Echo of our own save changes nothing.
        assert_eq!(
            sync.remote_saved(remote("C1", "D1", "hel", 12.0), "hel"),
            None
        );

        let sent = Drafts::new();
        sync.observe(&sent, 20.0);
        assert_eq!(
            sync.due(&sent, 22.0),
            vec![Push::Delete {
                key: key("C1"),
                id: "D1".into()
            }]
        );
        sync.pushed(&key("C1"), "", Ok(None), 22.5);
        assert!(sync.due(&sent, 30.0).is_empty());
    }

    #[test]
    fn changes_from_other_devices_apply_unless_the_composer_has_unsent_text() {
        let mut sync = DraftSync::default();
        let local = drafts(&[("C1", "hi")]);
        sync.load(vec![remote("C1", "D1", "hi", 1.0)], &local);
        assert_eq!(
            sync.remote_saved(remote("C1", "D1", "hi there", 2.0), "hi"),
            Some("hi there".into())
        );
        assert_eq!(
            sync.remote_saved(remote("C1", "D1", "stale", 1.5), "hi there"),
            None,
            "older events are ignored"
        );
        assert_eq!(
            sync.remote_saved(remote("C1", "D1", "from phone", 3.0), "hi there, typing"),
            None,
            "the unsent local change wins"
        );
        let local = drafts(&[("C2", "x")]);
        sync.load(vec![remote("C2", "D2", "x", 1.0)], &local);
        assert_eq!(sync.remote_deleted("D2", &local), Some(key("C2")));
        assert_eq!(sync.remote_deleted("D2", &local), None);
    }

    #[test]
    fn failed_pushes_retry_later() {
        let mut sync = DraftSync::default();
        sync.load(Vec::new(), &Drafts::new());
        let typed = drafts(&[("C1", "hello")]);
        assert_eq!(sync.due(&typed, 10.0).len(), 1);
        sync.pushed(&key("C1"), "hello", Err(()), 10.0);
        assert!(sync.due(&typed, 20.0).is_empty());
        assert_eq!(sync.due(&typed, 41.0).len(), 1);
    }
}
