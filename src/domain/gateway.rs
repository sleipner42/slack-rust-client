use std::collections::{HashMap, HashSet};

use crate::domain::model::{
    Conversation, LiveEvent, Message, ReadState, RemoteDraft, SavedMessage, SavedTodo,
    SavedTodoPage, SidebarSection, SlackError, UserProfile,
};

pub trait SlackGateway: Send + Sync {
    fn upload_file(&self, _file: &super::uploads::UploadFile) -> Result<String, SlackError> {
        Err(SlackError(
            "File uploads are unavailable for this workspace.".into(),
        ))
    }
    fn complete_uploads(
        &self,
        _conversation_id: &str,
        _batch: &super::uploads::UploadBatch,
        _text: &str,
        _thread_ts: Option<&str>,
    ) -> Result<(), super::uploads::UploadCompletionError> {
        Err(super::uploads::UploadCompletionError {
            error: SlackError("File uploads are unavailable for this workspace.".into()),
            may_have_posted: false,
        })
    }
    fn uploaded_message_ts(
        &self,
        _conversation_id: &str,
        _file_ids: &[String],
    ) -> Result<Option<String>, SlackError> {
        Ok(None)
    }
    fn mark_activity_read(&self, _item: &super::activity::ActivityItem) -> Result<(), SlackError> {
        Err(SlackError(
            "Updating Activity is unavailable for this workspace.".into(),
        ))
    }
    fn activity(
        &self,
        _query: super::activity::ActivityQuery,
        _cursor: &str,
    ) -> Result<super::activity::ActivityPage, SlackError> {
        Err(SlackError(
            "Activity is unavailable for this workspace.".into(),
        ))
    }
    fn activity_messages(
        &self,
        _items: &[super::activity::ActivityItem],
    ) -> Result<HashMap<(String, String), SavedMessage>, SlackError> {
        Ok(HashMap::new())
    }
    fn workspace_namespace(&self) -> Option<String> {
        None
    }
    fn cache_namespace(&self) -> Option<String> {
        None
    }
    fn saved_todos(&self, _completed: bool, _cursor: &str) -> Result<SavedTodoPage, SlackError> {
        Err(SlackError(
            "Slack to-dos are unavailable for this workspace.".into(),
        ))
    }
    fn saved_message(&self, _item: &SavedTodo) -> Result<Option<SavedMessage>, SlackError> {
        Ok(None)
    }
    fn complete_todo(&self, _item: &SavedTodo) -> Result<(), SlackError> {
        Err(SlackError(
            "Completing Slack to-dos is unavailable for this workspace.".into(),
        ))
    }
    fn current_user_id(&self) -> Result<String, SlackError>;
    fn list_conversations(&self) -> Result<Vec<Conversation>, SlackError>;
    fn list_sections(&self) -> Result<Vec<SidebarSection>, SlackError>;
    fn read_states(&self) -> Result<HashMap<String, ReadState>, SlackError>;
    fn muted_conversations(&self) -> Result<HashSet<String>, SlackError>;
    fn custom_emoji(&self) -> Result<HashMap<String, String>, SlackError>;
    fn fetch_messages(&self, conversation_id: &str) -> Result<Vec<Message>, SlackError>;
    /// Optional metadata work, kept separate from the first history response.
    fn enrich_messages(&self, _messages: &mut [Message]) {}
    fn fetch_replies(
        &self,
        conversation_id: &str,
        thread_ts: &str,
    ) -> Result<Vec<Message>, SlackError>;
    fn user_profile(&self, user_id: &str) -> Result<UserProfile, SlackError>;
    fn post_message(
        &self,
        conversation_id: &str,
        text: &str,
        thread_ts: Option<&str>,
        client_msg_id: &str,
    ) -> Result<String, SlackError>;
    fn update_message(
        &self,
        _conversation_id: &str,
        _ts: &str,
        _text: &str,
    ) -> Result<(), SlackError> {
        Err(SlackError(
            "Editing messages is unavailable for this workspace.".into(),
        ))
    }
    /// How often and recently the user picked each emoji in Slack.
    fn emoji_usage(&self) -> Result<crate::domain::emoji_usage::EmojiUsage, SlackError> {
        Ok(Default::default())
    }
    /// The user's unsent drafts that belong to a conversation or thread.
    fn drafts(&self) -> Result<Vec<RemoteDraft>, SlackError> {
        Ok(Vec::new())
    }
    /// Create or update the draft for a conversation or thread.
    fn save_draft(
        &self,
        _existing: Option<&RemoteDraft>,
        _conversation_id: &str,
        _thread_ts: Option<&str>,
        _text: &str,
    ) -> Result<RemoteDraft, SlackError> {
        Err(SlackError("Drafts do not sync for this workspace.".into()))
    }
    fn delete_draft(&self, _id: &str) -> Result<(), SlackError> {
        Ok(())
    }
    /// Everyone in the workspace who can be mentioned.
    fn list_people(&self) -> Result<Vec<super::people::Person>, SlackError> {
        Ok(Vec::new())
    }
    /// Join a conversation's huddle, starting one (and in a DM, ringing the
    /// other person) when there is none.
    fn join_huddle(&self, _conversation_id: &str) -> Result<super::model::ChimeJoin, SlackError> {
        Err(SlackError(
            "Huddles are unavailable for this workspace.".into(),
        ))
    }
    fn leave_huddle(&self, _join: &super::model::ChimeJoin) -> Result<(), SlackError> {
        Ok(())
    }
    /// Tell the caller you will not answer.
    fn decline_huddle(&self, _conversation_id: &str, _call_id: &str) -> Result<(), SlackError> {
        Ok(())
    }
    /// One page of messages matching a Slack search query.
    fn search_messages(
        &self,
        _query: &str,
        _sort: super::message_search::SearchSort,
        _page: u32,
    ) -> Result<super::message_search::SearchPage, SlackError> {
        Err(SlackError(
            "Searching messages is unavailable for this workspace.".into(),
        ))
    }
    /// A link to the message that Slack unfurls as a shared message.
    fn permalink(&self, _conversation_id: &str, _ts: &str) -> Result<String, SlackError> {
        Err(SlackError(
            "Sharing messages is unavailable for this workspace.".into(),
        ))
    }
    fn mark_read(&self, conversation_id: &str, ts: &str) -> Result<(), SlackError>;
    fn set_reaction(
        &self,
        conversation_id: &str,
        ts: &str,
        name: &str,
        add: bool,
    ) -> Result<(), SlackError>;
    fn download(&self, url: &str) -> Result<Vec<u8>, SlackError>;
}

pub trait RealtimeSource: Send + Sync {
    fn listen(&self, on_event: &mut dyn FnMut(LiveEvent)) -> Result<(), SlackError>;
}
