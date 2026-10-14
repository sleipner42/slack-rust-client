use serde::{Deserialize, Serialize};

use super::chat_service::DisplayMessage;
use crate::domain::activity::ActivityItem;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayActivity {
    pub item: ActivityItem,
    pub message: Option<DisplayMessage>,
    pub actor: String,
    pub avatar_url: Option<String>,
    pub source: String,
    pub preview: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ActivityPage {
    pub items: Vec<DisplayActivity>,
    pub next_cursor: String,
    pub warning: Option<String>,
}
