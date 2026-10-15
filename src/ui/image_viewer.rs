use eframe::egui;

use super::message_view::{AttachmentClick, image_menu};
use super::{images::ImageCache, theme};
use crate::domain::model::Attachment;

pub struct ImageViewer {
    attachment: Attachment,
}

impl ImageViewer {
    pub fn new(attachment: Attachment) -> Self {
        Self { attachment }
    }

    pub fn attachment(&self) -> &Attachment {
        &self.attachment
    }

    /// Returns whether the viewer should close, and any image menu choice.
    pub fn show(
        &self,
        ctx: &egui::Context,
        images: &mut ImageCache,
    ) -> (bool, Option<AttachmentClick>) {
        let bounds = ctx.content_rect().size();
        let limit = egui::vec2(
            (bounds.x - 80.0).clamp(1.0, 1100.0),
            (bounds.y - 160.0).max(1.0),
        );
        let source = self
            .attachment
            .full_url
            .as_deref()
            .or(self.attachment.preview_url.as_deref());
        let original = source.and_then(|url| images.image(url));
        let loading = original.is_none() && source.is_some_and(|url| !images.failed(url));
        let image = original.or_else(|| {
            self.attachment
                .preview_url
                .as_deref()
                .and_then(|url| images.image(url))
        });
        let dimensions = self
            .attachment
            .width
            .zip(self.attachment.height)
            .filter(|(width, height)| *width > 0 && *height > 0)
            .map(|(width, height)| egui::vec2(width as f32, height as f32))
            .or_else(|| {
                image
                    .as_ref()
                    .and_then(|image| image.load_for_size(ctx, limit).ok()?.size())
            });
        let image_size = dimensions
            .map(|size| size * (limit.x / size.x).min(limit.y / size.y).min(1.0))
            .unwrap_or(egui::vec2(limit.x.min(480.0), limit.y.min(320.0)));
        let canvas = egui::vec2(image_size.x.max(320.0).min(limit.x), image_size.y);
        let mut close = false;
        let mut menu = None;
        let response = egui::Modal::new(egui::Id::new("image_viewer"))
            .frame(theme::modal_frame())
            .show(ctx, |ui| {
                ui.set_width(canvas.x);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui.button("Close ×").on_hover_text("Escape").clicked();
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&self.attachment.name).strong(),
                                )
                                .truncate(),
                            );
                        });
                    });
                });
                ui.add_space(12.0);
                ui.allocate_ui_with_layout(
                    canvas,
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        if let Some(image) = image {
                            let response = ui.add(
                                image
                                    .fit_to_exact_size(image_size)
                                    .sense(egui::Sense::click()),
                            );
                            menu = image_menu(&response, &self.attachment);
                        } else if loading {
                            ui.spinner();
                        } else {
                            ui.weak("This image could not be loaded.");
                        }
                    },
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if loading {
                        ui.label(egui::RichText::new("Loading full image…").small().weak());
                    } else if let Some(url) = source.filter(|url| images.failed(url)) {
                        ui.weak("Full image unavailable");
                        if ui.small_button("Retry").clicked() {
                            images.retry(url);
                        }
                    } else {
                        ui.label(egui::RichText::new("Esc to close").small().weak());
                    }
                });
            });
        (close || response.should_close(), menu)
    }
}
