#[derive(Clone, Debug)]
pub struct Credentials {
    pub token: String,
    pub cookie: String,
    pub workspace_url: String,
    pub team_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConversationKind {
    PublicChannel,
    PrivateChannel,
    DirectMessage { user_id: String },
    GroupMessage,
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub kind: ConversationKind,
}

impl Conversation {
    pub fn is_direct(&self) -> bool {
        matches!(
            self.kind,
            ConversationKind::DirectMessage { .. } | ConversationKind::GroupMessage
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SectionKind {
    Channels,
    DirectMessages,
    Starred,
    Custom,
}

#[derive(Clone, Debug)]
pub struct SidebarSection {
    pub kind: SectionKind,
    pub name: String,
    pub conversation_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadState {
    pub latest_ts: Option<String>,
    pub last_read_ts: Option<String>,
    pub has_unreads: bool,
    pub mention_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Author {
    User(String),
    Named(String),
    Bot {
        id: String,
        name: Option<String>,
        avatar_url: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Reaction {
    pub name: String,
    pub count: u32,
    pub user_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub file_id: Option<String>,
    pub name: String,
    pub preview_url: Option<String>,
    pub full_url: Option<String>,
    pub permalink: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(default)]
    pub preview_pending: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub client_msg_id: Option<String>,
    pub ts: String,
    pub author: Author,
    pub text: String,
    pub thread_ts: Option<String>,
    pub reply_count: u32,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<Attachment>,
    pub shares: Vec<SharedMessage>,
    pub edited: bool,
}

/// A message shared into another one, or unfurled from a pasted message link.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedMessage {
    pub author: Author,
    pub avatar_url: Option<String>,
    pub ts: Option<String>,
    pub channel_id: Option<String>,
    pub in_thread: bool,
    /// Root of the thread the original belongs to, if any.
    pub thread_ts: Option<String>,
    /// Slack's own "Thread in #channel" style caption, used when the channel is unknown.
    pub footer: Option<String>,
    pub text: String,
}

impl Message {
    pub fn is_thread_reply(&self) -> bool {
        self.thread_ts
            .as_ref()
            .is_some_and(|thread_ts| thread_ts != &self.ts)
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedTodo {
    pub conversation_id: String,
    pub item_type: String,
    pub ts: String,
    pub created_at: i64,
    pub completed_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct SavedTodoPage {
    pub items: Vec<SavedTodo>,
    pub open_count: usize,
    pub completed_count: usize,
    pub next_cursor: String,
}

pub struct SavedMessage {
    pub message: Message,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    Connected,
    MessagePosted {
        conversation_id: String,
        message: Message,
        is_broadcast: bool,
    },
    MessageChanged {
        conversation_id: String,
        message: Message,
    },
    MessageDeleted {
        conversation_id: String,
        ts: String,
    },
    ConversationMarked {
        conversation_id: String,
        ts: String,
    },
    ReactionChanged {
        conversation_id: String,
        ts: String,
        name: String,
        user_id: String,
        added: bool,
    },
}

#[derive(Clone, Debug)]
pub struct SlackError(pub String);

impl std::fmt::Display for SlackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn ts_seconds(ts: &str) -> f64 {
    ts.parse().unwrap_or(0.0)
}
