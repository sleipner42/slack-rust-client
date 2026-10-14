use crate::application::chat_service::DisplayMessage;
use crate::domain::message_text::{Block, Inline};
use crate::domain::model::SavedTodo;

#[derive(Clone, Debug)]
pub struct DisplayTodo {
    pub item: SavedTodo,
    pub message: Option<DisplayMessage>,
    pub title: String,
    pub preview: String,
    pub source: String,
    pub source_url: Option<String>,
    pub error: Option<String>,
}

pub struct TodoPage {
    pub items: Vec<DisplayTodo>,
    pub open_count: usize,
    pub completed_count: usize,
    pub next_cursor: String,
}

pub fn preview_lines(message: &DisplayMessage) -> (String, String) {
    let text = message
        .blocks
        .iter()
        .map(|block| match block {
            Block::Code(text) => text.clone(),
            Block::Paragraph(inlines) | Block::Quote(inlines) => inlines
                .iter()
                .map(|inline| match inline {
                    Inline::Text { text, .. } => text.clone(),
                    Inline::Link { label, .. } | Inline::Mention { label, .. } => label.clone(),
                    Inline::Emoji(name) => emojis::get_by_shortcode(name)
                        .map(|emoji| emoji.as_str().to_owned())
                        .unwrap_or_else(|| format!(":{name}:")),
                })
                .collect::<String>(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let title = lines.next().map(str::to_owned).unwrap_or_else(|| {
        message
            .attachments
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Saved message".into())
    });
    (title, lines.collect::<Vec<_>>().join(" "))
}
