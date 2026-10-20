//! Suggestions while typing, as in Slack: `:name` for emoji (custom workspace
//! emoji included) and `@name` for people. Arrows choose, Tab or Enter
//! inserts, Escape dismisses.

use eframe::egui;

use super::images::ImageCache;
use super::theme;
use crate::application::emoji_catalog::EmojiCatalog;
use crate::domain::people::{self, Person};

pub struct EmojiCompletion<'a> {
    pub emoji: &'a EmojiCatalog,
    pub people: &'a [Person],
    /// People to suggest first: those the user has conversations with.
    pub preferred: &'a [String],
    pub images: &'a mut ImageCache,
}

/// A person picked from the suggestions: the name inserted, and the Slack
/// markup it stands for when sent.
pub type Mention = (String, String);

#[derive(Clone, Default)]
struct State {
    highlighted: usize,
    /// The trigger position the user dismissed with Escape.
    dismissed: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Trigger {
    Emoji,
    Person,
}

#[derive(Clone)]
enum Suggestion {
    Emoji(String),
    Person(Person),
}

/// The `:query` or `@query` before the cursor: the char index of the trigger,
/// which it is, and the query.
fn query_at(text: &str, cursor: usize) -> Option<(usize, Trigger, String)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_name_char(chars[start - 1]) {
        start -= 1;
    }
    let at = start.checked_sub(1)?;
    let before_ok = at == 0 || !chars[at - 1].is_alphanumeric();
    let query: String = chars[start..cursor].iter().collect();
    let trigger = match chars[at] {
        ':' if query.chars().all(is_emoji_char) && query.chars().count() >= MIN_EMOJI_QUERY => {
            Trigger::Emoji
        }
        '@' => Trigger::Person,
        _ => return None,
    };
    before_ok.then_some((at, trigger, query))
}

fn is_name_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '+' | '-' | '.')
}

fn is_emoji_char(ch: char) -> bool {
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
    completion: &EmojiCompletion<'_>,
) -> Option<(usize, usize, Vec<Suggestion>, State)> {
    if !ui.memory(|memory| memory.has_focus(id)) {
        return None;
    }
    let cursor = collapsed_cursor(ui.ctx(), id)?;
    let (at, trigger, query) = query_at(text, cursor)?;
    let state = ui
        .data(|data| data.get_temp::<State>(state_id(id)))
        .unwrap_or_default();
    if state.dismissed == Some(at) {
        return None;
    }
    let suggestions: Vec<Suggestion> = match trigger {
        Trigger::Emoji => completion
            .emoji
            .search(&query, MAX_SUGGESTIONS)
            .into_iter()
            .map(Suggestion::Emoji)
            .collect(),
        Trigger::Person => people::search(
            completion.people,
            &query,
            completion.preferred,
            MAX_SUGGESTIONS,
        )
        .into_iter()
        .cloned()
        .map(Suggestion::Person)
        .collect(),
    };
    (!suggestions.is_empty()).then_some((at, cursor, suggestions, state))
}

/// Handle the suggestion keys before the editor and the send key see them.
/// Returns the person inserted, if one was.
pub fn keys(
    ui: &egui::Ui,
    id: egui::Id,
    text: &mut String,
    completion: &EmojiCompletion<'_>,
) -> Option<Mention> {
    let (at, cursor, suggestions, mut state) = active(ui, id, text, completion)?;
    let (up, down, accept, dismiss) = ui.input_mut(|input| {
        (
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    state.highlighted = state.highlighted.min(suggestions.len() - 1);
    if down {
        state.highlighted = (state.highlighted + 1) % suggestions.len();
    }
    if up {
        state.highlighted = (state.highlighted + suggestions.len() - 1) % suggestions.len();
    }
    if dismiss {
        state.dismissed = Some(at);
    }
    ui.data_mut(|data| data.insert_temp(state_id(id), state.clone()));
    accept
        .then(|| {
            insert(
                ui.ctx(),
                id,
                text,
                at..cursor,
                &suggestions[state.highlighted],
            )
        })
        .flatten()
}

/// Draw the suggestions above the cursor; a click inserts one. Returns the
/// person inserted, if one was.
pub fn popup(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    id: egui::Id,
    text: &mut String,
    completion: &mut EmojiCompletion<'_>,
) -> Option<Mention> {
    let (at, cursor, suggestions, state) = active(ui, id, text, completion)?;
    let caret = output.galley.pos_from_cursor(egui::text::CCursor::new(at));
    let anchor = output.galley_pos + caret.left_top().to_vec2();
    let mut chosen = None;
    egui::Area::new(id.with("emoji_completion_popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(anchor - egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                for (index, suggestion) in suggestions.iter().enumerate() {
                    let selected = index == state.highlighted.min(suggestions.len() - 1);
                    let (image_url, label, detail) = match suggestion {
                        Suggestion::Emoji(name) => {
                            (completion.emoji.image_url(name), format!(":{name}:"), None)
                        }
                        Suggestion::Person(person) => (
                            person.avatar_url.clone(),
                            person.name.clone(),
                            (person.real_name != person.name && !person.real_name.is_empty())
                                .then(|| person.real_name.clone()),
                        ),
                    };
                    let row = ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                        if let Some(image) = image_url.and_then(|url| completion.images.image(&url))
                        {
                            image.paint_at(ui, rect);
                        }
                        let clicked = ui
                            .add(
                                egui::Button::selectable(selected, label)
                                    .frame_when_inactive(selected),
                            )
                            .clicked();
                        if let Some(detail) = detail {
                            ui.label(egui::RichText::new(detail).small().color(theme::TEXT_MUTED));
                        }
                        clicked
                    });
                    if row.inner {
                        chosen = Some(suggestion.clone());
                    }
                }
                ui.label(
                    egui::RichText::new("Tab or Enter to insert")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
        });
    let chosen = chosen?;
    let mention = insert(ui.ctx(), id, text, at..cursor, &chosen);
    super::markup_editor::request_refocus(ui.ctx(), id);
    mention
}

fn insert(
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    chars: std::ops::Range<usize>,
    suggestion: &Suggestion,
) -> Option<Mention> {
    let byte = |index: usize| {
        text.char_indices()
            .nth(index)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let range = byte(chars.start)..byte(chars.end);
    let (inserted, mention) = match suggestion {
        Suggestion::Emoji(name) => (format!(":{name}: "), None),
        Suggestion::Person(person) => (
            format!("{} ", person.label()),
            Some((person.label(), person.markup())),
        ),
    };
    text.replace_range(range, &inserted);
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(chars.start + inserted.chars().count()),
        )));
    state.store(ctx, id);
    ctx.data_mut(|data| data.remove_temp::<State>(state_id(id)));
    mention
}

const MIN_EMOJI_QUERY: usize = 2;
const MAX_SUGGESTIONS: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_emoji_query_before_the_cursor() {
        let emoji = |text: &str, cursor| {
            query_at(text, cursor)
                .filter(|(_, trigger, _)| *trigger == Trigger::Emoji)
                .map(|(at, _, query)| (at, query))
        };
        assert_eq!(emoji("hi :pa", 6), Some((3, "pa".into())));
        assert_eq!(emoji(":party_p", 8), Some((0, "party_p".into())));
        assert_eq!(emoji("hi :p", 5), None, "too short");
        assert_eq!(emoji("at 12:30", 8), None, "not after a digit");
        assert_eq!(emoji("hi :pa x", 8), None, "the query ended");
        assert_eq!(emoji(":tada: ok", 9), None);
    }

    #[test]
    fn finds_the_person_query_before_the_cursor() {
        assert_eq!(query_at("@", 1), Some((0, Trigger::Person, String::new())));
        assert_eq!(
            query_at("hej @axel", 9),
            Some((4, Trigger::Person, "axel".into()))
        );
        assert_eq!(
            query_at("@max.a", 6),
            Some((0, Trigger::Person, "max.a".into()))
        );
        assert_eq!(
            query_at("@wennström", 10),
            Some((0, Trigger::Person, "wennström".into())),
            "letters beyond ASCII"
        );
        assert_eq!(query_at("mail me@home", 12), None, "an address");
        assert_eq!(query_at("@axel hi", 8), None, "the query ended");
    }
}
