//! Searching old messages: Slack runs the query, the results list here and a
//! click opens the message, with its thread, beside them.

use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, channel},
};

use eframe::egui;

use super::activity::{avatar, skeleton};
use super::{images::ImageCache, theme};
use crate::application::chat_service::ChatService;
use crate::application::message_search::{SearchResult, SearchResults};
use crate::domain::message_search::{SearchSort, highlight_runs};

struct Loaded {
    request: u64,
    page: u32,
    result: Result<SearchResults, String>,
}

pub struct Search {
    /// What is typed in the search field.
    pub query: String,
    sort: SearchSort,
    /// The query and order the results are for.
    shown: Option<(String, SearchSort)>,
    results: Vec<SearchResult>,
    page: u32,
    pages: u32,
    total: u32,
    busy: bool,
    error: Option<(String, u32)>,
    /// Answers to older requests are dropped.
    request: u64,
    focus: bool,
    pub selected: Option<SearchResult>,
    service: Arc<ChatService>,
    ctx: egui::Context,
    sender: Sender<Loaded>,
    receiver: Receiver<Loaded>,
}

impl Search {
    pub fn new(service: Arc<ChatService>, ctx: egui::Context, sort: SearchSort) -> Self {
        let (sender, receiver) = channel();
        Self {
            query: String::new(),
            sort,
            shown: None,
            results: Vec::new(),
            page: 0,
            pages: 0,
            total: 0,
            busy: false,
            error: None,
            request: 0,
            focus: false,
            selected: None,
            service,
            ctx,
            sender,
            receiver,
        }
    }

    /// The order chosen, remembered between runs.
    pub fn sort(&self) -> SearchSort {
        self.sort
    }

    #[cfg(test)]
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// Put the cursor in the search field, optionally starting a new query.
    pub fn focus(&mut self, query: Option<String>) {
        if let Some(query) = query {
            self.query = query;
        }
        self.focus = true;
    }

    fn run(&mut self, page: u32) {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.request += 1;
        self.busy = true;
        self.error = None;
        if page <= 1 {
            self.shown = Some((query.clone(), self.sort));
            self.results.clear();
            self.page = 0;
        }
        let (request, sort) = (self.request, self.sort);
        let (service, sender, ctx) = (self.service.clone(), self.sender.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = service
                .search(&query, sort, page)
                .map_err(|error| error.to_string());
            let _ = sender.send(Loaded {
                request,
                page,
                result,
            });
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(loaded) = self.receiver.try_recv() {
            if loaded.request != self.request {
                continue;
            }
            self.busy = false;
            match loaded.result {
                Ok(found) => {
                    if loaded.page <= 1 {
                        self.results = found.results;
                    } else {
                        self.results.extend(found.results);
                    }
                    self.page = found.page;
                    self.pages = found.pages.min(MAX_PAGES);
                    self.total = found.total;
                }
                Err(error) => self.error = Some((error, loaded.page)),
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, images: &mut ImageCache) -> Option<SearchResult> {
        let mut run = None;
        let mut open = None;
        egui::Panel::top("search_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading("Search");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.busy {
                            ui.weak("Searching…");
                        } else if self.shown.is_some() && self.error.is_none() {
                            ui.weak(match self.total {
                                1 => "1 result".to_owned(),
                                total => format!("{total} results"),
                            });
                        }
                    });
                });
            });
        theme::timeline_frame().show(ui, |ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id_salt("search_query")
                    .hint_text("Search messages")
                    .desired_width(f32::INFINITY)
                    .margin(egui::vec2(8.0, 6.0)),
            );
            if std::mem::take(&mut self.focus) {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                run = Some(1);
                field.request_focus();
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let before = self.sort;
                for sort in SearchSort::ALL {
                    ui.selectable_value(&mut self.sort, sort, sort.label());
                }
                if before != self.sort && self.shown.is_some() {
                    run = Some(1);
                }
            });
            ui.add_space(8.0);
            ui.separator();
            if let Some((error, page)) = &self.error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(theme::SELF_MENTION_TEXT, error);
                    if ui.button("Retry").clicked() {
                        run = Some(*page);
                    }
                });
            }
            egui::ScrollArea::vertical()
                .id_salt("search_results")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.shown.is_none() {
                        ui.add_space(12.0);
                        help(ui);
                        return;
                    }
                    if self.busy && self.results.is_empty() {
                        for _ in 0..7 {
                            skeleton(ui);
                        }
                        return;
                    }
                    if !self.busy && self.error.is_none() && self.results.is_empty() {
                        ui.add_space(24.0);
                        ui.label("No messages match");
                        ui.add_space(12.0);
                        help(ui);
                    }
                    for result in &self.results {
                        let selected = self.selected.as_ref().is_some_and(|selected| {
                            selected.conversation_id == result.conversation_id
                                && selected.message.ts == result.message.ts
                        });
                        if result_row(ui, result, images, selected) {
                            open = Some(result.clone());
                        }
                    }
                    if self.page < self.pages && !self.results.is_empty() {
                        ui.add_space(12.0);
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Load more"))
                            .clicked()
                        {
                            run = Some(self.page + 1);
                        }
                        ui.add_space(12.0);
                    }
                });
        });
        if let Some(page) = run {
            self.run(page);
        }
        if let Some(result) = &open {
            self.selected = Some(result.clone());
        }
        open
    }
}

fn help(ui: &mut egui::Ui) {
    ui.weak("Press Enter to search. Narrow it down like in Slack:");
    ui.add_space(4.0);
    for (example, meaning) in [
        ("from:@name", "sent by someone"),
        ("in:#channel", "in a channel or conversation"),
        ("before:2026-01-31  after:2025-12-01", "by date"),
        ("\"exact phrase\"", "these words in this order"),
        ("has:link  is:thread", "with a link, or in a thread"),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(example).monospace());
            ui.weak(meaning);
        });
    }
}

const ROW_HEIGHT: f32 = 82.0;
/// Slack serves at most this many pages of results.
const MAX_PAGES: u32 = 100;

fn result_row(
    ui: &mut egui::Ui,
    result: &SearchResult,
    images: &mut ImageCache,
    selected: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("{} · {}", result.message.author, result.source),
        )
    });
    if ui.is_rect_visible(rect) {
        if selected || (ui.is_enabled() && ui.rect_contains_pointer(rect)) {
            ui.painter().rect_filled(rect, 2.0, theme::HOVER);
        }
        if selected {
            ui.painter().vline(
                rect.left(),
                rect.y_range(),
                egui::Stroke::new(2.0, theme::LINK),
            );
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, theme::BORDER),
        );
        let mut content = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
            rect.min + egui::vec2(24.0, 12.0),
            rect.max - egui::vec2(12.0, 10.0),
        )));
        content.set_clip_rect(rect.intersect(ui.clip_rect()));
        content.horizontal_top(|ui| {
            let message = &result.message;
            avatar(ui, message.avatar_url.as_deref(), &message.author, images);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                ui.horizontal(|ui| {
                    let available = ui.available_width();
                    ui.allocate_ui_with_layout(
                        egui::vec2((available - 120.0).max(24.0), 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(&message.author)
                                    .truncate()
                                    .selectable(false)
                                    .show_tooltip_when_elided(false),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(date(&message.ts)).small().weak());
                    });
                });
                ui.add(
                    egui::Label::new(highlighted(ui, &result.snippet))
                        .truncate()
                        .selectable(false)
                        .show_tooltip_when_elided(false),
                );
                let place = if result.is_reply() {
                    format!("{} · reply in a thread", result.source)
                } else {
                    result.source.clone()
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(place).small().weak())
                        .truncate()
                        .selectable(false)
                        .show_tooltip_when_elided(false),
                );
            });
        });
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// The snippet with its matched words standing out.
fn highlighted(ui: &egui::Ui, snippet: &str) -> egui::text::LayoutJob {
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    let mut job = egui::text::LayoutJob::default();
    for (text, hit) in highlight_runs(snippet) {
        job.append(
            &text,
            0.0,
            if hit {
                egui::TextFormat {
                    font_id: egui::FontId::new(size, theme::bold_family()),
                    color: theme::TEXT_STRONG,
                    background: theme::SELF_MENTION_BACKGROUND,
                    ..Default::default()
                }
            } else {
                egui::TextFormat {
                    font_id: egui::FontId::proportional(size),
                    color: theme::TEXT,
                    ..Default::default()
                }
            },
        );
    }
    job
}

/// When a message was sent; the year only when it was not this year.
fn date(ts: &str) -> String {
    let Some(date) =
        chrono::DateTime::from_timestamp(crate::domain::model::ts_seconds(ts) as i64, 0)
    else {
        return String::new();
    };
    let date = date.with_timezone(&chrono::Local);
    let format = if chrono::Datelike::year(&date) == chrono::Datelike::year(&chrono::Local::now()) {
        "%-d %b · %H:%M"
    } else {
        "%-d %b %Y · %H:%M"
    };
    date.format(format).to_string()
}
