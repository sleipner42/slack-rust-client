use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivityFilter {
    #[default]
    All,
    Dms,
    Mentions,
    Threads,
    Reactions,
}

impl ActivityFilter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Dms,
        Self::Mentions,
        Self::Threads,
        Self::Reactions,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Dms => "DMs",
            Self::Mentions => "Mentions",
            Self::Threads => "Threads",
            Self::Reactions => "Reactions",
        }
    }

    pub fn types(self) -> &'static str {
        match self {
            Self::All => {
                "at_user,at_user_group,at_channel,at_everyone,keyword,unjoined_channel_mention,thread_v2,message_reaction,bot_dm_bundle,dm,channel"
            }
            Self::Dms => "dm,bot_dm_bundle",
            Self::Mentions => {
                "at_user,at_user_group,at_channel,at_everyone,keyword,unjoined_channel_mention"
            }
            Self::Threads => "thread_v2",
            Self::Reactions => "message_reaction",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityQuery {
    pub filter: ActivityFilter,
    pub unread_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityItem {
    pub key: String,
    pub kind: String,
    pub feed_ts: String,
    pub unread: bool,
    pub unread_count: u32,
    pub conversation_id: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    pub actor_id: Option<String>,
    pub reaction: Option<String>,
}

impl ActivityItem {
    pub fn label(&self) -> &'static str {
        match self.kind.as_str() {
            "dm" | "bot_dm_bundle" => "Direct message",
            "thread_v2" => "Thread reply",
            "message_reaction" => "Reaction",
            "channel" => "Channel update",
            _ => "Mention",
        }
    }
}

pub struct ActivityPage {
    pub items: Vec<ActivityItem>,
    pub next_cursor: String,
}
