use super::chat_service::DisplayMessage;

/// A search result ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub conversation_id: String,
    /// The thread the message is in, if any.
    pub thread_ts: Option<String>,
    /// `#channel`, or the person or group for direct messages.
    pub source: String,
    /// One line of plain text, the matched words between the highlight marks.
    pub snippet: String,
    pub message: DisplayMessage,
}

impl SearchResult {
    pub fn is_reply(&self) -> bool {
        self.thread_ts
            .as_ref()
            .is_some_and(|thread| *thread != self.message.ts)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    pub page: u32,
    pub pages: u32,
    pub total: u32,
}
