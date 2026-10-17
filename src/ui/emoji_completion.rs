//! Emoji suggestions while typing `:name`, as in Slack: arrows choose, Tab or
//! Enter inserts, Escape dismisses. Custom workspace emoji included.

use eframe::egui;

use super::images::ImageCache;
use super::theme;
use crate::application::emoji_catalog::EmojiCatalog;

pub struct EmojiCompletion<'a> {
    pub emoji: &'a EmojiCatalog,
    pub images: &'a mut ImageCache,
}

#[derive(Clone, Default)]
struct State {
    highlighted: usize,
    /// The `:` position the user dismissed with Escape.
    dismissed: Option<usize>,
}

/// The `:query` before the cursor: the char index of the colon and the query.
fn query_at(text: &str, cursor: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_name_char(chars[start - 1]) {
        start -= 1;
    }
    let colon = start.checked_sub(1)?;
    let before_ok = colon == 0 || !chars[colon - 1].is_alphanumeric();
    let query: String = chars[start..cursor].iter().collect();
    (chars[colon] == ':' && before_ok && query.chars().count() >= MIN_QUERY)
        .then_some((colon, query))
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '+' | '-')
}

fn collapsed_cursor(ctx: &egui::Context, id: egui::Id) -> Option<usize> {
    let range = egui::text_edit::TextEditState::load(ctx, id)?
        .cursor
        .char_range()?;
    (range.primary == range.secondary).then_some(range.primary.index.0)
}

fn state_id(id: egui::Id) -> egui::Id {
    id.with("emoji_completion")
}

/// What to suggest now, if anything.
fn active(
    ui: &egui::Ui,
    id: egui::Id,
    text: &str,
    emoji: &EmojiCatalog,
) -> Option<(usize, usize, Vec<String>, State)> {
    if !ui.memory(|memory| memory.has_focus(id)) {
        return None;
    }
    let cursor = collapsed_cursor(ui.ctx(), id)?;
    let (colon, query) = query_at(text, cursor)?;
    let state = ui
        .data(|data| data.get_temp::<State>(state_id(id)))
        .unwrap_or_default();
    if state.dismissed == Some(colon) {
        return None;
    }
    let names = emoji.search(&query, MAX_SUGGESTIONS);
    (!names.is_empty()).then_some((colon, cursor, names, state))
}

/// Handle the suggestion keys before the editor and the send key see them.
pub fn keys(ui: &egui::Ui, id: egui::Id, text: &mut String, emoji: &EmojiCatalog) {
    let Some((colon, cursor, names, mut state)) = active(ui, id, text, emoji) else {
        return;
    };
    let (up, down, accept, dismiss) = ui.input_mut(|input| {
        (
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    state.highlighted = state.highlighted.min(names.len() - 1);
    if down {
        state.highlighted = (state.highlighted + 1) % names.len();
    }
    if up {
        state.highlighted = (state.highlighted + names.len() - 1) % names.len();
    }
    if dismiss {
        state.dismissed = Some(colon);
    }
    ui.data_mut(|data| data.insert_temp(state_id(id), state.clone()));
    if accept {
        insert(ui.ctx(), id, text, colon..cursor, &names[state.highlighted]);
    }
}

/// Draw the suggestions above the cursor; a click inserts one.
pub fn popup(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    id: egui::Id,
    text: &mut String,
    completion: &mut EmojiCompletion<'_>,
) {
    let Some((colon, cursor, names, state)) = active(ui, id, text, completion.emoji) else {
        return;
    };
    let caret = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(colon));
    let anchor = output.galley_pos + caret.left_top().to_vec2();
    let mut chosen = None;
    egui::Area::new(id.with("emoji_completion_popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(anchor - egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                for (index, name) in names.iter().enumerate() {
                    let selected = index == state.highlighted.min(names.len() - 1);
                    let row = ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                        if let Some(image) = completion
                            .emoji
                            .image_url(name)
                            .and_then(|url| completion.images.image(&url))
                        {
                            image.paint_at(ui, rect);
                        }
                        ui.add(
                            egui::Button::selectable(selected, format!(":{name}:"))
                                .frame_when_inactive(selected),
                        )
                    });
                    if row.inner.clicked() {
                        chosen = Some(name.clone());
                    }
                }
                ui.label(
                    egui::RichText::new("Tab or Enter to insert")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });
    if let Some(name) = chosen {
        insert(ui.ctx(), id, text, colon..cursor, &name);
        super::markup_editor::request_refocus(ui.ctx(), id);
    }
}

fn insert(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    chars: std::ops::Range<usize>,
    name: &str,
) {
    let byte = |index: usize| {
        text.char_indices()
            .nth(index)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let range = byte(chars.start)..byte(chars.end);
    let inserted = format!(":{name}: ");
    text.replace_range(range, &inserted);
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(chars.start + inserted.chars().count()),
        )));
    state.store(ctx, id);
    ctx.data_mut(|data| data.remove_temp::<State>(state_id(id)));
}

const MIN_QUERY: usize = 2;
const MAX_SUGGESTIONS: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_emoji_query_before_the_cursor() {
        assert_eq!(query_at("hi :pa", 6), Some((3, "pa".into())));
        assert_eq!(query_at(":party_p", 8), Some((0, "party_p".into())));
        assert_eq!(query_at("hi :p", 5), None, "too short");
        assert_eq!(query_at("at 12:30", 8), None, "not after a digit");
        assert_eq!(query_at("hi :pa x", 8), None, "the query ended");
        assert_eq!(query_at(":tada: ok", 9), None);
    }
}
