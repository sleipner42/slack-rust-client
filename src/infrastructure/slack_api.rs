use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{AUTHORIZATION, COOKIE, RETRY_AFTER};
use serde_json::Value;

use crate::domain::gateway::SlackGateway;
use crate::domain::model::{
    Attachment, Author, ChimeJoin, Conversation, Credentials, Message, ReadState, RemoteDraft,
    SavedMessage, SavedTodo, SavedTodoPage, SidebarSection, SlackError, UserProfile,
};
use crate::infrastructure::slack_parsing::{
    next_cursor, order_sections, parse_attachment, parse_chime_join, parse_conversations,
    parse_custom_emoji, parse_draft, parse_message, parse_muted, parse_read_states,
    parse_saved_todos, parse_search, parse_section, parse_user_profile,
};

pub struct SlackHttpGateway {
    client: Client,
    credentials: Credentials,
    file_previews: Mutex<HashMap<String, (Instant, Option<Attachment>)>>,
    bot_profiles: Mutex<HashMap<String, UserProfile>>,
}

impl SlackHttpGateway {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("could not initialize HTTP client"),
            credentials,
            file_previews: Mutex::new(HashMap::new()),
            bot_profiles: Mutex::new(HashMap::new()),
        }
    }

    pub fn realtime_url(&self) -> Result<String, SlackError> {
        let body = self.call("rtm.connect", &[])?;
        body["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SlackError(String::from("rtm.connect returned no url")))
    }

    pub fn session_cookie(&self) -> &str {
        &self.credentials.cookie
    }

    fn call(&self, method: &str, params: &[(&str, &str)]) -> Result<Value, SlackError> {
        let url = format!("{}api/{}", self.credentials.workspace_url, method);
        let mut form: Vec<(&str, &str)> = vec![("token", self.credentials.token.as_str())];
        form.extend_from_slice(params);
        for _ in 0..MAX_ATTEMPTS {
            let response = self
                .authenticated(self.client.post(&url))
                .form(&form)
                .send()
                .map_err(|error| SlackError(error.to_string()))?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                thread::sleep(retry_delay(&response));
                continue;
            }
            let body: Value = response
                .json()
                .map_err(|error| SlackError(error.to_string()))?;
            if body["ok"].as_bool() == Some(true) {
                return Ok(body);
            }
            return Err(SlackError(format!(
                "{} failed: {}",
                method,
                body["error"].as_str().unwrap_or("unknown_error")
            )));
        }
        Err(SlackError(format!("{method} failed: rate limited")))
    }

    fn authenticated(&self, request: RequestBuilder) -> RequestBuilder {
        request
            .header(COOKIE, format!("d={}", self.credentials.cookie))
            .header(AUTHORIZATION, format!("Bearer {}", self.credentials.token))
    }

    fn message_page(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<Vec<Message>, SlackError> {
        let body = self.call(method, params)?;
        Ok(body["messages"]
            .as_array()
            .map(|items| items.iter().filter_map(parse_message).collect())
            .unwrap_or_default())
    }

    fn resolve_file_previews(&self, messages: &mut [Message]) {
        let ids: Vec<_> = messages
            .iter()
            .flat_map(|message| &message.attachments)
            .filter(|attachment| attachment.preview_url.is_none())
            .filter_map(|attachment| attachment.file_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut previews = HashMap::new();
        thread::scope(|scope| {
            let jobs: Vec<_> = ids
                .chunks(ids.len().div_ceil(4).max(1))
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|id| (id.clone(), self.file_preview(id)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for job in jobs {
                previews.extend(job.join().expect("file preview worker panicked"));
            }
        });
        for attachment in messages
            .iter_mut()
            .flat_map(|message| &mut message.attachments)
        {
            if attachment.preview_url.is_some() {
                continue;
            }
            if let Some(Some(preview)) = attachment.file_id.as_ref().and_then(|id| previews.get(id))
            {
                attachment.preview_url.clone_from(&preview.preview_url);
                attachment.full_url = preview.full_url.clone().or(attachment.full_url.take());
                attachment.width = preview.width.or(attachment.width);
                attachment.height = preview.height.or(attachment.height);
            }
        }
    }

    fn file_preview(&self, file_id: &str) -> Option<Attachment> {
        let cached = self
            .file_previews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(file_id)
            .filter(|(fetched, value)| {
                value.is_some() || fetched.elapsed() < Duration::from_secs(60)
            })
            .cloned();
        if let Some((_, preview)) = cached {
            return preview;
        }
        let preview = self
            .call("files.info", &[("file", file_id)])
            .ok()
            .map(|body| parse_attachment(&body["file"]));
        self.file_previews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(file_id.to_owned(), (Instant::now(), preview.clone()));
        preview
    }

    fn bot_profile(&self, bot_id: &str) -> Result<UserProfile, SlackError> {
        if let Some(profile) = self
            .bot_profiles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(bot_id)
            .cloned()
        {
            return Ok(profile);
        }
        let body = self.call("bots.info", &[("bot", bot_id)])?;
        let profile = UserProfile {
            id: bot_id.to_owned(),
            name: body["bot"]["name"].as_str().unwrap_or("Bot").to_owned(),
            avatar_url: body["bot"]["icons"]["image_72"].as_str().map(str::to_owned),
        };
        self.bot_profiles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(bot_id.to_owned(), profile.clone());
        Ok(profile)
    }
}

impl SlackGateway for SlackHttpGateway {
    fn upload_file(&self, file: &crate::domain::uploads::UploadFile) -> Result<String, SlackError> {
        let length = file.bytes.len().to_string();
        let body = self.call(
            "files.getUploadURLExternal",
            &[("filename", &file.name), ("length", &length)],
        )?;
        let id = body["file_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| SlackError("Slack returned no upload ID.".into()))?;
        let url = body["upload_url"]
            .as_str()
            .ok_or_else(|| SlackError("Slack returned no upload URL.".into()))?;
        if !valid_upload_url(url) {
            return Err(SlackError(
                "Slack returned an invalid upload destination.".into(),
            ));
        }
        // The signed upload URL needs no workspace cookie/token. Do not send
        // credentials or follow redirects, and keep large uploads off the UI thread.
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| SlackError("Could not create the upload connection.".into()))?;
        let response = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::blocking::Body::sized(
                std::io::Cursor::new(file.bytes.clone()),
                file.bytes.len() as u64,
            ))
            .send()
            .map_err(|_| {
                SlackError(format!(
                    "Could not upload {}. Check your connection and retry.",
                    file.name
                ))
            })?;
        if !response.status().is_success() {
            return Err(SlackError(format!(
                "Upload of {} failed ({}).",
                file.name,
                response.status()
            )));
        }
        Ok(id.to_owned())
    }

    fn complete_uploads(
        &self,
        conversation_id: &str,
        batch: &crate::domain::uploads::UploadBatch,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<(), crate::domain::uploads::UploadCompletionError> {
        use crate::domain::uploads::UploadCompletionError;
        let files = upload_descriptors(batch).to_string();
        let mut params = vec![
            ("files", files.as_str()),
            ("token", self.credentials.token.as_str()),
            ("channel_id", conversation_id),
            ("initial_comment", text),
        ];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts));
        }
        let response = self
            .authenticated(self.client.post(format!(
                "{}api/files.completeUploadExternal",
                self.credentials.workspace_url
            )))
            .form(&params)
            .send()
            .map_err(|_| UploadCompletionError {
                error: SlackError(
                    "Upload confirmation was interrupted. Retry checks whether Slack received it."
                        .into(),
                ),
                may_have_posted: true,
            })?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(UploadCompletionError {
                error: SlackError(
                    "Slack is rate limiting uploads. Wait a moment and retry.".into(),
                ),
                may_have_posted: false,
            });
        }
        let body: Value = response.json().map_err(|_| UploadCompletionError {
            error: SlackError(
                "Slack returned an unreadable upload confirmation. Retry checks its status.".into(),
            ),
            may_have_posted: true,
        })?;
        if body["ok"].as_bool() == Some(true) {
            return Ok(());
        }
        let code = body["error"].as_str().unwrap_or("unknown_error");
        Err(UploadCompletionError {
            error: SlackError(format!("Could not share attachments: {code}")),
            may_have_posted: matches!(
                code,
                "internal_error"
                    | "fatal_error"
                    | "request_timeout"
                    | "service_unavailable"
                    | "unknown_error"
                    | "already_complete"
            ),
        })
    }

    fn uploaded_message_ts(
        &self,
        conversation_id: &str,
        file_ids: &[String],
    ) -> Result<Option<String>, SlackError> {
        let mut timestamp = None;
        for id in file_ids {
            let body = self.call("files.info", &[("file", id)])?;
            let Some(ts) = uploaded_share_ts(&body["file"], conversation_id) else {
                return Ok(None);
            };
            timestamp = Some(ts);
        }
        Ok(timestamp)
    }
    fn mark_activity_read(
        &self,
        item: &crate::domain::activity::ActivityItem,
    ) -> Result<(), SlackError> {
        let entries =
            serde_json::json!([{ "type":item.kind, "key":item.key, "feed_ts":item.feed_ts }])
                .to_string();
        self.call("activity.markRead", &[("entries", &entries)])?;
        Ok(())
    }
    fn activity(
        &self,
        query: crate::domain::activity::ActivityQuery,
        cursor: &str,
    ) -> Result<crate::domain::activity::ActivityPage, SlackError> {
        let mut params = vec![
            ("types", query.filter.types()),
            ("limit", "20"),
            ("mode", "chrono_v1"),
            ("archive_only", "false"),
            (
                "unread_only",
                if query.unread_only { "true" } else { "false" },
            ),
            ("priority_only", "false"),
            ("exclude_automations", "false"),
            ("is_activity_inbox", "true"),
        ];
        // Slack rejects an explicitly empty cursor on the initial page.
        if !cursor.is_empty() {
            params.push(("cursor", cursor));
        }
        let body = self.call("activity.feed", &params)?;
        super::activity_parsing::parse_activity(&body)
    }

    fn activity_messages(
        &self,
        items: &[crate::domain::activity::ActivityItem],
    ) -> Result<HashMap<(String, String), SavedMessage>, SlackError> {
        let mut grouped = std::collections::BTreeMap::<&str, HashSet<&str>>::new();
        for item in items {
            grouped
                .entry(&item.conversation_id)
                .or_default()
                .insert(&item.ts);
        }
        let groups: Vec<_> = grouped.into_iter().map(|(channel, timestamps)| {
            serde_json::json!({"channel":channel, "timestamps":timestamps})
        }).collect();
        let mut result = HashMap::new();
        // Batch exact references, including replies. Never scan channel histories.
        for batch in groups.chunks(4) {
            let ids = serde_json::to_string(batch).map_err(|e| SlackError(e.to_string()))?;
            let body = self.call(
                "messages.list",
                &[
                    ("message_ids", &ids),
                    ("org_wide_aware", "true"),
                    ("cached_latest_updates", "{}"),
                ],
            )?;
            let data = body["messages_data"].as_object().ok_or_else(|| {
                SlackError("Slack returned unrecognized Activity previews.".into())
            })?;
            for (channel, data) in data {
                for raw in data["messages"].as_array().into_iter().flatten() {
                    if let Some(mut message) = parse_message(raw) {
                        let mut avatar_url = raw["bot_profile"]["icons"]["image_72"]
                            .as_str()
                            .or(raw["user_profile"]["image_72"].as_str())
                            .map(str::to_owned);
                        if let Some(bot_id) = raw["bot_id"].as_str()
                            && let Ok(profile) = self.bot_profile(bot_id)
                        {
                            message.author = Author::Named(profile.name);
                            avatar_url = avatar_url.or(profile.avatar_url);
                        }
                        result.insert(
                            (channel.clone(), message.ts.clone()),
                            SavedMessage {
                                message,
                                avatar_url,
                            },
                        );
                    }
                }
            }
        }
        Ok(result)
    }

    fn complete_todo(&self, item: &SavedTodo) -> Result<(), SlackError> {
        self.call(
            "saved.update",
            &[
                ("item_type", item.item_type.as_str()),
                ("item_id", item.conversation_id.as_str()),
                ("ts", item.ts.as_str()),
                ("mark", "completed"),
            ],
        )?;
        Ok(())
    }

    fn saved_todos(&self, completed: bool, cursor: &str) -> Result<SavedTodoPage, SlackError> {
        let body = self.call(
            "saved.list",
            &[
                ("filter", if completed { "completed" } else { "saved" }),
                ("limit", "49"),
                ("cursor", cursor),
            ],
        )?;
        parse_saved_todos(&body, completed)
    }

    fn saved_message(&self, item: &SavedTodo) -> Result<Option<SavedMessage>, SlackError> {
        if item.item_type != "message" || item.ts.is_empty() {
            return Ok(None);
        }
        let params = [
            ("channel", item.conversation_id.as_str()),
            ("ts", item.ts.as_str()),
            ("oldest", item.ts.as_str()),
            ("latest", item.ts.as_str()),
            ("inclusive", "true"),
            ("limit", "5"),
        ];
        // replies accepts the saved reply's timestamp, including replies absent
        // from channel history. Fall back to history for standalone messages.
        for method in ["conversations.replies", "conversations.history"] {
            let body = match self.call(method, &params) {
                Ok(body) => body,
                Err(error)
                    if method == "conversations.replies"
                        && error.0.contains("thread_not_found") =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(raw) = body["messages"].as_array().and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message["ts"].as_str() == Some(&item.ts))
            }) && let Some(mut message) = parse_message(raw)
            {
                let mut avatar_url = raw["bot_profile"]["icons"]["image_72"]
                    .as_str()
                    .or(raw["user_profile"]["image_72"].as_str())
                    .map(str::to_owned);
                if let Some(bot_id) = raw["bot_id"].as_str()
                    && let Ok(profile) = self.bot_profile(bot_id)
                {
                    message.author = Author::Named(profile.name);
                    avatar_url = avatar_url.or(profile.avatar_url);
                }
                return Ok(Some(SavedMessage {
                    message,
                    avatar_url,
                }));
            }
        }
        Ok(None)
    }

    fn workspace_namespace(&self) -> Option<String> {
        Some(
            self.credentials
                .workspace_url
                .trim_end_matches('/')
                .to_lowercase(),
        )
    }

    fn cache_namespace(&self) -> Option<String> {
        Some(crate::application::cache::key(&format!(
            "{}:{}",
            self.credentials.workspace_url, self.credentials.token
        )))
    }

    fn current_user_id(&self) -> Result<String, SlackError> {
        let body = self.call("auth.test", &[])?;
        body["user_id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SlackError(String::from("auth.test returned no user")))
    }

    fn list_conversations(&self) -> Result<Vec<Conversation>, SlackError> {
        let mut conversations = Vec::new();
        let mut cursor = String::new();
        loop {
            let body = self.call(
                "users.conversations",
                &[
                    ("types", CONVERSATION_TYPES),
                    ("exclude_archived", "true"),
                    ("limit", PAGE_SIZE),
                    ("cursor", cursor.as_str()),
                ],
            )?;
            conversations.extend(parse_conversations(&body));
            cursor = next_cursor(&body);
            if cursor.is_empty() {
                return Ok(conversations);
            }
        }
    }

    fn list_sections(&self) -> Result<Vec<SidebarSection>, SlackError> {
        let body = self.call("users.channelSections.list", &[])?;
        let raw = body["channel_sections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(order_sections(&raw)
            .into_iter()
            .filter_map(parse_section)
            .collect())
    }

    fn read_states(&self) -> Result<HashMap<String, ReadState>, SlackError> {
        let body = self.call("client.counts", &[("thread_counts_by_channel", "false")])?;
        Ok(parse_read_states(&body))
    }

    fn muted_conversations(&self) -> Result<HashSet<String>, SlackError> {
        let body = self.call("users.prefs.get", &[])?;
        Ok(parse_muted(&body["prefs"]))
    }

    fn emoji_usage(&self) -> Result<crate::domain::emoji_usage::EmojiUsage, SlackError> {
        let body = self.call("users.prefs.get", &[])?;
        Ok(crate::infrastructure::slack_parsing::parse_emoji_usage(
            &body["prefs"],
        ))
    }

    fn custom_emoji(&self) -> Result<HashMap<String, String>, SlackError> {
        let body = self.call("emoji.list", &[])?;
        Ok(parse_custom_emoji(&body["emoji"]))
    }

    fn fetch_messages(&self, conversation_id: &str) -> Result<Vec<Message>, SlackError> {
        let mut messages = self.message_page(
            "conversations.history",
            &[("channel", conversation_id), ("limit", HISTORY_LIMIT)],
        )?;
        messages.reverse();
        Ok(messages)
    }

    fn enrich_messages(&self, messages: &mut [Message]) {
        self.resolve_file_previews(messages);
    }

    fn fetch_replies(
        &self,
        conversation_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<Message>, SlackError> {
        let mut messages = Vec::new();
        let mut cursor = String::new();
        let mut seen = HashSet::new();
        loop {
            let body = self.call(
                "conversations.replies",
                &[
                    ("channel", conversation_id),
                    ("ts", thread_ts),
                    ("limit", REPLIES_LIMIT),
                    ("cursor", &cursor),
                ],
            )?;
            if let Some(items) = body["messages"].as_array() {
                messages.extend(items.iter().filter_map(parse_message));
            }
            cursor = next_cursor(&body);
            if cursor.is_empty() {
                break;
            }
            if !seen.insert(cursor.clone()) {
                return Err(SlackError("Repeated thread pagination cursor".into()));
            }
        }
        messages.sort_by(|a, b| {
            crate::domain::model::ts_seconds(&a.ts)
                .total_cmp(&crate::domain::model::ts_seconds(&b.ts))
        });
        messages.dedup_by(|a, b| a.ts == b.ts);
        self.resolve_file_previews(&mut messages);
        Ok(messages)
    }

    fn user_profile(&self, user_id: &str) -> Result<UserProfile, SlackError> {
        if user_id.starts_with('B') {
            return self.bot_profile(user_id);
        }
        let body = self.call("users.info", &[("user", user_id)])?;
        Ok(parse_user_profile(&body["user"], user_id))
    }

    fn post_message(
        &self,
        conversation_id: &str,
        text: &str,
        thread_ts: Option<&str>,
        client_msg_id: &str,
    ) -> Result<String, SlackError> {
        let mut params = vec![
            ("channel", conversation_id),
            ("text", text),
            ("client_msg_id", client_msg_id),
        ];
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts));
        }
        let body = self.call("chat.postMessage", &params)?;
        body["ts"].as_str().or_else(|| body["message"]["ts"].as_str())
            .filter(|ts| !ts.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| SlackError("Slack accepted the request but returned no message timestamp; check the conversation before retrying.".into()))
    }

    fn update_message(
        &self,
        conversation_id: &str,
        ts: &str,
        text: &str,
    ) -> Result<(), SlackError> {
        let params = [("channel", conversation_id), ("ts", ts), ("text", text)];
        // Without an empty block list Slack keeps showing the old rich-text blocks.
        let cleared = [params.as_slice(), &[("blocks", "[]")]].concat();
        match self.call("chat.update", &cleared) {
            Err(error) if error.0.ends_with("invalid_blocks") => {
                self.call("chat.update", &params).map(|_| ())
            }
            result => result.map(|_| ()),
        }
    }

    fn drafts(&self) -> Result<Vec<RemoteDraft>, SlackError> {
        let body = self.call("drafts.list", &[("is_active", "true"), ("limit", "100")])?;
        Ok(body["drafts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(parse_draft)
            .collect())
    }

    fn save_draft(
        &self,
        existing: Option<&RemoteDraft>,
        conversation_id: &str,
        thread_ts: Option<&str>,
        text: &str,
    ) -> Result<RemoteDraft, SlackError> {
        let destinations = match thread_ts {
            Some(ts) => serde_json::json!([{"channel_id": conversation_id, "thread_ts": ts}]),
            None => serde_json::json!([{"channel_id": conversation_id}]),
        }
        .to_string();
        let blocks = crate::infrastructure::draft_blocks::draft_blocks(text).to_string();
        let update = |draft: &RemoteDraft| {
            let file_ids = serde_json::json!(draft.file_ids).to_string();
            self.call(
                "drafts.update",
                &[
                    ("draft_id", draft.id.as_str()),
                    // Slack rejects an update not newer than its own last change.
                    ("client_last_updated_ts", &now_ts()),
                    ("blocks", &blocks),
                    ("destinations", &destinations),
                    ("file_ids", &file_ids),
                    ("is_from_composer", "false"),
                ],
            )
        };
        let create = || {
            self.call(
                "drafts.create",
                &[
                    ("blocks", &blocks),
                    ("client_msg_id", &uuid::Uuid::new_v4().to_string()),
                    ("destinations", &destinations),
                    ("file_ids", "[]"),
                    ("is_from_composer", "false"),
                ],
            )
        };
        let body = match existing {
            Some(draft) => match update(draft) {
                Err(error) if error.0.ends_with("draft_not_found") => create(),
                result => result,
            },
            None => match create() {
                // Another client made one meanwhile; Slack keeps one per destination.
                Err(error) if error.0.ends_with("attached_draft_exists") => {
                    let current = self
                        .drafts()?
                        .into_iter()
                        .find(|draft| {
                            draft.conversation_id == conversation_id
                                && draft.thread_ts.as_deref() == thread_ts
                        })
                        .ok_or(error)?;
                    update(&current)
                }
                result => result,
            },
        }?;
        parse_draft(&body["draft"]).ok_or_else(|| SlackError("Slack returned no draft.".into()))
    }

    fn delete_draft(&self, id: &str) -> Result<(), SlackError> {
        match self.call(
            "drafts.delete",
            &[("draft_id", id), ("client_last_updated_ts", &now_ts())],
        ) {
            Err(error) if error.0.ends_with("draft_not_found") => Ok(()),
            result => result.map(|_| ()),
        }
    }

    fn permalink(&self, conversation_id: &str, ts: &str) -> Result<String, SlackError> {
        let body = self.call(
            "chat.getPermalink",
            &[("channel", conversation_id), ("message_ts", ts)],
        )?;
        body["permalink"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| SlackError("Slack returned no link for this message.".into()))
    }

    fn join_huddle(&self, conversation_id: &str) -> Result<ChimeJoin, SlackError> {
        let body = self.call(
            "rooms.join",
            &[
                ("channel_id", conversation_id),
                ("regions", ""),
                ("multidevice", "false"),
            ],
        )?;
        parse_chime_join(&body, conversation_id)
            .ok_or_else(|| SlackError(String::from("Slack sent no way to join the huddle")))
    }

    fn leave_huddle(&self, join: &ChimeJoin) -> Result<(), SlackError> {
        self.call(
            "rooms.leave",
            &[
                ("channel_id", &join.conversation_id),
                ("call_id", &join.call_id),
                ("attendee_id", &join.attendee_id),
            ],
        )
        .map(|_| ())
    }

    fn decline_huddle(&self, conversation_id: &str, call_id: &str) -> Result<(), SlackError> {
        self.call(
            "rooms.inviteResponse",
            &[
                ("response", "decline"),
                ("channel_id", conversation_id),
                ("room_id", call_id),
            ],
        )
        .map(|_| ())
    }

    fn search_messages(
        &self,
        query: &str,
        sort: crate::domain::message_search::SearchSort,
        page: u32,
    ) -> Result<crate::domain::message_search::SearchPage, SlackError> {
        use crate::domain::message_search::SearchSort;
        let page = page.to_string();
        let sort = match sort {
            SearchSort::Newest => "timestamp",
            SearchSort::Relevant => "score",
        };
        let body = self.call(
            "search.messages",
            &[
                ("query", query),
                ("count", SEARCH_PAGE_SIZE),
                ("page", &page),
                ("highlight", "true"),
                ("sort", sort),
                ("sort_dir", "desc"),
            ],
        )?;
        Ok(parse_search(&body))
    }

    fn mark_read(&self, conversation_id: &str, ts: &str) -> Result<(), SlackError> {
        self.call(
            "conversations.mark",
            &[("channel", conversation_id), ("ts", ts)],
        )
        .map(|_| ())
    }

    fn set_reaction(
        &self,
        conversation_id: &str,
        ts: &str,
        name: &str,
        add: bool,
    ) -> Result<(), SlackError> {
        let method = if add {
            "reactions.add"
        } else {
            "reactions.remove"
        };
        let result = self
            .call(
                method,
                &[
                    ("channel", conversation_id),
                    ("timestamp", ts),
                    ("name", name),
                ],
            )
            .map(|_| ());
        match result {
            Err(error) if reaction_already_applied(&error, add) => Ok(()),
            result => result,
        }
    }

    fn download(&self, url: &str) -> Result<Vec<u8>, SlackError> {
        let request = self.client.get(url);
        let request = if is_slack_host(url) {
            self.authenticated(request)
        } else {
            request
        };
        let response = request
            .send()
            .map_err(|error| SlackError(error.to_string()))?;
        if !response.status().is_success() {
            return Err(SlackError(format!(
                "download failed: {}",
                response.status()
            )));
        }
        let html = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
        if html && !url.to_lowercase().ends_with(".html") {
            return Err(SlackError(
                "Slack returned a web page instead of the file; try signing in again.".into(),
            ));
        }
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|error| SlackError(error.to_string()))
    }
}

fn valid_upload_url(url: &str) -> bool {
    reqwest::Url::parse(url).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("files.slack.com")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
    })
}

fn reaction_already_applied(error: &SlackError, add: bool) -> bool {
    error.0
        == if add {
            "reactions.add failed: already_reacted"
        } else {
            "reactions.remove failed: no_reaction"
        }
}

fn upload_descriptors(batch: &crate::domain::uploads::UploadBatch) -> Value {
    Value::Array(
        batch
            .uploaded_ids
            .iter()
            .zip(&batch.files)
            .map(|(id, file)| serde_json::json!({"id": id, "title": file.name}))
            .collect(),
    )
}

fn uploaded_share_ts(file: &Value, conversation_id: &str) -> Option<String> {
    ["public", "private"].iter().find_map(|kind| {
        file["shares"][kind][conversation_id]
            .as_array()?
            .iter()
            .find_map(|share| share["ts"].as_str().map(str::to_owned))
    })
}

fn retry_delay(response: &reqwest::blocking::Response) -> Duration {
    let seconds = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_SECONDS);
    Duration::from_secs(seconds)
}

fn is_slack_host(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .is_some_and(|host| {
            SLACK_HOST_SUFFIXES
                .iter()
                .any(|suffix| host == suffix[1..] || host.ends_with(suffix))
        })
}

const CONVERSATION_TYPES: &str = "public_channel,private_channel,mpim,im";
const PAGE_SIZE: &str = "1000";
const HISTORY_LIMIT: &str = "100";
const SEARCH_PAGE_SIZE: &str = "20";
const REPLIES_LIMIT: &str = "200";
const MAX_ATTEMPTS: usize = 4;
const DEFAULT_RETRY_SECONDS: u64 = 2;
const SLACK_HOST_SUFFIXES: [&str; 2] = [".slack.com", ".slack-edge.com"];

/// Now as a Slack timestamp, for draft changes.
fn now_ts() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:06}", now.as_secs(), now.subsec_micros())
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn reaction_replays_are_successful_but_real_errors_are_not_hidden() {
        assert!(reaction_already_applied(
            &SlackError("reactions.add failed: already_reacted".into()),
            true
        ));
        assert!(reaction_already_applied(
            &SlackError("reactions.remove failed: no_reaction".into()),
            false
        ));
        assert!(!reaction_already_applied(
            &SlackError("reactions.add failed: no_permission".into()),
            true
        ));
        assert!(!reaction_already_applied(
            &SlackError("reactions.add failed: already_reacted".into()),
            false
        ));
    }

    #[test]
    fn signed_upload_destinations_must_stay_on_slack_https() {
        assert!(valid_upload_url(
            "https://files.slack.com/upload/v1/example"
        ));
        for url in [
            "http://files.slack.com/upload",
            "https://files.slack.com.attacker.test/upload",
            "https://attacker.test/upload",
            "https://user@files.slack.com/upload",
            "file:///tmp/upload",
        ] {
            assert!(!valid_upload_url(url), "{url}");
        }
    }

    #[test]
    fn confirmation_uses_the_target_conversation_not_an_unrelated_share() {
        let file = serde_json::json!({"shares": {
            "public": {"COTHER": [{"ts":"1.0"}]},
            "private": {"DTARGET": [{"ts":"2.0", "thread_ts":"0.5"}]}
        }});
        assert_eq!(uploaded_share_ts(&file, "DTARGET").as_deref(), Some("2.0"));
        assert_eq!(uploaded_share_ts(&file, "CMISSING"), None);
    }
}
