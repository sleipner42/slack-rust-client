use crate::application::chat_service::Workspace;

#[derive(Default)]
pub struct DockBadge {
    label: Option<String>,
    initialized: bool,
}

impl DockBadge {
    pub fn update(&mut self, workspace: Option<&Workspace>) {
        let label = workspace.and_then(badge_label);
        if !self.initialized || label != self.label {
            set_badge(label.as_deref());
            self.label = label;
            self.initialized = true;
        }
    }
}

fn badge_label(workspace: &Workspace) -> Option<String> {
    let mut mentions = 0_u32;
    let mut unread = false;
    for conversation in &workspace.conversations {
        let Some(state) = workspace.read_states.get(&conversation.id) else {
            continue;
        };
        let muted = workspace.muted.contains(&conversation.id);
        // Mentions still count in muted channels. Ordinary muted activity is silent.
        mentions = mentions.saturating_add(state.mention_count);
        if state.has_unreads && !muted {
            unread = true;
            if conversation.is_direct() && state.mention_count == 0 {
                mentions = mentions.saturating_add(1);
            }
        }
    }
    match mentions {
        0 if unread => Some("•".into()),
        0 => None,
        1..=99 => Some(mentions.to_string()),
        _ => Some("99+".into()),
    }
}

#[cfg(target_os = "macos")]
fn set_badge(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    // eframe calls app logic on the native event-loop thread, including while occluded.
    if let Some(main_thread) = MainThreadMarker::new() {
        let label = label.map(NSString::from_str);
        NSApplication::sharedApplication(main_thread)
            .dockTile()
            .setBadgeLabel(label.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
fn set_badge(_label: Option<&str>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{Conversation, ConversationKind, ReadState};

    fn workspace() -> Workspace {
        Workspace {
            self_user_id: "me".into(),
            conversations: vec![
                Conversation {
                    id: "C".into(),
                    name: "general".into(),
                    kind: ConversationKind::PublicChannel,
                },
                Conversation {
                    id: "D".into(),
                    name: "friend".into(),
                    kind: ConversationKind::DirectMessage {
                        user_id: "friend".into(),
                    },
                },
            ],
            sidebar: vec![],
            sections: vec![],
            read_states: Default::default(),
            muted: Default::default(),
            emoji: Default::default(),
        }
    }

    #[test]
    fn unread_dot_mentions_direct_messages_and_clear() {
        let mut workspace = workspace();
        assert_eq!(badge_label(&workspace), None);
        workspace.read_states.insert(
            "C".into(),
            ReadState {
                has_unreads: true,
                ..Default::default()
            },
        );
        assert_eq!(badge_label(&workspace).as_deref(), Some("•"));
        workspace.read_states.insert(
            "D".into(),
            ReadState {
                has_unreads: true,
                ..Default::default()
            },
        );
        assert_eq!(badge_label(&workspace).as_deref(), Some("1"));
        workspace.read_states.get_mut("D").unwrap().mention_count = 3;
        assert_eq!(badge_label(&workspace).as_deref(), Some("3"));
        workspace.read_states.clear();
        assert_eq!(badge_label(&workspace), None);
    }

    #[test]
    fn muted_activity_is_silent_but_mentions_count_and_large_counts_are_capped() {
        let mut workspace = workspace();
        workspace.muted.insert("C".into());
        workspace.read_states.insert(
            "C".into(),
            ReadState {
                has_unreads: true,
                ..Default::default()
            },
        );
        assert_eq!(badge_label(&workspace), None);
        workspace.read_states.get_mut("C").unwrap().mention_count = 120;
        assert_eq!(badge_label(&workspace).as_deref(), Some("99+"));
    }
}
