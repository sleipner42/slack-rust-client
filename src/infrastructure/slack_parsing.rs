use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::domain::model::{
    Attachment, Author, ChimeJoin, Conversation, ConversationKind, Huddle, LiveEvent, Message,
    Reaction, ReadState, RemoteDraft, SavedTodo, SavedTodoPage, SectionKind, SharedMessage,
    SidebarSection, SlackError, UserProfile, ts_seconds,
};
use crate::domain::people::Person;

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
        shares: item["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|attachment| super::block_text::is_shared_message(attachment))
            .map(parse_share)
            .collect(),
        edited: item["edited"].is_object(),
        huddle: item["room"]
            .is_object()
            .then(|| parse_huddle(&item["room"], item["channel"].as_str()))
            .flatten(),
    })
}

/// A huddle from Slack's `room` object.
pub fn parse_huddle(room: &Value, conversation_id: Option<&str>) -> Option<Huddle> {
    if room["call_family"]
        .as_str()
        .is_some_and(|family| family != "huddle")
    {
        return None;
    }
    let ids = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect()
    };
    Some(Huddle {
        id: room["id"].as_str()?.to_owned(),
        conversation_id: conversation_id.or(room["channels"][0].as_str())?.to_owned(),
        participants: ids(&room["participants"]),
        joined_ever: ids(&room["participant_history"]),
        link: room["huddle_link"].as_str().map(str::to_owned),
        ended: room["has_ended"].as_bool().unwrap_or(false),
        started: room["date_start"].as_i64().unwrap_or(0),
        ended_at: room["date_end"].as_i64().unwrap_or(0),
    })
}

/// How to join a huddle's audio, from `rooms.join` (under `call`) or from a
/// `huddle_invite` event, which both carry Chime's meeting and attendee.
pub fn parse_chime_join(body: &Value, conversation_id: &str) -> Option<ChimeJoin> {
    let call = if body["call"].is_object() {
        &body["call"]
    } else {
        body
    };
    let chime = &call["free_willy"];
    let placement = &chime["meeting"]["MediaPlacement"];
    let attendee = &chime["attendee"];
    let text = |value: &Value| {
        value
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Some(ChimeJoin {
        call_id: text(&call["call_id"]).or_else(|| text(&call["id"]))?,
        conversation_id: text(&call["channel_id"]).unwrap_or_else(|| conversation_id.to_owned()),
        attendee_id: text(&attendee["AttendeeId"])?,
        join_token: text(&attendee["JoinToken"])?,
        signaling_url: text(&placement["SignalingUrl"])?,
        audio_host_url: text(&placement["AudioHostUrl"])?,
    })
}

pub fn parse_search(body: &Value) -> crate::domain::message_search::SearchPage {
    let messages = &body["messages"];
    let paging = &messages["paging"];
    let number = |value: &Value| value.as_u64().unwrap_or(0) as u32;
    crate::domain::message_search::SearchPage {
        matches: messages["matches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_search_match)
            .collect(),
        page: number(&paging["page"]).max(1),
        pages: number(&paging["pages"]),
        total: number(&paging["total"]),
    }
}

fn parse_search_match(item: &Value) -> Option<crate::domain::message_search::SearchMatch> {
    use crate::domain::message_search::{SearchMatch, permalink_thread, strip_highlights};
    let channel = &item["channel"];
    let highlighted = item["text"].as_str().unwrap_or_default().to_owned();
    // The message itself without the match markers.
    let mut clean = item.clone();
    clean["text"] = Value::String(strip_highlights(&highlighted));
    let thread_ts = item["permalink"].as_str().and_then(permalink_thread);
    let mut message = parse_message(&clean)?;
    message.thread_ts = message.thread_ts.or(thread_ts.clone());
    Some(SearchMatch {
        conversation_id: channel["id"].as_str()?.to_owned(),
        conversation_name: channel["name"].as_str().unwrap_or_default().to_owned(),
        is_channel: !(channel["is_im"].as_bool().unwrap_or(false)
            || channel["is_mpim"].as_bool().unwrap_or(false)),
        thread_ts,
        message,
        highlighted,
    })
}

fn parse_share(attachment: &Value) -> SharedMessage {
    let name = ["author_name", "author_subname"]
        .iter()
        .find_map(|key| attachment[*key].as_str().filter(|name| !name.is_empty()));
    let author = match attachment["author_id"].as_str().filter(|id| !id.is_empty()) {
        Some(id) => Author::User(id.into()),
        None => Author::Named(name.unwrap_or("Shared message").into()),
    };
    let channel_id = attachment["channel_id"]
        .as_str()
        .or(attachment["message_blocks"][0]["channel"].as_str())
        .map(str::to_owned);
    let ts = attachment["ts"]
        .as_str()
        .or(attachment["message_blocks"][0]["ts"].as_str())
        .map(str::to_owned);
    let root = attachment["is_thread_root_unfurl"].as_bool() == Some(true);
    // A reply's permalink names its thread: .../p1700000000000200?thread_ts=1700000000.000100
    let thread_ts = attachment["from_url"]
        .as_str()
        .and_then(|url| url.split_once("thread_ts="))
        .map(|(_, rest)| rest.split('&').next().unwrap_or_default().to_owned())
        .or_else(|| {
            attachment["message_blocks"][0]["message"]["thread_ts"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|ts| !ts.is_empty())
        .or_else(|| ts.clone().filter(|_| root));
    SharedMessage {
        author,
        avatar_url: attachment["author_icon"].as_str().map(str::to_owned),
        ts,
        channel_id,
        in_thread: attachment["is_reply_unfurl"].as_bool() == Some(true) || root,
        thread_ts,
        footer: attachment["footer"].as_str().map(str::to_owned),
        text: super::block_text::shared_text(attachment),
    }
}

pub fn parse_live_event(event: &Value) -> Option<LiveEvent> {
    match event["type"].as_str()? {
        "hello" => Some(LiveEvent::Connected),
        "emoji_changed" => Some(LiveEvent::EmojiChanged),
        "channel_joined" | "group_joined" | "mpim_joined" | "im_created" | "channel_left"
        | "group_left" | "mpim_close" | "channel_rename" | "group_rename" | "channel_archive"
        | "group_archive" | "channel_unarchive" | "group_unarchive" => {
            Some(LiveEvent::ConversationsChanged)
        }
        "draft_create" | "draft_update" => Some(match parse_draft(&event["draft"]) {
            Some(draft) => LiveEvent::DraftSaved(draft),
            // Sent, scheduled or no longer tied to one conversation.
            None => LiveEvent::DraftDeleted {
                id: event["draft"]["id"]
                    .as_str()
                    .or(event["draft_id"].as_str())?
                    .to_string(),
            },
        }),
        "draft_delete" => Some(LiveEvent::DraftDeleted {
            id: event["draft_id"].as_str()?.to_string(),
        }),
        "message" => parse_message_event(event),
        "sh_room_join" | "sh_room_leave" | "sh_room_update" => Some(LiveEvent::HuddleChanged(
            parse_huddle(&event["room"], event["huddle"]["channel_id"].as_str())?,
        )),
        "huddle_invite" => {
            let conversation_id = event["channel_id"].as_str()?.to_owned();
            Some(LiveEvent::HuddleRinging {
                conversation_id,
                caller_id: event["sender_user_id"].as_str()?.to_owned(),
                call_id: event["call_id"].as_str()?.to_owned(),
            })
        }
        "huddle_invite_cancel" => Some(LiveEvent::HuddleRingStopped {
            conversation_id: event["channel_id"].as_str()?.to_owned(),
        }),
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
            let latest_ts = non_zero_ts(&item["latest"]);
            let last_read_ts = non_zero_ts(&item["last_read"]);
            // Slack can keep counting a conversation as unread, with a mention,
            // after it was read past its latest message; nothing is left to read.
            let read_past_latest = matches!(
                (&latest_ts, &last_read_ts),
                (Some(latest), Some(read)) if ts_seconds(read) >= ts_seconds(latest)
            );
            Some((
                id,
                ReadState {
                    has_unreads: !read_past_latest
                        && item["has_unreads"].as_bool().unwrap_or(false),
                    mention_count: if read_past_latest {
                        0
                    } else {
                        item["mention_count"].as_u64().unwrap_or(0) as u32
                    },
                    latest_ts,
                    last_read_ts,
                },
            ))
        })
        .collect()
}

/// Emoji entries ("E" + name) of Slack's `frecency_jumper` preference, which
/// also tracks channels and people under other prefixes.
pub fn parse_emoji_usage(prefs: &Value) -> crate::domain::emoji_usage::EmojiUsage {
    use crate::domain::emoji_usage::{EmojiUsage, EmojiUse};
    let jumper: Value = prefs["frecency_jumper"]
        .as_str()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    let emoji = jumper
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, entry)| {
            let name = id.strip_prefix('E')?;
            let visits: Vec<i64> = entry["visits"]
                .as_array()?
                .iter()
                .filter_map(Value::as_i64)
                .collect();
            Some((
                name.to_string(),
                EmojiUse {
                    count: entry["count"].as_u64().unwrap_or(visits.len() as u64) as u32,
                    visits,
                },
            ))
        })
        .collect();
    EmojiUsage { emoji }
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
                    Some(alias) => match entries.get(alias) {
                        Some(next) => target = next.as_str()?,
                        // An alias of a standard emoji, such as alias:tada.
                        None => return Some((name.clone(), target.to_string())),
                    },
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

/// Someone from `users.list`; `None` for deactivated accounts.
pub fn parse_person(user: &Value) -> Option<Person> {
    if user["deleted"].as_bool() == Some(true) {
        return None;
    }
    let id = user["id"].as_str()?;
    let profile = parse_user_profile(user, id);
    Some(Person {
        id: id.to_owned(),
        name: profile.name,
        real_name: user["real_name"]
            .as_str()
            .or(user["profile"]["real_name"].as_str())
            .unwrap_or_default()
            .to_owned(),
        handle: user["name"].as_str().unwrap_or_default().to_owned(),
        avatar_url: profile.avatar_url,
    })
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

/// A draft waiting in one conversation or thread; none for sent, deleted or
/// scheduled drafts and ones meant for several destinations.
pub fn parse_draft(draft: &Value) -> Option<RemoteDraft> {
    if draft["is_sent"] == true
        || draft["is_deleted"] == true
        || draft["date_scheduled"].as_i64().unwrap_or(0) > 0
    {
        return None;
    }
    let [destination] = draft["destinations"].as_array()?.as_slice() else {
        return None;
    };
    Some(RemoteDraft {
        id: draft["id"].as_str()?.to_string(),
        conversation_id: destination["channel_id"].as_str()?.to_string(),
        thread_ts: destination["thread_ts"].as_str().map(str::to_string),
        text: super::draft_blocks::draft_text(&draft["blocks"]),
        file_ids: draft["file_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect(),
        updated: draft["last_updated_ts"]
            .as_str()
            .and_then(|ts| ts.parse().ok())
            .unwrap_or(0.0),
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
    let video = item["mimetype"]
        .as_str()
        .is_some_and(|mime| mime.starts_with("video/"))
        || VIDEO_EXTENSIONS
            .iter()
            .any(|extension| name.to_lowercase().ends_with(&format!(".{extension}")));
    let preview = std::iter::once("thumb_video")
        .filter(|_| video)
        .chain(PREVIEW_KEYS)
        .find_map(|key| item[key].as_str().filter(|value| !value.is_empty()));
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
            .or(item["thumb_video_w"].as_u64())
            .or(item["image_width"].as_u64())
            .map(|w| w as u32),
        height: item["original_h"]
            .as_u64()
            .or(item["thumb_video_h"].as_u64())
            .or(item["image_height"].as_u64())
            .map(|h| h as u32),
        preview_pending: false,
        video,
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
const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "mov", "m4v", "webm", "mkv"];
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
    fn videos_use_their_still_frame_and_play_the_original() {
        let video = parse_attachment(&json!({
            "id": "F1",
            "name": "Rendered video.mp4",
            "mimetype": "video/mp4",
            "url_private": "https://files.slack.com/files-pri/T1-F1/rendered_video.mp4",
            "thumb_video": "https://files.slack.com/files-tmb/T1-F1/rendered_video_thumb_video.jpeg",
            "thumb_video_w": 1280,
            "thumb_video_h": 720,
        }));
        assert!(video.video);
        assert!(video.preview_url.unwrap().ends_with("thumb_video.jpeg"));
        assert!(video.full_url.unwrap().ends_with(".mp4"));
        assert_eq!((video.width, video.height), (Some(1280), Some(720)));
        let image = parse_attachment(&json!({"name": "a.png", "mimetype": "image/png"}));
        assert!(!image.video);
    }

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
    fn shared_messages_become_cards_instead_of_vanishing() {
        let message = parse_message(&json!({
            "ts": "2.0", "user": "U1", "text": "",
            "attachments": [{
                "is_msg_unfurl": true, "is_reply_unfurl": true, "is_share": true,
                "author_id": "U2", "author_name": "Mart Bakler",
                "author_icon": "https://example.test/mart.png",
                "channel_id": "C9", "ts": "1.5",
                "from_url": "https://team.slack.com/archives/C9/p1500000?thread_ts=1.0&cid=C9",
                "text": "flat <@U3> fallback",
                "message_blocks": [{"channel": "C9", "ts": "1.5", "message": {"blocks": [
                    {"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [
                        {"type": "user", "user_id": "U3"},
                        {"type": "text", "text": " Out of interest"}
                    ]}]}
                ]}}]
            }]
        }))
        .unwrap();
        assert_eq!(message.text, "", "the share is not repeated as body text");
        assert_eq!(
            message.shares,
            vec![SharedMessage {
                author: Author::User("U2".into()),
                avatar_url: Some("https://example.test/mart.png".into()),
                ts: Some("1.5".into()),
                channel_id: Some("C9".into()),
                in_thread: true,
                thread_ts: Some("1.0".into()),
                footer: None,
                text: "<@U3> Out of interest".into(),
            }]
        );
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
    fn draft_events_carry_the_composer_text_and_destination() {
        // Shapes as Slack sent them, ids changed.
        let saved = parse_live_event(&json!({
            "type": "draft_update",
            "draft_id": "Dr1",
            "draft": {
                "id": "Dr1",
                "blocks": [{"type": "rich_text", "block_id": "x", "elements": [{
                    "type": "rich_text_section",
                    "elements": [
                        {"type": "text", "text": "almost "},
                        {"type": "text", "text": "done", "style": {"bold": true}}
                    ]
                }]}],
                "destinations": [{"channel_id": "C1", "thread_ts": "17.5", "broadcast": false}],
                "file_ids": ["F1"],
                "is_deleted": false,
                "is_sent": false,
                "date_scheduled": 0,
                "last_updated_ts": "1792262272.500234"
            }
        }));
        let Some(LiveEvent::DraftSaved(draft)) = saved else {
            panic!("{saved:?}");
        };
        assert_eq!(draft.id, "Dr1");
        assert_eq!(draft.conversation_id, "C1");
        assert_eq!(draft.thread_ts.as_deref(), Some("17.5"));
        assert_eq!(draft.text, "almost *done*");
        assert_eq!(draft.file_ids, ["F1"]);
        assert!((draft.updated - 1_792_262_272.5).abs() < 0.01);

        let sent = parse_live_event(&json!({
            "type": "draft_update",
            "draft": {"id": "Dr1", "is_sent": true, "destinations": [{"channel_id": "C1"}]}
        }));
        assert!(matches!(sent, Some(LiveEvent::DraftDeleted { ref id }) if id == "Dr1"));
        let deleted = parse_live_event(&json!({"type": "draft_delete", "draft_id": "Dr2"}));
        assert!(matches!(deleted, Some(LiveEvent::DraftDeleted { ref id }) if id == "Dr2"));
    }

    #[test]
    fn emoji_frecency_comes_from_the_jumper_preference() {
        let prefs = json!({
            "frecency_jumper": json!({
                "Erocket": {"id": "Erocket", "count": 10, "visits": [1761241041152_i64, 1775058940667_i64]},
                "C123": {"id": "C123", "count": 99, "visits": [1775058940667_i64]},
            }).to_string()
        });
        let usage = parse_emoji_usage(&prefs);
        assert_eq!(usage.emoji.len(), 1, "channels are not emoji");
        assert_eq!(usage.emoji["rocket"].count, 10);
        assert_eq!(usage.emoji["rocket"].visits.len(), 2);
        assert!(parse_emoji_usage(&json!({})).emoji.is_empty());
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
        assert_eq!(
            emoji.get("ok").map(String::as_str),
            Some("alias:thumbsup"),
            "aliases of standard emoji resolve in the catalog"
        );
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

    #[test]
    fn search_matches_keep_their_highlights_thread_and_conversation() {
        let body = json!({"ok": true, "messages": {
            "matches": [
                {
                    "type": "message", "ts": "1700000000.000200", "user": "U1",
                    "text": "the \u{e000}deploy\u{e001} is done",
                    "permalink": "https://x.slack.com/archives/C1/p1700000000000200?thread_ts=1700000000.000100&cid=C1",
                    "channel": {"id": "C1", "name": "ops", "is_channel": true, "is_im": false, "is_mpim": false}
                },
                {
                    "type": "message", "ts": "1700000001.000000", "user": "U2",
                    "text": "\u{e000}deploy\u{e001}?",
                    "permalink": "https://x.slack.com/archives/D1/p1700000001000000",
                    "channel": {"id": "D1", "name": "U2", "is_channel": false, "is_im": true}
                }
            ],
            "paging": {"count": 20, "page": 2, "pages": 9, "total": 170}
        }});
        let page = parse_search(&body);
        assert_eq!((page.page, page.pages, page.total), (2, 9, 170));
        let [reply, direct] = &page.matches[..] else {
            panic!("two matches expected");
        };
        assert_eq!(reply.message.text, "the deploy is done");
        assert_eq!(reply.highlighted, "the \u{e000}deploy\u{e001} is done");
        assert_eq!(reply.thread_ts.as_deref(), Some("1700000000.000100"));
        assert_eq!(
            reply.message.thread_ts.as_deref(),
            Some("1700000000.000100")
        );
        assert!(reply.is_channel);
        assert_eq!(reply.conversation_name, "ops");
        assert_eq!(direct.thread_ts, None);
        assert!(!direct.is_channel);
    }

    #[test]
    fn a_conversation_read_past_its_latest_message_is_read() {
        let states = parse_read_states(&json!({"channels": [
            {"id": "C1", "latest": "20.000000", "last_read": "20.500000",
             "has_unreads": true, "mention_count": 1},
            {"id": "C2", "latest": "20.000000", "last_read": "10.000000",
             "has_unreads": true, "mention_count": 1}
        ]}));
        assert!(!states["C1"].has_unreads);
        assert_eq!(states["C1"].mention_count, 0);
        assert!(states["C2"].has_unreads);
        assert_eq!(states["C2"].mention_count, 1);
    }

    #[test]
    fn joining_or_leaving_a_conversation_reloads_the_list() {
        for kind in [
            "channel_joined",
            "group_joined",
            "channel_left",
            "im_created",
        ] {
            assert_eq!(
                parse_live_event(&json!({"type": kind, "channel": {"id": "C1"}})),
                Some(LiveEvent::ConversationsChanged),
                "{kind}"
            );
        }
    }

    #[test]
    fn huddle_events_ring_track_participants_and_stop() {
        let room = json!({
            "id": "R1", "call_family": "huddle", "channels": ["D1"],
            "participants": ["U1"], "participant_history": ["U1"],
            "huddle_link": "https://app.slack.com/huddle/T1/D1",
            "has_ended": false, "date_start": 1000, "date_end": 0
        });
        let joined = parse_live_event(&json!({
            "type": "sh_room_join", "user": "U1", "huddle": {"channel_id": "D1"}, "room": room
        }));
        let Some(LiveEvent::HuddleChanged(huddle)) = joined else {
            panic!("a huddle change: {joined:?}");
        };
        assert_eq!(huddle.conversation_id, "D1");
        assert_eq!(huddle.participants, ["U1"]);
        assert!(huddle.is_live());
        assert_eq!(
            parse_live_event(&json!({
                "type": "huddle_invite", "call_id": "R1", "channel_id": "D1",
                "sender_user_id": "U1", "team_id": "T1", "free_willy": {}
            })),
            Some(LiveEvent::HuddleRinging {
                conversation_id: "D1".into(),
                caller_id: "U1".into(),
                call_id: "R1".into(),
            })
        );
        assert_eq!(
            parse_live_event(
                &json!({"type": "huddle_invite_cancel", "channel_id": "D1", "team_id": "T1"})
            ),
            Some(LiveEvent::HuddleRingStopped {
                conversation_id: "D1".into()
            })
        );
        let message = parse_message(&json!({
            "type": "message", "subtype": "huddle_thread", "user": "USLACKBOT",
            "ts": "1000.000100", "channel": "D1", "text": "", "room": room
        }))
        .unwrap();
        assert_eq!(message.huddle.unwrap().id, "R1");
    }
}
