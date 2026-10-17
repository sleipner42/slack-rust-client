//! A text box for Slack markup: styles the markup as it is typed, and applies
//! formatting from the toolbar or Slack's shortcuts. The markers stay visible.

use std::sync::Arc;

use eframe::egui;

use super::theme;
use crate::domain::markup::{Format, MarkupStyle, apply_format, continue_line, markup_styles};

/// Draw the draft with its markup styled; pass to `TextEdit::layouter`.
pub fn layouter() -> impl FnMut(&egui::Ui, &dyn egui::TextBuffer, f32) -> Arc<egui::Galley> {
    |ui, text, wrap_width| {
        let text = text.as_str();
        let body = egui::TextStyle::Body.resolve(ui.style());
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = wrap_width;
        for (range, style) in markup_styles(text) {
            job.append(&text[range], 0.0, format(&body, style));
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    }
}

fn format(body: &egui::FontId, style: MarkupStyle) -> egui::TextFormat {
    let mut format = egui::TextFormat {
        font_id: body.clone(),
        color: theme::TEXT,
        valign: egui::Align::Center,
        ..Default::default()
    };
    if style.code || style.code_block {
        format.font_id = egui::FontId::monospace(body.size - 1.0);
        format.background = theme::CODE_BACKGROUND;
        if style.code {
            format.color = theme::SELF_MENTION_TEXT;
        }
    } else if style.bold {
        format.font_id.family = theme::bold_family();
        format.color = theme::TEXT_STRONG;
    }
    format.italics = style.italic;
    if style.strike {
        format.strikethrough = egui::Stroke::new(1.0, format.color);
    }
    if style.list {
        format.color = theme::LINK;
    }
    if style.marker {
        format.color = theme::TEXT_MUTED;
    }
    format
}

/// Slack's formatting shortcuts, and list continuation on Shift+Enter, for the
/// focused editor `id`. Call before the editor so it never sees the keys.
pub fn shortcuts(ui: &egui::Ui, id: egui::Id, text: &mut String) {
    if !ui.memory(|memory| memory.has_focus(id)) {
        return;
    }
    let mut action = None;
    ui.input_mut(|input| {
        let held = input.modifiers;
        input.events.retain(|event| {
            // The window layer turns Ctrl+Shift+X/C into Cut/Copy before we see the keys.
            let clipboard = match event {
                egui::Event::Cut if held.shift => Some(Format::Strike),
                egui::Event::Copy if held.shift && held.alt => Some(Format::CodeBlock),
                egui::Event::Copy if held.shift => Some(Format::Code),
                _ => None,
            };
            if let Some(format) = clipboard {
                if action.is_none() {
                    action = Some(Some(format));
                }
                return false;
            }
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let is = |wanted: egui::Key| *key == wanted || *physical_key == Some(wanted);
            let found = if modifiers.matches_exact(egui::Modifiers::SHIFT) && is(egui::Key::Enter) {
                Some(None)
            } else if !modifiers.command {
                None
            } else {
                let (shift, alt) = (modifiers.shift, modifiers.alt);
                [
                    (egui::Key::B, false, false, Format::Bold),
                    (egui::Key::I, false, false, Format::Italic),
                    (egui::Key::X, true, false, Format::Strike),
                    (egui::Key::C, true, false, Format::Code),
                    (egui::Key::C, true, true, Format::CodeBlock),
                    (egui::Key::Num9, true, false, Format::Quote),
                    (egui::Key::Num8, true, false, Format::Bullets),
                    (egui::Key::Num7, true, false, Format::Numbered),
                ]
                .into_iter()
                .find(|(wanted, s, a, _)| is(*wanted) && *s == shift && *a == alt)
                .map(|(.., format)| Some(format))
            };
            match found {
                Some(found) if action.is_none() => {
                    action = Some(found);
                    false
                }
                _ => true,
            }
        });
    });
    match action {
        Some(Some(format)) => apply(ui.ctx(), id, text, format),
        Some(None) => {
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let cursor = state
                .cursor
                .char_range()
                .map_or(text.chars().count(), |range| range.primary.index.0);
            let (changed, cursor) = continue_line(text, cursor).unwrap_or_else(|| {
                // Not a list: a plain newline, as the editor would have inserted.
                let mut chars: Vec<char> = text.chars().collect();
                let at = cursor.min(chars.len());
                chars.insert(at, '\n');
                (chars.into_iter().collect(), at + 1)
            });
            *text = changed;
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(cursor),
                )));
            state.store(ui.ctx(), id);
        }
        None => {}
    }
}

/// Apply a format to the editor's selection and keep the editor focused.
pub fn apply(ctx: &egui::Context, id: egui::Id, text: &mut String, format: Format) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = text.chars().count();
    let selection = state.cursor.char_range().map_or(end..end, |range| {
        let range = range.as_sorted_char_range();
        range.start.0..range.end.0
    });
    let (changed, selection) = apply_format(text, selection, format);
    *text = changed;
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(selection.start),
            egui::text::CCursor::new(selection.end),
        )));
    state.store(ctx, id);
    // A toolbar press has already taken focus from the editor this frame, and
    // the editor would drop a request made now; give it back next frame.
    ctx.data_mut(|data| data.insert_temp(refocus_id(id), true));
    ctx.request_repaint();
}

/// Return focus to the editor after a toolbar button; call before drawing it.
pub fn restore_focus(ui: &egui::Ui, id: egui::Id) {
    if ui
        .data_mut(|data| data.remove_temp::<bool>(refocus_id(id)))
        .is_some()
    {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
}

/// Give the editor focus back next frame, after a click elsewhere took it.
pub fn request_refocus(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|data| data.insert_temp(refocus_id(id), true));
    ctx.request_repaint();
}

fn refocus_id(id: egui::Id) -> egui::Id {
    id.with("refocus")
}

/// The formatting buttons above the composer.
pub fn toolbar(ui: &mut egui::Ui, id: egui::Id, text: &mut String) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding = egui::vec2(6.0, 1.0);
        let command = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let buttons = [
            (egui::RichText::new("B").strong(), "Bold", "B", Format::Bold),
            (
                egui::RichText::new("I").italics(),
                "Italic",
                "I",
                Format::Italic,
            ),
            (
                egui::RichText::new("S").strikethrough(),
                "Strikethrough",
                "Shift+X",
                Format::Strike,
            ),
            (
                egui::RichText::new("•").strong(),
                "Bulleted list",
                "Shift+8",
                Format::Bullets,
            ),
            (
                egui::RichText::new("1.").strong(),
                "Numbered list",
                "Shift+7",
                Format::Numbered,
            ),
            (
                egui::RichText::new(">").strong(),
                "Quote",
                "Shift+9",
                Format::Quote,
            ),
            (
                egui::RichText::new("</>").monospace(),
                "Code",
                "Shift+C",
                Format::Code,
            ),
            (
                egui::RichText::new("{ }").monospace(),
                "Code block",
                "Alt+Shift+C",
                Format::CodeBlock,
            ),
        ];
        for (label, name, keys, format) in buttons {
            if matches!(format, Format::Bullets | Format::Code) {
                ui.separator();
            }
            if ui
                .add(
                    egui::Button::new(label.color(theme::TEXT_MUTED))
                        .frame_when_inactive(false)
                        .min_size(egui::vec2(26.0, 22.0)),
                )
                .on_hover_text(format!("{name}  {command}+{keys}"))
                .clicked()
            {
                apply(ui.ctx(), id, text, format);
            }
        }
    });
}

/// Underline misspelled words in a shown editor, and offer corrections on a
/// right click. The word being typed is left alone until the cursor leaves it.
pub fn spelling(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    id: egui::Id,
    text: &mut String,
) {
    let Some(checker) = crate::infrastructure::spell_checker::get() else {
        return;
    };
    let char_to_byte = |index: usize| {
        text.char_indices()
            .nth(index)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let byte_to_char = |byte: usize| {
        text.get(..byte)
            .map_or_else(|| text.chars().count(), |before| before.chars().count())
    };
    let typing_at = output
        .cursor_range
        .filter(|_| output.response.has_focus())
        .map(|range| char_to_byte(range.primary.index.0));
    let words = crate::domain::spelling::checkable_words(text);
    let painter = ui.painter_at(output.text_clip_rect);
    for word in &words {
        if typing_at.is_some_and(|at| word.start <= at && at <= word.end)
            || checker.check(&text[word.clone()])
        {
            continue;
        }
        let start = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(byte_to_char(word.start)));
        let end = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(byte_to_char(word.end)));
        if (start.bottom() - end.bottom()).abs() < 1.0 {
            let offset = output.galley_pos.to_vec2();
            squiggle(
                &painter,
                start.left() + offset.x..end.left() + offset.x,
                start.bottom() + offset.y,
            );
        }
    }

    let menu_id = id.with("spelling_menu");
    if output.response.secondary_clicked() {
        let target = output.response.interact_pointer_pos().and_then(|pos| {
            let at = output.galley.cursor_from_pos(pos - output.galley_pos).index;
            let at = char_to_byte(at.0);
            let word = words
                .iter()
                .find(|word| word.start <= at && at <= word.end)?
                .clone();
            let spelled = &text[word.clone()];
            (!checker.check(spelled)).then(|| SpellingMenu {
                word: word.clone(),
                original: spelled.to_owned(),
                suggestions: checker.suggest(spelled),
            })
        });
        ui.data_mut(|data| match target {
            Some(target) => {
                data.insert_temp(menu_id, target);
            }
            None => {
                data.remove_temp::<SpellingMenu>(menu_id);
            }
        });
    }
    let Some(menu) = ui.data(|data| data.get_temp::<SpellingMenu>(menu_id)) else {
        return;
    };
    let mut choice = None;
    output.response.context_menu(|ui| {
        if menu.suggestions.is_empty() {
            ui.weak("No suggestions");
        }
        for suggestion in &menu.suggestions {
            if ui.button(suggestion).clicked() {
                choice = Some(Some(suggestion.clone()));
            }
        }
        ui.separator();
        if ui.button("Add to dictionary").clicked() {
            checker.add(&menu.original);
            choice = Some(None);
        }
        if ui.button("Ignore").clicked() {
            checker.ignore(&menu.original);
            choice = Some(None);
        }
        if choice.is_some() {
            ui.close();
        }
    });
    if let Some(choice) = choice {
        ui.data_mut(|data| data.remove_temp::<SpellingMenu>(menu_id));
        let mut cursor = byte_to_char(menu.word.end);
        // Only replace if the word is still where the menu found it.
        if let Some(replacement) = choice
            && text.get(menu.word.clone()) == Some(menu.original.as_str())
        {
            cursor = byte_to_char(menu.word.start) + replacement.chars().count();
            text.replace_range(menu.word.clone(), &replacement);
        }
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(cursor),
            )));
        state.store(ui.ctx(), id);
        ui.data_mut(|data| data.insert_temp(refocus_id(id), true));
        ui.ctx().request_repaint();
    }
}

#[derive(Clone, Default)]
struct SpellingMenu {
    word: std::ops::Range<usize>,
    original: String,
    suggestions: Vec<String>,
}

fn squiggle(painter: &egui::Painter, x: std::ops::Range<f32>, y: f32) {
    let points: Vec<egui::Pos2> = (0..)
        .map(|step| x.start + step as f32 * 2.0)
        .take_while(|px| *px <= x.end)
        .enumerate()
        .map(|(step, px)| egui::pos2(px, y + if step % 2 == 0 { 0.0 } else { -2.0 }))
        .collect();
    if points.len() > 1 {
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.0, theme::SPELLING),
        ));
    }
}
