use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;

use super::{images::ImageCache, message_view, theme};
use crate::application::chat_service::ChatService;
use crate::application::emoji_catalog::EmojiCatalog;
use crate::application::todos::{DisplayTodo, TodoPage};
use crate::domain::model::{Attachment, SavedTodo};

#[derive(Clone, Debug)]
struct Request {
    completed: bool,
    cursor: String,
}

enum Command {
    Load(Request),
    Complete(SavedTodo),
}

enum TodoResult {
    Page(Request, Result<TodoPage, String>),
    Completed(SavedTodo, Result<(), String>),
}

struct Selection {
    task: DisplayTodo,
    completed: bool,
}

#[derive(Default)]
struct List {
    items: Vec<DisplayTodo>,
    next_cursor: String,
    loaded: bool,
    busy: bool,
    error: Option<String>,
    failed: Option<Request>,
}

pub struct Todos {
    lists: [List; 2],
    counts: [usize; 2],
    completed: bool,
    selected: Option<Selection>,
    completing: Option<SavedTodo>,
    completion_error: Option<(SavedTodo, String)>,
    commands: Sender<Command>,
    results: Receiver<TodoResult>,
}

impl Todos {
    pub fn new(service: Arc<ChatService>, ctx: egui::Context) -> Self {
        let (commands, requests) = channel::<Command>();
        let (sender, results) = channel();
        thread::spawn(move || {
            while let Ok(command) = requests.recv() {
                let result = match command {
                    Command::Load(request) => {
                        let result = service
                            .todos(request.completed, &request.cursor)
                            .map_err(|e| e.to_string());
                        TodoResult::Page(request, result)
                    }
                    Command::Complete(item) => {
                        let result = service.complete_todo(&item).map_err(|e| e.to_string());
                        TodoResult::Completed(item, result)
                    }
                };
                if sender.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        let mut todos = Self {
            lists: Default::default(),
            counts: [0, 0],
            completed: false,
            selected: None,
            completing: None,
            completion_error: None,
            commands,
            results,
        };
        todos.open();
        todos
    }

    pub fn open(&mut self) {
        self.submit(Request {
            completed: false,
            cursor: String::new(),
        });
        if self.completed {
            self.submit(Request {
                completed: true,
                cursor: String::new(),
            });
        }
    }

    pub fn open_count(&self) -> usize {
        self.counts[0]
    }

    pub fn has_selection(&self) -> bool {
        self.selected.is_some()
    }

    fn complete_selected(&mut self) {
        let Some(selected) = &self.selected else {
            return;
        };
        if selected.completed || self.completing.is_some() {
            return;
        }
        let item = selected.task.item.clone();
        if self.commands.send(Command::Complete(item.clone())).is_ok() {
            self.completing = Some(item);
            self.completion_error = None;
        } else {
            self.completion_error = Some((
                item,
                "The to-do worker stopped. Reopen the app to retry.".into(),
            ));
        }
    }

    fn finish_completion(&mut self, item: SavedTodo, result: Result<(), String>) {
        self.completing = None;
        match result {
            Err(error) => self.completion_error = Some((item, error)),
            Ok(()) => {
                self.completion_error = None;
                self.lists[0]
                    .items
                    .retain(|task| !same_saved_item(&task.item, &item));
                self.counts[0] = self.counts[0].saturating_sub(1);
                self.counts[1] += 1;
                if let Some(selected) = &mut self.selected
                    && same_saved_item(&selected.task.item, &item)
                {
                    selected.completed = true;
                }
                // Refresh authoritative membership/counts after Slack confirms.
                self.submit(Request {
                    completed: false,
                    cursor: String::new(),
                });
                if self.lists[1].loaded {
                    self.submit(Request {
                        completed: true,
                        cursor: String::new(),
                    });
                }
            }
        }
    }

    pub fn show_details(
        &mut self,
        ui: &mut egui::Ui,
        emoji: &EmojiCatalog,
        images: &mut ImageCache,
    ) -> Option<Attachment> {
        let mut close = false;
        egui::Panel::top("todo_detail_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading("To-do");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui.button("Close ×").clicked();
                        if let Some(selected) = &self.selected
                            && let Some(message) = &selected.task.message
                        {
                            ui.push_id(
                                (
                                    "copy_todo",
                                    &selected.task.item.conversation_id,
                                    &selected.task.item.ts,
                                ),
                                |ui| copy_text_button(ui, &message.blocks),
                            );
                        } else {
                            ui.add_enabled(false, egui::Button::new("Copy text"));
                        }
                    });
                });
            });
        if close {
            self.selected = None;
            return None;
        }
        let selected = self.selected.as_ref()?;
        let mut complete = false;
        egui::Panel::bottom("todo_detail_actions")
            .frame(theme::composer_panel_frame())
            .show(ui, |ui| {
                if let Some((item, error)) = &self.completion_error
                    && same_saved_item(item, &selected.task.item)
                {
                    ui.colored_label(
                        theme::SELF_MENTION_TEXT,
                        format!("Could not complete: {error}"),
                    );
                    ui.add_space(8.0);
                }
                let pending = self
                    .completing
                    .as_ref()
                    .is_some_and(|item| same_saved_item(item, &selected.task.item));
                let label = if selected.completed {
                    "Completed"
                } else if pending {
                    "Completing…"
                } else {
                    "Mark completed"
                };
                complete = ui
                    .add_enabled(
                        !selected.completed && self.completing.is_none(),
                        egui::Button::new(label).min_size(egui::vec2(ui.available_width(), 30.0)),
                    )
                    .clicked();
            });
        let mut image = None;
        theme::timeline_frame().show(ui, |ui| {
            let scroll = egui::ScrollArea::vertical()
                .id_salt((
                    "todo_detail",
                    &selected.task.item.conversation_id,
                    &selected.task.item.ts,
                ))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(&selected.task.source).small().weak());
                    if let Some(url) = &selected.task.source_url {
                        ui.hyperlink_to(egui::RichText::new("Open in Slack").small(), url);
                    }
                    ui.add_space(16.0);
                    let mut selecting = false;
                    if let Some(message) = &selected.task.message {
                        selecting = super::rich_text::render_detail_blocks(
                            ui,
                            &message.blocks,
                            emoji,
                            images,
                        );
                        for attachment in &message.attachments {
                            ui.add_space(12.0);
                            if message_view::attachment_view(ui, attachment, images) {
                                image = Some(attachment.clone());
                            }
                        }
                    } else {
                        ui.weak(
                            selected
                                .task
                                .error
                                .as_deref()
                                .unwrap_or("Message unavailable"),
                        );
                    }
                    ui.add_space(16.0);
                    selecting
                });
            super::rich_text::scroll_text_selection(ui.ctx(), &scroll);
        });
        if complete {
            self.complete_selected();
        }
        image
    }

    pub fn poll(&mut self) {
        while let Ok(event) = self.results.try_recv() {
            let (request, result) = match event {
                TodoResult::Page(request, result) => (request, result),
                TodoResult::Completed(item, result) => {
                    self.finish_completion(item, result);
                    continue;
                }
            };
            let list = &mut self.lists[request.completed as usize];
            list.busy = false;
            match result {
                Ok(page) => {
                    self.counts = [page.open_count, page.completed_count];
                    // Preserve a readable cached preview when just that message
                    // fails to refresh. Keep its error visible for retry.
                    let items: Vec<_> = page
                        .items
                        .into_iter()
                        .map(|mut item| {
                            if item.message.is_none()
                                && let Some(previous) = list
                                    .items
                                    .iter()
                                    .find(|old| old.item == item.item && old.message.is_some())
                            {
                                let error = item.error;
                                item = previous.clone();
                                item.error = error;
                            }
                            item
                        })
                        .collect();
                    if request.cursor.is_empty() {
                        list.items = items;
                    } else {
                        for item in items {
                            if let Some(old) =
                                list.items.iter_mut().find(|old| same_item(old, &item))
                            {
                                *old = item;
                            } else {
                                list.items.push(item);
                            }
                        }
                    }
                    if let Some(selected) = &mut self.selected
                        && let Some(task) = list
                            .items
                            .iter()
                            .find(|task| same_item(task, &selected.task))
                    {
                        selected.task = task.clone();
                        selected.completed = request.completed;
                    }
                    list.next_cursor = page.next_cursor;
                    if !list.next_cursor.is_empty() && list.next_cursor == request.cursor {
                        list.next_cursor.clear();
                        list.error = Some("Slack repeated a page. Refresh to try again.".into());
                    } else {
                        list.error = None;
                    }
                    list.loaded = true;
                    list.failed = None;
                }
                Err(error) => {
                    list.error = Some(error);
                    list.failed = Some(request);
                }
            }
        }
    }

    fn submit(&mut self, request: Request) {
        let list = &mut self.lists[request.completed as usize];
        if list.busy {
            return;
        }
        if self.commands.send(Command::Load(request)).is_ok() {
            list.busy = true;
        } else {
            list.error = Some("The to-do loader stopped. Reopen the app to retry.".into());
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, images: &mut ImageCache) {
        let mut request = None;
        let mut target = None;
        let index = self.completed as usize;
        egui::Panel::top("todos_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading("To-dos");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.lists[index].busy, egui::Button::new("Refresh"))
                            .clicked()
                        {
                            request = Some(Request {
                                completed: self.completed,
                                cursor: String::new(),
                            });
                        }
                        if self.lists[index].busy {
                            ui.weak("Updating…");
                        }
                    });
                });
            });
        theme::timeline_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.completed,
                    false,
                    format!("To-do · {}", self.counts[0]),
                );
                ui.selectable_value(
                    &mut self.completed,
                    true,
                    format!("Completed · {}", self.counts[1]),
                );
            });
            ui.add_space(8.0);
            let list = &self.lists[self.completed as usize];
            if !list.loaded && !list.busy && list.error.is_none() {
                request = Some(Request {
                    completed: self.completed,
                    cursor: String::new(),
                });
            }
            if let Some(error) = &list.error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(
                        theme::SELF_MENTION_TEXT,
                        format!("Could not load to-dos: {error}"),
                    );
                    if ui
                        .add_enabled(!list.busy, egui::Button::new("Retry"))
                        .clicked()
                    {
                        request = Some(list.failed.clone().unwrap_or(Request {
                            completed: self.completed,
                            cursor: String::new(),
                        }));
                    }
                });
                ui.add_space(8.0);
            }
            egui::ScrollArea::vertical()
                .id_salt(("slack_todos", self.completed))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !list.loaded && list.error.is_none() {
                        for _ in 0..6 {
                            skeleton(ui);
                        }
                    } else if list.loaded && list.items.is_empty() && list.next_cursor.is_empty() {
                        ui.add_space(12.0);
                        ui.weak(if self.completed {
                            "No completed to-dos."
                        } else {
                            "No open to-dos in Slack."
                        });
                    }
                    for task in &list.items {
                        if task_row(
                            ui,
                            task,
                            images,
                            self.completed,
                            self.selected
                                .as_ref()
                                .is_some_and(|selection| same_item(&selection.task, task)),
                        ) {
                            target = Some(task.clone());
                        }
                    }
                    if !list.next_cursor.is_empty() {
                        ui.add_space(12.0);
                        if ui
                            .add_enabled(!list.busy, egui::Button::new("Load more"))
                            .clicked()
                        {
                            request = Some(Request {
                                completed: self.completed,
                                cursor: list.next_cursor.clone(),
                            });
                        }
                        ui.add_space(12.0);
                    }
                });
        });
        if let Some(request) = request {
            self.submit(request);
        }
        if let Some(task) = target {
            self.selected = Some(Selection {
                task,
                completed: self.completed,
            });
        }
    }
}

fn same_item(a: &DisplayTodo, b: &DisplayTodo) -> bool {
    same_saved_item(&a.item, &b.item)
}

fn same_saved_item(a: &SavedTodo, b: &SavedTodo) -> bool {
    a.conversation_id == b.conversation_id && a.ts == b.ts && a.item_type == b.item_type
}

const ROW_HEIGHT: f32 = 86.0;

fn task_row(
    ui: &mut egui::Ui,
    task: &DisplayTodo,
    images: &mut ImageCache,
    completed: bool,
    selected: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &task.title)
    });
    if ui.is_rect_visible(rect) {
        if (ui.is_enabled() && ui.rect_contains_pointer(rect)) || selected {
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
        // The parent has already allocated this fixed-height row. Rendering
        // children must not advance its cursor again as rows enter the viewport.
        let mut content =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 12.0))));
        content.set_clip_rect(rect.intersect(ui.clip_rect()));
        content.horizontal_top(|ui| {
            if let Some(message) = &task.message {
                message_view::avatar(ui, message, images);
            } else {
                ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                let title = egui::RichText::new(&task.title)
                    .strong()
                    .color(if completed {
                        theme::TEXT_MUTED
                    } else {
                        theme::TEXT_STRONG
                    });
                ui.add(
                    egui::Label::new(title)
                        .truncate()
                        .selectable(false)
                        .show_tooltip_when_elided(false),
                );
                ui.add(
                    egui::Label::new(&task.preview)
                        .truncate()
                        .selectable(false)
                        .show_tooltip_when_elided(false),
                );
                let date = chrono::DateTime::from_timestamp(task.item.created_at, 0)
                    .map(|date| {
                        date.with_timezone(&chrono::Local)
                            .format("%-d %b")
                            .to_string()
                    })
                    .unwrap_or_default();
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{} · {date}", task.source))
                            .small()
                            .weak(),
                    )
                    .truncate()
                    .selectable(false)
                    .show_tooltip_when_elided(false),
                );
            });
        });
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.clicked()
}

fn copy_text_button(ui: &mut egui::Ui, blocks: &[crate::domain::message_text::Block]) {
    let id = ui.make_persistent_id("copied_until");
    let now = ui.input(|input| input.time);
    let copied_until = ui.data(|data| data.get_temp::<f64>(id)).unwrap_or(0.0);
    let copied = now < copied_until;
    if copied {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(copied_until - now));
    }
    let label = if copied { "Copied" } else { "Copy text" };
    if ui
        .add(egui::Button::new(label).min_size(egui::vec2(72.0, 0.0)))
        .clicked()
    {
        ui.ctx()
            .copy_text(crate::domain::message_text::clipboard_text(blocks));
        ui.data_mut(|data| data.insert_temp(id, now + 2.0));
        ui.ctx().request_repaint();
    }
}

fn skeleton(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let origin = rect.min + egui::vec2(52.0, 14.0);
    for (y, width) in [(0.0, 0.55), (22.0, 0.8), (44.0, 0.35)] {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                origin + egui::vec2(0.0, y),
                egui::vec2(((rect.width() - 64.0) * width).max(1.0), 10.0),
            ),
            2.0,
            theme::SURFACE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::SavedTodo;
    use crate::infrastructure::demo::DemoGateway;

    fn task(ts: &str) -> DisplayTodo {
        DisplayTodo {
            item: SavedTodo {
                conversation_id: "C1".into(),
                item_type: "message".into(),
                ts: ts.into(),
                created_at: 123,
                completed_at: 0,
            },
            message: None,
            title: "A long to-do title that must remain inside the row ".repeat(10),
            preview: "Summary of a saved Slack message ".repeat(30),
            source: "System Bot in a very long channel ".repeat(10),
            source_url: None,
            error: None,
        }
    }

    #[test]
    fn refresh_errors_preserve_items_and_pagination_does_not_duplicate_them() {
        let (sender, results) = channel();
        let (commands, _requests) = channel();
        let mut todos = Todos {
            lists: Default::default(),
            counts: [15, 83],
            completed: false,
            selected: None,
            completing: None,
            completion_error: None,
            commands,
            results,
        };
        todos.lists[0].items = vec![task("1")];
        todos.lists[0].loaded = true;
        sender
            .send(TodoResult::Page(
                Request {
                    completed: false,
                    cursor: String::new(),
                },
                Err("Offline".into()),
            ))
            .unwrap();
        todos.poll();
        assert_eq!(todos.lists[0].items.len(), 1);
        assert_eq!(todos.counts, [15, 83]);
        assert_eq!(todos.lists[0].error.as_deref(), Some("Offline"));
        sender
            .send(TodoResult::Page(
                Request {
                    completed: false,
                    cursor: "next".into(),
                },
                Ok(TodoPage {
                    items: vec![task("1"), task("2")],
                    open_count: 15,
                    completed_count: 83,
                    next_cursor: String::new(),
                }),
            ))
            .unwrap();
        todos.poll();
        assert_eq!(todos.lists[0].items.len(), 2);
        assert!(todos.lists[0].error.is_none());
        // A late result for Completed must not replace the open list.
        sender
            .send(TodoResult::Page(
                Request {
                    completed: true,
                    cursor: String::new(),
                },
                Ok(TodoPage {
                    items: vec![task("3")],
                    open_count: 15,
                    completed_count: 83,
                    next_cursor: "more".into(),
                }),
            ))
            .unwrap();
        todos.poll();
        assert_eq!(todos.lists[0].items.len(), 2);
        assert_eq!(todos.lists[1].items[0].item.ts, "3");
        assert_eq!(todos.lists[1].next_cursor, "more");
    }

    #[test]
    fn completion_waits_for_slack_and_never_completes_a_different_selection() {
        let (sender, results) = channel();
        let (commands, requests) = channel();
        let mut todos = Todos {
            lists: Default::default(),
            counts: [2, 83],
            completed: false,
            selected: Some(Selection {
                task: task("1"),
                completed: false,
            }),
            completing: None,
            completion_error: None,
            commands,
            results,
        };
        todos.lists[0].items = vec![task("1"), task("2")];
        todos.lists[0].loaded = true;
        todos.complete_selected();
        assert!(matches!(requests.try_recv().unwrap(), Command::Complete(item) if item.ts == "1"));
        todos.complete_selected();
        assert!(
            requests.try_recv().is_err(),
            "double clicks must not send twice"
        );
        assert_eq!(
            todos.lists[0].items.len(),
            2,
            "keep the task until Slack confirms"
        );
        sender
            .send(TodoResult::Completed(task("1").item, Err("Offline".into())))
            .unwrap();
        todos.poll();
        assert_eq!(todos.counts, [2, 83]);
        assert!(!todos.selected.as_ref().unwrap().completed);
        assert!(todos.completion_error.is_some());
        todos.complete_selected();
        assert!(matches!(requests.try_recv().unwrap(), Command::Complete(_)));
        todos.selected = Some(Selection {
            task: task("2"),
            completed: false,
        });
        sender
            .send(TodoResult::Completed(task("1").item, Ok(())))
            .unwrap();
        todos.poll();
        assert_eq!(todos.counts, [1, 84]);
        assert_eq!(todos.lists[0].items.len(), 1);
        assert_eq!(todos.lists[0].items[0].item.ts, "2");
        assert!(!todos.selected.as_ref().unwrap().completed);
        assert!(todos.completion_error.is_none());
        assert!(matches!(
            requests.try_recv().unwrap(),
            Command::Load(Request {
                completed: false,
                ..
            })
        ));
    }

    #[test]
    fn hovering_truncated_text_has_no_tooltip_and_clicks_select_the_row() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut images = ImageCache::new(
            Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        let task = task("1");
        // Pointer is over the preview text, rather than the blank row padding.
        let pos = egui::pos2(140.0, 40.0);
        let mut frame = |time, events| {
            let mut clicked = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 200.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    clicked = task_row(ui, &task, &mut images, false, false);
                },
            );
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| shape.clip_rect.height() > 200.0)
            );
            assert!(
                !ctx.memory(|memory| memory
                    .areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Tooltip)),
                "hover must not create a tooltip"
            );
            output.textures_delta.clear();
            clicked
        };
        frame(0.0, vec![egui::Event::PointerMoved(pos)]);
        frame(1.0, vec![]);
        frame(2.0, vec![]);
        assert!(frame(
            2.1,
            vec![
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default()
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default()
                },
            ]
        ));
    }

    #[test]
    fn long_todo_previews_do_not_change_row_size_on_resize_or_scroll() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut images = ImageCache::new(
            Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx.clone(),
        );
        let task = task("1");
        for width in [280.0, 720.0, 360.0] {
            let mut heights = vec![];
            for offset in [0.0, 600.0] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let size = egui::ScrollArea::vertical()
                            .vertical_scroll_offset(offset)
                            .show(ui, |ui| {
                                for _ in 0..20 {
                                    task_row(ui, &task, &mut images, false, false);
                                }
                            })
                            .content_size;
                        assert!(size.x <= width, "{size:?} exceeds {width}");
                        heights.push(size.y);
                    },
                );
                output.textures_delta.clear();
            }
            assert!(
                (heights[0] - heights[1]).abs() < 0.5,
                "{width}: {heights:?}"
            );
        }
    }
}
