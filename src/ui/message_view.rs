use chrono::{DateTime, Local, TimeZone};
use eframe::egui::{self, RichText};

use super::outbox::{Delivery, Outbox};
use crate::application::chat_service::DisplayMessage;
use crate::application::emoji_catalog::{EmojiCatalog, EmojiGlyph};
use crate::domain::model::{Attachment, Reaction, ts_seconds};
use crate::ui::images::ImageCache;
use crate::ui::rich_text::render_blocks;
use crate::ui::theme;

pub enum MessageAction {
    RetrySend(String),
    OpenImage(Attachment),
    OpenThread(String),
    ToggleReaction { ts: String, name: String, add: bool },
    PickReaction(String),
}

pub struct MessageContext<'a> {
    pub emoji: &'a EmojiCatalog,
    pub images: &'a mut ImageCache,
    pub self_user_id: &'a str,
    pub allow_threads: bool,
    pub outbox: &'a Outbox,
}

#[derive(Clone, Default)]
struct RowLayoutCache {
    width: f32,
    rows: std::collections::HashMap<String, (RowSignature, f32)>,
    toolbar: Option<HoveredToolbar>,
}

#[derive(Clone)]
struct HoveredToolbar {
    ts: String,
    rect: egui::Rect,
}

#[derive(Default)]
struct ToolbarHover {
    previous: Option<HoveredToolbar>,
    current: Option<HoveredToolbar>,
}

pub(super) const QUICK_REACTIONS: [(&str, &str); 3] = [
    ("+1", "Thumbs up"),
    ("white_check_mark", "Check mark"),
    ("question", "Question mark"),
];

type RowSignature = (u64, bool, bool, u8);

pub fn message_list(
    ui: &mut egui::Ui,
    id_salt: &str,
    messages: &[DisplayMessage],
    context: &mut MessageContext<'_>,
) -> Option<MessageAction> {
    let mut action = None;
    let mut previous: Option<&DisplayMessage> = None;
    let cache_id = ui.id().with(("message_layout", id_salt));
    let mut cache = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<RowLayoutCache>(cache_id))
        .unwrap_or_default();
    let width = ui.available_width();
    if (cache.width - width).abs() > 0.5 {
        cache.rows.clear();
        cache.toolbar = None;
        cache.width = width;
    }
    let mut toolbar_hover = ToolbarHover {
        previous: cache.toolbar.take(),
        current: None,
    };
    let mut measured = false;
    for message in messages {
        let time = local_time(&message.ts);
        let day = previous
            .is_none_or(|previous| local_time(&previous.ts).date_naive() != time.date_naive());
        let compact = !day && previous.is_some_and(|previous| can_group(previous, message));
        let signature = (
            message.layout_key,
            compact,
            day,
            context
                .outbox
                .delivery(message)
                .map_or(0, Delivery::layout_key),
        );
        if let Some((cached, height)) = cache.rows.get(&message.ts).copied()
            && cached == signature
        {
            let rect =
                egui::Rect::from_min_size(ui.next_widget_position(), egui::vec2(width, height));
            if !ui.is_rect_visible(rect.expand2(egui::vec2(0.0, 160.0))) {
                ui.allocate_exact_size(rect.size(), egui::Sense::hover());
                previous = Some(message);
                continue;
            }
        }
        let response = ui.push_id((id_salt, &message.ts), |ui| {
            if day {
                day_separator(ui, &time);
            }
            if let Some(row_action) =
                message_row(ui, message, compact, &time, context, &mut toolbar_hover)
            {
                action = Some(row_action);
            }
        });
        let height = response.response.rect.height();
        measured |= cache
            .rows
            .get(&message.ts)
            .is_none_or(|(old, old_height)| *old != signature || (height - old_height).abs() > 0.5);
        cache.rows.insert(message.ts.clone(), (signature, height));
        previous = Some(message);
    }
    if cache.rows.len() > messages.len() + 32 {
        cache
            .rows
            .retain(|ts, _| messages.iter().any(|message| &message.ts == ts));
    }
    if let Some(hovered) = &mut toolbar_hover.current
        && let Some(message) = messages.iter().find(|message| message.ts == hovered.ts)
    {
        let (toolbar_action, rect) = message_toolbar(ui, message, hovered.rect, context);
        if toolbar_action.is_some() {
            action = toolbar_action;
        }
        hovered.rect = rect;
    }
    if toolbar_hover.previous.is_some() && toolbar_hover.current.is_none() {
        // A scroll can move the old toolbar away from the pointer. Let the row
        // now beneath it pick up hover on the next frame.
        ui.ctx().request_repaint();
    }
    cache.toolbar = toolbar_hover.current;
    ui.ctx().data_mut(|data| data.insert_temp(cache_id, cache));
    if measured {
        // Settle content size and the saved/bottom scroll position before presenting the frame.
        ui.ctx().request_discard("message layout changed");
    }
    action
}

pub fn loading_messages(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let rows = (ui.available_height() / 76.0).floor().clamp(1.0, 8.0) as usize;
    for index in 0..rows {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 68.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(28.0, 28.0)),
            6.0,
            theme::SURFACE,
        );
        let left = rect.min + egui::vec2(40.0, 2.0);
        painter.rect_filled(
            egui::Rect::from_min_size(left, egui::vec2(92.0, 10.0)),
            3.0,
            theme::SURFACE,
        );
        for line in 0..2 {
            let fraction = if (index + line) % 2 == 0 { 0.68 } else { 0.45 };
            painter.rect_filled(
                egui::Rect::from_min_size(
                    left + egui::vec2(0.0, 22.0 + line as f32 * 16.0),
                    egui::vec2((width - 40.0).max(1.0) * fraction, 9.0),
                ),
                3.0,
                theme::SURFACE,
            );
        }
    }
}

fn can_group(previous: &DisplayMessage, message: &DisplayMessage) -> bool {
    let delta = ts_seconds(&message.ts) - ts_seconds(&previous.ts);
    previous.author_key == message.author_key
        && (0.0..GROUP_WINDOW_SECONDS).contains(&delta)
        && local_time(&previous.ts).date_naive() == local_time(&message.ts).date_naive()
}

fn message_row(
    ui: &mut egui::Ui,
    message: &DisplayMessage,
    compact: bool,
    time: &DateTime<Local>,
    context: &mut MessageContext<'_>,
    toolbar_hover: &mut ToolbarHover,
) -> Option<MessageAction> {
    let mut action = None;
    let delivery = context.outbox.delivery(message);
    let confirmed = delivery.is_none_or(|state| matches!(state, Delivery::Confirmed));
    ui.add_space(if compact { 0.0 } else { 8.0 });
    let background = ui.painter().add(egui::Shape::Noop);
    let response = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(0, if compact { 2 } else { 6 }))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if compact {
                    ui.add_sized(
                        [AVATAR_SIZE, 18.0],
                        egui::Label::new(
                            RichText::new(time.format("%H:%M").to_string())
                                .size(9.0)
                                .color(theme::TEXT_MUTED),
                        ),
                    )
                    .on_hover_text(time.format("%A, %B %e, %Y at %H:%M:%S").to_string());
                } else {
                    avatar(ui, message, context.images);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                    ui.set_width(ui.available_width().max(1.0));
                    if !compact {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(&message.author)
                                    .strong()
                                    .color(theme::TEXT_STRONG),
                            );
                            ui.label(
                                RichText::new(time.format("%H:%M").to_string())
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            )
                            .on_hover_text(time.format("%A, %B %e, %Y at %H:%M:%S").to_string());
                        });
                    }
                    render_blocks(ui, &message.blocks, context.emoji, context.images);
                    match delivery {
                        Some(Delivery::Sending) => {
                            let label = if message.attachments.is_empty() {
                                "Sending…"
                            } else {
                                "Uploading attachments…"
                            };
                            ui.label(RichText::new(label).small().weak());
                        }
                        Some(Delivery::Failed(error)) => {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Send not confirmed")
                                        .small()
                                        .color(theme::UNREAD_DOT),
                                )
                                .on_hover_text(format!(
                                    "{error}\nCheck the conversation before retrying."
                                ));
                                if ui.small_button("Retry").clicked() {
                                    action =
                                        message.client_msg_id.clone().map(MessageAction::RetrySend);
                                }
                            });
                        }
                        _ => {}
                    }
                    if message.edited {
                        ui.label(RichText::new("(edited)").small().color(theme::TEXT_MUTED));
                    }
                    for attachment in &message.attachments {
                        ui.add_space(4.0);
                        if attachment_view(ui, attachment, context.images) {
                            action = Some(MessageAction::OpenImage(attachment.clone()));
                        }
                    }
                    if !message.reactions.is_empty()
                        || (context.allow_threads && message.reply_count > 0)
                    {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for reaction in &message.reactions {
                                if reaction_button(ui, reaction, context).clicked() {
                                    action = Some(MessageAction::ToggleReaction {
                                        ts: message.ts.clone(),
                                        name: reaction.name.clone(),
                                        add: !reaction
                                            .user_ids
                                            .iter()
                                            .any(|id| id == context.self_user_id),
                                    });
                                }
                            }
                            if context.allow_threads && message.reply_count > 0 {
                                let label = match message.reply_count {
                                    0 => "Reply".into(),
                                    1 => "1 reply".into(),
                                    n => format!("{n} replies"),
                                };
                                if ui.link(label).on_hover_text("Open thread").clicked() {
                                    action = Some(MessageAction::OpenThread(
                                        message.thread_ts.as_ref().unwrap_or(&message.ts).clone(),
                                    ));
                                }
                            }
                        });
                    }
                });
            })
        })
        .response;
    let toolbar_width = if context.allow_threads { 202.0 } else { 154.0 };
    let toolbar_rect = egui::Rect::from_min_size(
        egui::pos2(
            (response.rect.right() - toolbar_width).max(ui.clip_rect().left()),
            response.rect.top().max(ui.clip_rect().top()),
        ),
        egui::vec2(toolbar_width, 34.0),
    );
    // Retain hover while moving into the floating controls, including on short
    // grouped messages. Neighbouring rows must not take the toolbar away.
    let held_toolbar = toolbar_hover
        .previous
        .as_ref()
        .filter(|previous| ui.rect_contains_pointer(previous.rect));
    let hovered = match held_toolbar {
        Some(previous) => {
            previous.ts == message.ts
                && (ui.rect_contains_pointer(toolbar_rect)
                    || ui.rect_contains_pointer(response.rect))
        }
        None => ui.rect_contains_pointer(response.rect),
    };
    if confirmed && ui.is_enabled() && hovered {
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                response.rect.expand2(egui::vec2(6.0, 0.0)),
                2.0,
                theme::HOVER,
            ),
        );
        toolbar_hover.current = Some(HoveredToolbar {
            ts: message.ts.clone(),
            rect: toolbar_rect,
        });
    }
    response.context_menu(|ui| {
        if ui.button("Copy message text").clicked() {
            ui.ctx()
                .copy_text(crate::domain::message_text::plain_text(&message.blocks));
            ui.close();
        }
    });
    action
}

// Draw after all message rows so short grouped rows cannot paint over the controls.
// A separate child UI keeps the toolbar entirely out of the message layout.
fn message_toolbar(
    ui: &mut egui::Ui,
    message: &DisplayMessage,
    toolbar_rect: egui::Rect,
    context: &mut MessageContext<'_>,
) -> (Option<MessageAction>, egui::Rect) {
    let mut action = None;
    let mut toolbar = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("message_actions", &message.ts))
            .max_rect(toolbar_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    toolbar.spacing_mut().item_spacing.x = 4.0;
    toolbar.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(2)
        .inner_margin(2)
        .show(&mut toolbar, |ui| {
            for (name, label) in QUICK_REACTIONS {
                let mine = message.reactions.iter().any(|reaction| {
                    reaction.name == name
                        && reaction
                            .user_ids
                            .iter()
                            .any(|id| id == context.self_user_id)
                });
                if quick_reaction_button(ui, name, label, mine, context).clicked() {
                    action = Some(MessageAction::ToggleReaction {
                        ts: message.ts.clone(),
                        name: name.into(),
                        add: !mine,
                    });
                }
            }
            if ui
                .add_sized(
                    [48.0, 28.0],
                    egui::Button::new("React").frame_when_inactive(false),
                )
                .on_hover_text("Choose another reaction")
                .clicked()
            {
                action = Some(MessageAction::PickReaction(message.ts.clone()));
            }
            if context.allow_threads
                && ui
                    .add_sized(
                        [44.0, 28.0],
                        egui::Button::new("Reply").frame_when_inactive(false),
                    )
                    .clicked()
            {
                action = Some(MessageAction::OpenThread(
                    message.thread_ts.as_ref().unwrap_or(&message.ts).clone(),
                ));
            }
        });
    (action, toolbar.min_rect())
}

fn quick_reaction_button(
    ui: &mut egui::Ui,
    name: &str,
    label: &str,
    mine: bool,
    context: &mut MessageContext<'_>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let description = format!("{} {label} reaction", if mine { "Remove" } else { "Add" });
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &description)
    });
    if mine || response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            2,
            if mine {
                theme::REACTION_MINE
            } else {
                theme::ACCENT
            },
        );
    }
    if let Some(image) = context
        .emoji
        .image_url(name)
        .and_then(|url| context.images.image(&url))
    {
        image.paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0)),
        );
    } else {
        let glyph = match context.emoji.resolve(name) {
            EmojiGlyph::Unicode(glyph) => glyph,
            _ => "·".into(),
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(18.0),
            theme::TEXT,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(description)
}

pub(super) fn avatar(ui: &mut egui::Ui, message: &DisplayMessage, images: &mut ImageCache) {
    let size = egui::vec2(AVATAR_SIZE, AVATAR_SIZE);
    if !ui.is_rect_visible(egui::Rect::from_min_size(ui.next_widget_position(), size)) {
        ui.allocate_exact_size(size, egui::Sense::hover());
        return;
    }
    if let Some(image) = message
        .avatar_url
        .as_deref()
        .and_then(|url| images.image(url))
    {
        ui.add(
            image
                .fit_to_exact_size(egui::vec2(AVATAR_SIZE, AVATAR_SIZE))
                .corner_radius(6),
        )
        .on_hover_text(&message.author);
        return;
    }
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(AVATAR_SIZE, AVATAR_SIZE), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, theme::ACCENT);
    let initials: String = message
        .author
        .split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .collect();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initials.to_uppercase(),
        egui::FontId::proportional(14.0),
        theme::TEXT_STRONG,
    );
    response.on_hover_text(&message.author);
}

pub fn emoji_button(
    ui: &mut egui::Ui,
    name: &str,
    label: &str,
    selected: bool,
    emoji: &EmojiCatalog,
    images: &mut ImageCache,
) -> egui::Response {
    let image = emoji.image_url(name).and_then(|url| images.image(&url));
    let button = if let Some(image) = image {
        let size = if label.is_empty() { 24.0 } else { 18.0 };
        let image = image.fit_to_exact_size(egui::vec2(size, size));
        if label.is_empty() {
            egui::Button::new(image)
        } else {
            egui::Button::new((image, label))
        }
    } else if label.is_empty() {
        egui::Button::new("·")
    } else {
        match emoji.resolve(name) {
            EmojiGlyph::Unicode(glyph) => egui::Button::new(format!("{glyph} {label}")),
            EmojiGlyph::Image(url) => match images.image(&url) {
                Some(image) => egui::Button::image_and_text(
                    image.fit_to_exact_size(egui::vec2(18.0, 18.0)),
                    label,
                ),
                None => egui::Button::new(format!(":{name}: {label}")),
            },
            EmojiGlyph::Missing(_) => egui::Button::new(format!(":{name}: {label}")),
        }
    };
    let button = if label.is_empty() {
        button
            .min_size(egui::vec2(40.0, 36.0))
            .frame_when_inactive(false)
    } else {
        button
            .fill(if selected {
                theme::REACTION_MINE
            } else {
                theme::SURFACE
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    theme::REACTION_MINE_BORDER
                } else {
                    theme::BORDER
                },
            ))
    };
    ui.add(button).on_hover_text(format!(":{name}:"))
}

fn reaction_button(
    ui: &mut egui::Ui,
    reaction: &Reaction,
    context: &mut MessageContext<'_>,
) -> egui::Response {
    let mine = reaction
        .user_ids
        .iter()
        .any(|id| id == context.self_user_id);
    emoji_button(
        ui,
        &reaction.name,
        &reaction.count.to_string(),
        mine,
        context.emoji,
        context.images,
    )
    .on_hover_text(if mine {
        "Click to remove your reaction"
    } else {
        "Click to react"
    })
}

pub(super) fn attachment_view(
    ui: &mut egui::Ui,
    attachment: &Attachment,
    images: &mut ImageCache,
) -> bool {
    let mut open = false;
    let target = attachment
        .permalink
        .as_ref()
        .or(attachment.full_url.as_ref());
    let slot_id = ui.id().with((
        "image_slot",
        attachment
            .file_id
            .as_deref()
            .or(attachment.preview_url.as_deref())
            .unwrap_or(&attachment.name),
    ));
    let previous_size = ui.ctx().data(|data| data.get_temp::<egui::Vec2>(slot_id));
    if attachment.preview_url.is_some() || attachment.preview_pending || previous_size.is_some() {
        // Keep the initial slot's aspect ratio when missing metadata arrives.
        // The image fits inside it; resizing the window still scales the slot.
        let native_size =
            previous_size.unwrap_or_else(|| match (attachment.width, attachment.height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => egui::vec2(w as f32, h as f32),
                _ => egui::vec2(320.0, 180.0),
            });
        ui.ctx()
            .data_mut(|data| data.insert_temp(slot_id, native_size));
        let max_size = egui::vec2(ui.available_width().clamp(1.0, 480.0), 280.0);
        let scale = (max_size.x / native_size.x)
            .min(max_size.y / native_size.y)
            .min(1.0);
        let size = native_size * scale;
        let sense = if attachment.preview_url.is_some() {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        if attachment.preview_url.is_some() {
            open = response
                .on_hover_cursor(egui::CursorIcon::ZoomIn)
                .on_hover_text("View image")
                .clicked();
        }
        if ui.is_rect_visible(rect) {
            if let Some(image) = attachment
                .preview_url
                .as_deref()
                .and_then(|url| images.image(url))
            {
                let native = image.size().unwrap_or(size);
                let scale = (size.x / native.x).min(size.y / native.y);
                image.corner_radius(4).paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), native * scale),
                );
            } else {
                ui.painter().rect_filled(rect, 4.0, theme::SURFACE);
                let label = match attachment.preview_url.as_deref() {
                    Some(url) if images.failed(url) => "Image unavailable · click to retry",
                    None if !attachment.preview_pending => "Image unavailable",
                    _ => "Loading image…",
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::TextStyle::Small.resolve(ui.style()),
                    theme::TEXT_MUTED,
                );
                if open && let Some(url) = &attachment.preview_url {
                    images.retry(url);
                }
            }
        }
    }

    if attachment.preview_url.is_some() {
        open |= ui.link(RichText::new(&attachment.name).small()).clicked();
    } else if let Some(target) = target {
        ui.hyperlink_to(RichText::new(&attachment.name).small(), target);
    } else if attachment.preview_url.is_none() {
        ui.weak(RichText::new(&attachment.name).small());
    }
    open
}

fn day_separator(ui: &mut egui::Ui, time: &DateTime<Local>) {
    ui.add_space(12.0);
    let today = Local::now().date_naive();
    let label = if time.date_naive() == today {
        "Today".into()
    } else if Some(time.date_naive()) == today.pred_opt() {
        "Yesterday".into()
    } else {
        time.format("%A, %B %e, %Y").to_string()
    };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    let galley = ui.painter().layout_no_wrap(
        label,
        egui::TextStyle::Small.resolve(ui.style()),
        theme::TEXT_MUTED,
    );
    let text_rect = egui::Rect::from_center_size(rect.center(), galley.size());
    let stroke = egui::Stroke::new(1.0, theme::BORDER);
    if text_rect.left() - 12.0 > rect.left() {
        ui.painter().hline(
            rect.left()..=text_rect.left() - 12.0,
            rect.center().y,
            stroke,
        );
        ui.painter().hline(
            text_rect.right() + 12.0..=rect.right(),
            rect.center().y,
            stroke,
        );
    }
    ui.put(text_rect, egui::Label::new(galley));
    ui.add_space(4.0);
}

fn local_time(ts: &str) -> DateTime<Local> {
    Local
        .timestamp_opt(ts_seconds(ts) as i64, 0)
        .single()
        .unwrap_or_default()
}

const AVATAR_SIZE: f32 = 28.0;
const GROUP_WINDOW_SECONDS: f64 = 5.0 * 60.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::chat_service::ChatService;
    use crate::domain::message_text::{Block, Inline, TextStyle};
    use crate::infrastructure::demo::DemoGateway;
    use std::sync::Arc;

    #[test]
    fn hover_reactions_toggle_the_right_message_without_shifting_rows() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.enable_accesskit();
        let service = Arc::new(ChatService::new(Arc::new(DemoGateway::new())));
        let template = service.messages("C1").unwrap().remove(0);
        let mut messages: Vec<_> = ["First message", "Second message", "Third message"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                let mut message = template.clone();
                message.ts = (1_790_000_000 + index).to_string();
                message.avatar_url = None;
                message.attachments.clear();
                message.reactions.clear();
                message.reply_count = 0;
                message.blocks = vec![Block::Paragraph(vec![Inline::Text {
                    text: text.into(),
                    style: Default::default(),
                }])];
                message
            })
            .collect();
        let emoji = EmojiCatalog::default();
        let mut images = ImageCache::new(service, ctx.clone());
        let outbox = Outbox::default();
        let mut frame = |messages: &[DisplayMessage], allow_threads, events| {
            let mut action = None;
            let mut height = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Frame::new().inner_margin(16).show(ui, |ui| {
                        action = message_list(
                            ui,
                            "quick_reactions",
                            messages,
                            &mut MessageContext {
                                emoji: &emoji,
                                images: &mut images,
                                self_user_id: "me",
                                allow_threads,
                                outbox: &outbox,
                            },
                        );
                        height = ui.min_rect().height();
                    });
                },
            );
            let nodes: std::collections::HashMap<_, _> = output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .into_iter()
                .filter_map(|(_, node)| {
                    let label = node.label().or(node.value())?.to_owned();
                    let bounds = node.bounds()?;
                    Some((
                        label,
                        egui::Rect::from_min_max(
                            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
                            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
                        ),
                    ))
                })
                .collect();
            output.textures_delta.clear();
            (action, nodes, height)
        };
        let click = |point| {
            vec![
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
            ]
        };
        let (_, initial, height) = frame(&messages, true, vec![]);
        assert!(!initial.contains_key("Add Thumbs up reaction"));
        let body = initial["Second message"];
        let (_, hovered, hover_height) = frame(
            &messages,
            true,
            vec![egui::Event::PointerMoved(egui::pos2(
                body.left() + 2.0,
                body.center().y,
            ))],
        );
        assert_eq!(height, hover_height);
        assert_eq!(initial["Third message"], hovered["Third message"]);
        for (_, label) in QUICK_REACTIONS {
            let rect = hovered[&format!("Add {label} reaction")];
            assert!(
                rect.left() >= 16.0 && rect.right() <= 304.0,
                "toolbar stays inside narrow timeline gutters"
            );
        }
        // Hover the bottom of the toolbar, below this short grouped message.
        let button = hovered["Add Thumbs up reaction"];
        let point = egui::pos2(button.center().x, button.bottom() - 1.0);
        let (_, held, held_height) = frame(&messages, true, vec![egui::Event::PointerMoved(point)]);
        assert_eq!(held["Add Thumbs up reaction"], button);
        assert_eq!(height, held_height);
        for (name, label) in QUICK_REACTIONS {
            let point = held[&format!("Add {label} reaction")].center();
            let (action, _, _) = frame(&messages, true, click(point));
            assert!(
                matches!(action, Some(MessageAction::ToggleReaction { ts, name: chosen, add: true }) if ts == messages[1].ts && chosen == name)
            );
        }
        messages[1].reactions.push(Reaction {
            name: "+1".into(),
            count: 1,
            user_ids: vec!["me".into()],
        });
        let (_, selected, _) = frame(&messages, true, vec![]);
        let (action, _, _) = frame(
            &messages,
            true,
            click(selected["Remove Thumbs up reaction"].center()),
        );
        assert!(
            matches!(action, Some(MessageAction::ToggleReaction { ts, name, add: false }) if ts == messages[1].ts && name == "+1")
        );
        let (action, _, _) = frame(&messages, true, click(selected["React"].center()));
        assert!(matches!(action, Some(MessageAction::PickReaction(ts)) if ts == messages[1].ts));
        let (_, thread, _) = frame(
            &messages,
            false,
            vec![egui::Event::PointerMoved(body.center())],
        );
        assert!(thread.contains_key("Add Check mark reaction"));
        assert!(!thread.contains_key("Reply"));
    }

    #[test]
    fn deferred_image_metadata_keeps_its_slot_and_resizes_with_the_window() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let service = Arc::new(ChatService::new(Arc::new(DemoGateway::new())));
        let mut images = ImageCache::new(service, ctx.clone());
        let mut attachment = Attachment {
            file_id: Some("F1".into()),
            name: "image.png".into(),
            preview_url: None,
            full_url: None,
            permalink: None,
            width: None,
            height: None,
            preview_pending: true,
        };
        let mut frame = |attachment: &Attachment, width| {
            let mut height = 0.0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().top();
                    attachment_view(ui, attachment, &mut images);
                    height = ui.cursor().top() - top;
                },
            );
            output.textures_delta.clear();
            height
        };
        let pending_height = frame(&attachment, 700.0);
        attachment.preview_pending = false;
        attachment.preview_url = Some("demo://preview.png".into());
        attachment.width = Some(600);
        attachment.height = Some(1600);
        let resolved_height = frame(&attachment, 700.0);
        assert!(
            (pending_height - resolved_height).abs() < 1.0,
            "metadata arriving must not shift messages below the image: {pending_height} / {resolved_height}"
        );
        assert!(frame(&attachment, 200.0) < resolved_height);
        assert!((frame(&attachment, 700.0) - resolved_height).abs() < 1.0);
    }

    #[test]
    fn offscreen_rows_keep_scroll_geometry_stable_and_resize_reflows() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let service = Arc::new(ChatService::new(Arc::new(DemoGateway::new())));
        let workspace = service.load_workspace().unwrap();
        let template = service.messages("C1").unwrap().remove(0);
        let messages: Vec<_> = (0..180).map(|index| {
            let mut message = template.clone();
            message.ts = (1_790_000_000 + index * 400).to_string();
            message.avatar_url = None;
            message.attachments.clear();
            message.reactions.clear();
            message.reply_count = 0;
            message.blocks = vec![Block::Paragraph(vec![Inline::Text {
                text: "A message long enough to wrap at a narrow window width, with enough words to expose layout and scroll height changes.".into(),
                style: TextStyle::default(),
            }])];
            message
        }).collect();
        let mut images = ImageCache::new(service, ctx.clone());
        let mut frame = |width: f32, offset: f32| {
            let mut height = 0.0;
            let start = std::time::Instant::now();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 420.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    height = egui::ScrollArea::vertical()
                        .id_salt("test")
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| {
                            message_list(
                                ui,
                                "test",
                                &messages,
                                &mut MessageContext {
                                    emoji: &workspace.emoji,
                                    images: &mut images,
                                    self_user_id: "me",
                                    allow_threads: true,
                                    outbox: &Outbox::default(),
                                },
                            )
                        })
                        .content_size
                        .y;
                },
            );
            let elapsed = start.elapsed();
            let shapes = output.shapes.len();
            output.textures_delta.clear();
            (height, shapes, elapsed)
        };
        let first = frame(700.0, 0.0);
        let scrolled = frame(700.0, 3000.0);
        let again = frame(700.0, 6000.0);
        assert!(
            (first.0 - scrolled.0).abs() < 1.0,
            "{first:?} vs {scrolled:?}"
        );
        assert!((first.0 - again.0).abs() < 1.0);
        assert!(
            scrolled.1 < 250,
            "offscreen rows should not produce widgets: {scrolled:?}"
        );
        let narrow = frame(320.0, 3000.0);
        assert!(narrow.0 > first.0, "narrow text must reflow");
        let wide = frame(700.0, 3000.0);
        assert!((wide.0 - first.0).abs() < 1.0);
        eprintln!(
            "180 messages: initial {:?}, cached scroll {:?}; {} visible shapes",
            first.2, scrolled.2, scrolled.1
        );
        let mut drawn_offset = 0.0;
        let mut settled_offset = 0.0;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 420.0),
                )),
                ..Default::default()
            },
            |ui| {
                let output = egui::ScrollArea::vertical()
                    .id_salt("fresh_channel")
                    .stick_to_bottom(true)
                    .show_viewport(ui, |ui, viewport| {
                        drawn_offset = viewport.min.y;
                        message_list(
                            ui,
                            "fresh_channel",
                            &messages,
                            &mut MessageContext {
                                emoji: &workspace.emoji,
                                images: &mut images,
                                self_user_id: "me",
                                allow_threads: true,
                                outbox: &Outbox::default(),
                            },
                        );
                    });
                settled_offset = output.state.offset.y;
            },
        );
        output.textures_delta.clear();
        assert!(
            drawn_offset > 0.0,
            "first presented frame should already show recent messages"
        );
        assert!(
            (drawn_offset - settled_offset).abs() < 1.0,
            "channel entry must not show a scroll-to-bottom transition"
        );
    }
}
