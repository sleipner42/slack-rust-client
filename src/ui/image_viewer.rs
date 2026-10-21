use eframe::egui;

use super::message_view::{AttachmentClick, image_menu};
use super::{images::ImageCache, theme};
use crate::domain::model::Attachment;

pub struct ImageViewer {
    attachment: Attachment,
    /// Scale chosen by zooming; `None` fits the image to the window.
    zoom: Option<f32>,
}

impl ImageViewer {
    pub fn new(attachment: Attachment) -> Self {
        Self {
            attachment,
            zoom: None,
        }
    }

    pub fn attachment(&self) -> &Attachment {
        &self.attachment
    }

    /// Returns whether the viewer should close, and any image menu choice.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        images: &mut ImageCache,
    ) -> (bool, Option<AttachmentClick>) {
        let bounds = ctx.content_rect().size();
        // Nearly the whole window, leaving room for the title and the hint.
        let limit = egui::vec2(
            (bounds.x - MARGIN.x).max(1.0),
            (bounds.y - MARGIN.y).max(1.0),
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
        let fit = dimensions.map_or(1.0, |size| fit_scale(size, limit));
        let scale = self.zoom.unwrap_or(fit);
        let image_size = dimensions
            .map(|size| size * scale)
            .unwrap_or(egui::vec2(limit.x.min(480.0), limit.y.min(320.0)));
        let canvas = egui::vec2(
            image_size.x.clamp(320.0_f32.min(limit.x), limit.x),
            image_size.y.min(limit.y),
        );
        let mut close = false;
        let mut menu = None;
        let mut clicked = false;
        let mut hovered = false;
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
                egui::ScrollArea::both()
                    .id_salt("image_viewer_scroll")
                    .max_width(canvas.x)
                    .max_height(canvas.y)
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        ui.allocate_ui_with_layout(
                            image_size.max(canvas),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                if let Some(image) = image {
                                    let response = ui.add(
                                        image
                                            .fit_to_exact_size(image_size)
                                            .sense(egui::Sense::click()),
                                    );
                                    hovered = response.hovered();
                                    clicked = response.clicked();
                                    menu = image_menu(&response, &self.attachment, false);
                                    response.on_hover_cursor(if self.zoom.is_some() {
                                        egui::CursorIcon::ZoomOut
                                    } else {
                                        egui::CursorIcon::ZoomIn
                                    });
                                } else if loading {
                                    ui.spinner();
                                } else {
                                    ui.weak("This image could not be loaded.");
                                }
                            },
                        );
                    });
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
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.0}% · Click or Ctrl+scroll to zoom · 0 to fit · Esc to close",
                                scale * 100.0
                            ))
                            .small()
                            .weak(),
                        );
                    }
                });
            });
        let (pinch, fit_key, zoom_in, zoom_out) = ctx.input(|input| {
            (
                input.zoom_delta(),
                input.key_pressed(egui::Key::Num0),
                input.key_pressed(egui::Key::Plus) || input.key_pressed(egui::Key::Equals),
                input.key_pressed(egui::Key::Minus),
            )
        });
        if clicked {
            self.zoom = match self.zoom {
                Some(_) => None,
                None => Some(click_zoom(fit)),
            };
        } else if hovered && pinch != 1.0 {
            self.zoom = Some((scale * pinch).clamp(MIN_ZOOM, MAX_ZOOM));
        } else if fit_key {
            self.zoom = None;
        } else if zoom_in || zoom_out {
            let step = if zoom_in { STEP } else { 1.0 / STEP };
            self.zoom = Some((scale * step).clamp(MIN_ZOOM, MAX_ZOOM));
        }
        (close || response.should_close(), menu)
    }
}

/// The scale that fits `size` in `limit`, never enlarging past the pixels
/// an image has.
fn fit_scale(size: egui::Vec2, limit: egui::Vec2) -> f32 {
    (limit.x / size.x).min(limit.y / size.y).min(1.0)
}

/// Clicking a fitted image shows its real pixels, or doubles one already
/// shown at full size.
fn click_zoom(fit: f32) -> f32 {
    if fit < 1.0 { 1.0 } else { 2.0 }
}

const MARGIN: egui::Vec2 = egui::vec2(48.0, 140.0);
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 8.0;
const STEP: f32 = 1.25;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_fit_the_window_and_a_click_shows_their_pixels() {
        let window = egui::vec2(1900.0, 1000.0);
        // A wide screenshot is as wide as the window allows, not 1100 points.
        assert_eq!(
            fit_scale(egui::vec2(2400.0, 300.0), window),
            1900.0 / 2400.0
        );
        assert_eq!(
            fit_scale(egui::vec2(800.0, 600.0), window),
            1.0,
            "never blown up"
        );
        assert_eq!(click_zoom(0.5), 1.0);
        assert_eq!(click_zoom(1.0), 2.0);
    }
}
