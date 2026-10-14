use eframe::egui;
use egui::AtomExt as _;

use crate::domain::model::Conversation;
use crate::domain::search::rank_conversations;
use crate::ui::labels::conversation_icon;

pub enum SwitcherOutcome {
    Open,
    Chosen(Conversation),
    Closed,
}

pub struct QuickSwitcher {
    query: String,
    highlighted: usize,
}

impl QuickSwitcher {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            highlighted: 0,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, conversations: &[Conversation]) -> SwitcherOutcome {
        let matches = rank_conversations(&self.query, conversations, MAX_RESULTS);
        let keys = SwitcherKeys::consume(ctx);
        self.move_highlight(&keys, matches.len());
        if keys.confirm {
            return matches
                .get(self.highlighted)
                .cloned()
                .map(SwitcherOutcome::Chosen)
                .unwrap_or(SwitcherOutcome::Closed);
        }

        let mut clicked: Option<Conversation> = None;
        let response = egui::Modal::new(egui::Id::new(MODAL_ID))
            .frame(super::theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(MODAL_WIDTH);
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(PLACEHOLDER)
                        .margin(egui::Margin::symmetric(10, 8))
                        .desired_width(f32::INFINITY),
                );
                input.request_focus();
                if input.changed() {
                    self.highlighted = 0;
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                for (index, conversation) in matches.iter().enumerate() {
                    let row = ui.add_sized(
                        [ui.available_width(), 32.0],
                        egui::Button::selectable(
                            index == self.highlighted,
                            (
                                conversation_icon(conversation).atom_size(egui::vec2(18.0, 18.0)),
                                conversation.name.as_str().atom_shrink(true),
                                egui::Atom::grow(),
                            ),
                        )
                        .gap(6.0)
                        .truncate(),
                    );
                    if index == self.highlighted {
                        row.scroll_to_me(None);
                    }
                    if row.clicked() {
                        clicked = Some(conversation.clone());
                    }
                }
                if matches.is_empty() {
                    ui.weak(NO_RESULTS);
                }
            });

        match clicked {
            Some(conversation) => SwitcherOutcome::Chosen(conversation),
            None if keys.cancel || response.should_close() => SwitcherOutcome::Closed,
            None => SwitcherOutcome::Open,
        }
    }

    fn move_highlight(&mut self, keys: &SwitcherKeys, match_count: usize) {
        if match_count == 0 {
            self.highlighted = 0;
            return;
        }
        if keys.down {
            self.highlighted = (self.highlighted + 1) % match_count;
        }
        if keys.up {
            self.highlighted = (self.highlighted + match_count - 1) % match_count;
        }
        self.highlighted = self.highlighted.min(match_count - 1);
    }
}

struct SwitcherKeys {
    up: bool,
    down: bool,
    confirm: bool,
    cancel: bool,
}

impl SwitcherKeys {
    fn consume(ctx: &egui::Context) -> Self {
        ctx.input_mut(|input| Self {
            up: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            down: input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            confirm: input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            cancel: input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        })
    }
}

const MODAL_ID: &str = "quick_switcher";
const MODAL_WIDTH: f32 = 480.0;
const MAX_RESULTS: usize = 12;
const PLACEHOLDER: &str = "Jump to a channel or person…";
const NO_RESULTS: &str = "No matches";
