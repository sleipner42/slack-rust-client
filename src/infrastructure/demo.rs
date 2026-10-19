//! An in-memory workspace for development and UI checks. Never connects to Slack.
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Mutex;

use crate::domain::gateway::SlackGateway;
use crate::domain::model::*;
use crate::domain::unread::record_read;

pub struct DemoGateway {
    pub activity_reads: Mutex<Vec<crate::domain::activity::ActivityItem>>,
    pub upload_attempts: Mutex<Vec<String>>,
    pub upload_failure: Mutex<Option<String>>,
    pub upload_completions: Mutex<Vec<TestUploadCompletion>>,
    pub completion_failure: Mutex<Option<bool>>,
    pub upload_confirmed_ts: Mutex<Option<String>>,
    messages: Mutex<HashMap<String, Vec<Message>>>,
    states: Mutex<HashMap<String, ReadState>>,
}

pub struct TestUploadCompletion {
    pub channel: String,
    pub ids: Vec<String>,
    pub text: String,
    pub thread: Option<String>,
}

impl DemoGateway {
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp();
        let ts = |offset: i64| format!("{}.000000", now + offset);
        let mut first = message(
            ts(-600),
            "U1",
            "The new message view is ready to try. *Bold*, _italic_, ~strikethrough~, and `inline code` all belong here. :tada:",
        );
        first.reply_count = 2;
        first.reactions = vec![
            Reaction {
                name: "+1".into(),
                count: 2,
                user_ids: vec!["U1".into(), "UME".into()],
            },
            Reaction {
                name: "tada".into(),
                count: 1,
                user_ids: vec!["U2".into()],
            },
        ];
        let root = first.ts.clone();
        let mut image = message(
            ts(-470),
            "U2",
            "Here's the preview, <@UME>. Try opening the thread or adding a reaction.",
        );
        image.attachments.push(Attachment {
            file_id: None,
            name: "workspace-preview.png".into(),
            preview_url: Some("demo://preview.png".into()),
            full_url: None,
            permalink: None,
            width: Some(640),
            height: Some(240),
            preview_pending: false,
            video: false,
        });
        let mut reply1 = message(
            ts(-450),
            "U2",
            "Looks good! Shift+Enter gives the composer a second line.",
        );
        reply1.thread_ts = Some(root.clone());
        let mut reply2 = message(
            ts(-430),
            "UME",
            "And drafts stay with each channel and thread. :white_check_mark:",
        );
        reply2.thread_ts = Some(root);
        let messages = HashMap::from([
            (
                "C1".into(),
                vec![
                    message(
                        ts(-86400),
                        "U2",
                        "Yesterday's update: the Rust client is taking shape.",
                    ),
                    first,
                    message(
                        ts(-580),
                        "U1",
                        "&gt; Keep conversations easy to read.\n&gt; One small detail at a time.\n\n```let client = SlackClient::new();\nclient.connect().await?;```",
                    ),
                    image,
                    reply1,
                    reply2,
                    message(
                        ts(-120),
                        "U1",
                        "Useful links: <https://www.rust-lang.org|Rust> and <#C2|design>. :rocket:",
                    ),
                ],
            ),
            (
                "C2".into(),
                vec![message(
                    ts(-300),
                    "U2",
                    "Design notes\n\nTry resizing the sidebar and thread panel.\n:heart: :tada: :rocket:",
                )],
            ),
            (
                "C3".into(),
                vec![message(
                    ts(-200),
                    "U1",
                    "This channel is muted; it should look dimmed in the sidebar.",
                )],
            ),
            (
                "D1".into(),
                vec![message(
                    ts(-100),
                    "U1",
                    "Hey! Have you tried the new emoji picker yet?",
                )],
            ),
            (
                "D2".into(),
                vec![message(
                    ts(-10 * 86400),
                    "U2",
                    "An older DM still appears in the quick switcher.",
                )],
            ),
        ]);
        let states = HashMap::from([
            (
                "C1".into(),
                ReadState {
                    latest_ts: Some(ts(-120)),
                    last_read_ts: Some(ts(-900)),
                    has_unreads: true,
                    mention_count: 1,
                },
            ),
            (
                "C2".into(),
                ReadState {
                    latest_ts: Some(ts(-300)),
                    last_read_ts: Some(ts(-800)),
                    has_unreads: true,
                    mention_count: 0,
                },
            ),
            (
                "C3".into(),
                ReadState {
                    latest_ts: Some(ts(-200)),
                    last_read_ts: Some(ts(-800)),
                    has_unreads: true,
                    mention_count: 0,
                },
            ),
            (
                "D1".into(),
                ReadState {
                    latest_ts: Some(ts(-100)),
                    last_read_ts: None,
                    has_unreads: true,
                    mention_count: 2,
                },
            ),
            (
                "D2".into(),
                ReadState {
                    latest_ts: Some(ts(-10 * 86400)),
                    last_read_ts: Some(ts(-10 * 86400)),
                    ..ReadState::default()
                },
            ),
        ]);
        Self {
            upload_attempts: Default::default(),
            upload_failure: Default::default(),
            upload_completions: Default::default(),
            completion_failure: Default::default(),
            upload_confirmed_ts: Default::default(),
            activity_reads: Mutex::new(Vec::new()),
            messages: Mutex::new(messages),
            states: Mutex::new(states),
        }
    }
}

impl SlackGateway for DemoGateway {
    fn upload_file(&self, file: &crate::domain::uploads::UploadFile) -> Result<String, SlackError> {
        self.upload_attempts.lock().unwrap().push(file.name.clone());
        let mut failure = self.upload_failure.lock().unwrap();
        if failure.as_ref() == Some(&file.name) {
            *failure = None;
            return Err(SlackError("Upload interrupted".into()));
        }
        Ok(format!("F-{}", file.name))
    }

    fn complete_uploads(
        &self,
        channel: &str,
        batch: &crate::domain::uploads::UploadBatch,
        text: &str,
        thread: Option<&str>,
    ) -> Result<(), crate::domain::uploads::UploadCompletionError> {
        self.upload_completions
            .lock()
            .unwrap()
            .push(TestUploadCompletion {
                channel: channel.into(),
                ids: batch.uploaded_ids.clone(),
                text: text.into(),
                thread: thread.map(str::to_owned),
            });
        if let Some(may_have_posted) = self.completion_failure.lock().unwrap().take() {
            return Err(crate::domain::uploads::UploadCompletionError {
                error: SlackError("Completion failed".into()),
                may_have_posted,
            });
        }
        Ok(())
    }

    fn uploaded_message_ts(&self, _: &str, _: &[String]) -> Result<Option<String>, SlackError> {
        Ok(self.upload_confirmed_ts.lock().unwrap().clone())
    }
    fn mark_activity_read(
        &self,
        item: &crate::domain::activity::ActivityItem,
    ) -> Result<(), SlackError> {
        self.activity_reads.lock().unwrap().push(item.clone());
        Ok(())
    }
    fn team_id(&self) -> Option<String> {
        Some("TDEMO".into())
    }
    fn current_user_id(&self) -> Result<String, SlackError> {
        Ok("UME".into())
    }
    fn list_conversations(&self) -> Result<Vec<Conversation>, SlackError> {
        Ok(vec![
            Conversation {
                id: "C1".into(),
                name: "general".into(),
                kind: ConversationKind::PublicChannel,
            },
            Conversation {
                id: "C2".into(),
                name: "design".into(),
                kind: ConversationKind::PublicChannel,
            },
            Conversation {
                id: "C3".into(),
                name: "announcements".into(),
                kind: ConversationKind::PublicChannel,
            },
            Conversation {
                id: "D1".into(),
                name: "Alex Morgan".into(),
                kind: ConversationKind::DirectMessage {
                    user_id: "U1".into(),
                },
            },
            Conversation {
                id: "D2".into(),
                name: "Sam Rivera".into(),
                kind: ConversationKind::DirectMessage {
                    user_id: "U2".into(),
                },
            },
        ])
    }
    fn list_sections(&self) -> Result<Vec<SidebarSection>, SlackError> {
        Ok(vec![SidebarSection {
            kind: SectionKind::Starred,
            name: "Starred".into(),
            conversation_ids: vec!["C1".into()],
        }])
    }
    fn read_states(&self) -> Result<HashMap<String, ReadState>, SlackError> {
        Ok(self.states.lock().unwrap().clone())
    }
    fn muted_conversations(&self) -> Result<HashSet<String>, SlackError> {
        Ok(HashSet::from(["C3".into()]))
    }
    fn custom_emoji(&self) -> Result<HashMap<String, String>, SlackError> {
        Ok(HashMap::from([(
            "demo_badge".into(),
            "demo://avatar-U1.png".into(),
        )]))
    }
    fn fetch_messages(&self, conversation_id: &str) -> Result<Vec<Message>, SlackError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(conversation_id)
            .into_iter()
            .flatten()
            .filter(|message| !message.is_thread_reply())
            .cloned()
            .collect())
    }
    fn fetch_replies(
        &self,
        conversation_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<Message>, SlackError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(conversation_id)
            .into_iter()
            .flatten()
            .filter(|message| {
                message.ts == thread_ts || message.thread_ts.as_deref() == Some(thread_ts)
            })
            .cloned()
            .collect())
    }
    /// Messages containing every plain word of the query; `from:` and the
    /// other modifiers are ignored.
    fn search_messages(
        &self,
        query: &str,
        _sort: crate::domain::message_search::SearchSort,
        page: u32,
    ) -> Result<crate::domain::message_search::SearchPage, SlackError> {
        use crate::domain::message_search::{
            HIGHLIGHT_END, HIGHLIGHT_START, SearchMatch, SearchPage,
        };
        let words: Vec<String> = query
            .split_whitespace()
            .filter(|word| !word.contains(':'))
            .map(str::to_lowercase)
            .collect();
        let names = self.list_conversations()?;
        let mut matches: Vec<SearchMatch> = Vec::new();
        for (conversation_id, messages) in self.messages.lock().unwrap().iter() {
            for message in messages {
                let lower = message.text.to_lowercase();
                if words.is_empty() || !words.iter().all(|word| lower.contains(word.as_str())) {
                    continue;
                }
                let mut highlighted = message.text.clone();
                for word in &words {
                    if let Some(start) = highlighted.to_lowercase().find(word.as_str()) {
                        let end = start + word.len();
                        highlighted = format!(
                            "{}{HIGHLIGHT_START}{}{HIGHLIGHT_END}{}",
                            &highlighted[..start],
                            &highlighted[start..end],
                            &highlighted[end..]
                        );
                    }
                }
                let conversation = names.iter().find(|c| c.id == *conversation_id);
                matches.push(SearchMatch {
                    conversation_id: conversation_id.clone(),
                    conversation_name: conversation.map_or_else(String::new, |c| c.name.clone()),
                    is_channel: conversation.is_some_and(|c| !c.is_direct()),
                    thread_ts: message.thread_ts.clone(),
                    message: message.clone(),
                    highlighted,
                });
            }
        }
        matches.sort_by(|a, b| b.message.ts.cmp(&a.message.ts));
        let total = matches.len() as u32;
        Ok(SearchPage {
            matches: if page <= 1 { matches } else { Vec::new() },
            page: page.max(1),
            pages: total.min(1),
            total,
        })
    }
    fn user_profile(&self, user_id: &str) -> Result<UserProfile, SlackError> {
        let name = match user_id {
            "U1" => "Alex Morgan",
            "U2" => "Sam Rivera",
            _ => "You",
        };
        Ok(UserProfile {
            id: user_id.into(),
            name: name.into(),
            avatar_url: Some(format!("demo://avatar-{user_id}.png")),
        })
    }
    fn post_message(
        &self,
        conversation_id: &str,
        text: &str,
        thread_ts: Option<&str>,
        client_msg_id: &str,
    ) -> Result<String, SlackError> {
        let ts = format!(
            "{:.6}",
            chrono::Utc::now().timestamp_micros() as f64 / 1_000_000.0
        );
        let mut message = message(ts.clone(), "UME", text);
        message.thread_ts = thread_ts.map(str::to_string);
        message.client_msg_id = Some(client_msg_id.to_owned());
        let mut messages = self.messages.lock().unwrap();
        let conversation = messages.entry(conversation_id.into()).or_default();
        if let Some(existing) = conversation
            .iter()
            .find(|m| m.client_msg_id.as_deref() == Some(client_msg_id))
        {
            return Ok(existing.ts.clone());
        }
        if let Some(root) = conversation
            .iter_mut()
            .find(|message| Some(message.ts.as_str()) == thread_ts)
        {
            root.reply_count += 1;
        }
        conversation.push(message);
        if thread_ts.is_none() {
            let mut states = self.states.lock().unwrap();
            let state = states.entry(conversation_id.into()).or_default();
            state.latest_ts = Some(ts.clone());
            record_read(state, &ts);
        }
        Ok(ts)
    }
    fn permalink(&self, conversation_id: &str, ts: &str) -> Result<String, SlackError> {
        Ok(format!(
            "https://demo.slack.com/archives/{conversation_id}/p{}",
            ts.replace('.', "")
        ))
    }

    fn update_message(
        &self,
        conversation_id: &str,
        ts: &str,
        text: &str,
    ) -> Result<(), SlackError> {
        let mut messages = self.messages.lock().unwrap();
        let message = messages
            .get_mut(conversation_id)
            .and_then(|messages| messages.iter_mut().find(|message| message.ts == ts))
            .ok_or_else(|| SlackError("chat.update failed: message_not_found".into()))?;
        message.text = text.into();
        message.edited = true;
        Ok(())
    }

    fn mark_read(&self, conversation_id: &str, ts: &str) -> Result<(), SlackError> {
        record_read(
            self.states
                .lock()
                .unwrap()
                .entry(conversation_id.into())
                .or_default(),
            ts,
        );
        Ok(())
    }
    fn set_reaction(
        &self,
        conversation_id: &str,
        ts: &str,
        name: &str,
        add: bool,
    ) -> Result<(), SlackError> {
        let mut messages = self.messages.lock().unwrap();
        let message = messages
            .get_mut(conversation_id)
            .and_then(|messages| messages.iter_mut().find(|message| message.ts == ts))
            .ok_or_else(|| SlackError("message not found".into()))?;
        if let Some(reaction) = message
            .reactions
            .iter_mut()
            .find(|reaction| reaction.name == name)
        {
            let mine = reaction.user_ids.iter().any(|id| id == "UME");
            if add && !mine {
                reaction.user_ids.push("UME".into());
                reaction.count += 1;
            }
            if !add && mine {
                reaction.user_ids.retain(|id| id != "UME");
                reaction.count = reaction.count.saturating_sub(1);
            }
        } else if add {
            message.reactions.push(Reaction {
                name: name.into(),
                count: 1,
                user_ids: vec!["UME".into()],
            });
        }
        message.reactions.retain(|reaction| reaction.count > 0);
        Ok(())
    }
    fn download(&self, url: &str) -> Result<Vec<u8>, SlackError> {
        if !url.starts_with("demo://") {
            return Err(SlackError("Unknown demo image".into()));
        }
        let avatar = url.contains("avatar");
        let (width, height) = if avatar { (72, 72) } else { (640, 240) };
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            if avatar {
                let face = (x as i32 - 36).pow(2) + (y as i32 - 26).pow(2) < 12 * 12;
                let body = (x as i32 - 36).pow(2) + (y as i32 - 67).pow(2) < 25 * 25;
                if face || body {
                    image::Rgb([236, 209, 180])
                } else if url.contains("U1") {
                    image::Rgb([50, 100, 147])
                } else {
                    image::Rgb([121, 76, 121])
                }
            } else {
                let bar = x > 35 && x < 170 && y > 30 && y < 210;
                let line = x > 200
                    && x < 595
                    && (y > 40 && y < 75 || y > 105 && y < 135 || y > 165 && y < 195);
                image::Rgb(if bar {
                    [61, 48, 80]
                } else if line {
                    [60, 122, 151]
                } else {
                    [31, 35, 45]
                })
            }
        });
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Png)
            .map_err(|error| SlackError(error.to_string()))?;
        Ok(bytes.into_inner())
    }
}

fn message(ts: String, author: &str, text: &str) -> Message {
    Message {
        client_msg_id: None,
        ts,
        author: Author::User(author.into()),
        text: text.into(),
        thread_ts: None,
        reply_count: 0,
        reactions: Vec::new(),
        attachments: Vec::new(),
        shares: Vec::new(),
        edited: false,
        huddle: None,
    }
}
