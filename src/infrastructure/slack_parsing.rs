use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::domain::model::{
    Attachment, Author, Conversation, ConversationKind, LiveEvent, Message, Reaction, ReadState,
    SavedTodo, SavedTodoPage, SectionKind, SidebarSection, SlackError, UserProfile,
};

pub fn parse_saved_todos(body: &Value, completed: bool) -> Result<SavedTodoPage, SlackError> {
    let items = body["saved_items"]
        .as_array()
        .ok_or_else(|| SlackError("Slack returned an unrecognized to-do list.".into()))?;
    let items = items
        .iter()
        .filter(|item| {
            !item["is_archived"].as_bool().unwrap_or(false)
                && if completed {
                    item["todo_state"] == "completed" || item["state"] == "completed"
                } else {
                    // Ordinary saved items also appear in the `saved` filter. They are
                    // bookmarks, not entries in Slack's To-dos view.
                    item["todo_state"] == "to_do"
                }
        })
        .map(|item| {
            Ok(SavedTodo {
                conversation_id: item["item_id"]
                    .as_str()
                    .ok_or_else(|| SlackError("A Slack to-do has no source.".into()))?
                    .to_owned(),
                item_type: item["item_type"].as_str().unwrap_or("message").to_owned(),
                ts: item["ts"].as_str().unwrap_or_default().to_owned(),
                created_at: item["date_created"].as_i64().unwrap_or(0),
                completed_at: item["date_completed"].as_i64().unwrap_or(0),
            })
        })
        .collect::<Result<Vec<_>, SlackError>>()?;
    Ok(SavedTodoPage {
        items,
        open_count: body["counts"]["todo_count"].as_u64().unwrap_or(0) as usize,
        completed_count: body["counts"]["completed_count"].as_u64().unwrap_or(0) as usize,
        next_cursor: next_cursor(body),
    })
}

pub fn parse_conversations(body: &Value) -> Vec<Conversation> {
    body["channels"]
        .as_array()
        .map(|items| items.iter().filter_map(parse_conversation).collect())
        .unwrap_or_default()
}

pub fn parse_message(item: &Value) -> Option<Message> {
    let ts = item["ts"].as_str()?.to_string();
    if HIDDEN_SUBTYPES.contains(&item["subtype"].as_str().unwrap_or("")) {
        return None;
    }
    Some(Message {
        client_msg_id: item["client_msg_id"].as_str().map(str::to_owned),
        ts,
        author: parse_author(item),
        text: message_text(item),
        thread_ts: item["thread_ts"].as_str().map(str::to_string),
        reply_count: item["reply_count"].as_u64().unwrap_or(0) as u32,
        reactions: parse_reactions(&item["reactions"]),
        attachments: parse_attachments(&item["files"])
            .into_iter()
            .chain(
                item["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|attachment| {
                        attachment["image_url"].is_string() || attachment["thumb_url"].is_string()
                    })
                    .map(parse_attachment),
            )
            .collect(),
        edited: item["edited"].is_object(),
    })
}

pub fn parse_live_event(event: &Value) -> Option<LiveEvent> {
    match event["type"].as_str()? {
        "hello" => Some(LiveEvent::Connected),
        "message" => parse_message_event(event),
        "channel_marked" | "group_marked" | "im_marked" | "mpim_marked" => {
            Some(LiveEvent::ConversationMarked {
                conversation_id: event["channel"].as_str()?.to_string(),
                ts: event["ts"].as_str()?.to_string(),
            })
        }
        kind @ ("reaction_added" | "reaction_removed") => Some(LiveEvent::ReactionChanged {
            conversation_id: event["item"]["channel"].as_str()?.to_string(),
            ts: event["item"]["ts"].as_str()?.to_string(),
            name: event["reaction"].as_str()?.to_string(),
            user_id: event["user"].as_str()?.to_string(),
            added: kind == "reaction_added",
        }),
        _ => None,
    }
}

pub fn parse_read_states(body: &Value) -> HashMap<String, ReadState> {
    COUNT_GROUPS
        .iter()
        .flat_map(|group| body[*group].as_array().cloned().unwrap_or_default())
        .filter_map(|item| {
            let id = item["id"].as_str()?.to_string();
            Some((
                id,
                ReadState {
                    latest_ts: non_zero_ts(&item["latest"]),
                    last_read_ts: non_zero_ts(&item["last_read"]),
                    has_unreads: item["has_unreads"].as_bool().unwrap_or(false),
                    mention_count: item["mention_count"].as_u64().unwrap_or(0) as u32,
                },
            ))
        })
        .collect()
}

pub fn parse_muted(prefs: &Value) -> HashSet<String> {
    let mut muted: HashSet<String> = prefs["muted_channels"]
        .as_str()
        .unwrap_or("")
        .split(',')
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let notifications: Value = prefs["all_notifications_prefs"]
        .as_str()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    if let Some(channels) = notifications["channels"].as_object() {
        muted.extend(
            channels
                .iter()
                .filter(|(_, settings)| settings["muted"].as_bool() == Some(true))
                .map(|(id, _)| id.clone()),
        );
    }
    muted
}

pub fn parse_custom_emoji(emoji: &Value) -> HashMap<String, String> {
    let Some(entries) = emoji.as_object() else {
        return HashMap::new();
    };
    entries
        .iter()
        .filter_map(|(name, value)| {
            let mut target = value.as_str()?;
            for _ in 0..MAX_ALIAS_DEPTH {
                match target.strip_prefix(ALIAS_PREFIX) {
                    Some(alias) => target = entries.get(alias)?.as_str()?,
                    None => break,
                }
            }
            target
                .starts_with("http")
                .then(|| (name.clone(), target.to_string()))
        })
        .collect()
}

pub fn parse_user_profile(user: &Value, user_id: &str) -> UserProfile {
    let candidates = [
        user["profile"]["display_name"].as_str(),
        user["real_name"].as_str(),
        user["name"].as_str(),
    ];
    UserProfile {
        id: user_id.to_string(),
        name: candidates
            .into_iter()
            .flatten()
            .find(|name| !name.is_empty())
            .unwrap_or(user_id)
            .to_string(),
        avatar_url: ["image_72", "image_48", "image_192"]
            .iter()
            .find_map(|key| user["profile"][*key].as_str())
            .map(str::to_string),
    }
}

pub fn order_sections(raw: &[Value]) -> Vec<&Value> {
    let by_id: HashMap<&str, &Value> = raw
        .iter()
        .filter_map(|section| Some((section["channel_section_id"].as_str()?, section)))
        .collect();
    let referenced: HashSet<&str> = raw
        .iter()
        .filter_map(|section| section["next_channel_section_id"].as_str())
        .collect();
    let Some(head) = raw.iter().find(|section| {
        section["channel_section_id"]
            .as_str()
            .is_some_and(|id| !referenced.contains(id))
    }) else {
        return raw.iter().collect();
    };
    let mut ordered = vec![head];
    let mut current = head;
    while let Some(next) = current["next_channel_section_id"]
        .as_str()
        .and_then(|id| by_id.get(id))
    {
        if ordered.len() >= raw.len() {
            break;
        }
        ordered.push(next);
        current = next;
    }
    if ordered.len() < raw.len() {
        return raw.iter().collect();
    }
    ordered
}

pub fn parse_section(raw: &Value) -> Option<SidebarSection> {
    let kind = match raw["type"].as_str()? {
        "channels" => SectionKind::Channels,
        "direct_messages" => SectionKind::DirectMessages,
        "stars" => SectionKind::Starred,
        "standard" => SectionKind::Custom,
        _ => return None,
    };
    Some(SidebarSection {
        kind,
        name: raw["name"].as_str().unwrap_or("").to_string(),
        conversation_ids: raw["channel_ids_page"]["channel_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

pub fn next_cursor(body: &Value) -> String {
    body["response_metadata"]["next_cursor"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn parse_message_event(event: &Value) -> Option<LiveEvent> {
    let conversation_id = event["channel"].as_str()?.to_string();
    match event["subtype"].as_str() {
        Some("message_changed") => Some(LiveEvent::MessageChanged {
            conversation_id,
            message: parse_message(&event["message"])?,
        }),
        Some("message_deleted") => Some(LiveEvent::MessageDeleted {
            conversation_id,
            ts: event["deleted_ts"].as_str()?.to_string(),
        }),
        Some("message_replied") => Some(LiveEvent::MessageChanged {
            conversation_id,
            message: parse_message(&event["message"])?,
        }),
        subtype => Some(LiveEvent::MessagePosted {
            conversation_id,
            message: parse_message(event)?,
            is_broadcast: subtype == Some("thread_broadcast"),
        }),
    }
}

fn parse_conversation(item: &Value) -> Option<Conversation> {
    let id = item["id"].as_str()?.to_string();
    if item["is_im"].as_bool() == Some(true) {
        if item["is_user_deleted"].as_bool() == Some(true) {
            return None;
        }
        let user_id = item["user"].as_str()?.to_string();
        return Some(Conversation {
            id,
            name: user_id.clone(),
            kind: ConversationKind::DirectMessage { user_id },
        });
    }
    let name = item["name"].as_str()?.to_string();
    if item["is_mpim"].as_bool() == Some(true) {
        return Some(Conversation {
            id,
            name: group_display_name(&name),
            kind: ConversationKind::GroupMessage,
        });
    }
    let kind = if item["is_private"].as_bool() == Some(true) {
        ConversationKind::PrivateChannel
    } else {
        ConversationKind::PublicChannel
    };
    Some(Conversation { id, name, kind })
}

fn group_display_name(raw_name: &str) -> String {
    let trimmed = raw_name.strip_prefix(GROUP_NAME_PREFIX).unwrap_or(raw_name);
    let without_suffix = trimmed
        .rsplit_once('-')
        .filter(|(_, suffix)| suffix.chars().all(|c| c.is_ascii_digit()))
        .map(|(head, _)| head)
        .unwrap_or(trimmed);
    without_suffix.split("--").collect::<Vec<_>>().join(", ")
}

fn parse_author(item: &Value) -> Author {
    if let Some(id) = item["bot_id"]
        .as_str()
        .or(item["bot_profile"]["id"].as_str())
    {
        return Author::Bot {
            id: id.into(),
            name: item["bot_profile"]["name"]
                .as_str()
                .or(item["username"].as_str())
                .map(str::to_owned),
            avatar_url: ["image_72", "image_48", "image_36"]
                .iter()
                .find_map(|key| {
                    item["bot_profile"]["icons"][key]
                        .as_str()
                        .or(item["icons"][key].as_str())
                })
                .map(str::to_owned),
        };
    }
    if let Some(name) = item["bot_profile"]["name"]
        .as_str()
        .or(item["username"].as_str())
        && (item["user"].as_str().is_none() || item["subtype"].as_str() == Some("bot_message"))
    {
        return Author::Named(name.to_string());
    }
    match item["user"].as_str() {
        Some(user_id) => Author::User(user_id.to_string()),
        None => Author::Named(String::from("unknown")),
    }
}

fn message_text(item: &Value) -> String {
    super::block_text::message_text(item)
}

fn parse_reactions(raw: &Value) -> Vec<Reaction> {
    raw.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(Reaction {
                        name: item["name"].as_str()?.to_string(),
                        count: item["count"].as_u64().unwrap_or(0) as u32,
                        user_ids: item["users"]
                            .as_array()
                            .map(|users| {
                                users
                                    .iter()
                                    .filter_map(|u| u.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_attachments(raw: &Value) -> Vec<Attachment> {
    raw.as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item["mode"].as_str() != Some("tombstone"))
                .map(parse_attachment)
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_attachment(item: &Value) -> Attachment {
    let name = item["name"]
        .as_str()
        .or(item["title"].as_str())
        .unwrap_or("image");
    let is_image = item["mimetype"]
        .as_str()
        .is_some_and(|mime| mime.starts_with("image/"))
        || ["png", "jpg", "jpeg", "gif", "webp"]
            .iter()
            .any(|extension| name.to_lowercase().ends_with(&format!(".{extension}")));
    let preview = PREVIEW_KEYS
        .iter()
        .find_map(|key| item[*key].as_str().filter(|value| !value.is_empty()));
    let full = item["url_private"]
        .as_str()
        .or(item["url_private_download"].as_str())
        .or(item["image_url"].as_str());
    Attachment {
        file_id: item["id"].as_str().map(str::to_string),
        name: name.to_string(),
        preview_url: preview
            .or(if is_image { full } else { None })
            .map(str::to_string),
        full_url: full.map(str::to_string),
        permalink: item["permalink"]
            .as_str()
            .or(item["title_link"].as_str())
            .map(str::to_string),
        width: item["original_w"]
            .as_u64()
            .or(item["image_width"].as_u64())
            .map(|w| w as u32),
        height: item["original_h"]
            .as_u64()
            .or(item["image_height"].as_u64())
            .map(|h| h as u32),
        preview_pending: false,
    }
}

fn non_zero_ts(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|ts| ts.parse::<f64>().is_ok_and(|seconds| seconds > 0.0))
        .map(str::to_string)
}

const GROUP_NAME_PREFIX: &str = "mpdm-";
const COUNT_GROUPS: [&str; 3] = ["channels", "mpims", "ims"];
const ALIAS_PREFIX: &str = "alias:";
const MAX_ALIAS_DEPTH: usize = 4;
const PREVIEW_KEYS: [&str; 9] = [
    "thumb_720",
    "thumb_480",
    "thumb_360",
    "thumb_1024",
    "thumb_960",
    "image_url",
    "thumb_url",
    "thumb_160",
    "thumb_80",
];
const HIDDEN_SUBTYPES: [&str; 6] = [
    "channel_join",
    "channel_leave",
    "group_join",
    "group_leave",
    "channel_purpose",
    "channel_topic",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_files_and_embedded_previews_keep_their_dimensions() {
        let message = parse_message(&json!({
            "ts": "1.0", "user": "U1", "text": "screenshots",
            "files": [
                {"id": "F1", "name": "image.png", "url_private": "https://files.slack.com/image.png", "original_w": 1600, "original_h": 900},
                {"id": "F2", "name": "small.png", "thumb_160": "https://files.slack.com/small.png"},
                {"id": "F3", "mode": "tombstone", "name": "deleted.png"}
            ],
            "attachments": [{"title": "Preview", "image_url": "https://example.com/preview.jpg", "image_width": 800, "image_height": 600}]
        })).unwrap();
        assert_eq!(message.attachments.len(), 3);
        assert_eq!(message.attachments[0].file_id.as_deref(), Some("F1"));
        assert_eq!(message.attachments[0].width, Some(1600));
        assert_eq!(message.attachments[0].height, Some(900));
        assert!(
            message
                .attachments
                .iter()
                .all(|attachment| attachment.preview_url.is_some())
        );
        assert_eq!(message.attachments[2].height, Some(600));
    }

    #[test]
    fn parses_counts_and_muted() {
        let body = json!({
            "channels": [{"id": "C1", "latest": "20.5", "last_read": "10.0", "has_unreads": true, "mention_count": 2}],
            "ims": [{"id": "D1", "latest": "0000000000.000000", "has_unreads": false, "mention_count": 0}]
        });
        let states = parse_read_states(&body);
        assert_eq!(states["C1"].mention_count, 2);
        assert_eq!(states["D1"].latest_ts, None);

        let prefs = json!({
            "muted_channels": "C1,C2",
            "all_notifications_prefs": "{\"channels\":{\"C3\":{\"muted\":true},\"C4\":{\"muted\":false}}}"
        });
        let muted = parse_muted(&prefs);
        assert!(muted.contains("C1") && muted.contains("C3") && !muted.contains("C4"));
    }

    #[test]
    fn parses_live_events() {
        let posted = parse_live_event(
            &json!({"type": "message", "channel": "C1", "user": "U1", "text": "hi", "ts": "1.0"}),
        );
        assert!(
            matches!(posted, Some(LiveEvent::MessagePosted { ref conversation_id, .. }) if conversation_id == "C1")
        );
        let deleted = parse_live_event(
            &json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "1.0"}),
        );
        assert_eq!(
            deleted,
            Some(LiveEvent::MessageDeleted {
                conversation_id: "C1".into(),
                ts: "1.0".into()
            })
        );
        let reaction = parse_live_event(
            &json!({"type": "reaction_added", "user": "U1", "reaction": "tada", "item": {"channel": "C1", "ts": "1.0"}}),
        );
        assert!(matches!(
            reaction,
            Some(LiveEvent::ReactionChanged { added: true, .. })
        ));
    }

    #[test]
    fn resolves_emoji_aliases() {
        let emoji = parse_custom_emoji(
            &json!({"party": "https://e/party.gif", "yay": "alias:party", "ok": "alias:thumbsup"}),
        );
        assert_eq!(
            emoji.get("yay").map(String::as_str),
            Some("https://e/party.gif")
        );
        assert!(!emoji.contains_key("ok"));
    }
}

#[cfg(test)]
mod todo_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saved_bookmarks_are_not_todos_and_counts_are_not_page_lengths() {
        let body = json!({
            "saved_items": [
                {"item_id":"C1", "item_type":"message", "ts":"1.001", "todo_state":"to_do", "state":"in_progress", "date_created":12},
                {"item_id":"C2", "item_type":"message", "ts":"2.001", "todo_state":"saved", "state":"in_progress"},
                {"item_id":"C3", "item_type":"message", "ts":"3.001", "todo_state":"to_do", "is_archived":true}
            ],
            "counts":{"todo_count":15,"completed_count":83,"uncompleted_count":1},
            "response_metadata":{"next_cursor":"page-two"}
        });
        let page = parse_saved_todos(&body, false).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].conversation_id, "C1");
        assert_eq!(page.items[0].ts, "1.001");
        assert_eq!((page.open_count, page.completed_count), (15, 83));
        assert_eq!(page.next_cursor, "page-two");
    }

    #[test]
    fn completed_items_and_invalid_responses_are_distinguished_from_empty_lists() {
        let body = json!({
            "saved_items":[{"item_id":"C1","ts":"1.001","state":"completed","todo_state":"completed"}],
            "counts":{"todo_count":0,"completed_count":1}
        });
        assert_eq!(parse_saved_todos(&body, true).unwrap().items.len(), 1);
        assert!(parse_saved_todos(&body, false).unwrap().items.is_empty());
        assert!(parse_saved_todos(&json!({"ok":true}), false).is_err());
    }
}
