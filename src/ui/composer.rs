use eframe::egui;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ComposeTarget {
    pub conversation_id: String,
    pub thread_ts: Option<String>,
}

impl ComposeTarget {
    pub fn editor_id(&self) -> egui::Id {
        egui::Id::new(("composer", self))
    }
}

#[derive(Default)]
pub struct Draft {
    pub text: String,
    pub attachments: super::attachments::Attachments,
    ime_active: bool,
}

pub fn composer(ui: &mut egui::Ui, target: &ComposeTarget, draft: &mut Draft, hint: &str) -> bool {
    draft.attachments.poll(ui.ctx());
    let id = target.editor_id();
    let focused = ui.is_enabled() && ui.memory(|memory| memory.has_focus(id));
    if focused {
        super::markup_editor::shortcuts(ui, id, &mut draft.text);
    }
    let enter = if focused {
        ui.input_mut(|input| consume_submit(&mut input.events, &mut draft.ime_active))
    } else {
        false
    };
    let mut send = false;
    egui::Frame::new()
        .fill(super::theme::INPUT_BACKGROUND)
        .stroke(egui::Stroke::new(1.0, super::theme::BORDER))
        .corner_radius(2)
        .inner_margin(12)
        .show(ui, |ui| {
            super::markup_editor::toolbar(ui, id, &mut draft.text);
            ui.add_space(4.0);
            let mut layouter = super::markup_editor::layouter();
            egui::ScrollArea::vertical()
                .id_salt(("draft", target))
                .max_height(140.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.text)
                            .id(id)
                            .layouter(&mut layouter)
                            .hint_text(hint.to_string())
                            .desired_width(ui.available_width())
                            .desired_rows(2)
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin::ZERO)
                            .return_key(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                egui::Key::Enter,
                            )),
                    );
                });
            draft.attachments.show(ui);
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let response = ui.add_enabled(
                    can_send(draft),
                    egui::Button::new("Send").min_size(egui::vec2(56.0, 24.0)),
                );
                send = response.clicked();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Enter sends · Shift+Enter newline")
                                .small()
                                .weak(),
                        )
                        .truncate(),
                    );
                });
            });
        });
    let submitted = (enter || send) && can_send(draft);
    if submitted {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    submitted
}

fn can_send(draft: &Draft) -> bool {
    !draft.attachments.is_preparing()
        && (!draft.text.trim().is_empty() || !draft.attachments.is_empty())
}

// Only plain Enter submits. IME confirmation and key-repeat must never send a message.
pub(super) fn consume_submit(events: &mut Vec<egui::Event>, ime_active: &mut bool) -> bool {
    let mut submit = false;
    let ime_frame = *ime_active
        || events
            .iter()
            .any(|event| matches!(event, egui::Event::Ime(_)));
    events.retain(|event| {
        match event {
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                *ime_active = !text.is_empty()
            }
            egui::Event::Ime(egui::ImeEvent::Commit(_)) => *ime_active = false,
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                modifiers,
                repeat,
                ..
            } if *modifiers == egui::Modifiers::NONE && !ime_frame => {
                submit |= !repeat;
                return false;
            }
            _ => {}
        }
        true
    });
    submit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_continue_and_shortcuts_format_while_typing() {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: None,
        };
        let mut draft = Draft::default();
        ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
        let mut frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    composer(ui, &target, &mut draft, "Message");
                },
            );
            output.textures_delta.clear();
        };
        let key = |key, modifiers, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let shift_enter = vec![
            key(egui::Key::Enter, egui::Modifiers::SHIFT, true),
            key(egui::Key::Enter, egui::Modifiers::SHIFT, false),
        ];
        frame(vec![]);
        frame(vec![
            key(
                egui::Key::Num8,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                true,
            ),
            key(
                egui::Key::Num8,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                false,
            ),
        ]);
        frame(vec![egui::Event::Text("one".into())]);
        frame(shift_enter.clone());
        frame(vec![egui::Event::Text("two".into())]);
        frame(shift_enter.clone());
        frame(shift_enter);
        frame(vec![
            key(egui::Key::B, egui::Modifiers::COMMAND, true),
            key(egui::Key::B, egui::Modifiers::COMMAND, false),
        ]);
        frame(vec![egui::Event::Text("loud".into())]);
        assert_eq!(draft.text, "• one\n• two\n*loud*");
    }

    #[test]
    fn multiline_editor_accepts_shift_enter_and_submits_without_an_extra_newline() {
        let ctx = egui::Context::default();
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: None,
        };
        let mut draft = Draft {
            text: "first".into(),
            ..Draft::default()
        };
        let id = egui::Id::new(("composer", &target));
        ctx.memory_mut(|memory| memory.request_focus(id));
        let mut submitted = false;
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    submitted = composer(ui, &target, &mut draft, "Message");
                },
            );
            output.textures_delta.clear();
        };
        frame(Vec::new());
        frame(vec![enter(egui::Modifiers::SHIFT, false)]);
        frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        }]);
        frame(vec![
            egui::Event::Text("second".into()),
            enter(egui::Modifiers::NONE, false),
        ]);
        assert!(submitted);
        assert_eq!(draft.text, "first\nsecond");
    }

    fn enter(modifiers: egui::Modifiers, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    #[test]
    fn plain_enter_sends_but_shift_enter_stays_in_editor() {
        let mut events = vec![enter(egui::Modifiers::SHIFT, false)];
        assert!(!consume_submit(&mut events, &mut false));
        assert_eq!(events.len(), 1);
        events.push(enter(egui::Modifiers::NONE, false));
        assert!(consume_submit(&mut events, &mut false));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn ime_confirmation_and_repeat_never_submit() {
        let mut events = vec![
            egui::Event::Ime(egui::ImeEvent::Commit("日本語".into())),
            enter(egui::Modifiers::NONE, false),
        ];
        let mut ime = true;
        assert!(!consume_submit(&mut events, &mut ime));
        assert!(!ime);
        assert!(!consume_submit(
            &mut vec![enter(egui::Modifiers::NONE, true)],
            &mut ime
        ));
    }
}
