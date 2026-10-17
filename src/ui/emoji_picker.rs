use eframe::egui;

use super::images::ImageCache;
use super::message_view::emoji_button;
use crate::application::emoji_catalog::EmojiCatalog;

pub struct EmojiPicker {
    pub conversation_id: String,
    pub ts: String,
    query: String,
    focus: bool,
}

pub enum PickerOutcome {
    Open,
    Closed,
    Chosen(String),
}

impl EmojiPicker {
    pub fn new(conversation_id: String, ts: String) -> Self {
        Self {
            conversation_id,
            ts,
            query: String::new(),
            focus: true,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        emoji: &EmojiCatalog,
        images: &mut ImageCache,
    ) -> PickerOutcome {
        let mut chosen = None;
        let response = egui::Modal::new(egui::Id::new("emoji_picker"))
            .frame(super::theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(376.0);
                ui.strong("Add a reaction");
                ui.add_space(8.0);
                let input = ui.add_sized(
                    [376.0, 30.0],
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search emoji…")
                        .margin(egui::Margin::symmetric(8, 6)),
                );
                if self.focus {
                    input.request_focus();
                    self.focus = false;
                }
                ui.add_space(12.0);
                if self.query.trim().is_empty() {
                    // Slack's frequently used emoji, topped up with common ones.
                    let mut frequent = emoji.frequent(FREQUENT_COUNT);
                    for name in DEFAULT_FREQUENT {
                        if frequent.len() < FREQUENT_COUNT && !frequent.iter().any(|n| n == name) {
                            frequent.push(name.to_string());
                        }
                    }
                    ui.label(
                        egui::RichText::new("Frequently used")
                            .small()
                            .color(super::theme::TEXT_MUTED),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new("emoji_frequent")
                        .num_columns(8)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (index, name) in frequent.iter().enumerate() {
                                if emoji_button(ui, name, "", false, emoji, images)
                                    .on_hover_text(format!(":{name}:"))
                                    .clicked()
                                {
                                    chosen = Some(name.clone());
                                }
                                if index % 8 == 7 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                }
                let results = emoji.search(&self.query, 240);
                egui::ScrollArea::vertical()
                    .id_salt("emoji_results")
                    .max_height(280.0)
                    .show(ui, |ui| {
                        if results.is_empty() {
                            ui.weak("No matching emoji");
                        }
                        egui::Grid::new("emoji_grid")
                            .num_columns(8)
                            .spacing([8.0, 8.0])
                            .show(ui, |ui| {
                                for (index, name) in results.iter().enumerate() {
                                    if emoji_button(ui, name, "", false, emoji, images)
                                        .on_hover_text(format!(":{name}:"))
                                        .clicked()
                                    {
                                        chosen = Some(name.clone());
                                    }
                                    if index % 8 == 7 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
                ui.add_space(4.0);
            });
        if let Some(name) = chosen {
            PickerOutcome::Chosen(name)
        } else if response.should_close() {
            PickerOutcome::Closed
        } else {
            PickerOutcome::Open
        }
    }
}

const FREQUENT_COUNT: usize = 16;
const DEFAULT_FREQUENT: [&str; 8] = [
    "+1",
    "heart",
    "tada",
    "eyes",
    "rocket",
    "white_check_mark",
    "joy",
    "pray",
];
