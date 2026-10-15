use eframe::egui::{self, RichText};

use crate::application::emoji_catalog::{EmojiCatalog, EmojiGlyph};
use crate::domain::message_text::{Block, Inline};
use crate::ui::images::ImageCache;
use crate::ui::theme;

/// Returns whether a text selection is being dragged in these blocks.
pub fn render_blocks(
    ui: &mut egui::Ui,
    blocks: &[Block],
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
    offscreen_selection: Option<&egui::Ui>,
) -> bool {
    let mut selecting = false;
    for block in blocks {
        selecting |= match block {
            Block::Paragraph(inlines) => {
                render_inlines(ui, inlines, emoji, images, offscreen_selection)
            }
            Block::Quote(inlines) => render_quote(ui, inlines, emoji, images, offscreen_selection),
            Block::Code(code) => render_code(ui, code, offscreen_selection),
        };
    }
    selecting
}

pub fn render_emoji(
    ui: &mut egui::Ui,
    name: &str,
    size: f32,
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
) {
    if let Some(image) = emoji.image_url(name).and_then(|url| images.image(&url)) {
        ui.add(image.fit_to_exact_size(egui::vec2(size, size)))
            .on_hover_text(format!(":{name}:"));
        return;
    }
    match emoji.resolve(name) {
        EmojiGlyph::Unicode(glyph) => {
            ui.label(RichText::new(glyph).size(size));
        }
        EmojiGlyph::Image(url) => match images.image(&url) {
            Some(image) => {
                ui.add(image.fit_to_exact_size(egui::vec2(size, size)))
                    .on_hover_text(format!(":{name}:"));
            }
            None => {
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
            }
        },
        EmojiGlyph::Missing(name) => {
            ui.label(RichText::new(format!(":{name}:")).color(theme::TEXT_MUTED));
        }
    }
}

#[derive(Clone, Copy)]
struct TextMetrics {
    size: f32,
    line_height: f32,
}

struct LinkSpan {
    chars: std::ops::Range<usize>,
    label: String,
    url: String,
}

struct EmojiSpan {
    chars: std::ops::Range<usize>,
    name: String,
    size: f32,
}

struct Paragraph {
    job: egui::text::LayoutJob,
    links: Vec<LinkSpan>,
    emojis: Vec<EmojiSpan>,
}

fn paragraph(
    ui: &egui::Ui,
    inlines: &[Inline],
    emoji: &EmojiCatalog,
    metrics: TextMetrics,
) -> Paragraph {
    let mut paragraph = Paragraph {
        job: Default::default(),
        links: vec![],
        emojis: vec![],
    };
    paragraph.job.wrap.max_width = ui.available_width().max(1.0);
    let mut offset = 0;
    for inline in inlines {
        let mut format = egui::TextFormat {
            font_id: egui::FontId::proportional(metrics.size),
            line_height: Some(metrics.line_height),
            color: theme::TEXT,
            valign: egui::Align::Center,
            ..Default::default()
        };
        let text = match inline {
            Inline::Text { text, style } => {
                if style.code {
                    format.font_id = egui::FontId::monospace(metrics.size - 1.0);
                    format.background = theme::CODE_BACKGROUND;
                    format.color = theme::SELF_MENTION_TEXT;
                } else if style.bold {
                    format.font_id.family = theme::bold_family();
                    format.color = theme::TEXT_STRONG;
                }
                format.italics = style.italic;
                if style.strike {
                    format.strikethrough = egui::Stroke::new(1.0, format.color);
                }
                text.clone()
            }
            Inline::Mention { label, is_self } => {
                (format.color, format.background) = if *is_self {
                    (theme::SELF_MENTION_TEXT, theme::SELF_MENTION_BACKGROUND)
                } else {
                    (theme::LINK, theme::MENTION_BACKGROUND)
                };
                label.clone()
            }
            Inline::Link { label, url } => {
                paragraph.links.push(LinkSpan {
                    chars: offset..offset + label.chars().count(),
                    label: label.clone(),
                    url: url.clone(),
                });
                format.color = theme::LINK;
                label.clone()
            }
            Inline::Emoji(name) => {
                if emoji.image_url(name).is_some() {
                    // Reserve one stable inline object; the downloaded image is
                    // painted over it without changing wrapping when it arrives.
                    let size = metrics.size + 3.0;
                    let mut font = egui::FontId::proportional(size);
                    let width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, '\u{fffc}'));
                    if width > 0.0 {
                        font.size *= size / width;
                    }
                    format.font_id = font;
                    format.color = egui::Color32::TRANSPARENT;
                    format.line_height = Some(metrics.line_height.max(size));
                    paragraph.emojis.push(EmojiSpan {
                        chars: offset..offset + 1,
                        name: name.clone(),
                        size,
                    });
                    "\u{fffc}".into()
                } else {
                    format.color = theme::TEXT_MUTED;
                    format!(":{name}:")
                }
            }
        };
        offset += text.chars().count();
        paragraph.job.append(&text, 0.0, format);
    }
    paragraph
}

// Return a separate hit rectangle for each wrapped line. A link spanning lines
// must not cover unrelated text between its first and last positions.
fn span_rects(galley: &egui::Galley, chars: &std::ops::Range<usize>) -> Vec<egui::Rect> {
    let mut offset = 0;
    let mut rects = vec![];
    for row in &galley.rows {
        let start = chars.start.saturating_sub(offset).min(row.glyphs.len());
        let end = chars.end.saturating_sub(offset).min(row.glyphs.len());
        if start < end {
            let x1 = row.glyphs[start].pos.x;
            let x2 = row.glyphs[end - 1].max_x();
            rects.push(egui::Rect::from_min_max(
                row.pos + egui::vec2(x1, 0.0),
                row.pos + egui::vec2(x2, row.size.y),
            ));
        }
        offset += row.glyphs.len() + usize::from(row.ends_with_newline);
        if offset >= chars.end {
            break;
        }
    }
    rects
}

fn render_paragraph(
    ui: &mut egui::Ui,
    inlines: &[Inline],
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
    metrics: TextMetrics,
    offscreen_selection: Option<&egui::Ui>,
) -> bool {
    let paragraph = paragraph(ui, inlines, emoji, metrics);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph.job));
    let response = selectable_label(
        ui,
        egui::Label::new(std::sync::Arc::clone(&galley)),
        offscreen_selection,
    );
    let origin = response.rect.min.to_vec2();
    for (link_index, link) in paragraph.links.iter().enumerate() {
        let rects: Vec<_> = span_rects(&galley, &link.chars)
            .into_iter()
            .map(|rect| rect.translate(origin))
            .collect();
        // Keep precise per-line hit areas, but expose one logical link: one
        // focus stop, accessibility node, tooltip and hover state for the URL.
        let link_response = rects
            .iter()
            .enumerate()
            .map(|(row_index, &rect)| {
                ui.interact(
                    rect,
                    response.id.with(("link", link_index, row_index)),
                    if row_index == 0 {
                        egui::Sense::click()
                    } else {
                        egui::Sense::CLICK
                    },
                )
            })
            .reduce(|combined, row| combined.union(row));
        let Some(link_response) = link_response else {
            continue;
        };
        let link_response = link_response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(&link.url);
        link_response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Link, ui.is_enabled(), &link.label)
        });
        if link_response.hovered() || link_response.has_focus() {
            for rect in rects {
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 1.0,
                    egui::Stroke::new(1.0, theme::LINK),
                );
            }
        }
        if link_response.clicked() || link_response.middle_clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
        }
    }
    for emoji_span in &paragraph.emojis {
        if let Some(rect) = span_rects(&galley, &emoji_span.chars).first() {
            let rect = egui::Rect::from_center_size(
                rect.translate(origin).center(),
                egui::Vec2::splat(emoji_span.size),
            );
            if ui.is_rect_visible(rect)
                && let Some(image) = emoji
                    .image_url(&emoji_span.name)
                    .and_then(|url| images.image(&url))
            {
                image.paint_at(ui, rect);
            }
        }
    }
    response.dragged_by(egui::PointerButton::Primary)
}

fn render_inlines(
    ui: &mut egui::Ui,
    inlines: &[Inline],
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
    offscreen_selection: Option<&egui::Ui>,
) -> bool {
    let emoji_only = inlines.iter().all(|inline| match inline {
        Inline::Emoji(_) => true,
        Inline::Text { text, .. } => text.trim().is_empty(),
        _ => false,
    }) && inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Emoji(_)));
    if emoji_only {
        ui.horizontal_wrapped(|ui| {
            for inline in inlines {
                if let Inline::Emoji(name) = inline {
                    render_emoji(ui, name, LARGE_EMOJI_SIZE, emoji, images);
                }
            }
        });
        false
    } else {
        render_paragraph(
            ui,
            inlines,
            emoji,
            images,
            TextMetrics {
                size: 13.5,
                line_height: 18.0,
            },
            offscreen_selection,
        )
    }
}

/// Detail text gets comfortable leading and space between explicit source lines.
/// Styles and link targets stay intact; no content is rewritten or shortened.
pub fn render_detail_blocks(
    ui: &mut egui::Ui,
    blocks: &[Block],
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
) -> bool {
    // egui's normal Label drops selection endpoints when they leave the clip
    // rect. Keep offscreen text in the selection model, with its real geometry,
    // using an invisible painter so it cannot draw over the pane's header/footer.
    let mut offscreen_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("offscreen_selection")
            .invisible(),
    );
    offscreen_ui.set_clip_rect(egui::Rect::EVERYTHING);
    let mut selecting = false;
    for block in blocks {
        match block {
            Block::Paragraph(inlines) => {
                for line in inline_lines(inlines) {
                    if line.is_empty() {
                        ui.add_space(6.0);
                        continue;
                    }
                    let heading = line
                        .iter()
                        .any(|inline| matches!(inline, Inline::Text { style, .. } if style.bold))
                        && line.iter().all(|inline| match inline {
                            Inline::Text { text, style } => style.bold || text.trim().is_empty(),
                            Inline::Emoji(_) => true,
                            _ => false,
                        });
                    let metrics = if heading {
                        TextMetrics {
                            size: 15.0,
                            line_height: 22.0,
                        }
                    } else {
                        TextMetrics {
                            size: 13.5,
                            line_height: 20.0,
                        }
                    };
                    selecting |=
                        render_paragraph(ui, &line, emoji, images, metrics, Some(&offscreen_ui));
                    if heading {
                        ui.add_space(6.0);
                    }
                }
            }
            Block::Quote(inlines) => {
                selecting |= render_quote(ui, inlines, emoji, images, Some(&offscreen_ui));
            }
            Block::Code(code) => selecting |= render_code(ui, code, Some(&offscreen_ui)),
        }
    }
    selecting
}

fn selectable_label(
    ui: &mut egui::Ui,
    label: egui::Label,
    offscreen_selection: Option<&egui::Ui>,
) -> egui::Response {
    let label = label.selectable(true);
    let Some(offscreen_ui) = offscreen_selection else {
        return ui.add(label);
    };
    let (pos, galley, response) = label.layout_in_ui(ui);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), galley.text())
    });
    let selection_ui = if ui.is_rect_visible(response.rect) {
        ui
    } else {
        offscreen_ui
    };
    egui::text_selection::LabelSelectionState::label_text_selection(
        selection_ui,
        &response,
        pos,
        galley,
        theme::TEXT,
        egui::Stroke::NONE,
    );
    response
}

/// Call at the end of a scroll area's contents. Only runs while dragging text
/// in this pane; never animates an idle window. Scrolling through
/// `scroll_with_delta` also releases a stick-to-bottom timeline.
pub fn scroll_text_selection(ui: &egui::Ui, selecting: bool) {
    if !selecting {
        return;
    }
    let Some((pointer, dt)) = ui.input(|input| {
        input
            .pointer
            .interact_pos()
            .map(|pos| (pos, input.stable_dt.min(0.05)))
    }) else {
        return;
    };
    let rect = ui.clip_rect();
    if pointer.x < rect.left() - 24.0 || pointer.x > rect.right() + 24.0 {
        return;
    }
    let edge = 28.0_f32.min(rect.height() / 3.0);
    let speed = if pointer.y < rect.top() + edge {
        -((rect.top() + edge - pointer.y) / edge).clamp(0.0, 3.0) * 180.0
    } else if pointer.y > rect.bottom() - edge {
        ((pointer.y - rect.bottom() + edge) / edge).clamp(0.0, 3.0) * 180.0
    } else {
        0.0
    };
    if speed != 0.0 {
        // A positive delta moves the content down, i.e. scrolls up.
        ui.scroll_with_delta_animation(
            egui::vec2(0.0, -speed * dt),
            egui::style::ScrollAnimation::none(),
        );
        ui.ctx().request_repaint();
    }
}

fn inline_lines(inlines: &[Inline]) -> Vec<Vec<Inline>> {
    let mut lines = vec![vec![]];
    for inline in inlines {
        if let Inline::Text { text, style } = inline {
            for (index, piece) in text.split('\n').enumerate() {
                if index > 0 {
                    lines.push(vec![]);
                }
                if !piece.is_empty() {
                    lines.last_mut().unwrap().push(Inline::Text {
                        text: piece.to_owned(),
                        style: *style,
                    });
                }
            }
        } else {
            lines.last_mut().unwrap().push(inline.clone());
        }
    }
    lines
}

fn render_quote(
    ui: &mut egui::Ui,
    inlines: &[Inline],
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
    offscreen_selection: Option<&egui::Ui>,
) -> bool {
    let output = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 0,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| {
            render_inlines(ui, inlines, emoji, images, offscreen_selection)
        });
    let rect = output.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(QUOTE_BAR_WIDTH, rect.height())),
        2.0,
        theme::QUOTE_BAR,
    );
    output.inner
}

fn render_code(ui: &mut egui::Ui, code: &str, offscreen_selection: Option<&egui::Ui>) -> bool {
    egui::Frame::new()
        .fill(theme::CODE_BACKGROUND)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            selectable_label(
                ui,
                egui::Label::new(RichText::new(code).monospace()).wrap(),
                offscreen_selection,
            )
            .dragged_by(egui::PointerButton::Primary)
        })
        .inner
}

const LARGE_EMOJI_SIZE: f32 = 32.0;
const QUOTE_BAR_WIDTH: f32 = 3.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::chat_service::ChatService;
    use crate::domain::message_text::TextStyle;
    use crate::infrastructure::demo::DemoGateway;
    use std::sync::Arc;

    fn text(value: &str) -> Inline {
        Inline::Text {
            text: value.into(),
            style: Default::default(),
        }
    }

    #[test]
    fn selection_survives_scrolling_offscreen_and_dragging_scrolls_the_pane() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let emoji = EmojiCatalog::default();
        let blocks = vec![Block::Paragraph(vec![text(
            &(0..60)
                .map(|i| format!("Line {i:02} of the report"))
                .collect::<Vec<_>>()
                .join("\n"),
        )])];
        let mut images = ImageCache::new(
            Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        let mut time = 0.0;
        let mut frame = |events| {
            time += 1.0 / 60.0;
            let mut offset = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    egui::Frame::new().inner_margin(16).show(ui, |ui| {
                        let scroll = egui::ScrollArea::vertical()
                            .id_salt("detail_test")
                            .max_height(200.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let selecting =
                                    render_detail_blocks(ui, &blocks, &emoji, &mut images);
                                scroll_text_selection(ui, selecting);
                            });
                        offset = egui::scroll_area::State::load(ui.ctx(), scroll.id)
                            .unwrap()
                            .offset
                            .y;
                    });
                },
            );
            output.textures_delta.clear();
            (offset, output.platform_output.commands)
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![]);
        let start = egui::pos2(17.0, 25.0);
        frame(vec![egui::Event::PointerMoved(start), button(start, true)]);
        let bottom = egui::pos2(100.0, 230.0);
        frame(vec![egui::Event::PointerMoved(bottom)]);
        for _ in 0..90 {
            frame(vec![]);
        }
        let (offset, commands) = frame(vec![egui::Event::Copy]);
        assert!(offset > 200.0, "drag at the bottom must scroll: {offset}");
        let copied = commands
            .iter()
            .find_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .expect("the offscreen anchor must remain selected");
        assert!(copied.starts_with("Line 00"), "{copied}");
        assert!(copied.contains("Line 15"), "{copied}");
        assert!(
            !copied.contains("Line 59"),
            "do not select hidden text below the drag"
        );
        frame(vec![button(bottom, false)]);
        let (stopped, _) = frame(vec![]);
        assert_eq!(frame(vec![]).0, stopped, "stop scrolling on release");
        let (_, commands) = frame(vec![egui::Event::Copy]);
        assert!(commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text.starts_with("Line 00"))));
        let top = egui::pos2(100.0, 5.0);
        frame(vec![
            egui::Event::PointerMoved(egui::pos2(100.0, 170.0)),
            button(egui::pos2(100.0, 170.0), true),
        ]);
        frame(vec![egui::Event::PointerMoved(top)]);
        for _ in 0..90 {
            frame(vec![]);
        }
        let (up, commands) = frame(vec![egui::Event::Copy]);
        assert!(up < stopped - 200.0, "drag at the top must scroll up");
        assert!(commands.iter().any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text.contains("Line 10"))));
    }

    #[test]
    fn formatted_text_preserves_spaces_and_wrapped_link_hit_areas_stay_inside_gutters() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let emoji = EmojiCatalog::default();
        let url = format!(
            "https://example.test/compare?slug={}",
            "very-long-name-".repeat(18)
        );
        let inlines = vec![
            Inline::Text {
                text: "Åsa’s reference:".into(),
                style: TextStyle {
                    bold: true,
                    ..Default::default()
                },
            },
            text(" code "),
            Inline::Text {
                text: "run --port 4173".into(),
                style: TextStyle {
                    code: true,
                    ..Default::default()
                },
            },
            text(" and "),
            Inline::Link {
                label: url.clone(),
                url: url.clone(),
            },
            text(".\nNext line."),
        ];
        let mut images = ImageCache::new(
            Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        for width in [220.0, 390.0, 640.0] {
            let mut click_point = egui::Pos2::ZERO;
            {
                let mut frame = |events| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width + 32.0, 900.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            egui::Frame::new().inner_margin(16).show(ui, |ui| {
                            let metrics = TextMetrics {
                                size: 13.5,
                                line_height: 20.0,
                            };
                            let paragraph = paragraph(ui, &inlines, &emoji, metrics);
                            assert_eq!(
                                paragraph.job.text,
                                format!(
                                    "Åsa’s reference: code run --port 4173 and {url}.\nNext line."
                                )
                            );
                            assert_eq!(
                                paragraph.job.sections[0].format.font_id.family,
                                theme::bold_family()
                            );
                            let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph.job));
                            let rects = span_rects(&galley, &paragraph.links[0].chars);
                            assert!(rects.len() > 1, "long links must wrap");
                            for rect in &rects {
                                assert!(
                                    rect.left() >= 0.0 && rect.right() <= width + 0.5,
                                    "{rect:?} outside {width}"
                                );
                            }
                            // Use the second wrapped line, not just the link's start.
                            click_point = rects[1].center() + ui.next_widget_position().to_vec2();
                            render_paragraph(ui, &inlines, &emoji, &mut images, metrics, None);
                        });
                        },
                    );
                    output.textures_delta.clear();
                    output.platform_output.commands
                };
                frame(vec![]);
            }

            let events = vec![
                egui::Event::PointerMoved(click_point),
                egui::Event::PointerButton {
                    pos: click_point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: click_point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width + 32.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Frame::new().inner_margin(16).show(ui, |ui| {
                        render_paragraph(
                            ui,
                            &inlines,
                            &emoji,
                            &mut images,
                            TextMetrics {
                                size: 13.5,
                                line_height: 20.0,
                            },
                            None,
                        );
                    });
                },
            );
            assert!(
                output
                    .platform_output
                    .commands
                    .iter()
                    .any(|command| matches!(command,
                egui::OutputCommand::OpenUrl(open) if open.url == url)),
                "wrapped link should open its original URL"
            );
            output.textures_delta.clear();
        }
    }

    #[test]
    fn wrapped_url_has_one_hover_state_focus_target_and_complete_destination() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.enable_accesskit();
        let emoji = EmojiCatalog::default();
        let url = "https://workspace.example.test/archives/C0123456789/p1791952046722489?thread_ts=1791951637.248989&cid=C0123456789";
        let inlines = vec![
            text("Åsa: "),
            Inline::Link {
                label: url.into(),
                url: url.into(),
            },
            text(" after the URL"),
        ];
        let mut images = ImageCache::new(
            Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        for width in [220.0, 390.0, 640.0] {
            let mut frame = |events| {
                let mut rects = vec![];
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width + 32.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        egui::Frame::new().inner_margin(16).show(ui, |ui| {
                            let metrics = TextMetrics {
                                size: 13.5,
                                line_height: 20.0,
                            };
                            let paragraph = paragraph(ui, &inlines, &emoji, metrics);
                            let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph.job));
                            rects = span_rects(&galley, &paragraph.links[0].chars)
                                .into_iter()
                                .map(|rect| rect.translate(ui.next_widget_position().to_vec2()))
                                .collect();
                            render_paragraph(ui, &inlines, &emoji, &mut images, metrics, None);
                        });
                    },
                );
                output.textures_delta.clear();
                (output, rects)
            };
            let (output, rects) = frame(vec![]);
            assert!(rects.len() >= 2);
            let nodes = &output.platform_output.accesskit_update.unwrap().nodes;
            let links: Vec<_> = nodes
                .iter()
                .filter(|(_, node)| node.role() == egui::accesskit::Role::Link)
                .collect();
            assert_eq!(links.len(), 1, "one accessible link regardless of wrapping");
            let link_node = links[0].0;
            for rect in &rects {
                let point = rect.center();
                let (hovered, _) = frame(vec![egui::Event::PointerMoved(point)]);
                let underlines = hovered
                    .shapes
                    .iter()
                    .filter(|shape| {
                        matches!(&shape.shape,
                    egui::Shape::LineSegment { stroke, .. } if stroke.color == theme::LINK)
                    })
                    .count();
                assert_eq!(
                    underlines,
                    rects.len(),
                    "hovering any row should underline the whole URL"
                );
                let (clicked, _) = frame(vec![
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    },
                ]);
                let opened: Vec<_> = clicked
                    .platform_output
                    .commands
                    .iter()
                    .filter_map(|command| match command {
                        egui::OutputCommand::OpenUrl(open) => Some(open.url.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    opened,
                    [url],
                    "every row must open exactly the complete URL"
                );
            }
            // The shared hover state must not turn surrounding prose into a link.
            let point = egui::pos2(20.0, 26.0);
            assert!(rects.iter().all(|rect| !rect.contains(point)));
            let (output, _) = frame(vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]);
            assert!(
                !output
                    .platform_output
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::OutputCommand::OpenUrl(_)))
            );
            for _ in 0..3 {
                frame(vec![egui::Event::Key {
                    key: egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }]);
                if ctx.memory(|memory| memory.focused().map(|id| id.accesskit_id()))
                    == Some(link_node)
                {
                    break;
                }
            }
            assert_eq!(
                ctx.memory(|memory| memory.focused().map(|id| id.accesskit_id())),
                Some(link_node)
            );
            let (output, _) = frame(vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }]);
            assert!(output.platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::OpenUrl(open) if open.url == url)
            ));
            ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
    }

    #[test]
    fn detail_line_breaks_keep_styles_links_and_emoji_reserve_space() {
        let inlines = vec![
            text("One\n\n"),
            Inline::Text {
                text: "Label:".into(),
                style: TextStyle {
                    bold: true,
                    ..Default::default()
                },
            },
            text(" value "),
            Inline::Link {
                label: "docs".into(),
                url: "https://example.test".into(),
            },
            Inline::Emoji("tada".into()),
            text(" after"),
        ];
        let lines = inline_lines(&inlines);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].is_empty());
        assert!(matches!(&lines[2][0], Inline::Text { style, .. } if style.bold));
        assert!(matches!(&lines[2][2], Inline::Link { url, .. } if url == "https://example.test"));
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let paragraph = paragraph(
                ui,
                &lines[2],
                &EmojiCatalog::default(),
                TextMetrics {
                    size: 13.5,
                    line_height: 20.0,
                },
            );
            let span = &paragraph.emojis[0];
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph.job));
            let rect = span_rects(&galley, &span.chars)[0];
            assert!(
                (rect.width() - span.size).abs() < 1.0,
                "{rect:?} must reserve {} points",
                span.size
            );
            assert!(galley.text().ends_with(" after"));
        });
        output.textures_delta.clear();
    }
}
