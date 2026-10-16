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
    ctx.memory_mut(|memory| memory.request_focus(id));
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
