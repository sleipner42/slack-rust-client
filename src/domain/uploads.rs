use std::sync::Arc;

/// A snapshot of the pasted file. Retries never reread a changed local file.
#[derive(Clone, Debug)]
pub struct UploadFile {
    pub name: String,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, Default)]
pub struct UploadBatch {
    pub files: Vec<UploadFile>,
    /// IDs are retained across retries so successful uploads are not repeated.
    pub uploaded_ids: Vec<String>,
    pub completion_started: bool,
}

#[derive(Debug)]
pub struct UploadCompletionError {
    pub error: super::model::SlackError,
    pub may_have_posted: bool,
}

pub const MAX_FILES: usize = 10;
pub const MAX_BATCH_BYTES: usize = 100 * 1024 * 1024;
