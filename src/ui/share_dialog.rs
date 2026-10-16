use eframe::egui;
use egui::AtomExt as _;

use crate::domain::model::Conversation;
use crate::domain::search::rank_conversations;
use crate::ui::labels::{conversation_icon, conversation_label};

pub enum ShareOutcome {
    Open,
    Share {
        destination: Conversation,
        comment: String,
    },
    Closed,
}

/// Choose where to share a message, with an optional comment, as Slack's dialog does.
pub struct ShareDialog {
    pub conversation_id: String,
    pub ts: String,
    query: String,
    highlighted: usize,
    destination: Option<Conversation>,
    comment: String,
    focus_comment: bool,
    ime_active: bool,
}

impl ShareDialog {
    pub fn new(conversation_id: String, ts: String) -> Self {
        Self {
            conversation_id,
            ts,
            query: String::new(),
            highlighted: 0,
            destination: None,
            comment: String::new(),
            focus_comment: false,
            ime_active: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, conversations: &[Conversation]) -> ShareOutcome {
        let matches = rank_conversations(&self.query, conversations, MAX_RESULTS);
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return ShareOutcome::Closed;
        }
        if self.destination.is_none() {
            let (up, down, confirm) = ctx.input_mut(|input| {
                (
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                )
            });
            let count = matches.len().max(1);
            if down {
                self.highlighted = (self.highlighted + 1) % count;
            }
            if up {
                self.highlighted = (self.highlighted + count - 1) % count;
            }
            self.highlighted = self.highlighted.min(count - 1);
            if confirm && let Some(conversation) = matches.get(self.highlighted) {
                self.choose(conversation.clone());
            }
        }

        let mut share = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new(MODAL_ID))
            .frame(super::theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(MODAL_WIDTH);
                ui.heading("Share message");
                ui.add_space(8.0);
                match self.destination.clone() {
                    None => {
                        let input = ui.add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text("Share with a channel or person…")
                                .margin(egui::Margin::symmetric(10, 8))
                                .desired_width(f32::INFINITY),
                        );
                        input.request_focus();
                        if input.changed() {
                            self.highlighted = 0;
                        }
                        ui.add_space(8.0);
                        for (index, conversation) in matches.iter().enumerate() {
                            let row = ui.add_sized(
                                [ui.available_width(), 32.0],
                                egui::Button::selectable(
                                    index == self.highlighted,
                                    (
                                        conversation_icon(conversation)
                                            .atom_size(egui::vec2(18.0, 18.0)),
                                        conversation.name.as_str().atom_shrink(true),
                                        egui::Atom::grow(),
                                    ),
                                )
                                .gap(6.0)
                                .truncate(),
                            );
                            if row.clicked() {
                                self.choose(conversation.clone());
                            }
                        }
                        if matches.is_empty() {
                            ui.weak("No matches");
                        }
                    }
                    Some(destination) => {
                        ui.horizontal(|ui| {
                            ui.label("To");
                            ui.strong(conversation_label(&destination));
                            if ui.small_button("Change").clicked() {
                                self.destination = None;
                            }
                        });
                        ui.add_space(8.0);
                        let id = egui::Id::new((MODAL_ID, "comment"));
                        if ui.memory(|memory| memory.has_focus(id)) {
                            share |= ui.input_mut(|input| {
                                super::composer::consume_submit(
                                    &mut input.events,
                                    &mut self.ime_active,
                                )
                            });
                        }
                        let comment = ui.add(
                            egui::TextEdit::multiline(&mut self.comment)
                                .id(id)
                                .hint_text("Add a message, if you like")
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    egui::Key::Enter,
                                )),
                        );
                        if std::mem::take(&mut self.focus_comment) {
                            comment.request_focus();
                        }
                    }
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    cancel = ui.button("Cancel").clicked();
                    share |= ui
                        .add_enabled(self.destination.is_some(), egui::Button::new("Share"))
                        .clicked();
                });
            });

        if share && let Some(destination) = self.destination.clone() {
            return ShareOutcome::Share {
                destination,
                comment: self.comment.trim().to_owned(),
            };
        }
        if cancel || response.should_close() {
            ShareOutcome::Closed
        } else {
            ShareOutcome::Open
        }
    }

    fn choose(&mut self, conversation: Conversation) {
        self.destination = Some(conversation);
        self.focus_comment = true;
    }
}

const MODAL_ID: &str = "share_message";
const MODAL_WIDTH: f32 = 480.0;
const MAX_RESULTS: usize = 8;
