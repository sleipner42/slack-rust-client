use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, channel},
};
use std::time::{Duration, Instant};

use eframe::egui;

use super::{images::ImageCache, theme};
use crate::application::activity::{ActivityPage, DisplayActivity};
use crate::application::chat_service::ChatService;
use crate::domain::activity::{ActivityFilter, ActivityQuery};

#[derive(Clone)]
struct Request {
    query: ActivityQuery,
    cursor: String,
    read_revision: u64,
}

enum ResultEvent {
    Preview(Request, ActivityPage),
    Loaded(Request, Result<ActivityPage, String>),
    Marked(crate::domain::activity::ActivityItem, Result<(), String>),
}

#[derive(Default)]
struct List {
    items: Vec<DisplayActivity>,
    next_cursor: String,
    loaded: bool,
    busy: bool,
    error: Option<String>,
    failed_cursor: String,
    updated: Option<Instant>,
}

pub struct Activity {
    query: ActivityQuery,
    /// Something happened since the feed behind the sidebar dot last loaded.
    feed_stale: bool,
    /// Unread mentions and direct messages already reported by `take_fresh`.
    announced: HashSet<(String, String)>,
    primed: bool,
    fresh: Vec<DisplayActivity>,
    lists: HashMap<ActivityQuery, List>,
    pub selected: Option<DisplayActivity>,
    dirty: bool,
    marking: HashSet<(String, String)>,
    pub read_error: Option<String>,
    read_revision: u64,
    read_marks: HashMap<String, (String, u64)>,
    service: Arc<ChatService>,
    ctx: egui::Context,
    sender: Sender<ResultEvent>,
    receiver: Receiver<ResultEvent>,
}

impl Activity {
    pub fn new(service: Arc<ChatService>, ctx: egui::Context, query: ActivityQuery) -> Self {
        let (sender, receiver) = channel();
        let mut activity = Self {
            query,
            feed_stale: false,
            announced: HashSet::new(),
            primed: false,
            fresh: Vec::new(),
            lists: HashMap::new(),
            selected: None,
            dirty: false,
            marking: HashSet::new(),
            read_error: None,
            read_revision: 0,
            read_marks: HashMap::new(),
            service,
            ctx,
            sender,
            receiver,
        };
        activity.open();
        activity
    }

    pub fn open(&mut self) {
        if self
            .lists
            .get(&self.query)
            .and_then(|list| list.updated)
            .is_none_or(|time| time.elapsed() > Duration::from_secs(60))
        {
            self.load(String::new());
        }
    }

    /// The filter shown, remembered between runs.
    pub fn query(&self) -> ActivityQuery {
        self.query
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.feed_stale = true;
    }

    /// Keep the feed behind the sidebar dot and notifications current. Leaves a
    /// list the user is reading alone; its Refresh button covers that.
    pub fn refresh(&mut self, showing: bool) {
        let query = ActivityQuery::default();
        if showing && self.query == query {
            return;
        }
        let Some(list) = self.lists.get(&query) else {
            return;
        };
        let due = if self.feed_stale {
            STALE_REFRESH
        } else {
            IDLE_REFRESH
        };
        if !list.busy && list.updated.is_some_and(|time| time.elapsed() >= due) {
            self.feed_stale = false;
            self.load_query(query, String::new());
        }
    }

    /// Unread mentions and direct messages that arrived since the last call.
    pub fn take_fresh(&mut self) -> Vec<DisplayActivity> {
        std::mem::take(&mut self.fresh)
    }

    fn announce(&mut self) {
        let Some(list) = self.lists.get(&ActivityQuery::default()) else {
            return;
        };
        for row in &list.items {
            if row.item.unread
                && NOTIFY_KINDS.contains(&row.item.kind.as_str())
                && self
                    .announced
                    .insert((row.item.key.clone(), row.item.feed_ts.clone()))
                && self.primed
            {
                self.fresh.push(row.clone());
            }
        }
        self.primed = true;
    }

    fn select(&mut self, row: DisplayActivity) {
        self.read_error = None;
        self.selected = Some(row);
        self.mark_selected_read();
    }

    pub fn mark_selected_read(&mut self) {
        let Some(item) = self.selected.as_ref().map(|row| row.item.clone()) else {
            return;
        };
        if !item.unread
            || !self
                .marking
                .insert((item.key.clone(), item.feed_ts.clone()))
        {
            return;
        }
        self.read_error = None;
        let service = self.service.clone();
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let result = service.mark_activity_read(&item).map_err(|e| e.to_string());
            let _ = sender.send(ResultEvent::Marked(item, result));
            ctx.request_repaint();
        });
    }

    pub fn marking(&self) -> bool {
        self.selected.as_ref().is_some_and(|row| {
            self.marking
                .contains(&(row.item.key.clone(), row.item.feed_ts.clone()))
        })
    }

    pub fn has_unreads(&self) -> bool {
        self.lists
            .get(&ActivityQuery::default())
            .is_some_and(|list| list.items.iter().any(|row| row.item.unread))
    }

    fn load(&mut self, cursor: String) {
        self.load_query(self.query, cursor);
    }

    fn load_query(&mut self, query: ActivityQuery, cursor: String) {
        let list = self.lists.entry(query).or_default();
        if list.busy {
            return;
        }
        list.busy = true;
        list.error = None;
        if query == self.query {
            self.dirty = false;
        }
        let request = Request {
            query,
            cursor,
            read_revision: self.read_revision,
        };
        let service = self.service.clone();
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            if request.cursor.is_empty()
                && let Some(page) = service.cached_activity(request.query)
            {
                let _ = sender.send(ResultEvent::Preview(request.clone(), page));
                ctx.request_repaint();
            }
            let result = service
                .activity(request.query, &request.cursor, |page| {
                    let _ = sender.send(ResultEvent::Preview(request.clone(), page));
                    ctx.request_repaint();
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(ResultEvent::Loaded(request, result));
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ResultEvent::Marked(item, result) => {
                    self.marking
                        .remove(&(item.key.clone(), item.feed_ts.clone()));
                    let is_selected = self.selected.as_ref().is_some_and(|row| {
                        row.item.key == item.key && row.item.feed_ts == item.feed_ts
                    });
                    if let Err(error) = result {
                        if is_selected {
                            self.read_error = Some(error);
                        }
                        continue;
                    }
                    if is_selected {
                        self.read_error = None;
                    }
                    self.read_revision += 1;
                    self.read_marks
                        .insert(item.key.clone(), (item.feed_ts.clone(), self.read_revision));
                    for row in self
                        .lists
                        .values_mut()
                        .flat_map(|list| &mut list.items)
                        .chain(self.selected.iter_mut())
                    {
                        if row.item.key == item.key
                            && crate::domain::model::ts_seconds(&row.item.feed_ts)
                                <= crate::domain::model::ts_seconds(&item.feed_ts)
                        {
                            row.item.unread = false;
                            row.item.unread_count = 0;
                        }
                    }
                    for (query, list) in &mut self.lists {
                        if query.unread_only {
                            list.items.retain(|row| row.item.unread);
                        }
                    }
                }
                ResultEvent::Preview(request, page) => {
                    let list = self.lists.entry(request.query).or_default();
                    if !list.loaded && list.items.is_empty() {
                        list.items = page.items;
                    }
                }
                ResultEvent::Loaded(request, result) => {
                    let list = self.lists.entry(request.query).or_default();
                    list.busy = false;
                    match result {
                        Ok(page) => {
                            if request.cursor.is_empty() {
                                list.items.clear();
                            }
                            for mut row in page.items {
                                if let Some((ts, revision)) = self.read_marks.get(&row.item.key)
                                    && *revision > request.read_revision
                                    && crate::domain::model::ts_seconds(&row.item.feed_ts)
                                        <= crate::domain::model::ts_seconds(ts)
                                {
                                    row.item.unread = false;
                                    row.item.unread_count = 0;
                                }
                                if let Some(selection) = &mut self.selected
                                    && selection.item.key == row.item.key
                                    && crate::domain::model::ts_seconds(&row.item.feed_ts)
                                        >= crate::domain::model::ts_seconds(&selection.item.feed_ts)
                                {
                                    *selection = row.clone();
                                }
                                if let Some(old) = list
                                    .items
                                    .iter_mut()
                                    .find(|old| old.item.key == row.item.key)
                                {
                                    *old = row;
                                } else {
                                    list.items.push(row);
                                }
                            }
                            if request.query.unread_only {
                                list.items.retain(|row| row.item.unread);
                            }
                            list.next_cursor = if page.next_cursor == request.cursor {
                                String::new()
                            } else {
                                page.next_cursor
                            };
                            list.error = page.warning;
                            list.loaded = true;
                            list.updated = Some(Instant::now());
                            list.failed_cursor.clear();
                            if request.query == ActivityQuery::default()
                                && request.cursor.is_empty()
                            {
                                self.announce();
                            }
                        }
                        Err(error) => {
                            list.error = Some(error);
                            list.failed_cursor = request.cursor;
                        }
                    }
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, images: &mut ImageCache) -> Option<DisplayActivity> {
        let mut load = None;
        let mut open = None;
        let previous_query = self.query;
        let busy = self.lists.get(&self.query).is_some_and(|list| list.busy);
        egui::Panel::top("activity_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading("Activity");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(if self.dirty {
                                    "New activity · Refresh"
                                } else {
                                    "Refresh"
                                }),
                            )
                            .clicked()
                        {
                            load = Some(String::new());
                        }
                        if busy {
                            ui.weak("Updating…");
                        }
                    });
                });
            });
        theme::timeline_frame().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for filter in ActivityFilter::ALL {
                    ui.selectable_value(&mut self.query.filter, filter, filter.label());
                }
            });
            ui.add_space(4.0);
            ui.checkbox(&mut self.query.unread_only, "Unread only");
            ui.add_space(8.0);
            ui.separator();
            let list = self.lists.entry(self.query).or_default();
            if !list.loaded && !list.busy && list.error.is_none() {
                load = Some(String::new());
            }
            if let Some(error) = &list.error {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(theme::SELF_MENTION_TEXT, error);
                    if ui
                        .add_enabled(!list.busy, egui::Button::new("Retry"))
                        .clicked()
                    {
                        load = Some(list.failed_cursor.clone());
                    }
                });
            }
            egui::ScrollArea::vertical()
                .id_salt(("activity_list", self.query))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if list.items.is_empty() && list.busy {
                        for _ in 0..7 {
                            skeleton(ui);
                        }
                    } else if list.loaded && list.items.is_empty() {
                        ui.add_space(24.0);
                        ui.label(if self.query.unread_only {
                            "All caught up"
                        } else {
                            "No activity here yet"
                        });
                    }
                    for row in &list.items {
                        let selected = self
                            .selected
                            .as_ref()
                            .is_some_and(|s| s.item.key == row.item.key);
                        if activity_row(ui, row, images, selected) {
                            open = Some(row.clone());
                        }
                    }
                    if !list.next_cursor.is_empty() {
                        ui.add_space(12.0);
                        if ui
                            .add_enabled(!list.busy, egui::Button::new("Load more"))
                            .clicked()
                        {
                            load = Some(list.next_cursor.clone());
                        }
                        ui.add_space(12.0);
                    }
                });
        });
        if previous_query != self.query {
            self.open();
        }
        if let Some(cursor) = load {
            self.load(cursor);
        }
        if let Some(row) = &open {
            self.select(row.clone());
        }
        open
    }
}

/// Slack's default desktop notifications: mentions and direct messages.
const NOTIFY_KINDS: [&str; 7] = [
    "at_user",
    "at_user_group",
    "at_channel",
    "at_everyone",
    "keyword",
    "dm",
    "bot_dm_bundle",
];
const STALE_REFRESH: Duration = Duration::from_secs(15);
const IDLE_REFRESH: Duration = Duration::from_secs(90);
const ROW_HEIGHT: f32 = 82.0;

fn activity_row(
    ui: &mut egui::Ui,
    row: &DisplayActivity,
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
            format!(
                "{} · {} · {}{}",
                row.actor,
                row.item.label(),
                row.source,
                if row.item.unread { " · unread" } else { "" }
            ),
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
        if row.item.unread {
            ui.painter()
                .circle_filled(rect.min + egui::vec2(12.0, 25.0), 3.0, theme::UNREAD_DOT);
        }
        let mut content = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
            rect.min + egui::vec2(24.0, 12.0),
            rect.max - egui::vec2(12.0, 10.0),
        )));
        content.set_clip_rect(rect.intersect(ui.clip_rect()));
        content.horizontal_top(|ui| {
            avatar(ui, row.avatar_url.as_deref(), &row.actor, images);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut name = egui::RichText::new(&row.actor).color(if row.item.unread {
                        theme::TEXT_STRONG
                    } else {
                        theme::TEXT
                    });
                    if row.item.unread {
                        name = name.family(theme::bold_family());
                    }
                    let time = chrono::DateTime::from_timestamp(
                        crate::domain::model::ts_seconds(&row.item.feed_ts) as i64,
                        0,
                    )
                    .map(|date| {
                        date.with_timezone(&chrono::Local)
                            .format("%-d %b · %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                    let available = ui.available_width();
                    ui.allocate_ui_with_layout(
                        egui::vec2((available - 100.0).max(24.0), 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(name)
                                    .truncate()
                                    .selectable(false)
                                    .show_tooltip_when_elided(false),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(time).small().weak());
                    });
                });
                ui.add(
                    egui::Label::new(&row.preview)
                        .truncate()
                        .selectable(false)
                        .show_tooltip_when_elided(false),
                );
                let count = if row.item.unread_count > 1 {
                    format!(" · {} unread", row.item.unread_count)
                } else {
                    String::new()
                };
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} · {}{count}",
                            row.item.label(),
                            row.source
                        ))
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
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// A small avatar, or the name's initial while it loads.
pub(super) fn avatar(ui: &mut egui::Ui, url: Option<&str>, name: &str, images: &mut ImageCache) {
    if let Some(image) = url.and_then(|url| images.image(url)) {
        ui.add(
            image
                .fit_to_exact_size(egui::vec2(28.0, 28.0))
                .corner_radius(4),
        );
    } else {
        let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
        ui.painter().rect_filled(r, 4.0, theme::ACCENT);
        let initial = name
            .chars()
            .next()
            .unwrap_or('·')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(13.0),
            theme::TEXT,
        );
    }
}

pub(super) fn skeleton(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::hover(),
    );
    for (y, fraction) in [(14.0, 0.4), (35.0, 0.8), (56.0, 0.55)] {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.min + egui::vec2(64.0, y),
                egui::vec2(((rect.width() - 80.0) * fraction).max(1.0), 9.0),
            ),
            2.0,
            theme::SURFACE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::activity::ActivityItem;
    use crate::infrastructure::demo::DemoGateway;

    fn row(key: &str) -> DisplayActivity {
        DisplayActivity {
            item: ActivityItem {
                key: key.into(),
                kind: "thread_v2".into(),
                feed_ts: "100.0".into(),
                unread: true,
                unread_count: 2,
                conversation_id: "C1".into(),
                ts: "100.0".into(),
                thread_ts: Some("80.0".into()),
                actor_id: None,
                reaction: None,
            },
            message: None,
            actor: "A very long author name ".repeat(10),
            avatar_url: None,
            source: "#a-long-channel-name".repeat(20),
            preview: "Long message preview ".repeat(100),
        }
    }

    fn activity() -> Activity {
        let (sender, receiver) = channel();
        Activity {
            query: ActivityQuery::default(),
            feed_stale: false,
            announced: HashSet::new(),
            primed: false,
            fresh: Vec::new(),
            lists: HashMap::new(),
            selected: None,
            dirty: false,
            marking: HashSet::new(),
            read_error: None,
            read_revision: 0,
            read_marks: HashMap::new(),
            service: Arc::new(ChatService::new(Arc::new(DemoGateway::new()))),
            ctx: egui::Context::default(),
            sender,
            receiver,
        }
    }

    #[test]
    fn opening_activity_marks_it_read_once_even_when_switching_quickly() {
        let mut activity = activity();
        let gateway = Arc::new(DemoGateway::new());
        activity.service = Arc::new(ChatService::new(gateway.clone()));
        let first = row("1");
        let second = row("2");
        let unread_query = ActivityQuery {
            unread_only: true,
            ..Default::default()
        };
        activity.lists.insert(
            unread_query,
            List {
                items: vec![first.clone(), second.clone()],
                ..Default::default()
            },
        );
        // Hold the fixture's acknowledgements while several clicks arrive.
        let blocked = gateway.activity_reads.lock().unwrap();
        activity.select(first.clone());
        activity.select(second);
        activity.select(first);
        assert_eq!(
            activity.marking.len(),
            2,
            "each item is submitted once; switching must not skip the second"
        );
        assert!(activity.selected.as_ref().unwrap().item.unread);
        assert_eq!(
            activity.lists[&unread_query].items.len(),
            2,
            "wait for Slack before clearing unreads"
        );
        drop(blocked);
        for _ in 0..2 {
            let event = activity
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            activity.sender.send(event).unwrap();
            activity.poll();
            if activity.marking.is_empty() {
                break;
            }
        }
        assert!(activity.marking.is_empty());
        assert!(activity.lists[&unread_query].items.is_empty());
        assert!(!activity.selected.as_ref().unwrap().item.unread);
        let read = activity.selected.as_ref().unwrap().clone();
        activity.select(read);
        assert!(
            activity.marking.is_empty(),
            "reopening a read item does not send again"
        );
        let reads = gateway.activity_reads.lock().unwrap();
        assert_eq!(reads.len(), 2);
        assert!(reads.iter().any(|item| item.key == "1"));
        assert!(reads.iter().any(|item| item.key == "2"));
    }

    #[test]
    fn new_unread_mentions_and_dms_are_reported_once_but_not_the_backlog() {
        let mut activity = activity();
        let all = ActivityQuery::default();
        let item = |key: &str, kind: &str, feed_ts: &str, unread: bool| {
            let mut row = row(key);
            row.item.kind = kind.into();
            row.item.feed_ts = feed_ts.into();
            row.item.unread = unread;
            row
        };
        let load = |activity: &mut Activity, items: Vec<DisplayActivity>| {
            activity
                .sender
                .send(ResultEvent::Loaded(
                    Request {
                        query: all,
                        cursor: String::new(),
                        read_revision: 0,
                    },
                    Ok(ActivityPage {
                        items,
                        next_cursor: String::new(),
                        warning: None,
                    }),
                ))
                .unwrap();
            activity.poll();
            activity
                .take_fresh()
                .into_iter()
                .map(|row| row.item.key)
                .collect::<Vec<_>>()
        };
        let backlog = item("old", "at_user", "1.0", true);
        assert!(load(&mut activity, vec![backlog.clone()]).is_empty());
        let fresh = vec![
            item("mention", "at_user", "2.0", true),
            item("dm", "dm", "3.0", true),
            item("thread", "thread_v2", "4.0", true),
            item("read", "at_user_group", "5.0", false),
            backlog.clone(),
        ];
        assert_eq!(load(&mut activity, fresh.clone()), ["mention", "dm"]);
        assert!(
            load(&mut activity, fresh).is_empty(),
            "report each item once"
        );
        let again = item("mention", "at_user", "6.0", true);
        assert_eq!(load(&mut activity, vec![again]), ["mention"]);
    }

    #[test]
    fn the_sidebar_feed_refreshes_in_the_background_unless_it_is_being_read() {
        let mut activity = activity();
        let all = ActivityQuery::default();
        activity.lists.insert(
            all,
            List {
                updated: Some(Instant::now() - STALE_REFRESH),
                ..Default::default()
            },
        );
        activity.refresh(false);
        assert!(!activity.lists[&all].busy, "nothing new happened yet");
        activity.invalidate();
        activity.refresh(true);
        assert!(
            !activity.lists[&all].busy,
            "the open list waits for Refresh"
        );
        activity.refresh(false);
        assert!(activity.lists[&all].busy);
    }

    #[test]
    fn activity_refresh_failure_and_late_filter_results_preserve_the_visible_feed() {
        let mut activity = activity();
        let all = ActivityQuery::default();
        let request = |query, cursor: &str| Request {
            query,
            cursor: cursor.into(),
            read_revision: 0,
        };
        let page = |keys: &[&str], cursor: &str| ActivityPage {
            items: keys.iter().map(|key| row(key)).collect(),
            next_cursor: cursor.into(),
            warning: None,
        };
        activity
            .sender
            .send(ResultEvent::Loaded(
                request(all, ""),
                Ok(page(&["1"], "next")),
            ))
            .unwrap();
        activity.poll();
        activity
            .sender
            .send(ResultEvent::Preview(request(all, ""), page(&["stale"], "")))
            .unwrap();
        activity
            .sender
            .send(ResultEvent::Loaded(request(all, ""), Err("Offline".into())))
            .unwrap();
        activity.poll();
        assert_eq!(activity.lists[&all].items[0].item.key, "1");
        assert_eq!(activity.lists[&all].next_cursor, "next");
        assert!(activity.lists[&all].error.is_some());
        activity
            .sender
            .send(ResultEvent::Loaded(
                request(all, "next"),
                Ok(page(&["1", "2"], "next")),
            ))
            .unwrap();
        activity.query = ActivityQuery {
            filter: ActivityFilter::Dms,
            unread_only: true,
        };
        activity.poll();
        assert_eq!(activity.query.filter, ActivityFilter::Dms);
        assert_eq!(activity.lists[&all].items.len(), 2);
        assert!(
            activity.lists[&all].next_cursor.is_empty(),
            "repeated cursors cannot loop"
        );
        assert!(!activity.lists.contains_key(&activity.query));
    }

    #[test]
    fn marking_read_waits_for_confirmation_and_cannot_clear_a_newer_reply() {
        let mut activity = activity();
        let old = row("1");
        activity.selected = Some(old.clone());
        activity.lists.insert(
            ActivityQuery::default(),
            List {
                items: vec![old.clone()],
                ..Default::default()
            },
        );
        activity
            .sender
            .send(ResultEvent::Marked(old.item.clone(), Err("Offline".into())))
            .unwrap();
        activity.poll();
        assert!(activity.selected.as_ref().unwrap().item.unread);
        assert!(activity.read_error.is_some());
        activity.selected.as_mut().unwrap().item.feed_ts = "101.0".into();
        activity
            .sender
            .send(ResultEvent::Marked(old.item, Ok(())))
            .unwrap();
        activity.poll();
        assert!(
            !activity.lists[&ActivityQuery::default()].items[0]
                .item
                .unread
        );
        assert!(activity.selected.as_ref().unwrap().item.unread);
        activity
            .sender
            .send(ResultEvent::Loaded(
                Request {
                    query: ActivityQuery::default(),
                    cursor: String::new(),
                    read_revision: 0,
                },
                Ok(ActivityPage {
                    items: vec![row("1")],
                    ..Default::default()
                }),
            ))
            .unwrap();
        activity.poll();
        assert!(
            !activity.lists[&ActivityQuery::default()].items[0]
                .item
                .unread,
            "an older in-flight response must not undo Mark read"
        );
    }

    #[test]
    fn activity_rows_keep_geometry_and_text_clicks_work_without_tooltips() {
        let activity = activity();
        let ctx = &activity.ctx;
        theme::apply(ctx);
        let mut images = ImageCache::new(activity.service.clone(), ctx.clone());
        let row = row("1");
        for width in [280.0, 400.0, 900.0] {
            let mut heights = vec![];
            for offset in [0.0, 500.0] {
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
                                    activity_row(ui, &row, &mut images, false);
                                }
                            })
                            .content_size;
                        assert!(size.x <= width, "{size:?} exceeds {width}");
                        heights.push(size.y);
                    },
                );
                output.textures_delta.clear();
            }
            assert!((heights[0] - heights[1]).abs() < 0.5);
        }
        let pos = egui::pos2(140.0, 40.0);
        for (time, click) in [(0.0, false), (1.0, false), (2.0, true)] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if click {
                for pressed in [true, false] {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            let mut clicked = false;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 200.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    clicked = activity_row(ui, &row, &mut images, false);
                },
            );
            assert_eq!(clicked, click);
            assert!(!ctx.memory(|m| {
                m.areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Tooltip)
            }));
            output.textures_delta.clear();
        }
    }
}
