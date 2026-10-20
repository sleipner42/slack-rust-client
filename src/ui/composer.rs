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
    /// People picked from the `@` suggestions, mentioned when sent.
    pub mentions: Vec<super::emoji_completion::Mention>,
    ime_active: bool,
}

pub fn composer(
    ui: &mut egui::Ui,
    target: &ComposeTarget,
    draft: &mut Draft,
    hint: &str,
    mut completion: Option<super::emoji_completion::EmojiCompletion<'_>>,
) -> bool {
    draft.attachments.poll(ui.ctx());
    let id = target.editor_id();
    super::markup_editor::restore_focus(ui, id);
    let focused = ui.is_enabled() && ui.memory(|memory| memory.has_focus(id));
    if focused {
        if let Some(completion) = &completion
            && let Some(mention) =
                super::emoji_completion::keys(ui, id, &mut draft.text, completion)
        {
            draft.mentions.push(mention);
        }
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
                    let output = egui::TextEdit::multiline(&mut draft.text)
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
                        ))
                        .show(ui);
                    super::markup_editor::spelling(ui, &output, id, &mut draft.text);
                    // Scroll the draft at its edges, or with the wheel, while selecting.
                    super::rich_text::scroll_text_selection(
                        ui,
                        output.response.dragged_by(egui::PointerButton::Primary),
                    );
                    if let Some(completion) = &mut completion
                        && let Some(mention) = super::emoji_completion::popup(
                            ui,
                            &output,
                            id,
                            &mut draft.text,
                            completion,
                        )
                    {
                        draft.mentions.push(mention);
                    }
                });
            draft.attachments.show(ui);
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let response = ui.add_enabled(
                    can_send(draft),
                    egui::Button::new("Send").min_size(egui::vec2(56.0, 24.0)),
                );
                send = response.clicked();
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
                    composer(ui, &target, &mut draft, "Message", None);
                },
            );
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "I" => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
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
        let italic = frame(vec![]).expect("toolbar drawn");
        let at = italic.center();
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(at)]);
        frame(vec![click(true)]);
        frame(vec![click(false)]);
        frame(vec![]);
        frame(vec![egui::Event::Text("soft".into())]);
        assert_eq!(draft.text, "• one\n• two\n*loud_soft_*");
    }

    #[test]
    fn misspelled_words_are_underlined_and_corrected_from_the_context_menu() {
        let checker = crate::infrastructure::spell_checker::load_now();
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: None,
        };
        let mut draft = Draft {
            text: "I recieve the mail".into(),
            ..Default::default()
        };
        // Shapes of a frame: squiggles drawn, and where a text sits.
        let mut frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    composer(ui, &target, &mut draft, "Message", None);
                },
            );
            output.textures_delta.clear();
            let squiggles = output
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(&clipped.shape, egui::Shape::Path(path)
                        if path.stroke.color == egui::epaint::ColorMode::Solid(crate::ui::theme::SPELLING))
                })
                .count();
            let texts: Vec<_> = output
                .shapes
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Text(text) => Some(text),
                    _ => None,
                })
                .collect();
            (squiggles, texts, draft.text.clone())
        };
        frame(vec![]);
        let (squiggles, texts, _) = frame(vec![]);
        assert_eq!(squiggles, 1, "only recieve is misspelled");
        let editor = texts
            .iter()
            .find(|text| text.galley.text() == "I recieve the mail")
            .unwrap();
        let word = editor.pos
            + editor
                .galley
                .pos_from_cursor(egui::text::CCursor::new(4))
                .center()
                .to_vec2();
        let button = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(word)]);
        frame(vec![button(word, egui::PointerButton::Secondary, true)]);
        frame(vec![button(word, egui::PointerButton::Secondary, false)]);
        let (_, texts, _) = frame(vec![]);
        let suggestion = texts
            .iter()
            .find(|text| text.galley.text() == "receive")
            .expect("the menu suggests receive")
            .visual_bounding_rect()
            .center();
        frame(vec![egui::Event::PointerMoved(suggestion)]);
        frame(vec![button(suggestion, egui::PointerButton::Primary, true)]);
        frame(vec![button(
            suggestion,
            egui::PointerButton::Primary,
            false,
        )]);
        let (squiggles, _, text) = frame(vec![]);
        assert_eq!(text, "I receive the mail");
        assert_eq!(squiggles, 0);
        assert!(checker.check("receive"));
    }

    #[test]
    fn typing_a_colon_or_at_suggests_and_enter_inserts_instead_of_sending() {
        use crate::application::chat_service::ChatService;
        use crate::application::emoji_catalog::EmojiCatalog;
        use crate::infrastructure::demo::DemoGateway;
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let emoji = EmojiCatalog::new(std::collections::HashMap::from([(
            "partyparrot".to_string(),
            "https://e/parrot.gif".to_string(),
        )]));
        let mut images = crate::ui::images::ImageCache::new(
            std::sync::Arc::new(ChatService::new(std::sync::Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: None,
        };
        let mut draft = Draft::default();
        let people = [crate::domain::people::Person {
            id: "U1".into(),
            name: "Axel Wennström".into(),
            real_name: "Axel Wennström".into(),
            handle: "axel".into(),
            avatar_url: None,
        }];
        ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
        let mut sent = false;
        let mut frame = |events: Vec<egui::Event>| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused: true,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.add_space(250.0);
                    let completion = crate::ui::emoji_completion::EmojiCompletion {
                        emoji: &emoji,
                        people: &people,
                        preferred: &[],
                        images: &mut images,
                    };
                    sent |= composer(ui, &target, &mut draft, "Message", Some(completion));
                },
            );
            output.textures_delta.clear();
            let shown = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape,
                    egui::Shape::Text(text) if text.galley.text() == ":partyparrot:")
            });
            (shown, draft.text.clone(), sent, draft.mentions.clone())
        };
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![]);
        frame(vec![egui::Event::Text("hi :partyp".into())]);
        assert!(frame(vec![]).0, "the custom emoji is suggested");
        frame(vec![enter(true), enter(false)]);
        let (shown, text, sent_now, _) = frame(vec![]);
        assert_eq!(text, "hi :partyparrot: ");
        assert!(!sent_now, "Enter picked the emoji instead of sending");
        assert!(!shown, "the list closes after inserting");
        frame(vec![egui::Event::Text("@ax".into())]);
        frame(vec![]);
        frame(vec![enter(true), enter(false)]);
        let (_, text, sent_now, mentions) = frame(vec![]);
        assert_eq!(text, "hi :partyparrot: @Axel Wennström ");
        assert!(!sent_now, "Enter picked the person instead of sending");
        assert_eq!(
            crate::domain::message_text::link_mentions(&text, &mentions),
            "hi :partyparrot: <@U1> ",
            "the name is sent as a mention"
        );
        assert!(
            frame(vec![enter(true), enter(false)]).2,
            "the next Enter sends"
        );
    }

    #[test]
    fn dragging_a_selection_past_the_draft_or_turning_the_wheel_scrolls_it() {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: None,
        };
        let mut draft = Draft {
            text: (0..30)
                .map(|line| format!("line {line:02}"))
                .collect::<Vec<_>>()
                .join("\n"),
            ..Default::default()
        };
        let mut time = 0.0;
        // Where the draft's text sits; it moves up as the box scrolls down.
        let mut frame = |events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    composer(ui, &target, &mut draft, "Message", None);
                },
            );
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with("line 00") => {
                        Some(text.pos)
                    }
                    _ => None,
                })
                .expect("the draft is drawn")
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![]);
        let top = frame(vec![]);
        let start = top + egui::vec2(10.0, 8.0);
        frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
        let below = egui::pos2(start.x, 380.0);
        frame(vec![egui::Event::PointerMoved(below)]);
        for _ in 0..60 {
            frame(vec![]);
        }
        let dragged = frame(vec![]);
        assert!(
            dragged.y < top.y - 100.0,
            "dragging below the draft scrolls it: {top:?} -> {dragged:?}"
        );

        let middle = egui::pos2(start.x, top.y + 40.0);
        frame(vec![egui::Event::PointerMoved(middle)]);
        let held = frame(vec![]);
        frame(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..30 {
            frame(vec![]);
        }
        let wheeled = frame(vec![]);
        assert!(
            wheeled.y > held.y + 60.0,
            "the wheel scrolls back up while selecting: {held:?} -> {wheeled:?}"
        );
        frame(vec![button(middle, false)]);
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
                    submitted = composer(ui, &target, &mut draft, "Message", None);
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
