use crate::domain::model::{Conversation, ConversationKind};
use eframe::egui;

pub fn conversation_label(conversation: &Conversation) -> String {
    format!("{} {}", conversation_icon(conversation), conversation.name)
}

pub fn conversation_icon(conversation: &Conversation) -> &'static str {
    match conversation.kind {
        ConversationKind::PublicChannel => "#",
        ConversationKind::PrivateChannel => "🔒",
        ConversationKind::DirectMessage { .. } => "@",
        ConversationKind::GroupMessage => "👥",
    }
}

/// Draw inside one fixed column. Emoji font bearings otherwise push the lock
/// to the right of the channel hash even when their allocated widths match.
pub fn paint_sidebar_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    conversation: &Conversation,
    color: egui::Color32,
) {
    let painter = ui.painter_at(rect);
    let center = rect.center();
    let point = |x, y| center + egui::vec2(x, y);
    let stroke = egui::Stroke::new(1.25, color);
    match conversation.kind {
        ConversationKind::PublicChannel => {
            for x in [-2.5, 2.5] {
                painter.line_segment([point(x + 1.0, -5.0), point(x - 1.0, 5.0)], stroke);
            }
            for y in [-2.0, 2.0] {
                painter.line_segment([point(-5.0, y), point(5.0, y)], stroke);
            }
        }
        ConversationKind::PrivateChannel => {
            painter.rect_stroke(
                egui::Rect::from_min_max(point(-4.0, -0.5), point(4.0, 5.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            let shackle = (0..=12)
                .map(|step| {
                    let angle = std::f32::consts::PI * step as f32 / 12.0;
                    point(-2.5 * angle.cos(), -0.5 - 4.5 * angle.sin())
                })
                .collect();
            painter.add(egui::Shape::line(shackle, stroke));
            painter.line_segment([point(0.0, 1.5), point(0.0, 3.0)], stroke);
        }
        ConversationKind::DirectMessage { .. } => {
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "@",
                egui::FontId::proportional(13.5),
                color,
            );
        }
        ConversationKind::GroupMessage => {
            painter.circle_stroke(point(-2.0, -2.5), 2.0, stroke);
            painter.circle_stroke(point(3.0, -3.0), 1.5, stroke);
            painter.add(egui::Shape::line(
                vec![
                    point(-5.0, 4.0),
                    point(-5.0, 2.0),
                    point(-3.5, 0.5),
                    point(-0.5, 0.5),
                    point(1.0, 2.0),
                    point(1.0, 4.0),
                ],
                stroke,
            ));
            painter.add(egui::Shape::line(
                vec![
                    point(2.0, 0.0),
                    point(4.0, 0.0),
                    point(5.0, 1.5),
                    point(5.0, 3.5),
                ],
                stroke,
            ));
        }
    }
}
