use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::AtomExt as _;

use crate::application::chat_service::{ChatService, DisplayMessage, Workspace};
use crate::application::live_updates::{LiveStatus, spawn_live_updates};
use crate::domain::gateway::RealtimeSource;
use crate::domain::model::{Conversation, LiveEvent, ReadState, SlackError, ts_seconds};
use crate::domain::notify::should_notify;
use crate::domain::sidebar::{SidebarInput, build_sidebar};
use crate::domain::unread::{record_incoming, record_read};
use crate::ui::activity::Activity;
use crate::ui::composer::{ComposeTarget, Draft, composer};
use crate::ui::delete_dialog::{DeleteDialog, DeleteOutcome, Removal};
use crate::ui::dock::DockBadge;
use crate::ui::emoji_picker::{EmojiPicker, PickerOutcome};
use crate::ui::huddles::{CallChoice, Huddles};
use crate::ui::image_viewer::ImageViewer;
use crate::ui::images::ImageCache;
use crate::ui::labels::{conversation_label, paint_sidebar_icon};
use crate::ui::message_view::{
    AttachmentClick, MessageAction, MessageContext, MessageEdit, loading_messages, message_list,
};
use crate::ui::outbox::{Outbox, SendRequest};
use crate::ui::quick_switcher::{QuickSwitcher, SwitcherOutcome};
use crate::ui::search::Search;
use crate::ui::share_dialog::{ShareDialog, ShareOutcome};
use crate::ui::theme;
use crate::ui::todos::Todos;

pub fn run(
    service: ChatService,
    team_name: String,
    realtime: Option<Arc<dyn RealtimeSource>>,
) -> eframe::Result {
    crate::infrastructure::spell_checker::start_loading();
    let mut wgpu_options = eframe::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.power_preference = eframe::wgpu::PowerPreference::LowPower;
    }
    let options = eframe::NativeOptions {
        wgpu_options,
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../assets/app-icon.png"))
                    .expect("bundled app icon is valid PNG"),
            )
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        &team_name.clone(),
        options,
        Box::new(move |cc| {
            Ok(Box::new(ChatApp::new(
                service,
                team_name,
                realtime,
                cc.egui_ctx.clone(),
            )))
        }),
    )
}

struct ThreadView {
    target: ComposeTarget,
    messages: Vec<DisplayMessage>,
    error: Option<String>,
}

struct ChatApp {
    service: Arc<ChatService>,
    ctx: egui::Context,
    sender: Sender<UiEvent>,
    receiver: Receiver<UiEvent>,
    team_name: String,
    workspace: Option<Workspace>,
    selected: Option<Conversation>,
    /// Read position of `selected` when it was opened, if it had unreads then.
    new_after: Option<String>,
    /// Places to go back and forward to, and the one last recorded.
    back: Vec<Place>,
    forward: Vec<Place>,
    here: Option<Place>,
    messages: Vec<DisplayMessage>,
    history_cache: HashMap<ComposeTarget, (Instant, Vec<DisplayMessage>)>,
    unread_only: bool,
    /// View choices as last saved.
    settings: crate::infrastructure::ui_settings::UiSettings,
    todos_open: bool,
    todos: Option<Todos>,
    activity_open: bool,
    activity: Option<Activity>,
    search_open: bool,
    search: Search,
    huddles: Huddles,
    drafts: HashMap<ComposeTarget, Draft>,
    draft_store: crate::infrastructure::draft_store::DraftStore,
    /// Drafts as last written to disk, to write only when something changed.
    saved_drafts: (
        crate::infrastructure::draft_store::Drafts,
        crate::infrastructure::draft_store::Synced,
    ),
    drafts_checked: Instant,
    draft_sync: crate::application::draft_sync::DraftSync,
    /// Emoji picked in this app; Slack's own history is in the workspace.
    local_emoji: crate::domain::emoji_usage::EmojiUsage,
    /// When Slack's drafts were last requested, while none have loaded.
    drafts_requested: Option<Instant>,
    paste_shortcut: crate::infrastructure::clipboard::PasteShortcut,
    composer_focus: Option<ComposeTarget>,
    outbox: Outbox,
    send_requests: Sender<SendRequest>,
    scroll_after_send: HashSet<ComposeTarget>,
    thread: Option<ThreadView>,
    /// The own message being edited, and the timeline it is shown in.
    editing: Option<(ComposeTarget, MessageEdit)>,
    images: ImageCache,
    status: Option<String>,
    message_error: Option<String>,
    loading: HashSet<ComposeTarget>,
    reload: HashSet<ComposeTarget>,
    pending_reactions: HashSet<(String, String, String)>,
    read_attempts: HashMap<String, String>,
    read_loading: HashSet<String>,
    last_poll: Instant,
    last_counts: Instant,
    counts_loading: bool,
    workspace_loading: bool,
    revision: u64,
    switcher: Option<QuickSwitcher>,
    picker: Option<EmojiPicker>,
    share: Option<ShareDialog>,
    deleting: Option<DeleteDialog>,
    image_viewer: Option<ImageViewer>,
    dock: DockBadge,
    realtime: Option<Arc<dyn RealtimeSource>>,
    live_connected: bool,
    /// Messages already notified, and when the app started: replays never notify.
    notified: HashSet<String>,
    started: f64,
}

enum UiEvent {
    WorkspaceLoaded(Result<Workspace, SlackError>),
    MessagesLoaded(ComposeTarget, Result<Vec<DisplayMessage>, SlackError>),
    MessagesCached(ComposeTarget, Vec<DisplayMessage>),
    MessagesPreview(ComposeTarget, Vec<DisplayMessage>),
    MessageSent(SendRequest, Result<String, SlackError>),
    Downloaded(String, Result<std::path::PathBuf, SlackError>),
    VideoOpened(crate::domain::model::Attachment, Result<(), SlackError>),
    ImageCopied(Result<egui::ColorImage, SlackError>),
    OpenFromNotification {
        conversation_id: String,
        thread_ts: Option<String>,
    },
    ShareLinked {
        destination: Conversation,
        comment: String,
        link: Result<String, SlackError>,
    },
    Deleted {
        conversation_id: String,
        removal: Removal,
        result: Result<(), SlackError>,
    },
    MessageEdited {
        list: ComposeTarget,
        ts: String,
        text: String,
        result: Result<(), SlackError>,
    },
    ReactionSet {
        conversation_id: String,
        ts: String,
        name: String,
        result: Result<(), SlackError>,
    },
    Marked(String, String, Result<(), SlackError>),
    Counts(u64, Result<HashMap<String, ReadState>, SlackError>),
    EmojiLoaded(Result<crate::application::emoji_catalog::EmojiCatalog, SlackError>),
    PeopleLoaded(Result<Vec<crate::domain::people::Person>, SlackError>),
    DraftsListed(Result<Vec<crate::domain::model::RemoteDraft>, SlackError>),
    DraftPushed(
        crate::application::draft_sync::Push,
        Result<Option<crate::domain::model::RemoteDraft>, SlackError>,
    ),
    Live(LiveStatus),
    HuddleNamed {
        conversation_id: String,
        participants: Vec<String>,
        names: Vec<String>,
    },
    CallerFound(crate::domain::model::UserProfile),
    /// Answer or Decline chosen in the call notification.
    CallChosen(String, CallChoice),
    /// Slack's answer to joining a conversation's huddle.
    HuddleJoined(String, Result<crate::domain::model::ChimeJoin, SlackError>),
    /// News from the call with this Slack call ID.
    CallUpdated(String, crate::infrastructure::huddle_call::CallUpdate),
}

impl eframe::App for ChatApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let shortcut = self.paste_shortcut.take(&input.events);
        let paste = input
            .events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_)));
        if !(shortcut || paste)
            || !input.focused
            || self.switcher.is_some()
            || self.picker.is_some()
            || self.share.is_some()
            || self.deleting.is_some()
            || self.image_viewer.is_some()
        {
            return;
        }
        let target = self
            .drafts
            .keys()
            .find(|target| ctx.memory(|memory| memory.has_focus(target.editor_id())))
            .cloned();
        let Some(target) = target else {
            return;
        };
        match crate::infrastructure::clipboard::read_attachment() {
            Ok(Some(content)) => {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Paste(_)));
                self.drafts
                    .entry(target)
                    .or_default()
                    .attachments
                    .paste(content, ctx);
            }
            Err(error) => {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Paste(_)));
                self.drafts.entry(target).or_default().attachments.error = Some(error.to_string());
            }
            Ok(None) => {} // egui handles ordinary text paste unchanged.
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.save_drafts();
        self.save_settings();
        if let Some(todos) = &mut self.todos {
            todos.poll();
        }
        let mut until_activity = None;
        let fresh = self.activity.as_mut().map_or_else(Vec::new, |activity| {
            activity.poll();
            until_activity = activity.refresh(self.activity_open);
            activity.take_fresh()
        });
        for row in fresh {
            self.notify_activity(row);
        }
        self.search.poll();
        if let Some(after) = self.huddles.tick() {
            ctx.request_repaint_after(after);
        }
        self.images.poll();
        self.poll();
        self.dock.update(self.workspace.as_ref());
        let until_poll = self
            .poll_interval()
            .saturating_sub(self.last_poll.elapsed());
        let until_counts = self
            .counts_interval()
            .saturating_sub(self.last_counts.elapsed());
        let next = until_activity.map_or(until_poll.min(until_counts), |until_activity| {
            until_poll.min(until_counts).min(until_activity)
        });
        ctx.request_repaint_after(next.max(Duration::from_secs(1)));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.input_mut(|input| {
            input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::U,
            )
        }) {
            self.unread_only = !self.unread_only;
        }
        self.track_place();
        self.history_navigation(ui.ctx());
        self.search_shortcuts(ui.ctx());
        self.quick_switcher(ui.ctx());
        self.emoji_picker(ui.ctx());
        self.share_dialog(ui.ctx());
        self.delete_dialog(ui.ctx());
        if let Some(choice) = self.huddles.banner(ui.ctx(), &mut self.images) {
            self.call_choice(choice);
        }
        if let Some(viewer) = &mut self.image_viewer {
            let (close, click) = viewer.show(ui.ctx(), &mut self.images);
            let attachment = viewer.attachment().clone();
            if close {
                self.image_viewer = None;
            }
            match click {
                Some(AttachmentClick::CopyImage) => self.copy_image(attachment),
                Some(AttachmentClick::Download) => self.download(attachment),
                Some(AttachmentClick::View | AttachmentClick::Delete) | None => {}
            }
        }
        let modal_open = self.switcher.is_some()
            || self.picker.is_some()
            || self.share.is_some()
            || self.deleting.is_some()
            || self.image_viewer.is_some();
        ui.add_enabled_ui(!modal_open, |ui| {
            let sidebar_max = (ui.available_width() * 0.3).clamp(180.0, 360.0);
            egui::Panel::left("sidebar")
                .resizable(true)
                .default_size(240.0)
                .size_range(180.0..=sidebar_max)
                .frame(
                    egui::Frame::new()
                        .fill(theme::SIDEBAR)
                        .inner_margin(theme::GUTTER),
                )
                .show(ui, |ui| self.sidebar(ui));
            let todo_selected =
                self.todos_open && self.todos.as_ref().is_some_and(Todos::has_selection);
            let inline_todo = todo_selected && ui.available_width() < 640.0;
            if todo_selected && !inline_todo {
                let max_width = (ui.available_width() - 280.0).clamp(280.0, 600.0);
                egui::Panel::right("todo_detail_panel")
                    .resizable(true)
                    .default_size(420.0)
                    .size_range(280.0..=max_width)
                    .frame(egui::Frame::new().fill(theme::BACKGROUND))
                    .show(ui, |ui| self.todo_details(ui));
            }
            let inline_thread =
                !self.todos_open && self.thread.is_some() && ui.available_width() < 640.0;
            if !self.todos_open && self.thread.is_some() && !inline_thread {
                egui::Panel::right("thread_panel")
                    .resizable(true)
                    .default_size(390.0)
                    .size_range(280.0..=600.0)
                    .frame(egui::Frame::new().fill(theme::BACKGROUND))
                    .show(ui, |ui| self.thread_panel(ui));
            }
            egui::CentralPanel::default_margins()
                .frame(egui::Frame::new().fill(theme::BACKGROUND))
                .show(ui, |ui| {
                    if self.todos_open {
                        if let Some(todos) = &mut self.todos {
                            if inline_todo {
                                self.todo_details(ui);
                            } else {
                                todos.show(ui, &mut self.images);
                            }
                        } else {
                            theme::timeline_frame().show(ui, |ui| {
                                ui.weak("To-dos will be available when the workspace has loaded.");
                            });
                        }
                    } else if self.activity_open && !inline_thread {
                        if let Some(activity) = &mut self.activity
                            && let Some(row) = activity.show(ui, &mut self.images)
                        {
                            self.open_activity_item(row);
                        }
                    } else if self.search_open && !inline_thread {
                        if let Some(result) = self.search.show(ui, &mut self.images) {
                            self.open_message(
                                result.conversation_id.clone(),
                                result
                                    .thread_ts
                                    .clone()
                                    .unwrap_or(result.message.ts.clone()),
                                Some(result.message),
                            );
                        }
                    } else if inline_thread {
                        self.thread_panel(ui);
                    } else {
                        self.conversation(ui);
                    }
                });
        });
    }
}

impl ChatApp {
    fn new(
        service: ChatService,
        team_name: String,
        realtime: Option<Arc<dyn RealtimeSource>>,
        ctx: egui::Context,
    ) -> Self {
        theme::apply(&ctx);
        egui_extras::install_image_loaders(&ctx);
        let (sender, receiver) = channel();
        let service = Arc::new(service);
        let images = ImageCache::new(Arc::clone(&service), ctx.clone());
        // Serialize outbound requests to preserve the user's message order.
        let (send_requests, send_queue) = channel::<SendRequest>();
        let send_service = Arc::clone(&service);
        let send_events = sender.clone();
        let send_ctx = ctx.clone();
        thread::spawn(move || {
            while let Ok(mut request) = send_queue.recv() {
                let result = if request.uploads.files.is_empty() {
                    send_service.send(
                        &request.target.conversation_id,
                        &request.text,
                        request.target.thread_ts.as_deref(),
                        &request.id,
                    )
                } else {
                    send_service.send_files(
                        &request.target.conversation_id,
                        &request.text,
                        request.target.thread_ts.as_deref(),
                        &mut request.uploads,
                    )
                };
                if send_events
                    .send(UiEvent::MessageSent(request, result))
                    .is_err()
                {
                    break;
                }
                send_ctx.request_repaint();
            }
        });
        let settings = crate::infrastructure::ui_settings::UiSettings::load();
        let search = Search::new(Arc::clone(&service), ctx.clone(), settings.search_sort);
        let mut app = Self {
            service,
            ctx,
            sender,
            receiver,
            team_name,
            workspace: None,
            selected: None,
            new_after: None,
            editing: None,
            back: Vec::new(),
            forward: Vec::new(),
            here: None,
            messages: Vec::new(),
            history_cache: HashMap::new(),
            unread_only: settings.unread_only,
            settings,
            todos_open: false,
            todos: None,
            activity_open: false,
            activity: None,
            search_open: false,
            search,
            huddles: Huddles::default(),
            drafts: HashMap::new(),
            draft_store: crate::infrastructure::draft_store::DraftStore::new(
                crate::infrastructure::draft_store::DraftStore::default_path(),
            ),
            saved_drafts: Default::default(),
            drafts_checked: Instant::now(),
            draft_sync: Default::default(),
            local_emoji: crate::infrastructure::emoji_usage_store::load(),
            drafts_requested: None,
            paste_shortcut: Default::default(),
            composer_focus: None,
            outbox: Outbox::default(),
            send_requests,
            scroll_after_send: HashSet::new(),
            thread: None,
            images,
            status: None,
            message_error: None,
            loading: HashSet::new(),
            reload: HashSet::new(),
            pending_reactions: HashSet::new(),
            read_attempts: HashMap::new(),
            read_loading: HashSet::new(),
            last_poll: Instant::now(),
            last_counts: Instant::now(),
            counts_loading: false,
            workspace_loading: false,
            revision: 0,
            switcher: None,
            picker: None,
            share: None,
            deleting: None,
            image_viewer: None,
            dock: DockBadge::default(),
            realtime,
            live_connected: false,
            notified: HashSet::new(),
            started: chrono::Utc::now().timestamp() as f64,
        };
        app.restore_drafts();
        app.load_workspace();
        app
    }

    fn load_workspace(&mut self) {
        if self.workspace_loading {
            return;
        }
        self.workspace_loading = true;
        self.status = None;
        self.spawn(|service| UiEvent::WorkspaceLoaded(service.load_workspace()));
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        if let Some(call) = self.huddles.call() {
            let names: HashMap<String, String> = self
                .huddles
                .live(&call.conversation_id)
                .filter(|live| live.names.len() == live.huddle.participants.len())
                .map(|live| {
                    live.huddle
                        .participants
                        .iter()
                        .cloned()
                        .zip(live.names.iter().cloned())
                        .collect()
                })
                .unwrap_or_default();
            let action = egui::Panel::bottom("huddle_call_bar")
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    top: 8,
                    ..Default::default()
                }))
                .show(ui, |ui| crate::ui::huddles::call_bar(ui, call, &names))
                .inner;
            if let Some(action) = action {
                self.call_action(action);
            }
        }
        ui.heading(&self.team_name);
        ui.add_space(8.0);
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                egui::Button::new((
                    "Jump to…",
                    egui::Atom::grow(),
                    egui::RichText::new(SWITCHER_SHORTCUT).weak(),
                )),
            )
            .clicked()
        {
            self.switcher = Some(QuickSwitcher::new());
        }
        if ui
            .add_sized(
                [ui.available_width(), 30.0],
                egui::Button::selectable(
                    self.search_open,
                    (
                        "Search",
                        egui::Atom::grow(),
                        egui::RichText::new(SEARCH_SHORTCUT).weak(),
                    ),
                ),
            )
            .clicked()
        {
            self.open_search(None);
        }
        let Some(workspace) = &self.workspace else {
            if self.workspace_loading {
                ui.weak("Loading conversations…");
            }
            return;
        };
        let unread_count = workspace
            .conversations
            .iter()
            .filter(|conversation| needs_attention(workspace, &conversation.id))
            .count();

        if ui
            .add_sized(
                [ui.available_width(), 30.0],
                egui::Button::selectable(
                    self.todos_open,
                    (
                        "To-dos",
                        egui::Atom::grow(),
                        egui::RichText::new(
                            self.todos.as_ref().map_or(0, Todos::open_count).to_string(),
                        )
                        .weak(),
                    ),
                ),
            )
            .clicked()
        {
            self.todos_open = true;
            self.activity_open = false;
            self.search_open = false;
            self.thread = None;
            self.composer_focus = None;
            if let Some(todos) = &mut self.todos {
                todos.open();
            }
        }
        let activity_response = ui.add_sized(
            [ui.available_width(), 30.0],
            egui::Button::selectable(self.activity_open, ("Activity", egui::Atom::grow())),
        );
        if self.activity.as_ref().is_some_and(Activity::has_unreads) {
            ui.painter().circle_filled(
                egui::pos2(
                    activity_response.rect.right() - 14.0,
                    activity_response.rect.center().y,
                ),
                3.0,
                theme::UNREAD_DOT,
            );
        }
        if activity_response.clicked() {
            if !self.activity_open {
                self.thread = None;
            }
            self.activity_open = true;
            self.todos_open = false;
            self.search_open = false;
            self.composer_focus = None;
            if let Some(activity) = &mut self.activity {
                activity.open();
            }
        }
        ui.add_space(8.0);
        // The unread filter is a view setting, so it sits on the divider above
        // the conversations rather than among the navigation entries.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 22.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let filter_id = ui.make_persistent_id("unread_filter_icon");
                let icon = egui::Atom::custom(filter_id, egui::vec2(12.0, 12.0));
                let button = if unread_count > 0 {
                    egui::Button::selectable(
                        self.unread_only,
                        (
                            icon,
                            egui::RichText::new(unread_count.to_string())
                                .small()
                                .color(theme::LINK),
                        ),
                    )
                } else {
                    egui::Button::selectable(self.unread_only, icon)
                };
                let output = button.gap(4.0).atom_ui(ui);
                if let Some(rect) = output.rect(filter_id) {
                    let color = if self.unread_only {
                        theme::TEXT_STRONG
                    } else {
                        theme::TEXT_MUTED
                    };
                    paint_filter(ui, rect, color);
                }
                let tooltip = if self.unread_only {
                    format!("Showing unread conversations · {UNREAD_SHORTCUT} shows all")
                } else {
                    format!("Show only unread conversations · {UNREAD_SHORTCUT}")
                };
                if output.response.on_hover_text(tooltip).clicked() {
                    self.unread_only = !self.unread_only;
                }
                let (line, _) = ui.allocate_exact_size(
                    egui::vec2((ui.available_width() - 6.0).max(0.0), 1.0),
                    egui::Sense::hover(),
                );
                ui.painter().hline(
                    line.x_range(),
                    line.center().y,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
            },
        );
        ui.add_space(6.0);
        let selected = self
            .selected
            .as_ref()
            .filter(|_| self.conversation_shown())
            .map(|conversation| conversation.id.as_str());
        let mut clicked = None;
        // Conversations with an unsent message, marked with a pencil as in Slack.
        let drafted: HashSet<&str> = self
            .drafts
            .iter()
            .filter(|(target, draft)| target.thread_ts.is_none() && !draft.text.trim().is_empty())
            .map(|(target, _)| target.conversation_id.as_str())
            .collect();
        egui::ScrollArea::vertical()
            .id_salt(("sidebar_scroll", self.unread_only))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.unread_only {
                    let mut conversations: Vec<_> = workspace
                        .conversations
                        .iter()
                        .filter(|conversation| {
                            needs_attention(workspace, &conversation.id)
                                || self.huddles.live(&conversation.id).is_some()
                        })
                        .collect();
                    conversations.sort_by_key(|conversation| {
                        (
                            workspace
                                .read_states
                                .get(&conversation.id)
                                .is_none_or(|state| state.mention_count == 0),
                            conversation.name.to_lowercase(),
                        )
                    });
                    if conversations.is_empty() {
                        ui.weak("All caught up");
                    }
                    for conversation in conversations {
                        let draft = drafted.contains(conversation.id.as_str());
                        let huddle = self.huddles.live(&conversation.id).is_some();
                        if sidebar_row(ui, workspace, conversation, selected, draft, huddle) {
                            clicked = Some(conversation.clone());
                        }
                    }
                } else {
                    let open = self
                        .selected
                        .as_ref()
                        .map(|conversation| conversation.id.as_str());
                    let mut first = true;
                    for (index, group) in workspace.sidebar.iter().enumerate() {
                        let conversations: Vec<_> = group
                            .conversations
                            .iter()
                            .filter(|conversation| shown_in_sidebar(workspace, conversation, open))
                            .collect();
                        if conversations.is_empty() {
                            continue;
                        }
                        if !first {
                            ui.add_space(8.0);
                        }
                        first = false;
                        let unread = conversations
                            .iter()
                            .filter(|conversation| needs_attention(workspace, &conversation.id))
                            .count();
                        let title = if unread > 0 {
                            format!("{}  ·  {unread}", group.title)
                        } else {
                            group.title.clone()
                        };
                        egui::CollapsingHeader::new(egui::RichText::new(title).strong().color(
                            if unread > 0 {
                                theme::TEXT_STRONG
                            } else {
                                theme::TEXT_MUTED
                            },
                        ))
                        .id_salt(("section", index))
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for conversation in conversations {
                                let draft = drafted.contains(conversation.id.as_str());
                                let huddle = self.huddles.live(&conversation.id).is_some();
                                if sidebar_row(ui, workspace, conversation, selected, draft, huddle)
                                {
                                    clicked = Some(conversation.clone());
                                }
                            }
                        });
                    }
                }
            });
        if let Some(conversation) = clicked {
            self.select(conversation);
        }
    }

    fn quick_switcher(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            self.picker = None;
            self.share = None;
            self.deleting = None;
            self.image_viewer = None;
            self.switcher = Some(QuickSwitcher::new());
        }
        let Some(switcher) = self.switcher.as_mut() else {
            return;
        };
        let conversations = self
            .workspace
            .as_ref()
            .map_or(&[][..], |workspace| workspace.conversations.as_slice());
        match switcher.show(ctx, conversations) {
            SwitcherOutcome::Open => {}
            SwitcherOutcome::Closed => self.switcher = None,
            SwitcherOutcome::Chosen(conversation) => {
                self.switcher = None;
                let target = ComposeTarget {
                    conversation_id: conversation.id.clone(),
                    thread_ts: None,
                };
                self.select(conversation);
                // Selecting the current channel should also return from its thread.
                self.thread = None;
                self.composer_focus = Some(target);
                ctx.request_repaint();
            }
        }
    }

    fn emoji_picker(&mut self, ctx: &egui::Context) {
        let (Some(picker), Some(workspace)) = (&mut self.picker, &self.workspace) else {
            return;
        };
        match picker.show(ctx, &workspace.emoji, &mut self.images) {
            PickerOutcome::Open => {}
            PickerOutcome::Closed => self.picker = None,
            PickerOutcome::Chosen(name) => {
                let picker = self.picker.take().unwrap();
                let mine = self
                    .messages
                    .iter()
                    .chain(self.thread.iter().flat_map(|thread| &thread.messages))
                    .filter(|message| message.ts == picker.ts)
                    .flat_map(|message| &message.reactions)
                    .any(|reaction| {
                        reaction.name == name && reaction.user_ids.contains(&workspace.self_user_id)
                    });
                self.set_reaction(picker.conversation_id, picker.ts, name, !mine);
            }
        }
    }

    fn share_dialog(&mut self, ctx: &egui::Context) {
        let (Some(share), Some(workspace)) = (&mut self.share, &self.workspace) else {
            return;
        };
        match share.show(ctx, &workspace.conversations) {
            ShareOutcome::Open => {}
            ShareOutcome::Closed => self.share = None,
            ShareOutcome::Share {
                destination,
                comment,
            } => {
                let share = self.share.take().unwrap();
                self.spawn(move |service| UiEvent::ShareLinked {
                    destination,
                    comment,
                    link: service.permalink(&share.conversation_id, &share.ts),
                });
            }
        }
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.deleting else {
            return;
        };
        match dialog.show(ctx) {
            DeleteOutcome::Open => {}
            DeleteOutcome::Closed => self.deleting = None,
            DeleteOutcome::Confirm => {
                dialog.busy = true;
                dialog.error = None;
                let conversation_id = dialog.conversation_id.clone();
                let removal = dialog.removal.clone();
                let files = dialog.file_ids();
                self.spawn(move |service| UiEvent::Deleted {
                    result: match &removal {
                        Removal::Message { ts, .. } => {
                            service.delete_message(&conversation_id, ts, &files)
                        }
                        Removal::File { id, .. } => service.delete_file(id),
                    },
                    conversation_id,
                    removal,
                });
            }
        }
    }

    /// Take a deleted message or file off the screen at once; the refresh
    /// brings what Slack shows in its place.
    fn finish_delete(
        &mut self,
        conversation_id: String,
        removal: Removal,
        result: Result<(), SlackError>,
    ) {
        if let Err(error) = result {
            match self
                .deleting
                .as_mut()
                .filter(|dialog| dialog.removal == removal)
            {
                Some(dialog) => {
                    dialog.busy = false;
                    dialog.error = Some(error.to_string());
                }
                None => notify(&format!("Could not delete: {error}")),
            }
            return;
        }
        if self
            .deleting
            .as_ref()
            .is_some_and(|dialog| dialog.removal == removal)
        {
            self.deleting = None;
        }
        let main = self.is_selected(&conversation_id);
        let lists = [
            main.then_some(&mut self.messages),
            self.thread
                .as_mut()
                .filter(|thread| thread.target.conversation_id == conversation_id)
                .map(|thread| &mut thread.messages),
        ];
        for messages in lists.into_iter().flatten() {
            match &removal {
                Removal::Message { ts, .. } => messages.retain(|message| message.ts != *ts),
                Removal::File { id, .. } => {
                    for message in messages.iter_mut() {
                        message
                            .attachments
                            .retain(|attachment| attachment.file_id.as_ref() != Some(id));
                    }
                }
            }
        }
        if let (Removal::Message { ts, .. }, Some((_, edit))) = (&removal, &self.editing)
            && edit.ts == *ts
        {
            self.editing = None;
        }
        self.refresh_conversation(&conversation_id);
    }

    fn conversation(&mut self, ui: &mut egui::Ui) {
        let Some(conversation) = self.selected.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(90.0);
                ui.heading(&self.team_name);
                ui.label(if self.workspace_loading {
                    String::from("Loading conversations…")
                } else {
                    format!("Choose a conversation or press {SWITCHER_SHORTCUT} to jump in.")
                });
                if let Some(status) = &self.status {
                    ui.colored_label(theme::SELF_MENTION_TEXT, status);
                }
                if self.workspace.is_none()
                    && !self.workspace_loading
                    && ui.button("Retry loading workspace").clicked()
                {
                    self.load_workspace();
                }
            });
            return;
        };
        egui::Panel::top("conversation_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if ui
                        .add_enabled(
                            !self.back.is_empty(),
                            egui::Button::new(BACK_ARROW).frame(false),
                        )
                        .on_hover_text(format!("Back · {BACK_LABEL} or Ctrl+O"))
                        .clicked()
                    {
                        self.go_back();
                    }
                    if ui
                        .add_enabled(
                            !self.forward.is_empty(),
                            egui::Button::new(FORWARD_ARROW).frame(false),
                        )
                        .on_hover_text(format!("Forward · {FORWARD_LABEL} or Ctrl+I"))
                        .clicked()
                    {
                        self.go_forward();
                    }
                    ui.heading(conversation_label(&conversation));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.live_connected {
                            ui.label(egui::RichText::new("Live").small().weak());
                        }
                        let live = self.huddles.live(&conversation.id);
                        let in_call = self.huddles.in_call(&conversation.id);
                        let headphones = ui.make_persistent_id("huddle_headphones");
                        let label = match live {
                            _ if in_call => "In huddle".into(),
                            Some(live) => {
                                format!("Join huddle · {}", live.huddle.participants.len())
                            }
                            None => "Huddle".into(),
                        };
                        let button = egui::Button::new((
                            egui::Atom::custom(headphones, egui::vec2(16.0, 16.0)),
                            label,
                        ));
                        let output = if live.is_some() && !in_call {
                            button.fill(theme::ACCENT)
                        } else {
                            button
                        }
                        .atom_ui(ui);
                        if let Some(rect) = output.rect(headphones) {
                            crate::ui::huddles::paint_headphones(ui, rect, theme::TEXT);
                        }
                        let hover = match live {
                            _ if in_call => "You are in this huddle".into(),
                            Some(live) if live.names.len() == live.huddle.participants.len() => {
                                format!("In the huddle: {}", live.names.join(", "))
                            }
                            Some(_) => "Join the huddle".into(),
                            None if matches!(
                                conversation.kind,
                                crate::domain::model::ConversationKind::DirectMessage { .. }
                            ) =>
                            {
                                "Start a huddle and call them".into()
                            }
                            None => "Start a huddle".into(),
                        };
                        if output.response.on_hover_text(hover).clicked() && !in_call {
                            self.join_huddle(&conversation.id);
                        }
                    });
                });
            });
        let target = ComposeTarget {
            conversation_id: conversation.id.clone(),
            thread_ts: None,
        };
        egui::Panel::bottom("main_composer")
            .frame(theme::composer_panel_frame())
            .show_separator_line(false)
            .show(ui, |ui| {
                self.compose(
                    ui,
                    &target,
                    &format!("Message {}", conversation_label(&conversation)),
                );
            });
        let output = theme::timeline_frame()
            .show(ui, |ui| {
                if let Some(status) = self.status.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(theme::SELF_MENTION_TEXT, status);
                        if ui.small_button("Dismiss").clicked() {
                            self.status = None;
                        }
                    });
                }
                if self.loading.contains(&target)
                    && self.messages.is_empty()
                    && !self.outbox.has_messages(&target)
                {
                    loading_messages(ui);
                    return None;
                }
                if let Some(error) = self.message_error.clone() {
                    ui.colored_label(theme::SELF_MENTION_TEXT, error);
                    if ui.button("Retry messages").clicked() {
                        self.load_messages(target.clone());
                    }
                } else if !self.loading.contains(&target)
                    && self.messages.is_empty()
                    && !self.outbox.has_messages(&target)
                {
                    ui.weak("No messages yet. Start the conversation.");
                }
                let workspace = self.workspace.as_ref()?;
                Some(
                    egui::ScrollArea::vertical()
                        .id_salt(("messages", &conversation.id))
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let list = message_list(
                                ui,
                                &conversation.id,
                                &self.outbox.messages(&target, &self.messages),
                                &mut MessageContext {
                                    huddles: self.huddles.known(),
                                    huddle_call: self
                                        .huddles
                                        .call()
                                        .map(|call| call.conversation_id.as_str()),
                                    people: &workspace.people,
                                    preferred: &preferred_people(workspace),
                                    emoji: &workspace.emoji,
                                    images: &mut self.images,
                                    self_user_id: &workspace.self_user_id,
                                    allow_threads: true,
                                    outbox: &self.outbox,
                                    new_after: self.new_after.as_deref(),
                                    edit: self
                                        .editing
                                        .as_mut()
                                        .filter(|(list, _)| *list == target)
                                        .map(|(_, edit)| edit),
                                },
                            );
                            if self.scroll_after_send.remove(&target) {
                                ui.scroll_to_cursor_animation(
                                    Some(egui::Align::BOTTOM),
                                    egui::style::ScrollAnimation::none(),
                                );
                                ui.ctx().request_discard("show outgoing message");
                            }
                            crate::ui::rich_text::scroll_text_selection(ui, list.selecting);
                            list
                        }),
                )
            })
            .inner;
        let Some(output) = output else {
            return;
        };
        if let Some(action) = output.inner.action {
            self.message_action(target, action);
        }
        let at_bottom =
            output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 4.0;
        if at_bottom
            && ui.is_enabled()
            && ui.input(|input| input.focused)
            && let Some(ts) = self.messages.last().map(|message| message.ts.clone())
        {
            self.mark_read(conversation.id, ts);
        }
    }

    fn todo_details(&mut self, ui: &mut egui::Ui) {
        if let (Some(todos), Some(workspace)) = (&mut self.todos, &self.workspace)
            && let Some((attachment, click)) =
                todos.show_details(ui, &workspace.emoji, &mut self.images)
        {
            match click {
                AttachmentClick::View => self.view_attachment(attachment),
                AttachmentClick::Download => self.download(attachment),
                AttachmentClick::CopyImage => self.copy_image(attachment),
                AttachmentClick::Delete => {}
            }
        }
    }

    fn open_activity_item(&mut self, row: crate::application::activity::DisplayActivity) {
        let root = row
            .item
            .thread_ts
            .clone()
            .unwrap_or_else(|| row.item.ts.clone());
        self.open_message(row.item.conversation_id, root, row.message);
    }

    /// Show a message, with its thread, in the side panel.
    fn open_message(
        &mut self,
        conversation_id: String,
        root: String,
        message: Option<DisplayMessage>,
    ) {
        let target = ComposeTarget {
            conversation_id,
            thread_ts: Some(root),
        };
        let messages = self
            .history_cache
            .get(&target)
            .map(|(_, messages)| messages.clone())
            .unwrap_or_else(|| message.into_iter().collect());
        self.thread = Some(ThreadView {
            target: target.clone(),
            messages,
            error: None,
        });
        self.load_messages(target);
    }

    fn thread_panel(&mut self, ui: &mut egui::Ui) {
        let mut close = false;
        egui::Panel::top("thread_header")
            .exact_size(theme::HEADER_HEIGHT)
            .frame(theme::header_frame())
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading(if self.activity_open || self.search_open {
                        "Message"
                    } else {
                        "Thread"
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui.button("Close ×").clicked();
                        if self.activity_open
                            && let Some(activity) = &mut self.activity
                            && activity
                                .selected
                                .as_ref()
                                .is_some_and(|row| row.item.unread)
                            && ui
                                .add_enabled(
                                    !activity.marking(),
                                    egui::Button::new(if activity.marking() {
                                        "Marking read…"
                                    } else {
                                        "Mark read"
                                    }),
                                )
                                .clicked()
                        {
                            activity.mark_selected_read();
                        }
                    });
                });
            });
        if close {
            self.thread = None;
            if self.activity_open
                && let Some(activity) = &mut self.activity
            {
                activity.selected = None;
            }
            if self.search_open {
                self.search.selected = None;
            }
            return;
        }
        let target = self.thread.as_ref().unwrap().target.clone();
        egui::Panel::bottom("thread_composer")
            .frame(theme::composer_panel_frame())
            .show_separator_line(false)
            .show(ui, |ui| {
                self.compose(ui, &target, "Reply in thread…");
            });
        let thread = self.thread.as_ref().unwrap();
        let mut retry = false;
        let action = theme::timeline_frame()
            .show(ui, |ui| {
                if self.activity_open
                    && let Some(selection) = self
                        .activity
                        .as_ref()
                        .and_then(|activity| activity.selected.as_ref())
                {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            selection.item.label(),
                            selection.source
                        ))
                        .small()
                        .weak(),
                    );
                    ui.add_space(8.0);
                    if let Some(error) = self
                        .activity
                        .as_ref()
                        .and_then(|activity| activity.read_error.as_ref())
                    {
                        ui.colored_label(theme::SELF_MENTION_TEXT, error);
                    }
                }
                if self.search_open
                    && let Some(selection) = &self.search.selected
                {
                    ui.label(egui::RichText::new(&selection.source).small().weak());
                    ui.add_space(8.0);
                }
                if let Some(error) = &thread.error {
                    ui.colored_label(theme::SELF_MENTION_TEXT, error);
                    retry = ui.button("Retry thread").clicked();
                }
                if self.loading.contains(&target)
                    && thread.messages.is_empty()
                    && !self.outbox.has_messages(&target)
                {
                    loading_messages(ui);
                    return None;
                }
                let workspace = self.workspace.as_ref()?;
                egui::ScrollArea::vertical()
                    .id_salt(("thread_messages", &target))
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let list = message_list(
                            ui,
                            "thread",
                            &self.outbox.messages(&target, &thread.messages),
                            &mut MessageContext {
                                huddles: self.huddles.known(),
                                huddle_call: self
                                    .huddles
                                    .call()
                                    .map(|call| call.conversation_id.as_str()),
                                people: &workspace.people,
                                preferred: &preferred_people(workspace),
                                emoji: &workspace.emoji,
                                images: &mut self.images,
                                self_user_id: &workspace.self_user_id,
                                allow_threads: false,
                                outbox: &self.outbox,
                                new_after: None,
                                edit: self
                                    .editing
                                    .as_mut()
                                    .filter(|(list, _)| *list == target)
                                    .map(|(_, edit)| edit),
                            },
                        );
                        if self.scroll_after_send.remove(&target) {
                            ui.scroll_to_cursor_animation(
                                Some(egui::Align::BOTTOM),
                                egui::style::ScrollAnimation::none(),
                            );
                            ui.ctx().request_discard("show outgoing reply");
                        }
                        crate::ui::rich_text::scroll_text_selection(ui, list.selecting);
                        list.action
                    })
                    .inner
            })
            .inner;
        if retry {
            self.load_messages(target.clone());
        }
        if let Some(action) = action {
            self.message_action(target, action);
        }
    }

    fn compose(&mut self, ui: &mut egui::Ui, target: &ComposeTarget, hint: &str) {
        let draft = self.drafts.entry(target.clone()).or_default();
        if draft.text.is_empty()
            && draft.attachments.is_empty()
            && ui.is_enabled()
            && ui.memory(|memory| memory.has_focus(target.editor_id()))
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
        {
            self.edit_latest(target);
        }
        let draft = self.drafts.entry(target.clone()).or_default();
        if self.composer_focus.as_ref() == Some(target) && ui.is_enabled() {
            // Modal interaction is cleared at the end of the closing pass. Wait for
            // that handoff, then focus before the editor processes keyboard input.
            if ui.memory(|memory| memory.allows_interaction(ui.layer_id())) {
                ui.memory_mut(|memory| memory.request_focus(target.editor_id()));
                self.composer_focus = None;
            } else {
                ui.ctx().request_repaint();
            }
        }
        let preferred = self
            .workspace
            .as_ref()
            .map(preferred_people)
            .unwrap_or_default();
        let completion =
            self.workspace
                .as_ref()
                .map(|workspace| crate::ui::emoji_completion::EmojiCompletion {
                    emoji: &workspace.emoji,
                    people: &workspace.people,
                    preferred: &preferred,
                    images: &mut self.images,
                });
        if composer(ui, target, draft, hint, completion) {
            let text = std::mem::take(&mut draft.text);
            let mentions = std::mem::take(&mut draft.mentions);
            let files = draft.attachments.take();
            let text = crate::domain::message_text::link_mentions(
                &text,
                &self.mention_tokens(&text, mentions),
            );
            self.queue_message(target.clone(), text, files);
        }
    }

    /// The people to mention in `text`: those picked from the suggestions,
    /// and anyone else written out as `@Full Name` whose name is unique.
    fn mention_tokens(
        &self,
        text: &str,
        mut picked: Vec<crate::ui::emoji_completion::Mention>,
    ) -> Vec<crate::ui::emoji_completion::Mention> {
        let Some(workspace) = &self.workspace else {
            return picked;
        };
        for person in workspace.people.iter() {
            let label = person.label();
            if text.contains(&label)
                && !picked.iter().any(|(picked, _)| *picked == label)
                && workspace
                    .people
                    .iter()
                    .filter(|other| other.name == person.name)
                    .count()
                    == 1
            {
                picked.push((label, person.markup()));
            }
        }
        picked
    }

    fn queue_message(
        &mut self,
        target: ComposeTarget,
        text: String,
        files: Vec<crate::domain::uploads::UploadFile>,
    ) {
        let used: Vec<String> = crate::domain::spelling::emoji_names(&text)
            .into_iter()
            .map(|range| text[range.start + 1..range.end - 1].to_owned())
            .filter(|name| {
                self.workspace.as_ref().is_some_and(|workspace| {
                    !matches!(
                        workspace.emoji.resolve(name),
                        crate::application::emoji_catalog::EmojiGlyph::Missing(_)
                    )
                })
            })
            .collect();
        self.record_emoji(used);
        let id = uuid::Uuid::new_v4().to_string();
        let mut message = self
            .service
            .outgoing_message(&text, target.thread_ts.as_deref(), &id);
        message.attachments = files
            .iter()
            .map(|file| crate::domain::model::Attachment {
                file_id: None,
                name: file.name.clone(),
                preview_url: None,
                full_url: None,
                permalink: None,
                width: None,
                height: None,
                preview_pending: false,
                video: false,
            })
            .collect();
        let request = SendRequest {
            id,
            target,
            text,
            uploads: crate::domain::uploads::UploadBatch {
                files,
                ..Default::default()
            },
        };
        self.outbox.insert(request.clone(), message);
        self.scroll_after_send.insert(request.target.clone());
        self.dispatch_send(request);
    }

    fn dispatch_send(&mut self, request: SendRequest) {
        if self.send_requests.send(request.clone()).is_err() {
            self.outbox.finish(
                &request.id,
                Err(SlackError(
                    "Send worker stopped. Reopen the app to retry.".into(),
                )),
            );
        }
    }

    fn select(&mut self, conversation: Conversation) {
        self.track_place();
        self.show_conversation(conversation);
        self.track_place();
    }

    /// The conversation on screen and the thread open beside it; `None`
    /// while another view (search, activity, to-dos) is shown.
    fn place(&self) -> Option<Place> {
        let conversation = self
            .selected
            .clone()
            .filter(|_| self.conversation_shown())?;
        let thread = self
            .thread
            .as_ref()
            .filter(|thread| thread.target.conversation_id == conversation.id)
            .and_then(|thread| thread.target.thread_ts.clone());
        Some(Place {
            conversation,
            thread,
        })
    }

    /// Record a move to another conversation or thread, however it happened,
    /// so Back returns to where the user was.
    fn track_place(&mut self) {
        let Some(now) = self.place() else {
            return;
        };
        if self.here.as_ref() == Some(&now) {
            return;
        }
        if let Some(previous) = self.here.replace(now) {
            push_history(&mut self.back, previous);
            self.forward.clear();
        }
    }

    fn go_back(&mut self) {
        self.track_place();
        if let Some(place) = self.back.pop() {
            if let Some(current) = self.here.take() {
                push_history(&mut self.forward, current);
            }
            self.go_to(place);
        }
    }

    fn go_forward(&mut self) {
        self.track_place();
        if let Some(place) = self.forward.pop() {
            if let Some(current) = self.here.take() {
                push_history(&mut self.back, current);
            }
            self.go_to(place);
        }
    }

    fn go_to(&mut self, place: Place) {
        let target = target_of(&place.conversation.id);
        self.show_conversation(place.conversation.clone());
        match &place.thread {
            Some(root) => self.message_action(target, MessageAction::OpenThread(root.clone())),
            None => {
                self.thread = None;
                self.composer_focus = Some(target);
            }
        }
        self.here = Some(place);
    }

    /// Whether the focused message box has text in it, so Ctrl+I there
    /// means italic rather than Forward.
    fn typing(&self, ctx: &egui::Context) -> bool {
        let Some(focused) = ctx.memory(|memory| memory.focused()) else {
            return false;
        };
        let draft = self
            .drafts
            .iter()
            .find(|(target, _)| target.editor_id() == focused);
        match draft {
            Some((_, draft)) => !draft.text.is_empty(),
            // Another text field, such as search or an edit.
            None => true,
        }
    }

    fn history_navigation(&mut self, ctx: &egui::Context) {
        if self.switcher.is_some()
            || self.picker.is_some()
            || self.share.is_some()
            || self.deleting.is_some()
            || self.image_viewer.is_some()
        {
            return;
        }
        let typing = self.typing(ctx);
        let (back, forward) = ctx.input_mut(|input| {
            (
                input.consume_key(BACK_SHORTCUT.0, BACK_SHORTCUT.1)
                    // Vim's jump back.
                    || input.consume_key(egui::Modifiers::CTRL, egui::Key::O)
                    || input.pointer.button_pressed(egui::PointerButton::Extra1),
                input.consume_key(FORWARD_SHORTCUT.0, FORWARD_SHORTCUT.1)
                    || (!typing && input.consume_key(egui::Modifiers::CTRL, egui::Key::I))
                    || input.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if back {
            self.go_back();
        } else if forward {
            self.go_forward();
        }
    }

    fn show_conversation(&mut self, conversation: Conversation) {
        self.todos_open = false;
        if self.activity_open || self.search_open {
            self.thread = None;
        }
        self.activity_open = false;
        self.search_open = false;
        self.composer_focus = None;
        if self.is_selected(&conversation.id) {
            return;
        }
        let target = ComposeTarget {
            conversation_id: conversation.id.clone(),
            thread_ts: None,
        };
        self.messages = self
            .history_cache
            .get_mut(&target)
            .map(|(used, messages)| {
                *used = Instant::now();
                messages.clone()
            })
            .unwrap_or_default();
        self.message_error = None;
        self.thread = None;
        self.picker = None;
        self.read_attempts.remove(&conversation.id);
        self.load_messages(ComposeTarget {
            conversation_id: conversation.id.clone(),
            thread_ts: None,
        });
        self.last_poll = Instant::now();
        self.new_after = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.read_states.get(&conversation.id))
            .filter(|state| state.has_unreads)
            .and_then(|state| state.last_read_ts.clone());
        self.selected = Some(conversation);
    }

    fn message_action(&mut self, list: ComposeTarget, action: MessageAction) {
        let conversation_id = list.conversation_id.clone();
        match action {
            MessageAction::Edit(ts) => self.start_edit(list, &ts),
            MessageAction::CancelEdit => {
                self.editing = None;
                self.composer_focus = Some(list);
            }
            MessageAction::SaveEdit => self.save_edit(),
            MessageAction::Delete(ts) => {
                let files = self
                    .list_messages(&list)
                    .iter()
                    .find(|message| message.ts == ts)
                    .map(|message| {
                        message
                            .attachments
                            .iter()
                            .filter_map(|attachment| attachment.file_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                self.deleting = Some(DeleteDialog::new(
                    conversation_id,
                    Removal::Message { ts, files },
                ));
            }
            MessageAction::DeleteFile(attachment) => {
                if let Some(id) = attachment.file_id {
                    self.deleting = Some(DeleteDialog::new(
                        conversation_id,
                        Removal::File {
                            id,
                            name: attachment.name,
                        },
                    ));
                }
            }
            MessageAction::RetrySend(id) => {
                if let Some(request) = self.outbox.retry(&id) {
                    self.dispatch_send(request);
                }
            }
            MessageAction::OpenThread(ts) => {
                let target = ComposeTarget {
                    conversation_id,
                    thread_ts: Some(ts.clone()),
                };
                let messages = self
                    .history_cache
                    .get(&target)
                    .map(|(_, messages)| messages.clone())
                    .unwrap_or_else(|| {
                        self.messages
                            .iter()
                            .filter(|message| message.ts == ts)
                            .cloned()
                            .collect()
                    });
                self.thread = Some(ThreadView {
                    target: target.clone(),
                    messages,
                    error: None,
                });
                self.load_messages(target);
            }
            MessageAction::Share(ts) => {
                self.picker = None;
                self.share = Some(ShareDialog::new(conversation_id, ts));
            }
            MessageAction::JoinHuddle(conversation_id) => self.join_huddle(&conversation_id),
            MessageAction::OpenShare {
                conversation_id,
                ts,
            } => {
                let target = ComposeTarget {
                    conversation_id,
                    thread_ts: Some(ts),
                };
                let messages = self
                    .history_cache
                    .get(&target)
                    .map(|(_, messages)| messages.clone())
                    .unwrap_or_default();
                self.thread = Some(ThreadView {
                    target: target.clone(),
                    messages,
                    error: None,
                });
                self.load_messages(target);
            }
            MessageAction::Download(attachment) => self.download(attachment),
            MessageAction::CopyImage(attachment) => self.copy_image(attachment),
            MessageAction::OpenImage(attachment) => self.view_attachment(attachment),
            MessageAction::PickReaction(ts) => {
                self.picker = Some(EmojiPicker::new(conversation_id, ts))
            }
            MessageAction::ToggleReaction { ts, name, add } => {
                self.set_reaction(conversation_id, ts, name, add)
            }
        }
    }

    fn view_attachment(&mut self, attachment: crate::domain::model::Attachment) {
        if attachment.video {
            self.play_video(attachment);
        } else {
            self.image_viewer = Some(ImageViewer::new(attachment));
        }
    }

    /// Fetch a video once and hand it to the system's default video player.
    fn play_video(&mut self, attachment: crate::domain::model::Attachment) {
        let Some(url) = attachment.full_url.clone() else {
            return;
        };
        let opening = crate::ui::message_view::video_opening_id(&attachment);
        if self
            .ctx
            .data(|data| data.get_temp::<bool>(opening))
            .is_some()
        {
            return;
        }
        self.ctx.data_mut(|data| data.insert_temp(opening, true));
        self.spawn(move |service| {
            let key = attachment.file_id.clone().unwrap_or_else(|| url.clone());
            let result = crate::infrastructure::downloads::video_path(&key, &attachment.name)
                .and_then(|path| {
                    if !path.exists() {
                        let bytes = service.download_file(&url)?;
                        crate::infrastructure::downloads::keep_video(&path, &bytes)?;
                    }
                    crate::infrastructure::downloads::open_with_default_app(&path)
                });
            UiEvent::VideoOpened(attachment, result)
        });
    }

    fn download(&mut self, attachment: crate::domain::model::Attachment) {
        let Some(url) = attachment.full_url.clone() else {
            return;
        };
        let name = attachment.name;
        self.spawn(move |service| {
            let result = service
                .download_file(&url)
                .and_then(|bytes| crate::infrastructure::downloads::save(&name, &bytes));
            UiEvent::Downloaded(name, result)
        });
    }

    fn copy_image(&mut self, attachment: crate::domain::model::Attachment) {
        let Some(url) = attachment.full_url.or(attachment.preview_url) else {
            return;
        };
        self.spawn(move |service| {
            UiEvent::ImageCopied(service.download(&url).and_then(|bytes| {
                let image = image::load_from_memory(&bytes)
                    .map_err(|error| SlackError(format!("Could not read the image: {error}")))?
                    .to_rgba8();
                Ok(egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    image.as_raw(),
                ))
            }))
        });
    }

    fn list_messages(&self, list: &ComposeTarget) -> &[DisplayMessage] {
        match &self.thread {
            Some(thread) if list.thread_ts.is_some() && thread.target == *list => &thread.messages,
            _ if list.thread_ts.is_none() && self.is_selected(&list.conversation_id) => {
                &self.messages
            }
            _ => &[],
        }
    }

    fn start_edit(&mut self, list: ComposeTarget, ts: &str) {
        let Some(message) = self
            .list_messages(&list)
            .iter()
            .find(|message| message.ts == ts)
        else {
            return;
        };
        let text = if message.text.is_empty() {
            // Rows cached before source text was stored; refreshed rows have it.
            crate::domain::message_text::plain_text(&message.blocks)
        } else {
            self.service.editable_text(&message.text).text
        };
        self.editing = Some((list, MessageEdit::new(ts.into(), text)));
    }

    /// Up in an empty composer edits your latest message, as in Slack.
    fn edit_latest(&mut self, list: &ComposeTarget) {
        let Some(self_id) = self.workspace.as_ref().map(|w| w.self_user_id.clone()) else {
            return;
        };
        if let Some(ts) = self
            .list_messages(list)
            .iter()
            .rev()
            .find(|message| message.author_key == self_id)
            .map(|message| message.ts.clone())
        {
            self.start_edit(list.clone(), &ts);
        }
    }

    fn save_edit(&mut self) {
        let Some((list, ts)) = self
            .editing
            .as_ref()
            .map(|(list, edit)| (list.clone(), edit.ts.clone()))
        else {
            return;
        };
        let Some(original) = self
            .list_messages(&list)
            .iter()
            .find(|message| message.ts == ts)
            .map(|message| message.text.clone())
        else {
            self.editing = None;
            return;
        };
        let Some((_, edit)) = &self.editing else {
            return;
        };
        let mentions = self.mention_tokens(&edit.text, edit.mentions.clone());
        let Some((_, edit)) = &mut self.editing else {
            return;
        };
        let editable = self
            .service
            .editable_text(&original)
            .with_mentions(&mentions);
        let text = editable.restore(&edit.text);
        if edit.text == editable.text || text == original {
            self.editing = None;
            self.composer_focus = Some(list);
            return;
        }
        edit.saving = true;
        edit.error = None;
        self.spawn(move |service| UiEvent::MessageEdited {
            result: service.edit_message(&list.conversation_id, &ts, &text),
            list,
            ts,
            text,
        });
    }

    fn finish_edit(
        &mut self,
        list: ComposeTarget,
        ts: String,
        text: String,
        result: Result<(), SlackError>,
    ) {
        let current = self
            .editing
            .as_mut()
            .filter(|(editing, edit)| *editing == list && edit.ts == ts);
        if let Err(error) = result {
            if let Some((_, edit)) = current {
                edit.saving = false;
                edit.error = Some(error.to_string());
            } else {
                notify(&format!("Could not edit the message: {error}"));
            }
            return;
        }
        if current.is_some() {
            self.editing = None;
            self.composer_focus = Some(list.clone());
        }
        // Show the edit now; the refresh below brings Slack's rendering.
        let service = Arc::clone(&self.service);
        let lists = [
            (list.thread_ts.is_none() && self.is_selected(&list.conversation_id))
                .then_some(&mut self.messages),
            self.thread
                .as_mut()
                .filter(|thread| thread.target.conversation_id == list.conversation_id)
                .map(|thread| &mut thread.messages),
        ];
        for messages in lists.into_iter().flatten() {
            if let Some(message) = messages.iter_mut().find(|message| message.ts == ts) {
                *message = service.edited_message(message, &text);
            }
        }
        self.refresh_conversation(&list.conversation_id);
    }

    fn save_settings(&mut self) {
        let current = crate::infrastructure::ui_settings::UiSettings {
            unread_only: self.unread_only,
            activity: self
                .activity
                .as_ref()
                .map_or(self.settings.activity, Activity::query),
            search_sort: self.search.sort(),
        };
        if current != self.settings {
            current.save();
            self.settings = current;
        }
    }

    /// Remember emoji the user picked, for the frequently used list.
    fn record_emoji(&mut self, names: impl IntoIterator<Item = String>) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut any = false;
        for name in names {
            self.local_emoji.record(&name, now);
            any = true;
        }
        if any {
            crate::infrastructure::emoji_usage_store::save(&self.local_emoji);
            self.refresh_frequent_emoji();
        }
    }

    fn refresh_frequent_emoji(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            let now = chrono::Utc::now().timestamp_millis();
            let ranked = workspace.emoji_usage.merged(&self.local_emoji).ranked(now);
            workspace.emoji.set_frequent(ranked);
        }
    }

    fn set_reaction(&mut self, conversation_id: String, ts: String, name: String, add: bool) {
        if add {
            self.record_emoji([name.clone()]);
        }
        if !self
            .pending_reactions
            .insert((conversation_id.clone(), ts.clone(), name.clone()))
        {
            return;
        }
        self.spawn(move |service| {
            let result = service.set_reaction(&conversation_id, &ts, &name, add);
            UiEvent::ReactionSet {
                conversation_id,
                ts,
                name,
                result,
            }
        });
    }

    fn load_messages(&mut self, target: ComposeTarget) {
        if !self.loading.insert(target.clone()) {
            self.reload.insert(target);
            return;
        }
        let read_cache = !self.history_cache.contains_key(&target);
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        self.spawn(move |service| {
            if read_cache
                && let Some(messages) =
                    service.cached_messages(&target.conversation_id, target.thread_ts.as_deref())
            {
                let _ = sender.send(UiEvent::MessagesCached(target.clone(), messages));
                ctx.request_repaint();
            }
            let result = match &target.thread_ts {
                Some(ts) => service.replies(&target.conversation_id, ts),
                None => service.messages_with_preview(&target.conversation_id, |messages| {
                    let _ = sender.send(UiEvent::MessagesPreview(target.clone(), messages));
                    ctx.request_repaint();
                }),
            };
            UiEvent::MessagesLoaded(target, result)
        });
    }

    fn refresh_conversation(&mut self, conversation_id: &str) {
        if self.is_selected(conversation_id) {
            self.load_messages(ComposeTarget {
                conversation_id: conversation_id.to_string(),
                thread_ts: None,
            });
        }
        if let Some(target) = self
            .thread
            .as_ref()
            .filter(|thread| thread.target.conversation_id == conversation_id)
            .map(|thread| thread.target.clone())
        {
            self.load_messages(target);
        }
    }

    fn mark_read(&mut self, conversation_id: String, ts: String) {
        let already_read = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.read_states.get(&conversation_id))
            .and_then(|state| state.last_read_ts.as_deref())
            .is_some_and(|read| ts_seconds(read) >= ts_seconds(&ts));
        if already_read
            || self.read_loading.contains(&conversation_id)
            || self
                .read_attempts
                .get(&conversation_id)
                .is_some_and(|attempt| ts_seconds(attempt) >= ts_seconds(&ts))
        {
            return;
        }
        self.read_loading.insert(conversation_id.clone());
        self.read_attempts
            .insert(conversation_id.clone(), ts.clone());
        self.spawn(move |service| {
            let result = service.mark_read(&conversation_id, &ts);
            UiEvent::Marked(conversation_id, ts, result)
        });
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                UiEvent::MessagesPreview(target, messages) => {
                    // Only fill an empty view. A refresh must not downgrade cached
                    // names/images while the enriched version is being prepared.
                    if !self.reload.contains(&target)
                        && self
                            .history_cache
                            .get(&target)
                            .is_none_or(|(_, items)| items.is_empty())
                    {
                        self.apply_messages(target, messages);
                    }
                }
                UiEvent::MessagesCached(target, messages) => {
                    if !self.reload.contains(&target) && !self.history_cache.contains_key(&target) {
                        self.apply_messages(target, messages);
                    }
                }
                UiEvent::WorkspaceLoaded(result) => {
                    self.workspace_loading = false;
                    match result {
                        Ok(workspace) => {
                            // Warm the three hover reactions before the first message hover.
                            for (name, _) in crate::ui::message_view::QUICK_REACTIONS {
                                if let Some(url) = workspace.emoji.image_url(name) {
                                    self.images.image(&url);
                                }
                            }
                            if self.todos.is_none() {
                                self.todos =
                                    Some(Todos::new(Arc::clone(&self.service), self.ctx.clone()));
                            }
                            if self.activity.is_none() {
                                self.activity = Some(Activity::new(
                                    Arc::clone(&self.service),
                                    self.ctx.clone(),
                                    self.settings.activity,
                                ));
                            }
                            self.workspace = Some(workspace);
                            self.refresh_frequent_emoji();
                            self.spawn(|service| UiEvent::PeopleLoaded(service.people()));
                            self.status = None;
                            if let Some(source) = self.realtime.take() {
                                let sender = self.sender.clone();
                                let ctx = self.ctx.clone();
                                spawn_live_updates(source, move |status| {
                                    let _ = sender.send(UiEvent::Live(status));
                                    ctx.request_repaint();
                                });
                            }
                        }
                        Err(error) => self.status = Some(error.to_string()),
                    }
                }
                UiEvent::MessagesLoaded(target, result) => {
                    self.loading.remove(&target);
                    // A mutation during this fetch invalidated its snapshot. Fetch again before replacing visible messages.
                    if self.reload.remove(&target) {
                        self.load_messages(target);
                        continue;
                    }
                    match result {
                        Ok(messages) => self.apply_messages(target, messages),
                        Err(error)
                            if target.thread_ts.is_none()
                                && self.is_selected(&target.conversation_id) =>
                        {
                            self.message_error = Some(error.to_string())
                        }
                        Err(error) => {
                            if let Some(thread) = self
                                .thread
                                .as_mut()
                                .filter(|thread| thread.target == target)
                            {
                                thread.error = Some(error.to_string());
                            }
                        }
                    }
                }
                UiEvent::MessageSent(request, result) => {
                    self.outbox.update_uploads(&request);
                    self.outbox.finish(&request.id, result);
                    self.refresh_conversation(&request.target.conversation_id);
                }
                UiEvent::MessageEdited {
                    list,
                    ts,
                    text,
                    result,
                } => self.finish_edit(list, ts, text, result),
                UiEvent::Deleted {
                    conversation_id,
                    removal,
                    result,
                } => self.finish_delete(conversation_id, removal, result),
                UiEvent::ImageCopied(result) => match result {
                    Ok(image) => {
                        self.ctx.copy_image(image);
                        notify("Image copied to the clipboard");
                    }
                    Err(error) => notify(&format!("Could not copy the image: {error}")),
                },
                UiEvent::ShareLinked {
                    destination,
                    comment,
                    link,
                } => match link {
                    Ok(link) => {
                        // Slack unfurls a message link as the shared-message card.
                        let text = if comment.is_empty() {
                            link
                        } else {
                            format!("{comment}\n{link}")
                        };
                        notify(&format!("Shared to {}", conversation_label(&destination)));
                        self.queue_message(
                            ComposeTarget {
                                conversation_id: destination.id,
                                thread_ts: None,
                            },
                            text,
                            Vec::new(),
                        );
                    }
                    Err(error) => {
                        notify(&format!("Could not share the message: {error}"));
                    }
                },
                UiEvent::Downloaded(name, result) => match result {
                    Ok(path) => crate::infrastructure::notifications::show(
                        APP_TITLE,
                        &format!("Saved {name} to {}", path.display()),
                        show_in_folder(path),
                    ),
                    Err(error) => notify(&format!("Could not download {name}: {error}")),
                },
                UiEvent::DraftsListed(result) => {
                    // A failure is retried by `save_drafts`; local drafts keep working.
                    if let Ok(remote) = result {
                        let texts = self.draft_texts();
                        for (key, text) in self.draft_sync.load(remote, &texts) {
                            self.set_draft_text(key, text);
                        }
                    }
                }
                UiEvent::DraftPushed(push, result) => {
                    let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
                    let (key, text) = match &push {
                        crate::application::draft_sync::Push::Save { key, text, .. } => {
                            (key, text.as_str())
                        }
                        crate::application::draft_sync::Push::Delete { key, .. } => (key, ""),
                    };
                    self.draft_sync
                        .pushed(key, text, result.map_err(|_| ()), now);
                }
                UiEvent::PeopleLoaded(result) => {
                    if let (Ok(people), Some(workspace)) = (result, &mut self.workspace) {
                        workspace.people = Arc::new(people);
                    }
                }
                UiEvent::EmojiLoaded(result) => {
                    if let (Ok(emoji), Some(workspace)) = (result, &mut self.workspace) {
                        workspace.emoji = emoji;
                        self.refresh_frequent_emoji();
                    }
                }
                UiEvent::VideoOpened(attachment, result) => {
                    let opening = crate::ui::message_view::video_opening_id(&attachment);
                    self.ctx.data_mut(|data| data.remove_temp::<bool>(opening));
                    if let Err(error) = result {
                        notify(&format!("Could not play {}: {error}", attachment.name));
                    }
                }
                UiEvent::OpenFromNotification {
                    conversation_id,
                    thread_ts,
                } => self.open_from_notification(&conversation_id, thread_ts),
                UiEvent::ReactionSet {
                    conversation_id,
                    ts,
                    name,
                    result,
                } => {
                    self.pending_reactions
                        .remove(&(conversation_id.clone(), ts, name));
                    if let Err(error) = result {
                        notify(&error.to_string());
                    }
                    self.refresh_conversation(&conversation_id);
                }
                UiEvent::Marked(conversation_id, ts, result) => {
                    self.read_loading.remove(&conversation_id);
                    match result {
                        Ok(()) => {
                            if let Some(workspace) = &mut self.workspace {
                                record_read(
                                    workspace.read_states.entry(conversation_id).or_default(),
                                    &ts,
                                );
                            }
                            self.revision += 1;
                            self.rebuild_sidebar();
                        }
                        Err(error) => notify(&format!("Could not mark conversation read: {error}")),
                    }
                }
                UiEvent::Counts(revision, result) => {
                    self.counts_loading = false;
                    if revision == self.revision
                        && let (Some(workspace), Ok(states)) = (&mut self.workspace, result)
                    {
                        workspace.read_states = states;
                        self.rebuild_sidebar();
                    }
                }
                UiEvent::Live(status) => self.live_event(status),
                UiEvent::HuddleNamed {
                    conversation_id,
                    participants,
                    names,
                } => self.huddles.named(&conversation_id, &participants, names),
                UiEvent::CallerFound(profile) => {
                    self.huddles
                        .caller_found(&profile.id, profile.name, profile.avatar_url);
                }
                UiEvent::CallChosen(conversation_id, choice) => {
                    if self
                        .huddles
                        .ringing()
                        .is_some_and(|ringing| ringing.conversation_id == conversation_id)
                    {
                        self.call_choice(choice);
                    }
                }
                UiEvent::HuddleJoined(conversation_id, result) => {
                    self.huddle_joined(conversation_id, result)
                }
                UiEvent::CallUpdated(call_id, update) => self.call_updated(&call_id, update),
            }
        }
    }

    fn apply_messages(&mut self, target: ComposeTarget, messages: Vec<DisplayMessage>) {
        self.outbox.reconcile(&target, &messages);
        for huddle in messages.iter().filter_map(|message| message.huddle.clone()) {
            self.huddle_changed(huddle, true);
        }
        self.history_cache
            .insert(target.clone(), (Instant::now(), messages.clone()));
        if self.history_cache.len() > 32
            && let Some(oldest) = self
                .history_cache
                .iter()
                .min_by_key(|(_, (time, _))| *time)
                .map(|(target, _)| target.clone())
        {
            self.history_cache.remove(&oldest);
        }
        if target.thread_ts.is_none() && self.is_selected(&target.conversation_id) {
            if self.messages != messages {
                self.messages = messages;
            }
            self.message_error = None;
        } else if let Some(thread) = self
            .thread
            .as_mut()
            .filter(|thread| thread.target == target)
        {
            if thread.messages != messages {
                thread.messages = messages;
            }
            thread.error = None;
        }
    }

    fn live_event(&mut self, status: LiveStatus) {
        let event = match status {
            LiveStatus::Event(event) => *event,
            LiveStatus::Disconnected(error) => {
                self.live_connected = false;
                // Polling continues while the existing realtime source reconnects.
                let _ = error;
                return;
            }
        };
        self.revision += 1;
        if let Some(activity) = &mut self.activity {
            activity.invalidate();
        }
        match event {
            LiveEvent::Connected => {
                self.live_connected = true;
                self.last_counts = Instant::now() - self.counts_interval();
                self.last_poll = Instant::now() - self.poll_interval();
                // Catch draft changes made elsewhere while disconnected.
                if self.draft_sync.is_loaded() {
                    self.list_drafts();
                }
            }
            LiveEvent::MessagePosted {
                conversation_id,
                message,
                is_broadcast,
            } => {
                self.notify_message(&conversation_id, &message);
                if !message.is_thread_reply() || is_broadcast {
                    if let Some(workspace) = &mut self.workspace {
                        let direct = workspace
                            .conversations
                            .iter()
                            .find(|c| c.id == conversation_id)
                            .is_some_and(Conversation::is_direct);
                        record_incoming(
                            workspace
                                .read_states
                                .entry(conversation_id.clone())
                                .or_default(),
                            &message,
                            direct,
                            &workspace.self_user_id,
                        );
                    }
                    self.rebuild_sidebar();
                }
                self.refresh_conversation(&conversation_id);
            }
            LiveEvent::MessageChanged {
                conversation_id, ..
            }
            | LiveEvent::MessageDeleted {
                conversation_id, ..
            }
            | LiveEvent::ReactionChanged {
                conversation_id, ..
            } => self.refresh_conversation(&conversation_id),
            LiveEvent::ConversationMarked {
                conversation_id,
                ts,
            } => {
                if let Some(workspace) = &mut self.workspace {
                    record_read(
                        workspace.read_states.entry(conversation_id).or_default(),
                        &ts,
                    );
                }
                self.rebuild_sidebar();
            }
            LiveEvent::DraftSaved(draft) => {
                let key = (draft.conversation_id.clone(), draft.thread_ts.clone());
                let local = self.draft_texts().remove(&key).unwrap_or_default();
                if let Some(text) = self.draft_sync.remote_saved(draft, &local) {
                    self.set_draft_text(key, text);
                }
            }
            LiveEvent::DraftDeleted { id } => {
                if let Some(key) = self.draft_sync.remote_deleted(&id, &self.draft_texts()) {
                    self.set_draft_text(key, String::new());
                }
            }
            LiveEvent::EmojiChanged => {
                self.spawn(|service| UiEvent::EmojiLoaded(service.emoji_catalog()));
            }
            LiveEvent::HuddleChanged(huddle) => self.huddle_changed(huddle, false),
            LiveEvent::HuddleRinging {
                conversation_id,
                caller_id,
                call_id,
            } => self.ring(conversation_id, caller_id, call_id),
            LiveEvent::HuddleRingStopped { conversation_id } => {
                self.huddles.stop_ringing_in(Some(&conversation_id));
            }
        }
    }

    /// Draft texts worth keeping, keyed like the drafts file.
    fn draft_texts(&self) -> crate::infrastructure::draft_store::Drafts {
        self.drafts
            .iter()
            .filter(|(_, draft)| !draft.text.trim().is_empty())
            .map(|(target, draft)| {
                (
                    (target.conversation_id.clone(), target.thread_ts.clone()),
                    draft.text.clone(),
                )
            })
            .collect()
    }

    /// About once a second: send paused edits to Slack and keep the drafts
    /// file current.
    fn save_drafts(&mut self) {
        if self.drafts_checked.elapsed() < DRAFT_SAVE_INTERVAL {
            return;
        }
        self.drafts_checked = Instant::now();
        let texts = self.draft_texts();
        let now = chrono::Utc::now().timestamp_millis() as f64 / 1000.0;
        if self.workspace.is_some()
            && !self.draft_sync.is_loaded()
            && self
                .drafts_requested
                .is_none_or(|at| at.elapsed() >= DRAFT_LIST_RETRY)
        {
            self.list_drafts();
        }
        self.draft_sync.observe(&texts, now);
        for push in self.draft_sync.due(&texts, now) {
            self.spawn(move |service| {
                let result = service.push_draft(&push);
                UiEvent::DraftPushed(push, result)
            });
        }
        let snapshot = (texts, self.draft_sync.synced().clone());
        // A failed write is retried on the next change.
        if snapshot != self.saved_drafts && self.draft_store.save(&snapshot.0, &snapshot.1).is_ok()
        {
            self.saved_drafts = snapshot;
        }
    }

    fn list_drafts(&mut self) {
        self.drafts_requested = Some(Instant::now());
        self.spawn(|service| UiEvent::DraftsListed(service.drafts()));
    }

    fn restore_drafts(&mut self) {
        let (texts, synced) = self.draft_store.load();
        for (key, text) in texts {
            self.set_draft_text(key, text);
        }
        self.draft_sync = crate::application::draft_sync::DraftSync::new(synced);
        self.saved_drafts = (self.draft_texts(), self.draft_sync.synced().clone());
    }

    fn set_draft_text(
        &mut self,
        (conversation_id, thread_ts): (String, Option<String>),
        text: String,
    ) {
        self.drafts
            .entry(ComposeTarget {
                conversation_id,
                thread_ts,
            })
            .or_default()
            .text = text;
    }

    fn rebuild_sidebar(&mut self) {
        if let Some(workspace) = &mut self.workspace {
            workspace.sidebar = build_sidebar(&SidebarInput {
                sections: &workspace.sections,
                conversations: &workspace.conversations,
                read_states: &workspace.read_states,
                direct_cutoff_seconds: chrono::Utc::now().timestamp() as f64 - 7.0 * 86400.0,
            });
        }
    }

    fn poll(&mut self) {
        if self.last_poll.elapsed() >= self.poll_interval() {
            if let Some(conversation_id) = self
                .selected
                .as_ref()
                .filter(|_| self.conversation_shown())
                .map(|conversation| conversation.id.clone())
            {
                // Periodic polls do not invalidate a still-running request.
                let target = ComposeTarget {
                    conversation_id,
                    thread_ts: None,
                };
                if !self.loading.contains(&target) {
                    self.load_messages(target);
                }
            }
            if !self.todos_open
                && let Some(target) = self.thread.as_ref().map(|thread| thread.target.clone())
                && !self.loading.contains(&target)
            {
                self.load_messages(target);
            }
            self.last_poll = Instant::now();
        }
        if self.workspace.is_some()
            && !self.counts_loading
            && self.last_counts.elapsed() >= self.counts_interval()
        {
            self.counts_loading = true;
            self.last_counts = Instant::now();
            let revision = self.revision;
            self.spawn(move |service| UiEvent::Counts(revision, service.read_states()));
        }
    }

    fn poll_interval(&self) -> Duration {
        if self.live_connected {
            Duration::from_secs(60)
        } else {
            POLL_INTERVAL
        }
    }

    fn counts_interval(&self) -> Duration {
        if self.live_connected {
            Duration::from_secs(120)
        } else {
            COUNTS_INTERVAL
        }
    }

    fn is_selected(&self, conversation_id: &str) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|conversation| conversation.id == conversation_id)
    }

    /// Show a desktop notification for a new direct message or mention, unless
    /// it is already on screen in the focused window.
    fn notify_message(&mut self, conversation_id: &str, message: &crate::domain::model::Message) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        // Calls ring instead; "a huddle started" alone is no news.
        if message.huddle.is_some() {
            return;
        }
        let conversation = workspace
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .cloned();
        let direct = conversation.as_ref().is_some_and(Conversation::is_direct)
            || conversation_id.starts_with('D');
        if !should_notify(
            message,
            direct,
            workspace.muted.contains(conversation_id),
            &workspace.self_user_id,
        ) || ts_seconds(&message.ts) < self.started
        {
            return;
        }
        let root = message
            .is_thread_reply()
            .then(|| message.thread_ts.clone())
            .flatten();
        if !self.should_announce(conversation_id, root.as_ref(), &message.ts) {
            return;
        }
        let place = conversation
            .filter(|conversation| !conversation.is_direct())
            .map(|conversation| conversation_label(&conversation));
        let message = message.clone();
        let conversation_id = conversation_id.to_owned();
        let open = self.notification_opener(conversation_id, root.clone());
        let service = Arc::clone(&self.service);
        thread::spawn(move || {
            let display = service.display(&message);
            let mut body = crate::domain::message_text::plain_text(&display.blocks);
            if body.trim().is_empty() {
                body = match (display.shares.first(), display.attachments.len()) {
                    (Some(share), _) => crate::domain::message_text::plain_text(&share.blocks),
                    (None, 0) => String::new(),
                    (None, 1) => format!("Sent {}", display.attachments[0].name),
                    (None, files) => format!("Sent {files} files"),
                };
            }
            let mut title = match place {
                Some(place) => format!("{} in {place}", display.author),
                None => display.author.clone(),
            };
            if root.is_some() {
                title.push_str(" · thread");
            }
            crate::infrastructure::notifications::show(&title, &body, Some(open));
        });
    }

    /// Notify about a mention or direct message Slack's activity feed reports;
    /// this also covers group mentions and anything the live connection missed.
    fn notify_activity(&mut self, row: crate::application::activity::DisplayActivity) {
        let item = &row.item;
        let broadcast = matches!(item.kind.as_str(), "at_channel" | "at_everyone");
        if self
            .workspace
            .as_ref()
            .is_none_or(|workspace| broadcast && workspace.muted.contains(&item.conversation_id))
        {
            return;
        }
        let root = item.thread_ts.clone().filter(|root| *root != item.ts);
        if !self.should_announce(&item.conversation_id, root.as_ref(), &item.ts) {
            return;
        }
        let mut title = if item.conversation_id.starts_with('D') {
            row.actor.clone()
        } else {
            format!("{} in {}", row.actor, row.source)
        };
        if root.is_some() {
            title.push_str(" · thread");
        }
        let body = row
            .message
            .as_ref()
            .map(|message| crate::domain::message_text::plain_text(&message.blocks))
            .filter(|body| !body.trim().is_empty())
            .unwrap_or_else(|| row.preview.clone());
        let open = self.notification_opener(item.conversation_id.clone(), root);
        thread::spawn(move || {
            crate::infrastructure::notifications::show(&title, &body, Some(open))
        });
    }

    /// False when the message is already on screen in the focused window, or
    /// was announced before; otherwise remembers it as announced.
    fn should_announce(&mut self, conversation_id: &str, root: Option<&String>, ts: &str) -> bool {
        let thread_open = self.thread.as_ref().is_some_and(|thread| {
            thread.target.conversation_id == conversation_id
                && thread.target.thread_ts.as_deref() == Some(root.map_or(ts, String::as_str))
        });
        let timeline_open =
            root.is_none() && self.is_selected(conversation_id) && self.conversation_shown();
        !(self.ctx.input(|input| input.focused) && (thread_open || timeline_open))
            && self.notified.insert(format!("{conversation_id}:{ts}"))
    }

    fn notification_opener(
        &self,
        conversation_id: String,
        thread_ts: Option<String>,
    ) -> Box<dyn FnOnce() + Send> {
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        Box::new(move || {
            let _ = sender.send(UiEvent::OpenFromNotification {
                conversation_id,
                thread_ts,
            });
            ctx.request_repaint();
        })
    }

    fn open_from_notification(&mut self, conversation_id: &str, thread_ts: Option<String>) {
        let Some(conversation) = self.workspace.as_ref().and_then(|workspace| {
            workspace
                .conversations
                .iter()
                .find(|conversation| conversation.id == conversation_id)
                .cloned()
        }) else {
            return;
        };
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.switcher = None;
        self.picker = None;
        self.share = None;
        self.deleting = None;
        self.image_viewer = None;
        self.select(conversation);
        match thread_ts {
            Some(root) => {
                self.message_action(target_of(conversation_id), MessageAction::OpenThread(root))
            }
            None => {
                self.thread = None;
                self.composer_focus = Some(target_of(conversation_id));
            }
        }
    }

    #[cfg(test)]
    fn search_results(&self) -> &[crate::application::message_search::SearchResult] {
        self.search.results()
    }

    /// Slack's latest word on a huddle, live or from history.
    fn huddle_changed(&mut self, huddle: crate::domain::model::Huddle, from_history: bool) {
        let self_user_id = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.self_user_id.clone())
            .unwrap_or_default();
        let (conversation_id, participants) =
            (huddle.conversation_id.clone(), huddle.participants.clone());
        let lookup = if from_history {
            self.huddles.seen(&huddle, &self_user_id)
        } else {
            self.huddles.update(huddle, &self_user_id)
        };
        if lookup {
            self.spawn(move |service| UiEvent::HuddleNamed {
                names: service.user_names(&participants),
                conversation_id,
                participants,
            });
        }
    }

    /// Someone is calling: ring here, and offer Answer in a notification.
    fn ring(&mut self, conversation_id: String, caller_id: String, call_id: String) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        if caller_id == workspace.self_user_id {
            return;
        }
        self.huddles
            .ring(conversation_id.clone(), caller_id.clone(), call_id);
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Critical,
            ));
        let (sender, ctx) = (self.sender.clone(), self.ctx.clone());
        self.spawn(move |service| {
            let caller = service.user(&caller_id);
            let choice = |choice| -> Box<dyn FnOnce() + Send> {
                let (sender, ctx, conversation_id) =
                    (sender.clone(), ctx.clone(), conversation_id.clone());
                Box::new(move || {
                    let _ = sender.send(UiEvent::CallChosen(conversation_id, choice));
                    ctx.request_repaint();
                })
            };
            crate::infrastructure::notifications::show_call(
                &caller.name,
                "is calling you in a huddle",
                choice(CallChoice::Answer),
                choice(CallChoice::Decline),
            );
            UiEvent::CallerFound(caller)
        });
    }

    fn call_choice(&mut self, choice: CallChoice) {
        let Some(ringing) = self.huddles.stop_ringing_in(None) else {
            return;
        };
        match choice {
            CallChoice::Answer => self.join_huddle(&ringing.conversation_id),
            CallChoice::Decline => {
                let (service, conversation_id, call_id) = (
                    Arc::clone(&self.service),
                    ringing.conversation_id,
                    ringing.call_id,
                );
                thread::spawn(move || {
                    let _ = service.decline_huddle(&conversation_id, &call_id);
                });
            }
        }
    }

    /// Join the huddle going on in a conversation, or start one, leaving any
    /// other huddle.
    fn join_huddle(&mut self, conversation_id: &str) {
        if self.huddles.in_call(conversation_id) {
            return;
        }
        self.huddles.stop_ringing_in(Some(conversation_id));
        let title = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .conversations
                    .iter()
                    .find(|conversation| conversation.id == conversation_id)
            })
            .map_or_else(|| String::from("Huddle"), conversation_label);
        if let Some(previous) = self.huddles.joining(conversation_id.to_owned(), title) {
            self.left_huddle(previous);
        }
        let conversation_id = conversation_id.to_owned();
        self.spawn(move |service| {
            let result = service.join_huddle(&conversation_id);
            UiEvent::HuddleJoined(conversation_id, result)
        });
    }

    fn huddle_joined(
        &mut self,
        conversation_id: String,
        result: Result<crate::domain::model::ChimeJoin, SlackError>,
    ) {
        let join = match result {
            Ok(join) => join,
            Err(error) => {
                if self.huddles.call().is_some_and(|call| {
                    call.join.is_none() && call.conversation_id == conversation_id
                }) {
                    self.huddles.leave();
                }
                self.status = Some(format!("Could not join the huddle: {error}"));
                return;
            }
        };
        if !self.huddles.joined(&join) {
            // Left, or went to another huddle, while Slack answered.
            let service = Arc::clone(&self.service);
            thread::spawn(move || {
                let _ = service.leave_huddle(&join);
            });
            return;
        }
        let (sender, ctx, call_id) = (self.sender.clone(), self.ctx.clone(), join.call_id.clone());
        let call = crate::infrastructure::huddle_call::Call::start(join, false, move |update| {
            let _ = sender.send(UiEvent::CallUpdated(call_id.clone(), update));
            ctx.request_repaint();
        });
        self.huddles.started(call);
    }

    fn call_updated(
        &mut self,
        call_id: &str,
        update: crate::infrastructure::huddle_call::CallUpdate,
    ) {
        let error = match &update {
            crate::infrastructure::huddle_call::CallUpdate::Ended(error) => error.clone(),
            _ => None,
        };
        if let Some(ended) = self.huddles.call_update(call_id, update) {
            self.left_huddle(ended);
            if let Some(error) = error {
                self.status = Some(error);
            }
        }
    }

    fn call_action(&mut self, action: crate::ui::huddles::CallAction) {
        match action {
            crate::ui::huddles::CallAction::ToggleMute => self.huddles.toggle_mute(),
            crate::ui::huddles::CallAction::Leave => {
                if let Some(call) = self.huddles.leave() {
                    self.left_huddle(call);
                }
            }
        }
    }

    /// Tell Slack the user left a huddle.
    fn left_huddle(&mut self, call: crate::ui::huddles::ActiveCall) {
        if let Some(join) = call.join {
            let service = Arc::clone(&self.service);
            thread::spawn(move || {
                let _ = service.leave_huddle(&join);
            });
        }
    }

    /// Whether the selected conversation's timeline is on screen.
    fn conversation_shown(&self) -> bool {
        !self.todos_open && !self.activity_open && !self.search_open
    }

    fn open_search(&mut self, query: Option<String>) {
        if !self.search_open {
            self.thread = None;
        }
        self.search_open = true;
        self.activity_open = false;
        self.todos_open = false;
        self.composer_focus = None;
        self.search.focus(query);
    }

    /// Search everywhere, or within the open channel as Slack does.
    fn search_shortcuts(&mut self, ctx: &egui::Context) {
        let (everywhere, here) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::G),
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
            )
        });
        if here {
            let channel = self
                .selected
                .as_ref()
                .filter(|conversation| self.conversation_shown() && !conversation.is_direct())
                .map(|conversation| format!("in:#{} ", conversation.name));
            self.open_search(channel);
        } else if everywhere {
            self.open_search(None);
        }
    }

    fn spawn(&self, job: impl FnOnce(&ChatService) -> UiEvent + Send + 'static) {
        let service = Arc::clone(&self.service);
        let sender = self.sender.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(job(&service));
            ctx.request_repaint();
        });
    }
}

/// People to suggest first for `@`: those the user has DMs with.
fn preferred_people(workspace: &Workspace) -> Vec<String> {
    workspace
        .conversations
        .iter()
        .filter_map(|conversation| match &conversation.kind {
            crate::domain::model::ConversationKind::DirectMessage { user_id } => {
                Some(user_id.clone())
            }
            _ => None,
        })
        .collect()
}

fn target_of(conversation_id: &str) -> ComposeTarget {
    ComposeTarget {
        conversation_id: conversation_id.into(),
        thread_ts: None,
    }
}

const APP_TITLE: &str = "Better Slack";

/// An app status, such as a finished download, shown as a desktop notification.
fn notify(text: &str) {
    crate::infrastructure::notifications::show(APP_TITLE, text, None);
}

/// Clicking the "Saved …" notification opens the folder (Linux only).
fn show_in_folder(path: std::path::PathBuf) -> Option<Box<dyn FnOnce() + Send>> {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return None;
    }
    let folder = path.parent()?.to_owned();
    Some(Box::new(move || {
        let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
    }))
}

/// Somewhere Back and Forward go: a conversation, with a thread open beside it.
#[derive(Clone, Debug)]
struct Place {
    conversation: Conversation,
    thread: Option<String>,
}

impl PartialEq for Place {
    fn eq(&self, other: &Self) -> bool {
        self.conversation.id == other.conversation.id && self.thread == other.thread
    }
}

fn push_history(stack: &mut Vec<Place>, place: Place) {
    if stack.last() == Some(&place) {
        return;
    }
    if stack.len() == HISTORY_LIMIT {
        stack.remove(0);
    }
    stack.push(place);
}

const HISTORY_LIMIT: usize = 50;
// The bundled fonts have no "←"/"→" glyphs.
const BACK_ARROW: &str = "‹";
const FORWARD_ARROW: &str = "›";
#[cfg(target_os = "macos")]
const BACK_SHORTCUT: (egui::Modifiers, egui::Key) =
    (egui::Modifiers::COMMAND, egui::Key::OpenBracket);
#[cfg(target_os = "macos")]
const FORWARD_SHORTCUT: (egui::Modifiers, egui::Key) =
    (egui::Modifiers::COMMAND, egui::Key::CloseBracket);
#[cfg(target_os = "macos")]
const BACK_LABEL: &str = "⌘[";
#[cfg(target_os = "macos")]
const FORWARD_LABEL: &str = "⌘]";
#[cfg(not(target_os = "macos"))]
const BACK_SHORTCUT: (egui::Modifiers, egui::Key) = (egui::Modifiers::ALT, egui::Key::ArrowLeft);
#[cfg(not(target_os = "macos"))]
const FORWARD_SHORTCUT: (egui::Modifiers, egui::Key) =
    (egui::Modifiers::ALT, egui::Key::ArrowRight);
#[cfg(not(target_os = "macos"))]
const BACK_LABEL: &str = "Alt+Left";
#[cfg(not(target_os = "macos"))]
const FORWARD_LABEL: &str = "Alt+Right";
#[cfg(target_os = "macos")]
const SWITCHER_SHORTCUT: &str = "⌘P";
#[cfg(not(target_os = "macos"))]
const SWITCHER_SHORTCUT: &str = "Ctrl+P";
#[cfg(target_os = "macos")]
const SEARCH_SHORTCUT: &str = "⌘G";
#[cfg(not(target_os = "macos"))]
const SEARCH_SHORTCUT: &str = "Ctrl+G";
#[cfg(target_os = "macos")]
const UNREAD_SHORTCUT: &str = "⌘⇧U";
#[cfg(not(target_os = "macos"))]
const UNREAD_SHORTCUT: &str = "Ctrl+Shift+U";
const POLL_INTERVAL: Duration = Duration::from_secs(10);
const DRAFT_SAVE_INTERVAL: Duration = Duration::from_secs(1);
const DRAFT_LIST_RETRY: Duration = Duration::from_secs(60);
const COUNTS_INTERVAL: Duration = Duration::from_secs(30);

fn needs_attention(workspace: &Workspace, id: &str) -> bool {
    workspace.read_states.get(id).is_some_and(|state| {
        state.mention_count > 0 || (state.has_unreads && !workspace.muted.contains(id))
    })
}

/// Muted channels are hidden like Slack's "Mute and hide", except while open
/// or when they mention you. They stay reachable through the quick switcher.
fn shown_in_sidebar(
    workspace: &Workspace,
    conversation: &Conversation,
    open: Option<&str>,
) -> bool {
    !workspace.muted.contains(&conversation.id)
        || open == Some(conversation.id.as_str())
        || workspace
            .read_states
            .get(&conversation.id)
            .is_some_and(|state| state.mention_count > 0)
}

fn sidebar_row(
    ui: &mut egui::Ui,
    workspace: &Workspace,
    conversation: &Conversation,
    selected: Option<&str>,
    draft: bool,
    huddle: bool,
) -> bool {
    ui.push_id(&conversation.id, |ui| {
        let size = egui::vec2(ui.available_width(), 26.0);
        if !ui.is_rect_visible(egui::Rect::from_min_size(ui.next_widget_position(), size)) {
            ui.allocate_exact_size(size, egui::Sense::hover());
            return false;
        }
        let unread = needs_attention(workspace, &conversation.id);
        let muted = workspace.muted.contains(&conversation.id);
        let mentions = workspace
            .read_states
            .get(&conversation.id)
            .map_or(0, |state| state.mention_count);
        let color = if unread {
            theme::TEXT_STRONG
        } else if muted {
            theme::TEXT_MUTED
        } else {
            theme::TEXT
        };
        let mut text = egui::RichText::new(&conversation.name).color(color);
        if unread {
            text = text.font(egui::FontId::new(13.5, theme::bold_family()));
        }
        ui.horizontal(|ui| {
            let width = (ui.available_width() - if mentions > 0 { 36.0 } else { 0.0 }).max(1.0);
            let clicked = ui
                .add_sized([width, 26.0], |ui: &mut egui::Ui| {
                    let dot_id = ui.make_persistent_id("unread_dot");
                    let icon_id = ui.make_persistent_id("conversation_icon");
                    let pencil_id = ui.make_persistent_id("draft_pencil");
                    let pencil_size = if draft { 14.0 } else { 0.0 };
                    let huddle_id = ui.make_persistent_id("huddle_live");
                    let huddle_size = if huddle { 16.0 } else { 0.0 };
                    let output = egui::Button::selectable(
                        selected == Some(&conversation.id),
                        (
                            egui::Atom::custom(dot_id, egui::vec2(6.0, 18.0)),
                            egui::Atom::custom(icon_id, egui::vec2(18.0, 18.0)),
                            text.atom_shrink(true),
                            egui::Atom::grow(),
                            egui::Atom::custom(huddle_id, egui::vec2(huddle_size, 18.0)),
                            egui::Atom::custom(pencil_id, egui::vec2(pencil_size, 18.0)),
                        ),
                    )
                    .gap(6.0)
                    .truncate()
                    .atom_ui(ui);
                    if unread && let Some(rect) = output.rect(dot_id) {
                        ui.painter()
                            .circle_filled(rect.center(), 3.0, theme::UNREAD_DOT);
                    }
                    if let Some(rect) = output.rect(icon_id) {
                        paint_sidebar_icon(ui, rect, conversation, color);
                    }
                    if huddle && let Some(rect) = output.rect(huddle_id) {
                        crate::ui::huddles::paint_headphones(ui, rect, HUDDLE_LIVE);
                    }
                    if draft && let Some(rect) = output.rect(pencil_id) {
                        paint_pencil(ui, rect, theme::TEXT_MUTED);
                    }
                    output.response
                })
                .on_hover_text(format!(
                    "{}{}{}{}",
                    conversation.name,
                    if muted { " · muted" } else { "" },
                    if huddle { " · huddle happening" } else { "" },
                    if draft { " · draft" } else { "" },
                ))
                .clicked();
            if mentions > 0 {
                let label = if mentions > 99 {
                    "99+".into()
                } else {
                    mentions.to_string()
                };
                egui::Frame::new()
                    .fill(theme::BADGE)
                    .corner_radius(9)
                    .inner_margin(egui::Margin::symmetric(5, 1))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .small()
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                    });
            }
            clicked
        })
        .inner
    })
    .inner
}

/// The headphones beside a conversation with a huddle going on.
const HUDDLE_LIVE: egui::Color32 = egui::Color32::from_rgb(46, 182, 125);

/// A funnel, for the unread filter.
fn paint_filter(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let (left, right, top) = (rect.left() + 0.5, rect.right() - 0.5, rect.top() + 1.5);
    let neck = rect.center().y + 1.0;
    let center = rect.center().x;
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(left, top),
            egui::pos2(right, top),
            egui::pos2(center + 1.6, neck),
            egui::pos2(center - 1.6, neck),
        ],
        color,
        egui::Stroke::NONE,
    ));
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(center - 1.6, neck),
            egui::pos2(center + 1.6, rect.bottom() - 0.5),
        ),
        0.0,
        color,
    );
}

/// A small slanted pencil, for conversations with a draft.
fn paint_pencil(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let along = egui::vec2(0.7, -0.7);
    let across = egui::vec2(0.7, 0.7);
    let tip = center - along * 5.5;
    let body_start = tip + along * 2.5;
    let end = center + along * 5.0;
    let painter = ui.painter();
    painter.add(egui::Shape::convex_polygon(
        vec![tip, body_start + across * 1.6, body_start - across * 1.6],
        color,
        egui::Stroke::NONE,
    ));
    painter.line_segment([body_start, end], egui::Stroke::new(3.2, color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::demo::DemoGateway;

    fn app() -> ChatApp {
        let mut app = ChatApp::new(
            ChatService::new(Arc::new(DemoGateway::new())),
            "Test".into(),
            None,
            egui::Context::default(),
        );
        let event = app.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        app.sender.send(event).unwrap();
        app.drain_events();
        assert!(app.workspace.is_some());
        app
    }

    fn target(id: &str) -> ComposeTarget {
        ComposeTarget {
            conversation_id: id.into(),
            thread_ts: None,
        }
    }

    #[test]
    fn direct_messages_notify_unless_already_on_screen() {
        let mut app = app();
        let ctx = app.ctx.clone();
        let message = |ts: &str, text: &str| crate::domain::model::Message {
            client_msg_id: None,
            ts: ts.into(),
            author: crate::domain::model::Author::User("U2".into()),
            text: text.into(),
            thread_ts: None,
            reply_count: 0,
            reactions: vec![],
            attachments: vec![],
            shares: vec![],
            edited: false,
            huddle: None,
        };
        let now = chrono::Utc::now().timestamp() + 1;
        let focus = |focused| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    focused,
                    ..Default::default()
                },
                |_| {},
            );
            output.textures_delta.clear();
        };
        focus(true);
        app.selected = Some(conversation(&app, "D1"));
        app.notify_message("D1", &message(&format!("{now}.1"), "hi"));
        assert!(app.notified.is_empty(), "the open DM is already on screen");
        app.notify_message("C1", &message(&format!("{now}.2"), "chatter"));
        assert!(app.notified.is_empty(), "plain channel messages stay quiet");
        app.notify_message("C1", &message(&format!("{now}.3"), "<!here> deploy"));
        app.notify_message("C1", &message(&format!("{now}.3"), "<!here> deploy"));
        assert_eq!(app.notified.len(), 1, "mentions notify once");
        app.notify_message("D1", &message("1.0", "old replay"));
        assert_eq!(
            app.notified.len(),
            1,
            "messages from before startup stay quiet"
        );
        focus(false);
        app.notify_message("D1", &message(&format!("{now}.4"), "hi"));
        assert_eq!(app.notified.len(), 2, "an unfocused window notifies");
    }

    #[test]
    fn sharing_a_message_posts_its_link_with_a_comment() {
        let mut app = app();
        let ctx = app.ctx.clone();
        let (send_requests, requests) = channel();
        app.send_requests = send_requests;
        let ts = app.service.messages("C1").unwrap()[0].ts.clone();
        app.message_action(target("C1"), MessageAction::Share(ts.clone()));
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![],
            vec![egui::Event::Text("design".into())],
            vec![enter(true), enter(false)],
            vec![],
            vec![egui::Event::Text("look".into())],
            vec![enter(true)],
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused: true,
                    ..Default::default()
                },
                |ui| app.share_dialog(ui.ctx()),
            );
            output.textures_delta.clear();
        }
        assert!(app.share.is_none(), "sharing closes the dialog");
        let deadline = Instant::now() + Duration::from_secs(5);
        let request = loop {
            let event = app.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            app.sender.send(event).unwrap();
            app.drain_events();
            match requests.try_recv() {
                Ok(request) => break request,
                Err(_) if Instant::now() < deadline => {}
                Err(error) => panic!("the share is sent: {error}"),
            }
        };
        assert_eq!(request.target, target("C2"));
        assert_eq!(
            request.text,
            format!(
                "look\nhttps://demo.slack.com/archives/C1/p{}",
                ts.replace('.', "")
            )
        );
    }

    #[test]
    fn up_in_an_empty_composer_edits_the_latest_own_message() {
        let mut app = app();
        let ctx = app.ctx.clone();
        let target = target("C1");
        app.selected = Some(conversation(&app, "C1"));
        app.service
            .send("C1", "hello & bye", None, "edit-test")
            .unwrap();
        app.messages = app.service.messages("C1").unwrap();
        ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
        let up = egui::Event::Key {
            key: egui::Key::ArrowUp,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![up],
                focused: true,
                ..Default::default()
            },
            |ui| app.compose(ui, &target, "Message"),
        );
        output.textures_delta.clear();
        let (list, edit) = app.editing.as_mut().expect("editing the latest message");
        assert_eq!(*list, target);
        assert_eq!(edit.text, "hello & bye");
        edit.text = "hello & goodbye".into();
        app.message_action(target.clone(), MessageAction::SaveEdit);
        assert!(app.editing.as_ref().is_some_and(|(_, edit)| edit.saving));
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.editing.is_some() && Instant::now() < deadline {
            let event = app.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            app.sender.send(event).unwrap();
            app.drain_events();
        }
        assert!(app.editing.is_none());
        let shown = app.messages.last().unwrap();
        assert!(shown.edited);
        assert_eq!(shown.text, "hello &amp; goodbye");
        let stored = app.service.messages("C1").unwrap();
        assert_eq!(stored.last().unwrap().text, "hello &amp; goodbye");
    }

    #[test]
    fn deleting_an_own_message_asks_first_and_removes_it() {
        let mut app = app();
        let ctx = app.ctx.clone();
        app.selected = Some(conversation(&app, "C1"));
        app.service.send("C1", "oops", None, "delete-test").unwrap();
        app.messages = app.service.messages("C1").unwrap();
        let ts = app.messages.last().unwrap().ts.clone();
        app.message_action(target("C1"), MessageAction::Delete(ts.clone()));
        assert!(app.deleting.is_some(), "asks before deleting");
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![enter],
                focused: true,
                ..Default::default()
            },
            |ui| app.delete_dialog(ui.ctx()),
        );
        output.textures_delta.clear();
        assert!(app.deleting.as_ref().is_some_and(|dialog| dialog.busy));
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.deleting.is_some() && Instant::now() < deadline {
            let event = app.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            app.sender.send(event).unwrap();
            app.drain_events();
        }
        assert!(app.deleting.is_none());
        assert!(app.messages.iter().all(|message| message.ts != ts));
        let stored = app.service.messages("C1").unwrap();
        assert!(stored.iter().all(|message| message.ts != ts));
    }

    #[test]
    fn sending_shows_immediately_keeps_focus_and_never_clears_the_next_draft() {
        for thread_ts in [None, Some("100.0".into())] {
            let mut app = app();
            let ctx = app.ctx.clone();
            let target = ComposeTarget {
                conversation_id: "C1".into(),
                thread_ts,
            };
            // Hold the request locally so both a slow send and a failure are deterministic.
            let (send_requests, requests) = channel();
            app.send_requests = send_requests;
            app.drafts.entry(target.clone()).or_default().text = "first message".into();
            ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
            let frame = |app: &mut ChatApp, events| {
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
                    |ui| app.compose(ui, &target, "Message"),
                );
                output.textures_delta.clear();
            };
            frame(&mut app, vec![]);
            let enter = |pressed| egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            frame(&mut app, vec![enter(true)]);
            let request = requests.try_recv().unwrap();
            assert_eq!(request.text, "first message");
            assert!(app.drafts[&target].text.is_empty());
            assert_eq!(app.outbox.messages(&target, &[]).len(), 1);
            assert!(ctx.memory(|memory| memory.has_focus(target.editor_id())));
            frame(
                &mut app,
                vec![enter(false), egui::Event::Text("next message".into())],
            );
            assert_eq!(app.drafts[&target].text, "next message");
            app.sender
                .send(UiEvent::MessageSent(
                    request.clone(),
                    Err(SlackError("offline".into())),
                ))
                .unwrap();
            app.drain_events();
            assert_eq!(app.drafts[&target].text, "next message");
            assert!(matches!(
                app.outbox.delivery(&app.outbox.messages(&target, &[])[0]),
                Some(super::super::outbox::Delivery::Failed(_))
            ));
            let retry = app.outbox.retry(&request.id).unwrap();
            app.sender
                .send(UiEvent::MessageSent(retry, Ok("200.0".into())))
                .unwrap();
            app.drain_events();
            frame(&mut app, vec![egui::Event::Text("!".into())]);
            assert_eq!(app.drafts[&target].text, "next message!");
            frame(&mut app, vec![enter(true)]);
            assert_eq!(requests.try_recv().unwrap().text, "next message!");
        }
    }

    #[test]
    fn quick_switcher_focuses_composer_without_submitting_existing_draft() {
        let mut app = app();
        let ctx = app.ctx.clone();
        // Limit the fixture so Enter deterministically chooses C1.
        app.workspace
            .as_mut()
            .unwrap()
            .conversations
            .retain(|c| c.id == "C1");
        app.drafts.entry(target("C1")).or_default().text = "existing draft".into();
        let frame = |app: &mut ChatApp, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.quick_switcher(ui.ctx());
                    if app.selected.is_some() {
                        app.compose(ui, &target("C1"), "Message # general");
                    }
                },
            );
            output.textures_delta.clear();
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        // Both a newly selected channel and selecting that same channel again.
        for _ in 0..2 {
            app.todos_open = true;
            frame(&mut app, vec![key(egui::Key::P, egui::Modifiers::COMMAND)]);
            frame(&mut app, vec![]);
            frame(&mut app, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
            assert!(app.switcher.is_none());
            assert!(!app.todos_open);
            // The first frame after the modal closes must already accept typing.
            frame(&mut app, vec![egui::Event::Text("!".into())]);
            assert!(ctx.memory(|memory| memory.has_focus(target("C1").editor_id())));
        }
        assert_eq!(app.drafts[&target("C1")].text, "existing draft!!");
    }

    #[test]
    fn search_within_a_channel_finds_messages_and_opens_them_with_their_thread() {
        let mut app = app();
        let ctx = app.ctx.clone();
        app.show_conversation(conversation(&app, "C1"));
        let frame = |app: &mut ChatApp, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    app.search_shortcuts(ui.ctx());
                    if app.search_open {
                        app.search.show(ui, &mut app.images);
                    }
                },
            );
            output.textures_delta.clear();
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        frame(&mut app, vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
        assert!(app.search_open && !app.conversation_shown());
        assert_eq!(app.search.query, "in:#general ");
        frame(&mut app, vec![]);
        frame(
            &mut app,
            vec![
                egui::Event::Text("drafts".into()),
                key(egui::Key::Enter, egui::Modifiers::NONE),
            ],
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.search.selected.is_none() && Instant::now() < deadline {
            app.search.poll();
            if let Some(result) = app.search_results().first() {
                app.search.selected = Some(result.clone());
            }
        }
        let result = app.search.selected.clone().expect("a result");
        assert!(result.message.text.contains("drafts stay"));
        app.open_message(
            result.conversation_id.clone(),
            result.thread_ts.clone().unwrap(),
            Some(result.message.clone()),
        );
        let thread = app.thread.as_ref().unwrap();
        assert_eq!(thread.target.conversation_id, "C1");
        assert_eq!(thread.target.thread_ts, result.thread_ts);
        // Going back to a conversation leaves search and its side panel.
        app.show_conversation(conversation(&app, "C2"));
        assert!(!app.search_open && app.thread.is_none());
    }

    #[test]
    fn calls_ring_until_answered_elsewhere_and_huddles_show_where_they_happen() {
        let mut app = app();
        let me = app.workspace.as_ref().unwrap().self_user_id.clone();
        let live = |app: &mut ChatApp, event| app.live_event(LiveStatus::Event(Box::new(event)));
        let huddle = |participants: Vec<String>| crate::domain::model::Huddle {
            id: "R1".into(),
            conversation_id: "D1".into(),
            participants,
            joined_ever: vec!["U1".into()],
            link: Some("https://app.slack.com/huddle/TDEMO/D1".into()),
            ended: false,
            started: 1_000,
            ended_at: 0,
        };
        live(
            &mut app,
            LiveEvent::HuddleRinging {
                conversation_id: "D1".into(),
                caller_id: me.clone(),
                call_id: "R1".into(),
            },
        );
        assert!(app.huddles.ringing().is_none(), "own calls never ring");
        live(
            &mut app,
            LiveEvent::HuddleRinging {
                conversation_id: "D1".into(),
                caller_id: "U1".into(),
                call_id: "R1".into(),
            },
        );
        assert!(app.huddles.ringing().is_some());
        live(
            &mut app,
            LiveEvent::HuddleChanged(huddle(vec!["U1".into()])),
        );
        assert!(
            app.huddles.ringing().is_some(),
            "the caller waiting keeps it ringing"
        );
        assert!(
            app.huddles.live("D1").is_some(),
            "the sidebar shows the huddle"
        );
        // Answered on the phone.
        live(
            &mut app,
            LiveEvent::HuddleChanged(huddle(vec!["U1".into(), me.clone()])),
        );
        assert!(app.huddles.ringing().is_none());
        // The caller's name arrives for the banner and the card.
        let deadline = Instant::now() + Duration::from_secs(5);
        while app
            .huddles
            .live("D1")
            .is_some_and(|live| live.names.len() < 2)
            && Instant::now() < deadline
        {
            app.drain_events();
        }
        assert_eq!(app.huddles.live("D1").unwrap().names[0], "Alex Morgan");
        // A Decline just stops the ring.
        live(
            &mut app,
            LiveEvent::HuddleRinging {
                conversation_id: "D2".into(),
                caller_id: "U2".into(),
                call_id: "R1".into(),
            },
        );
        app.call_choice(CallChoice::Decline);
        assert!(app.huddles.ringing().is_none());
        assert!(app.huddles.call().is_none(), "declining joins nothing");
        // Answering joins in the app; this workspace has no huddles, so it says so.
        live(
            &mut app,
            LiveEvent::HuddleRinging {
                conversation_id: "D2".into(),
                caller_id: "U2".into(),
                call_id: "R2".into(),
            },
        );
        app.call_choice(CallChoice::Answer);
        assert!(app.huddles.ringing().is_none());
        assert!(app.huddles.in_call("D2"), "joining while Slack answers");
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.huddles.call().is_some() && Instant::now() < deadline {
            app.drain_events();
        }
        assert!(app.huddles.call().is_none());
        assert!(
            app.status
                .as_deref()
                .is_some_and(|status| status.contains("huddle"))
        );
    }

    fn conversation(app: &ChatApp, id: &str) -> Conversation {
        app.workspace
            .as_ref()
            .unwrap()
            .conversations
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .unwrap()
    }

    #[test]
    fn back_and_forward_walk_visited_conversations() {
        let mut app = app();
        for id in ["C1", "C2", "C3"] {
            app.select(conversation(&app, id));
        }
        let selected = |app: &ChatApp| app.selected.as_ref().unwrap().id.clone();
        app.go_back();
        assert_eq!(selected(&app), "C2");
        app.go_back();
        assert_eq!(selected(&app), "C1");
        app.go_back();
        assert_eq!(selected(&app), "C1");
        app.go_forward();
        assert_eq!(selected(&app), "C2");
        // Opening a different conversation starts a new branch.
        app.select(conversation(&app, "D2"));
        assert!(app.forward.is_empty());
        app.go_back();
        assert_eq!(selected(&app), "C2");
    }

    #[test]
    fn ctrl_i_is_italic_while_typing_and_forward_otherwise() {
        let mut app = app();
        let ctx = app.ctx.clone();
        let target = target_of("C1");
        assert!(!app.typing(&ctx), "nothing focused");
        ctx.memory_mut(|memory| memory.request_focus(target.editor_id()));
        app.drafts.entry(target.clone()).or_default();
        assert!(!app.typing(&ctx), "an empty message box");
        app.drafts.get_mut(&target).unwrap().text = "hej".into();
        assert!(app.typing(&ctx));
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("search_query")));
        assert!(app.typing(&ctx), "another text field keeps its keys");
    }

    #[test]
    fn back_and_forward_include_the_thread_beside_the_conversation() {
        let mut app = app();
        let thread = |app: &ChatApp| {
            app.thread
                .as_ref()
                .and_then(|thread| thread.target.thread_ts.clone())
        };
        app.select(conversation(&app, "C1"));
        app.message_action(target_of("C1"), MessageAction::OpenThread("1.0".into()));
        app.track_place();
        app.message_action(target_of("C1"), MessageAction::OpenThread("2.0".into()));
        app.select(conversation(&app, "C2"));
        app.go_back();
        assert_eq!(app.selected.as_ref().unwrap().id, "C1");
        assert_eq!(thread(&app).as_deref(), Some("2.0"));
        app.go_back();
        assert_eq!(thread(&app).as_deref(), Some("1.0"), "the earlier thread");
        app.go_back();
        assert_eq!(thread(&app), None, "the conversation before any thread");
        app.go_forward();
        assert_eq!(thread(&app).as_deref(), Some("1.0"));
        // Closing the thread is a step too.
        app.thread = None;
        app.go_back();
        assert_eq!(thread(&app).as_deref(), Some("1.0"));
    }

    #[test]
    fn new_divider_marks_the_read_position_only_for_unread_conversations() {
        let mut app = app();
        app.select(conversation(&app, "C2"));
        let read = app.workspace.as_ref().unwrap().read_states["C2"]
            .last_read_ts
            .clone();
        assert!(read.is_some());
        assert_eq!(app.new_after, read);
        // Read conversations, and ones never read, get no divider.
        app.select(conversation(&app, "D2"));
        assert_eq!(app.new_after, None);
        app.select(conversation(&app, "D1"));
        assert_eq!(app.new_after, None);
    }

    #[test]
    fn muted_conversations_are_hidden_unless_open_or_mentioning() {
        let mut app = app();
        let workspace = app.workspace.as_mut().unwrap();
        workspace.muted.extend(["C1".to_string(), "C2".to_string()]);
        let workspace = app.workspace.as_ref().unwrap();
        let shown = |id, open| shown_in_sidebar(workspace, &conversation(&app, id), open);
        assert!(
            shown("C1", None),
            "a muted channel with a mention stays visible"
        );
        assert!(!shown("C2", None));
        assert!(shown("C2", Some("C2")));
        assert!(!shown("C3", None), "muted in the demo workspace");
        assert!(shown("D2", None));
    }

    #[test]
    fn header_symbols_and_shortcut_labels_have_glyphs() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        let font = egui::FontId::proportional(15.0);
        for text in [
            BACK_ARROW,
            FORWARD_ARROW,
            BACK_LABEL,
            FORWARD_LABEL,
            SWITCHER_SHORTCUT,
        ] {
            assert!(
                ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, text)),
                "{text}"
            );
        }
    }

    #[test]
    fn late_history_cannot_replace_a_different_conversation() {
        let mut app = app();
        app.selected = app
            .workspace
            .as_ref()
            .unwrap()
            .conversations
            .iter()
            .find(|c| c.id == "C2")
            .cloned();
        app.messages = app.service.messages("C2").unwrap();
        let original_ts = app.messages[0].ts.clone();
        app.sender
            .send(UiEvent::MessagesLoaded(
                target("C1"),
                Ok(app.service.messages("C1").unwrap()),
            ))
            .unwrap();
        app.drain_events();
        assert_eq!(app.messages[0].ts, original_ts);
    }

    #[test]
    fn closed_thread_ignores_late_replies_and_keeps_its_draft() {
        let mut app = app();
        let root = app
            .service
            .messages("C1")
            .unwrap()
            .into_iter()
            .find(|m| m.reply_count > 0)
            .unwrap();
        let target = ComposeTarget {
            conversation_id: "C1".into(),
            thread_ts: Some(root.ts.clone()),
        };
        app.drafts.entry(target.clone()).or_default().text = "thread draft".into();
        app.thread = None;
        app.sender
            .send(UiEvent::MessagesLoaded(
                target.clone(),
                Ok(app.service.replies("C1", &root.ts).unwrap()),
            ))
            .unwrap();
        app.drain_events();
        assert!(app.thread.is_none());
        assert_eq!(app.drafts[&target].text, "thread draft");
    }

    #[test]
    fn stale_counts_cannot_undo_a_new_read_state() {
        let mut app = app();
        app.revision = 2;
        app.sender
            .send(UiEvent::Counts(1, Ok(HashMap::new())))
            .unwrap();
        app.drain_events();
        assert!(!app.workspace.as_ref().unwrap().read_states.is_empty());
    }

    #[test]
    fn cached_channel_is_visible_immediately_on_return_and_late_disk_data_cannot_replace_it() {
        let mut app = app();
        let conversation = app
            .workspace
            .as_ref()
            .unwrap()
            .conversations
            .iter()
            .find(|c| c.id == "C1")
            .unwrap()
            .clone();
        let messages = app.service.messages("C1").unwrap();
        app.apply_messages(target("C1"), messages.clone());
        app.select(conversation);
        assert_eq!(app.messages, messages);
        assert!(app.loading.contains(&target("C1")));
        app.sender
            .send(UiEvent::MessagesCached(target("C1"), vec![]))
            .unwrap();
        app.drain_events();
        assert!(!app.messages.is_empty());
    }

    #[test]
    fn early_history_fills_only_empty_views_and_ignores_invalidated_requests() {
        let mut app = app();
        let conversation = app.workspace.as_ref().unwrap().conversations[0].clone();
        app.selected = Some(conversation);
        let messages = app.service.messages("C1").unwrap();
        app.loading.insert(target("C1"));
        app.sender
            .send(UiEvent::MessagesPreview(target("C1"), messages.clone()))
            .unwrap();
        app.drain_events();
        assert_eq!(app.messages, messages);
        assert!(
            app.loading.contains(&target("C1")),
            "metadata is still in flight"
        );

        let mut unresolved = messages.clone();
        unresolved[0].author = "UNKNOWN".into();
        app.sender
            .send(UiEvent::MessagesPreview(target("C1"), unresolved.clone()))
            .unwrap();
        app.drain_events();
        assert_eq!(
            app.messages, messages,
            "cached display must not lose resolved metadata"
        );

        app.history_cache.clear();
        app.reload.insert(target("C1"));
        app.sender
            .send(UiEvent::MessagesPreview(target("C1"), unresolved))
            .unwrap();
        app.sender
            .send(UiEvent::MessagesPreview(target("C2"), vec![]))
            .unwrap();
        app.drain_events();
        assert_eq!(
            app.messages, messages,
            "invalidated/other-channel previews cannot replace the view"
        );
    }

    #[test]
    fn view_choices_are_saved_and_activity_starts_with_the_saved_filter() {
        use crate::domain::activity::{ActivityFilter, ActivityQuery};
        let mut app = app();
        assert!(app.activity.is_some(), "workspace loaded");
        app.unread_only = true;
        app.save_settings();
        assert!(app.settings.unread_only);
        let saved = ActivityQuery {
            filter: ActivityFilter::Mentions,
            unread_only: true,
        };
        app.activity = Some(Activity::new(
            Arc::clone(&app.service),
            app.ctx.clone(),
            saved,
        ));
        app.save_settings();
        assert_eq!(app.settings.activity, saved);
        assert_eq!(app.activity.as_ref().unwrap().query(), saved);
    }

    #[test]
    fn unread_filter_includes_mentions_but_ignores_ordinary_muted_activity() {
        let mut app = app();
        let workspace = app.workspace.as_mut().unwrap();
        workspace.muted.insert("C1".into());
        workspace.read_states.insert(
            "C1".into(),
            ReadState {
                has_unreads: true,
                ..Default::default()
            },
        );
        assert!(!needs_attention(workspace, "C1"));
        workspace.read_states.get_mut("C1").unwrap().mention_count = 1;
        assert!(needs_attention(workspace, "C1"));
    }

    #[test]
    fn long_sidebar_names_and_badges_keep_the_same_bounds_while_scrolling() {
        let app = app();
        let mut workspace = app.workspace.unwrap();
        workspace.conversations = (0..120)
            .map(|index| Conversation {
                id: format!("long-{index}"),
                name: format!("A very long group conversation with many participants {index}"),
                kind: crate::domain::model::ConversationKind::GroupMessage,
            })
            .collect();
        for conversation in &workspace.conversations {
            workspace.read_states.insert(
                conversation.id.clone(),
                ReadState {
                    has_unreads: true,
                    mention_count: 123,
                    ..Default::default()
                },
            );
        }
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |offset| {
            let mut size = egui::Vec2::ZERO;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(200.0, 300.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    size = egui::ScrollArea::vertical()
                        .id_salt("long_sidebar")
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| {
                            for conversation in &workspace.conversations {
                                sidebar_row(ui, &workspace, conversation, None, false, false);
                            }
                        })
                        .content_size;
                },
            );
            output.textures_delta.clear();
            size
        };
        let top = frame(0.0);
        for offset in [900.0, 2200.0, 3200.0, 0.0] {
            let scrolled = frame(offset);
            assert!(
                scrolled.x <= 200.0,
                "long names must truncate: {scrolled:?}"
            );
            assert!(
                (top.y - scrolled.y).abs() < 0.5,
                "sidebar height changed: {top:?} -> {scrolled:?}"
            );
        }
    }
}
