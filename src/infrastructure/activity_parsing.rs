use serde_json::Value;

use crate::domain::activity::{ActivityItem, ActivityPage};
use crate::domain::model::SlackError;

pub fn parse_activity(body: &Value) -> Result<ActivityPage, SlackError> {
    let items = body["items"]
        .as_array()
        .ok_or_else(|| SlackError("Slack returned an unrecognized Activity feed.".into()))?;
    Ok(ActivityPage {
        items: items.iter().filter_map(parse_item).collect(),
        next_cursor: super::slack_parsing::next_cursor(body),
    })
}

fn parse_item(raw: &Value) -> Option<ActivityItem> {
    let item = &raw["item"];
    let kind = item["type"].as_str()?;
    let bundle = &item["bundle_info"];
    let payload = &bundle["payload"];
    let reference = match kind {
        "thread_v2" => &payload["thread_entry"],
        "dm" => &payload["dm_entry"]["latest_message"],
        "channel" => &payload["channel_entry"]["latest_message"],
        "bot_dm_bundle" => &payload["message"],
        _ => &item["message"],
    };
    let conversation_id = reference["channel"]
        .as_str()
        .or(reference["channel_id"].as_str())?;
    let ts = reference["ts"]
        .as_str()
        .or(reference["latest_ts"].as_str())?;
    if conversation_id.is_empty() || ts.is_empty() {
        return None;
    }
    let reaction = item["reaction"]["name"].as_str().map(str::to_owned);
    let actor_id = item["reaction"]["user"]
        .as_str()
        .or(reference["author_user_id"].as_str())
        .map(str::to_owned);
    let feed_ts = raw["feed_ts"].as_str().unwrap_or(ts).to_owned();
    let key = raw["key"].as_str().map(str::to_owned).unwrap_or_else(|| {
        format!(
            "{kind}:{conversation_id}:{ts}:{}:{}",
            actor_id.as_deref().unwrap_or_default(),
            reaction.as_deref().unwrap_or_default()
        )
    });
    Some(ActivityItem {
        key,
        kind: kind.into(),
        feed_ts,
        unread: raw["is_unread"].as_bool().unwrap_or(false),
        unread_count: bundle["unread_count"]
            .as_u64()
            .or(reference["unread_msg_count"].as_u64())
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        conversation_id: conversation_id.into(),
        ts: ts.into(),
        thread_ts: reference["thread_ts"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        actor_id,
        reaction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_preserves_thread_targets_reaction_actors_and_server_unreads() {
        let page = parse_activity(&json!({"items": [
            {"key":"mention", "feed_ts":"12.0", "is_unread":true, "item":{"type":"at_user", "message":{"channel":"C1", "ts":"12.0", "thread_ts":"10.0", "author_user_id":"U1"}}},
            {"key":"thread", "is_unread":true, "item":{"type":"thread_v2", "bundle_info":{"payload":{"thread_entry":{"channel_id":"C2", "thread_ts":"8.0", "latest_ts":"13.0", "unread_msg_count":3}}}}},
            {"item":{"type":"dm", "bundle_info":{"payload":{"dm_entry":{"latest_message":{"channel":"D1", "ts":"14.0"}}}}}},
            {"item":{"type":"message_reaction", "message":{"channel":"C1", "ts":"12.0", "author_user_id":"ME"}, "reaction":{"user":"U2", "name":"+1"}}},
            {"item":false}, {"item":{"type":"future_type"}}
        ], "response_metadata":{"next_cursor":"next"}})).unwrap();
        assert_eq!(page.items.len(), 4);
        assert_eq!(page.next_cursor, "next");
        assert_eq!(page.items[0].thread_ts.as_deref(), Some("10.0"));
        assert_eq!(page.items[1].ts, "13.0");
        assert_eq!(page.items[1].thread_ts.as_deref(), Some("8.0"));
        assert_eq!(page.items[1].unread_count, 3);
        assert_eq!(page.items[2].conversation_id, "D1");
        assert!(!page.items[2].unread);
        assert_eq!(page.items[3].actor_id.as_deref(), Some("U2"));
        assert_eq!(page.items[3].reaction.as_deref(), Some("+1"));
        assert!(parse_activity(&json!({"ok":true})).is_err());
    }
}
