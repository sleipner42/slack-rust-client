use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::emoji_catalog::EmojiCatalog;
use crate::domain::gateway::SlackGateway;
use crate::domain::message_text::{Block, NameLookup, parse_message_text};
use crate::domain::model::{
    Attachment, Author, Conversation, ConversationKind, Message, Reaction, ReadState,
    SidebarSection, SlackError, UserProfile,
};
use crate::domain::sidebar::{SidebarGroup, SidebarInput, build_sidebar};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisplayMessage {
    #[serde(default)]
    pub client_msg_id: Option<String>,
    #[serde(default)]
    pub layout_key: u64,
    pub ts: String,
    pub author_key: String,
    pub author: String,
    pub avatar_url: Option<String>,
    pub blocks: Vec<Block>,
    pub thread_ts: Option<String>,
    pub reply_count: u32,
    pub reactions: Vec<Reaction>,
    pub attachments: Vec<Attachment>,
    pub edited: bool,
}

impl DisplayMessage {
    fn update_layout_key(&mut self) {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.layout_key = 0;
        format!("{self:?}").hash(&mut hash);
        self.layout_key = hash.finish();
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub self_user_id: String,
    pub conversations: Vec<Conversation>,
    pub sidebar: Vec<SidebarGroup>,
    pub sections: Vec<SidebarSection>,
    pub read_states: HashMap<String, ReadState>,
    pub muted: HashSet<String>,
    pub emoji: EmojiCatalog,
}

pub struct ChatService {
    cache: Option<super::cache::DiskCache>,
    gateway: Arc<dyn SlackGateway>,
    self_user_id: Mutex<String>,
    profiles: Mutex<HashMap<String, UserProfile>>,
    conversation_names: Mutex<HashMap<String, String>>,
}

impl ChatService {
    pub fn mark_activity_read(
        &self,
        item: &crate::domain::activity::ActivityItem,
    ) -> Result<(), SlackError> {
        self.gateway.mark_activity_read(item)
    }
    pub fn cached_activity(
        &self,
        query: crate::domain::activity::ActivityQuery,
    ) -> Option<super::activity::ActivityPage> {
        let bytes = self
            .cache
            .as_ref()?
            .read("activity", &format!("{query:?}"))?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn activity(
        &self,
        query: crate::domain::activity::ActivityQuery,
        cursor: &str,
        preview: impl FnOnce(super::activity::ActivityPage),
    ) -> Result<super::activity::ActivityPage, SlackError> {
        use super::activity::{ActivityPage, DisplayActivity};
        let page = self.gateway.activity(query, cursor)?;
        let cached = self.cached_activity(query).unwrap_or_default();
        let mut rows = ActivityPage {
            items: page
                .items
                .iter()
                .map(|item| {
                    let old = cached
                        .items
                        .iter()
                        .find(|row| row.item.key == item.key && row.item.ts == item.ts);
                    let name = self.conversation_name(&item.conversation_id);
                    DisplayActivity {
                        item: item.clone(),
                        message: old.and_then(|row| row.message.clone()),
                        actor: old
                            .map(|row| row.actor.clone())
                            .unwrap_or_else(|| item.label().into()),
                        avatar_url: old.and_then(|row| row.avatar_url.clone()),
                        source: if item.conversation_id.starts_with('D') {
                            name
                        } else {
                            format!("#{name}")
                        },
                        preview: old
                            .map(|row| row.preview.clone())
                            .unwrap_or_else(|| "Loading message…".into()),
                    }
                })
                .collect(),
            next_cursor: page.next_cursor,
            warning: None,
        };
        preview(rows.clone());
        match self.gateway.activity_messages(&page.items) {
            Ok(messages) => {
                let raw: Vec<_> = messages
                    .values()
                    .map(|saved| saved.message.clone())
                    .collect();
                let mut ids = referenced_users(&raw);
                ids.extend(page.items.iter().filter_map(|item| item.actor_id.clone()));
                self.prefetch_profiles(&ids);
                for row in &mut rows.items {
                    if let Some(saved) =
                        messages.get(&(row.item.conversation_id.clone(), row.item.ts.clone()))
                    {
                        let mut display = self
                            .display_cached_all(std::slice::from_ref(&saved.message))
                            .remove(0);
                        display.avatar_url = saved.avatar_url.clone().or(display.avatar_url);
                        let (title, rest) = super::todos::preview_lines(&display);
                        row.preview = format!("{title} {rest}").trim().to_owned();
                        row.actor = display.author.clone();
                        row.avatar_url = display.avatar_url.clone();
                        row.item.thread_ts =
                            row.item.thread_ts.clone().or(display.thread_ts.clone());
                        row.message = Some(display);
                    } else if row.message.is_none() {
                        row.preview = "Message unavailable · open to retry".into();
                    }
                    if let Some(id) = &row.item.actor_id
                        && let Some(profile) = lock(&self.profiles).get(id)
                    {
                        row.actor = profile.name.clone();
                        row.avatar_url = if row.item.reaction.is_some() {
                            profile.avatar_url.clone()
                        } else {
                            profile.avatar_url.clone().or(row.avatar_url.take())
                        };
                    }
                    if let Some(reaction) = &row.item.reaction {
                        let emoji = emojis::get_by_shortcode(reaction)
                            .map(|e| e.as_str())
                            .unwrap_or(reaction);
                        let text = row
                            .message
                            .as_ref()
                            .map(|message| {
                                let (title, rest) = super::todos::preview_lines(message);
                                format!("{title} {rest}").trim().to_owned()
                            })
                            .unwrap_or_else(|| "Message unavailable".into());
                        row.preview = format!("{emoji}  Reacted to: {text}");
                    }
                }
            }
            Err(error) => rows.warning = Some(format!("Could not load message previews: {error}")),
        }
        if cursor.is_empty()
            && let (Some(cache), Ok(bytes)) = (&self.cache, serde_json::to_vec(&rows))
        {
            cache.write("activity", &format!("{query:?}"), &bytes);
        }
        Ok(rows)
    }

    pub fn complete_todo(&self, item: &crate::domain::model::SavedTodo) -> Result<(), SlackError> {
        self.gateway.complete_todo(item)
    }

    pub fn todos(
        &self,
        completed: bool,
        cursor: &str,
    ) -> Result<super::todos::TodoPage, SlackError> {
        let page = self.gateway.saved_todos(completed, cursor)?;
        let mut items = Vec::with_capacity(page.items.len());
        // A small bounded batch keeps HTTP/profile work off the UI thread without
        // flooding Slack when the completed list is opened.
        for batch in page.items.chunks(4) {
            thread::scope(|scope| {
                let jobs: Vec<_> = batch
                    .iter()
                    .map(|item| {
                        scope.spawn(move || {
                            let result = self.gateway.saved_message(item);
                            let (message, error) = match result {
                                Ok(Some(saved)) => {
                                    let mut message = self.display(&saved.message);
                                    message.avatar_url = saved.avatar_url.or(message.avatar_url);
                                    (Some(message), None)
                                }
                                Ok(None) => (
                                    None,
                                    Some(
                                        "This saved item's message is no longer available.".into(),
                                    ),
                                ),
                                Err(error) => (None, Some(error.to_string())),
                            };
                            let (title, preview) = message
                                .as_ref()
                                .map(super::todos::preview_lines)
                                .unwrap_or_else(|| {
                                    (
                                        "Message unavailable".into(),
                                        "Open the source in Slack to view this item.".into(),
                                    )
                                });
                            let name = self.conversation_name(&item.conversation_id);
                            let channel = if item.conversation_id.starts_with('D') {
                                name
                            } else {
                                format!("#{name}")
                            };
                            let source = message
                                .as_ref()
                                .map(|m| format!("{} in {channel}", m.author))
                                .unwrap_or(channel);
                            let source_url = self.gateway.workspace_namespace().map(|workspace| {
                                format!(
                                    "{workspace}/archives/{}/p{}",
                                    item.conversation_id,
                                    item.ts.replace('.', "")
                                )
                            });
                            super::todos::DisplayTodo {
                                item: item.clone(),
                                message,
                                title,
                                preview,
                                source,
                                source_url,
                                error,
                            }
                        })
                    })
                    .collect();
                for job in jobs {
                    items.push(job.join().expect("to-do worker panicked"));
                }
            });
        }
        Ok(super::todos::TodoPage {
            items,
            open_count: page.open_count,
            completed_count: page.completed_count,
            next_cursor: page.next_cursor,
        })
    }

    pub fn new(gateway: Arc<dyn SlackGateway>) -> Self {
        Self {
            cache: gateway
                .cache_namespace()
                .as_deref()
                .and_then(super::cache::DiskCache::for_account),
            gateway,
            self_user_id: Mutex::new(String::new()),
            profiles: Mutex::new(HashMap::new()),
            conversation_names: Mutex::new(HashMap::new()),
        }
    }

    pub fn load_workspace(&self) -> Result<Workspace, SlackError> {
        let self_user_id = self.gateway.current_user_id()?;
        *lock(&self.self_user_id) = self_user_id.clone();
        self.prefetch_profiles(std::slice::from_ref(&self_user_id));
        let conversations = self.with_resolved_names(self.gateway.list_conversations()?);
        lock(&self.conversation_names).extend(
            conversations
                .iter()
                .map(|conversation| (conversation.id.clone(), conversation.name.clone())),
        );
        let sections = self.gateway.list_sections().unwrap_or_default();
        let read_states = self.gateway.read_states().unwrap_or_default();
        let muted = self.gateway.muted_conversations().unwrap_or_default();
        let emoji = EmojiCatalog::new(self.gateway.custom_emoji().unwrap_or_default());
        let sidebar = build_sidebar(&SidebarInput {
            sections: &sections,
            conversations: &conversations,
            read_states: &read_states,
            direct_cutoff_seconds: now_seconds() - DIRECT_MESSAGE_WINDOW_SECONDS,
        });
        Ok(Workspace {
            self_user_id,
            conversations,
            sidebar,
            sections,
            read_states,
            muted,
            emoji,
        })
    }

    #[cfg(test)]
    pub fn messages(&self, conversation_id: &str) -> Result<Vec<DisplayMessage>, SlackError> {
        self.messages_with_preview(conversation_id, |_| {})
    }

    /// Publish the entire history page before optional profile/file HTTP requests.
    /// This runs on a worker; the callback must not wait for the UI thread.
    pub fn messages_with_preview(
        &self,
        conversation_id: &str,
        preview: impl FnOnce(Vec<DisplayMessage>),
    ) -> Result<Vec<DisplayMessage>, SlackError> {
        let mut messages = self.gateway.fetch_messages(conversation_id)?;
        let user_ids = referenced_users(&messages);
        self.restore_profiles(&user_ids);
        let mut initial = self.display_cached_all(&messages);
        for attachment in initial.iter_mut().flat_map(|m| &mut m.attachments) {
            attachment.preview_pending = attachment.file_id.is_some()
                && attachment.preview_url.is_none()
                && (attachment.name == "image"
                    || [".png", ".jpg", ".jpeg", ".gif", ".webp"]
                        .iter()
                        .any(|suffix| attachment.name.to_lowercase().ends_with(suffix)));
        }
        initial
            .iter_mut()
            .for_each(DisplayMessage::update_layout_key);
        preview(initial);
        thread::scope(|scope| {
            scope.spawn(|| self.prefetch_profiles(&user_ids));
            self.gateway.enrich_messages(&mut messages);
        });
        let messages = self.display_cached_all(&messages);
        self.cache_messages(conversation_id, None, &messages);
        Ok(messages)
    }

    pub fn replies(
        &self,
        conversation_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<DisplayMessage>, SlackError> {
        let messages = self.gateway.fetch_replies(conversation_id, thread_ts)?;
        let messages = self.display_all(&messages);
        self.cache_messages(conversation_id, Some(thread_ts), &messages);
        Ok(messages)
    }

    pub fn display(&self, message: &Message) -> DisplayMessage {
        let profile = match &message.author {
            Author::User(id) => Some(self.profile(id)),
            Author::Named(_) => None,
        };
        display_message(message, profile, self)
    }

    pub fn send(
        &self,
        conversation_id: &str,
        text: &str,
        thread_ts: Option<&str>,
        client_msg_id: &str,
    ) -> Result<String, SlackError> {
        self.gateway
            .post_message(conversation_id, text, thread_ts, client_msg_id)
    }

    /// Build the immediate local row using cached names only: no UI-thread I/O.
    pub fn outgoing_message(
        &self,
        text: &str,
        thread_ts: Option<&str>,
        id: &str,
    ) -> DisplayMessage {
        let author_key = lock(&self.self_user_id).clone();
        let profile = lock(&self.profiles).get(&author_key).cloned();
        DisplayMessage {
            client_msg_id: Some(id.into()),
            layout_key: 0,
            ts: format!("{:.6}", now_seconds()),
            author_key,
            author: profile
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "You".into()),
            avatar_url: profile.and_then(|p| p.avatar_url),
            blocks: parse_message_text(text, &CachedNames(self)),
            thread_ts: thread_ts.map(str::to_owned),
            reply_count: 0,
            reactions: vec![],
            attachments: vec![],
            edited: false,
        }
    }

    pub fn mark_read(&self, conversation_id: &str, ts: &str) -> Result<(), SlackError> {
        self.gateway.mark_read(conversation_id, ts)
    }

    pub fn read_states(&self) -> Result<HashMap<String, ReadState>, SlackError> {
        self.gateway.read_states()
    }

    pub fn set_reaction(
        &self,
        conversation_id: &str,
        ts: &str,
        name: &str,
        add: bool,
    ) -> Result<(), SlackError> {
        self.gateway.set_reaction(conversation_id, ts, name, add)
    }

    pub fn download(&self, url: &str) -> Result<Vec<u8>, SlackError> {
        if let Some(bytes) = self
            .cache
            .as_ref()
            .and_then(|cache| cache.read("image", url))
        {
            return Ok(bytes);
        }
        let bytes = self.gateway.download(url)?;
        if let Some(cache) = &self.cache {
            cache.write("image", url, &bytes);
        }
        Ok(bytes)
    }

    pub fn cached_messages(
        &self,
        conversation_id: &str,
        thread_ts: Option<&str>,
    ) -> Option<Vec<DisplayMessage>> {
        let bytes = self.cache.as_ref()?.read(
            "history",
            &format!("{conversation_id}:{}", thread_ts.unwrap_or_default()),
        )?;
        serde_json::from_slice(&bytes).ok()
    }

    fn cache_messages(
        &self,
        conversation_id: &str,
        thread_ts: Option<&str>,
        messages: &[DisplayMessage],
    ) {
        if let (Some(cache), Ok(bytes)) = (&self.cache, serde_json::to_vec(messages)) {
            cache.write(
                "history",
                &format!("{conversation_id}:{}", thread_ts.unwrap_or_default()),
                &bytes,
            );
        }
    }

    fn display_all(&self, messages: &[Message]) -> Vec<DisplayMessage> {
        self.prefetch_profiles(&referenced_users(messages));
        self.display_cached_all(messages)
    }

    fn display_cached_all(&self, messages: &[Message]) -> Vec<DisplayMessage> {
        messages
            .iter()
            .map(|message| {
                let profile = match &message.author {
                    Author::User(id) => lock(&self.profiles).get(id).cloned(),
                    Author::Named(_) => None,
                };
                display_message(message, profile, &CachedNames(self))
            })
            .collect()
    }

    fn restore_profiles(&self, user_ids: &[String]) {
        let Some(cache) = &self.cache else { return };
        for id in user_ids {
            if lock(&self.profiles).contains_key(id) {
                continue;
            }
            if let Some(profile) = cache
                .read("profile", id)
                .and_then(|bytes| serde_json::from_slice::<UserProfile>(&bytes).ok())
                .filter(|profile| profile.id == *id)
            {
                lock(&self.profiles).entry(id.clone()).or_insert(profile);
            }
        }
    }

    fn with_resolved_names(&self, conversations: Vec<Conversation>) -> Vec<Conversation> {
        let user_ids: Vec<String> = conversations
            .iter()
            .filter_map(|conversation| match &conversation.kind {
                ConversationKind::DirectMessage { user_id } => Some(user_id.clone()),
                _ => None,
            })
            .collect();
        self.prefetch_profiles(&user_ids);
        conversations
            .into_iter()
            .map(|conversation| match &conversation.kind {
                ConversationKind::DirectMessage { user_id } => Conversation {
                    name: self.profile(user_id).name,
                    ..conversation
                },
                _ => conversation,
            })
            .collect()
    }

    fn prefetch_profiles(&self, user_ids: &[String]) {
        self.restore_profiles(user_ids);
        let missing: Vec<&String> = {
            let cache = lock(&self.profiles);
            let mut seen = HashSet::new();
            user_ids
                .iter()
                .filter(|id| !cache.contains_key(*id) && seen.insert(*id))
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let chunk_size = missing.len().div_ceil(PROFILE_LOOKUP_WORKERS).max(1);
        thread::scope(|scope| {
            for chunk in missing.chunks(chunk_size) {
                scope.spawn(move || {
                    for user_id in chunk {
                        self.profile(user_id);
                    }
                });
            }
        });
    }

    fn profile(&self, user_id: &str) -> UserProfile {
        if let Some(profile) = lock(&self.profiles).get(user_id) {
            return profile.clone();
        }
        let result = self.gateway.user_profile(user_id);
        if let (Some(cache), Ok(profile)) = (&self.cache, &result)
            && let Ok(bytes) = serde_json::to_vec(profile)
        {
            cache.write("profile", user_id, &bytes);
        }
        let profile = result.unwrap_or_else(|_| UserProfile {
            id: user_id.to_string(),
            name: user_id.to_string(),
            avatar_url: None,
        });
        lock(&self.profiles).insert(user_id.to_string(), profile.clone());
        profile
    }
}

struct CachedNames<'a>(&'a ChatService);

impl NameLookup for CachedNames<'_> {
    fn user_name(&self, id: &str) -> String {
        lock(&self.0.profiles)
            .get(id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| id.into())
    }
    fn conversation_name(&self, id: &str) -> String {
        self.0.conversation_name(id)
    }
    fn is_self(&self, id: &str) -> bool {
        self.0.is_self(id)
    }
}

fn referenced_users(messages: &[Message]) -> Vec<String> {
    // Reuse the parser so escaped mentions and code do not trigger HTTP lookups.
    struct Users(std::cell::RefCell<HashSet<String>>);
    impl NameLookup for Users {
        fn user_name(&self, id: &str) -> String {
            self.0.borrow_mut().insert(id.into());
            id.into()
        }
        fn conversation_name(&self, id: &str) -> String {
            id.into()
        }
        fn is_self(&self, _id: &str) -> bool {
            false
        }
    }
    let users = Users(Default::default());
    for message in messages {
        if let Author::User(id) = &message.author {
            users.0.borrow_mut().insert(id.clone());
        }
        parse_message_text(&message.text, &users);
    }
    users.0.into_inner().into_iter().collect()
}

fn display_message(
    message: &Message,
    profile: Option<UserProfile>,
    names: &dyn NameLookup,
) -> DisplayMessage {
    let (author_key, author, avatar_url) = match (&message.author, profile) {
        (Author::Named(name), _) => (name.clone(), name.clone(), None),
        (Author::User(id), profile) => (
            id.clone(),
            profile
                .as_ref()
                .map_or_else(|| id.clone(), |p| p.name.clone()),
            profile.and_then(|p| p.avatar_url),
        ),
    };
    let mut display = DisplayMessage {
        client_msg_id: message.client_msg_id.clone(),
        layout_key: 0,
        ts: message.ts.clone(),
        author_key,
        author,
        avatar_url,
        blocks: parse_message_text(&message.text, names),
        thread_ts: message.thread_ts.clone(),
        reply_count: message.reply_count,
        reactions: message.reactions.clone(),
        attachments: message.attachments.clone(),
        edited: message.edited,
    };
    // Resolved mention names can wrap differently even if the raw text is unchanged.
    display.update_layout_key();
    display
}

impl NameLookup for ChatService {
    fn user_name(&self, user_id: &str) -> String {
        self.profile(user_id).name
    }

    fn conversation_name(&self, conversation_id: &str) -> String {
        lock(&self.conversation_names)
            .get(conversation_id)
            .cloned()
            .unwrap_or_else(|| conversation_id.to_string())
    }

    fn is_self(&self, user_id: &str) -> bool {
        *lock(&self.self_user_id) == user_id
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

const PROFILE_LOOKUP_WORKERS: usize = 8;
const DIRECT_MESSAGE_WINDOW_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::demo::DemoGateway;
    use std::sync::{Condvar, mpsc};
    use std::time::Duration;

    struct SlowMetadata {
        gate: (Mutex<bool>, Condvar),
        profile_calls: Mutex<Vec<String>>,
    }

    impl SlowMetadata {
        fn wait(&self) {
            let mut ready = lock(&self.gate.0);
            while !*ready {
                ready = self.gate.1.wait(ready).unwrap();
            }
        }
    }

    impl SlackGateway for SlowMetadata {
        fn fetch_messages(&self, _id: &str) -> Result<Vec<Message>, SlackError> {
            Ok((0..100).map(|index| crate::infrastructure::slack_parsing::parse_message(
                &serde_json::json!({
                    "ts": format!("{}.0", 1000 + index), "user": "UAUTHOR",
                    "text": "Hello <@UMENTION> and <@UMENTION>! `code <@UCODE>`",
                    "files": [{"id": "F1", "name": "image.png", "original_w": 640, "original_h": 240}]
                })
            ).unwrap()).collect())
        }
        fn enrich_messages(&self, messages: &mut [Message]) {
            self.wait();
            for message in messages {
                message.attachments[0].preview_url = Some("demo://preview.png".into());
            }
        }
        fn user_profile(&self, id: &str) -> Result<UserProfile, SlackError> {
            self.wait();
            lock(&self.profile_calls).push(id.into());
            Ok(UserProfile {
                id: id.into(),
                name: format!("Name of {id}"),
                avatar_url: None,
            })
        }
        fn current_user_id(&self) -> Result<String, SlackError> {
            unreachable!()
        }
        fn list_conversations(&self) -> Result<Vec<Conversation>, SlackError> {
            unreachable!()
        }
        fn list_sections(&self) -> Result<Vec<SidebarSection>, SlackError> {
            unreachable!()
        }
        fn read_states(&self) -> Result<HashMap<String, ReadState>, SlackError> {
            unreachable!()
        }
        fn muted_conversations(&self) -> Result<HashSet<String>, SlackError> {
            unreachable!()
        }
        fn custom_emoji(&self) -> Result<HashMap<String, String>, SlackError> {
            unreachable!()
        }
        fn fetch_replies(&self, _: &str, _: &str) -> Result<Vec<Message>, SlackError> {
            unreachable!()
        }
        fn post_message(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: &str,
        ) -> Result<String, SlackError> {
            unreachable!()
        }
        fn mark_read(&self, _: &str, _: &str) -> Result<(), SlackError> {
            unreachable!()
        }
        fn set_reaction(&self, _: &str, _: &str, _: &str, _: bool) -> Result<(), SlackError> {
            unreachable!()
        }
        fn download(&self, _: &str) -> Result<Vec<u8>, SlackError> {
            unreachable!()
        }
    }

    #[test]
    fn cold_history_is_visible_before_slow_metadata_and_profiles_survive_restart() {
        let root =
            std::env::temp_dir().join(format!("slack-profiles-test-{}", uuid::Uuid::new_v4()));
        let gateway = Arc::new(SlowMetadata {
            gate: (Mutex::new(false), Condvar::new()),
            profile_calls: Mutex::new(vec![]),
        });
        let mut service = ChatService::new(gateway.clone());
        service.cache = Some(super::super::cache::DiskCache::at(root.clone()));
        let (initial_tx, initial_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = service.messages_with_preview("C1", |messages| {
                initial_tx.send(messages).unwrap();
            });
            finished_tx.send(result).unwrap();
        });
        // No timing claim: metadata is deliberately blocked until after preview delivery.
        let initial = initial_rx.recv_timeout(Duration::from_secs(5));
        let waiting = matches!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        let calls_before_preview = lock(&gateway.profile_calls).len();
        *lock(&gateway.gate.0) = true;
        gateway.gate.1.notify_all();
        let final_messages = finished_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        let initial = initial.unwrap();
        assert!(waiting);
        assert_eq!(calls_before_preview, 0);
        assert_eq!(initial.len(), 100);
        assert_eq!(initial[0].author, "UAUTHOR");
        assert!(initial[0].attachments[0].preview_pending);
        assert_eq!(final_messages.len(), initial.len());
        assert_eq!(final_messages[0].author, "Name of UAUTHOR");
        assert!(!final_messages[0].attachments[0].preview_pending);
        assert!(final_messages[0].attachments[0].preview_url.is_some());
        assert_ne!(initial[0].layout_key, final_messages[0].layout_key);
        let text = crate::domain::message_text::plain_text(&final_messages[0].blocks);
        assert!(text.contains("Name of UMENTION"));
        let calls = lock(&gateway.profile_calls).clone();
        assert_eq!(
            calls.len(),
            2,
            "deduplicate authors/mentions and ignore mentions in code"
        );

        let mut restarted = ChatService::new(gateway.clone());
        restarted.cache = Some(super::super::cache::DiskCache::at(root.clone()));
        // A different uncached channel reuses the persisted names immediately.
        restarted
            .messages_with_preview("C2", |preview| {
                assert_eq!(preview[0].author, final_messages[0].author);
                assert_eq!(preview[0].blocks, final_messages[0].blocks);
            })
            .unwrap();
        assert_eq!(lock(&gateway.profile_calls).len(), 2);
        assert_eq!(
            restarted.cached_messages("C1", None).unwrap(),
            final_messages
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_messages_threads_and_reactions_are_wired_end_to_end() {
        let service = ChatService::new(Arc::new(DemoGateway::new()));
        let workspace = service.load_workspace().unwrap();
        assert!(workspace.muted.contains("C3"));
        assert!(workspace.conversations.iter().any(|c| c.id == "D2"));
        assert!(
            !workspace
                .sidebar
                .iter()
                .flat_map(|g| &g.conversations)
                .any(|c| c.id == "D2")
        );
        let messages = service.messages("C1").unwrap();
        assert!(
            messages
                .iter()
                .any(|message| !message.attachments.is_empty())
        );
        assert!(messages.iter().all(|message| message.author != "U1"));
        let root = messages
            .iter()
            .find(|message| message.reply_count == 2)
            .unwrap();
        service
            .send(
                "C1",
                "a threaded reply\nwith two lines",
                Some(&root.ts),
                "test-thread",
            )
            .unwrap();
        let replies = service.replies("C1", &root.ts).unwrap();
        assert_eq!(replies.len(), 4);
        assert_eq!(replies.last().unwrap().thread_ts.as_ref(), Some(&root.ts));
        assert_eq!(service.messages("C1").unwrap().len(), messages.len());
        service
            .set_reaction("C1", &root.ts, "rocket", true)
            .unwrap();
        service
            .set_reaction("C1", &root.ts, "rocket", true)
            .unwrap();
        let replies = service.replies("C1", &root.ts).unwrap();
        assert_eq!(
            replies[0]
                .reactions
                .iter()
                .find(|r| r.name == "rocket")
                .unwrap()
                .count,
            1
        );
        service
            .set_reaction("C1", &root.ts, "rocket", false)
            .unwrap();
        assert!(
            !service.replies("C1", &root.ts).unwrap()[0]
                .reactions
                .iter()
                .any(|r| r.name == "rocket")
        );
        let image = service.download("demo://preview.png").unwrap();
        assert!(image::load_from_memory(&image).is_ok());
    }
}
