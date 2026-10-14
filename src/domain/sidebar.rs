use std::collections::{HashMap, HashSet};

use crate::domain::model::{Conversation, ReadState, SectionKind, SidebarSection, ts_seconds};
use crate::domain::unread::is_active_since;

#[derive(Clone, Debug)]
pub struct SidebarGroup {
    pub title: String,
    pub conversations: Vec<Conversation>,
}

pub struct SidebarInput<'a> {
    pub sections: &'a [SidebarSection],
    pub conversations: &'a [Conversation],
    pub read_states: &'a HashMap<String, ReadState>,
    pub direct_cutoff_seconds: f64,
}

pub fn build_sidebar(input: &SidebarInput<'_>) -> Vec<SidebarGroup> {
    let by_id: HashMap<&str, &Conversation> = input
        .conversations
        .iter()
        .map(|conversation| (conversation.id.as_str(), conversation))
        .collect();
    let placed: HashSet<&str> = input
        .sections
        .iter()
        .filter(|section| is_explicit(&section.kind))
        .flat_map(|section| section.conversation_ids.iter().map(String::as_str))
        .filter(|id| by_id.contains_key(id))
        .collect();
    let unplaced = |direct: bool| -> Vec<Conversation> {
        input
            .conversations
            .iter()
            .filter(|conversation| {
                conversation.is_direct() == direct && !placed.contains(conversation.id.as_str())
            })
            .cloned()
            .collect()
    };

    with_default_sections(input.sections)
        .iter()
        .map(|section| {
            let conversations = match section.kind {
                SectionKind::Channels => sorted_by_name(unplaced(false)),
                SectionKind::DirectMessages => recent_direct(unplaced(true), input),
                SectionKind::Starred | SectionKind::Custom => sorted_by_name(
                    section
                        .conversation_ids
                        .iter()
                        .filter_map(|id| {
                            by_id
                                .get(id.as_str())
                                .map(|conversation| (*conversation).clone())
                        })
                        .collect(),
                ),
            };
            SidebarGroup {
                title: section_title(section),
                conversations,
            }
        })
        .filter(|group| !group.conversations.is_empty())
        .collect()
}

fn recent_direct(conversations: Vec<Conversation>, input: &SidebarInput<'_>) -> Vec<Conversation> {
    if input.read_states.is_empty() {
        return sorted_by_name(conversations);
    }
    let latest = |conversation: &Conversation| -> f64 {
        input
            .read_states
            .get(&conversation.id)
            .and_then(|state| state.latest_ts.as_deref())
            .map(ts_seconds)
            .unwrap_or(0.0)
    };
    let mut recent: Vec<Conversation> = conversations
        .into_iter()
        .filter(|conversation| {
            input
                .read_states
                .get(&conversation.id)
                .is_some_and(|state| is_active_since(state, input.direct_cutoff_seconds))
        })
        .collect();
    recent.sort_by(|a, b| latest(b).total_cmp(&latest(a)));
    recent
}

fn with_default_sections(sections: &[SidebarSection]) -> Vec<SidebarSection> {
    let mut all = sections.to_vec();
    for kind in [SectionKind::Channels, SectionKind::DirectMessages] {
        if !all.iter().any(|section| section.kind == kind) {
            all.push(SidebarSection {
                kind,
                name: String::new(),
                conversation_ids: Vec::new(),
            });
        }
    }
    all
}

fn is_explicit(kind: &SectionKind) -> bool {
    matches!(kind, SectionKind::Starred | SectionKind::Custom)
}

fn section_title(section: &SidebarSection) -> String {
    if !section.name.is_empty() {
        return section.name.clone();
    }
    match section.kind {
        SectionKind::Channels => String::from("Channels"),
        SectionKind::DirectMessages => String::from("Direct messages"),
        SectionKind::Starred => String::from("Starred"),
        SectionKind::Custom => String::from("Section"),
    }
}

fn sorted_by_name(mut conversations: Vec<Conversation>) -> Vec<Conversation> {
    conversations.sort_by_key(|conversation| conversation.name.to_lowercase());
    conversations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::ConversationKind;

    fn conversation(id: &str, name: &str, kind: ConversationKind) -> Conversation {
        Conversation {
            id: id.into(),
            name: name.into(),
            kind,
        }
    }

    fn state(latest: &str) -> ReadState {
        ReadState {
            latest_ts: Some(latest.into()),
            ..ReadState::default()
        }
    }

    #[test]
    fn places_custom_sections_and_filters_stale_direct_messages() {
        let conversations = vec![
            conversation("C1", "general", ConversationKind::PublicChannel),
            conversation("C2", "infra", ConversationKind::PublicChannel),
            conversation(
                "D1",
                "old friend",
                ConversationKind::DirectMessage {
                    user_id: "U1".into(),
                },
            ),
            conversation(
                "D2",
                "recent",
                ConversationKind::DirectMessage {
                    user_id: "U2".into(),
                },
            ),
            conversation(
                "D3",
                "newest",
                ConversationKind::DirectMessage {
                    user_id: "U3".into(),
                },
            ),
        ];
        let sections = vec![SidebarSection {
            kind: SectionKind::Custom,
            name: "Work".into(),
            conversation_ids: vec!["C2".into()],
        }];
        let read_states = HashMap::from([
            ("D1".to_string(), state("100.0")),
            ("D2".to_string(), state("1000.0")),
            ("D3".to_string(), state("2000.0")),
        ]);
        let groups = build_sidebar(&SidebarInput {
            sections: &sections,
            conversations: &conversations,
            read_states: &read_states,
            direct_cutoff_seconds: 500.0,
        });
        let names: Vec<(String, Vec<String>)> = groups
            .iter()
            .map(|group| {
                (
                    group.title.clone(),
                    group.conversations.iter().map(|c| c.name.clone()).collect(),
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![
                ("Work".to_string(), vec!["infra".to_string()]),
                ("Channels".to_string(), vec!["general".to_string()]),
                (
                    "Direct messages".to_string(),
                    vec!["newest".to_string(), "recent".to_string()]
                ),
            ]
        );
    }
}
