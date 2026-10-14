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
use crate::domain::sidebar::{SidebarInput, build_sidebar};
use crate::domain::unread::{record_incoming, record_read};
use crate::ui::activity::Activity;
use crate::ui::composer::{ComposeTarget, Draft, composer};
use crate::ui::dock::DockBadge;
use crate::ui::emoji_picker::{EmojiPicker, PickerOutcome};
use crate::ui::image_viewer::ImageViewer;
use crate::ui::images::ImageCache;
use crate::ui::labels::{conversation_label, paint_sidebar_icon};
use crate::ui::message_view::{MessageAction, MessageContext, loading_messages, message_list};
use crate::ui::outbox::{Outbox, SendRequest};
use crate::ui::quick_switcher::{QuickSwitcher, SwitcherOutcome};
use crate::ui::theme;
use crate::ui::todos::Todos;

pub fn run(
    service: ChatService,
    team_name: String,
    realtime: Option<Arc<dyn RealtimeSource>>,
) -> eframe::Result {
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
    messages: Vec<DisplayMessage>,
    history_cache: HashMap<ComposeTarget, (Instant, Vec<DisplayMessage>)>,
    unread_only: bool,
    todos_open: bool,
    todos: Option<Todos>,
    activity_open: bool,
    activity: Option<Activity>,
    drafts: HashMap<ComposeTarget, Draft>,
    paste_shortcut: crate::infrastructure::clipboard::PasteShortcut,
    composer_focus: Option<ComposeTarget>,
    outbox: Outbox,
    send_requests: Sender<SendRequest>,
    scroll_after_send: HashSet<ComposeTarget>,
    thread: Option<ThreadView>,
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
    image_viewer: Option<ImageViewer>,
    dock: DockBadge,
    realtime: Option<Arc<dyn RealtimeSource>>,
    live_connected: bool,
}

enum UiEvent {
    WorkspaceLoaded(Result<Workspace, SlackError>),
    MessagesLoaded(ComposeTarget, Result<Vec<DisplayMessage>, SlackError>),
    MessagesCached(ComposeTarget, Vec<DisplayMessage>),
    MessagesPreview(ComposeTarget, Vec<DisplayMessage>),
    MessageSent(SendRequest, Result<String, SlackError>),
    ReactionSet {
        conversation_id: String,
        ts: String,
        name: String,
        result: Result<(), SlackError>,
    },
    Marked(String, String, Result<(), SlackError>),
    Counts(u64, Result<HashMap<String, ReadState>, SlackError>),
    Live(LiveStatus),
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
        if let Some(todos) = &mut self.todos {
            todos.poll();
        }
        if let Some(activity) = &mut self.activity {
            activity.poll();
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
        ctx.request_repaint_after(until_poll.min(until_counts).max(Duration::from_secs(1)));
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
        self.quick_switcher(ui.ctx());
        self.emoji_picker(ui.ctx());
        if let Some(viewer) = &self.image_viewer
            && viewer.show(ui.ctx(), &mut self.images)
        {
            self.image_viewer = None;
        }
        let modal_open =
            self.switcher.is_some() || self.picker.is_some() || self.image_viewer.is_some();
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
        let mut app = Self {
            service,
            ctx,
            sender,
            receiver,
            team_name,
            workspace: None,
            selected: None,
            messages: Vec::new(),
            history_cache: HashMap::new(),
            unread_only: false,
            todos_open: false,
            todos: None,
            activity_open: false,
            activity: None,
            drafts: HashMap::new(),
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
            image_viewer: None,
            dock: DockBadge::default(),
            realtime,
            live_connected: false,
        };
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
                    self.unread_only,
                    (
                        egui::RichText::new("Unreads")
                            .strong()
                            .color(if unread_count > 0 {
                                theme::TEXT_STRONG
                            } else {
                                theme::TEXT_MUTED
                            }),
                        egui::Atom::grow(),
                        egui::RichText::new(unread_count.to_string()).color(theme::LINK),
                    ),
                ),
            )
            .on_hover_text(format!("Show unread conversations · {UNREAD_SHORTCUT}"))
            .clicked()
        {
            self.unread_only = !self.unread_only;
        }
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
            self.composer_focus = None;
            if let Some(activity) = &mut self.activity {
                activity.open();
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        let selected = self
            .selected
            .as_ref()
            .filter(|_| !self.todos_open && !self.activity_open)
            .map(|conversation| conversation.id.as_str());
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt(("sidebar_scroll", self.unread_only))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.unread_only {
                    let mut conversations: Vec<_> = workspace
                        .conversations
                        .iter()
                        .filter(|conversation| needs_attention(workspace, &conversation.id))
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
                        if sidebar_row(ui, workspace, conversation, selected) {
                            clicked = Some(conversation.clone());
                        }
                    }
                } else {
                    for (index, group) in workspace.sidebar.iter().enumerate() {
                        if index > 0 {
                            ui.add_space(8.0);
                        }
                        let unread = group
                            .conversations
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
                            for conversation in &group.conversations {
                                if sidebar_row(ui, workspace, conversation, selected) {
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
                    ui.heading(conversation_label(&conversation));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.live_connected {
                            ui.label(egui::RichText::new("Live").small().weak());
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
                            let action = message_list(
                                ui,
                                &conversation.id,
                                &self.outbox.messages(&target, &self.messages),
                                &mut MessageContext {
                                    emoji: &workspace.emoji,
                                    images: &mut self.images,
                                    self_user_id: &workspace.self_user_id,
                                    allow_threads: true,
                                    outbox: &self.outbox,
                                },
                            );
                            if self.scroll_after_send.remove(&target) {
                                ui.scroll_to_cursor_animation(
                                    Some(egui::Align::BOTTOM),
                                    egui::style::ScrollAnimation::none(),
                                );
                                ui.ctx().request_discard("show outgoing message");
                            }
                            action
                        }),
                )
            })
            .inner;
        let Some(output) = output else {
            return;
        };
        if let Some(action) = output.inner {
            self.message_action(conversation.id.clone(), action);
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
            && let Some(attachment) = todos.show_details(ui, &workspace.emoji, &mut self.images)
        {
            self.image_viewer = Some(ImageViewer::new(attachment));
        }
    }

    fn open_activity_item(&mut self, row: crate::application::activity::DisplayActivity) {
        let root = row
            .item
            .thread_ts
            .clone()
            .unwrap_or_else(|| row.item.ts.clone());
        let target = ComposeTarget {
            conversation_id: row.item.conversation_id,
            thread_ts: Some(root),
        };
        let messages = self
            .history_cache
            .get(&target)
            .map(|(_, messages)| messages.clone())
            .unwrap_or_else(|| row.message.into_iter().collect());
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
                    ui.heading(if self.activity_open {
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
                        let action = message_list(
                            ui,
                            "thread",
                            &self.outbox.messages(&target, &thread.messages),
                            &mut MessageContext {
                                emoji: &workspace.emoji,
                                images: &mut self.images,
                                self_user_id: &workspace.self_user_id,
                                allow_threads: false,
                                outbox: &self.outbox,
                            },
                        );
                        if self.scroll_after_send.remove(&target) {
                            ui.scroll_to_cursor_animation(
                                Some(egui::Align::BOTTOM),
                                egui::style::ScrollAnimation::none(),
                            );
                            ui.ctx().request_discard("show outgoing reply");
                        }
                        action
                    })
                    .inner
            })
            .inner;
        if retry {
            self.load_messages(target.clone());
        }
        if let Some(action) = action {
            self.message_action(target.conversation_id, action);
        }
    }

    fn compose(&mut self, ui: &mut egui::Ui, target: &ComposeTarget, hint: &str) {
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
        if composer(ui, target, draft, hint) {
            let text = std::mem::take(&mut draft.text);
            let files = draft.attachments.take();
            self.queue_message(target.clone(), text, files);
        }
    }

    fn queue_message(
        &mut self,
        target: ComposeTarget,
        text: String,
        files: Vec<crate::domain::uploads::UploadFile>,
    ) {
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
        self.todos_open = false;
        if self.activity_open {
            self.thread = None;
        }
        self.activity_open = false;
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
        self.selected = Some(conversation);
    }

    fn message_action(&mut self, conversation_id: String, action: MessageAction) {
        match action {
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
            MessageAction::OpenImage(attachment) => {
                self.image_viewer = Some(ImageViewer::new(attachment));
            }
            MessageAction::PickReaction(ts) => {
                self.picker = Some(EmojiPicker::new(conversation_id, ts))
            }
            MessageAction::ToggleReaction { ts, name, add } => {
                self.set_reaction(conversation_id, ts, name, add)
            }
        }
    }

    fn set_reaction(&mut self, conversation_id: String, ts: String, name: String, add: bool) {
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
                                ));
                            }
                            self.workspace = Some(workspace);
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
                UiEvent::ReactionSet {
                    conversation_id,
                    ts,
                    name,
                    result,
                } => {
                    self.pending_reactions
                        .remove(&(conversation_id.clone(), ts, name));
                    if let Err(error) = result {
                        self.status = Some(error.to_string());
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
                        Err(error) => {
                            self.status = Some(format!("Could not mark conversation read: {error}"))
                        }
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
            }
        }
    }

    fn apply_messages(&mut self, target: ComposeTarget, messages: Vec<DisplayMessage>) {
        self.outbox.reconcile(&target, &messages);
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
            }
            LiveEvent::MessagePosted {
                conversation_id,
                message,
                is_broadcast,
            } => {
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
        }
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
                .filter(|_| !self.todos_open && !self.activity_open)
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

#[cfg(target_os = "macos")]
const SWITCHER_SHORTCUT: &str = "⌘P";
#[cfg(not(target_os = "macos"))]
const SWITCHER_SHORTCUT: &str = "Ctrl+P";
#[cfg(target_os = "macos")]
const UNREAD_SHORTCUT: &str = "⌘⇧U";
#[cfg(not(target_os = "macos"))]
const UNREAD_SHORTCUT: &str = "Ctrl+Shift+U";
const POLL_INTERVAL: Duration = Duration::from_secs(10);
const COUNTS_INTERVAL: Duration = Duration::from_secs(30);

fn needs_attention(workspace: &Workspace, id: &str) -> bool {
    workspace.read_states.get(id).is_some_and(|state| {
        state.mention_count > 0 || (state.has_unreads && !workspace.muted.contains(id))
    })
}

fn sidebar_row(
    ui: &mut egui::Ui,
    workspace: &Workspace,
    conversation: &Conversation,
    selected: Option<&str>,
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
                    let output = egui::Button::selectable(
                        selected == Some(&conversation.id),
                        (
                            egui::Atom::custom(dot_id, egui::vec2(6.0, 18.0)),
                            egui::Atom::custom(icon_id, egui::vec2(18.0, 18.0)),
                            text.atom_shrink(true),
                            egui::Atom::grow(),
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
                    output.response
                })
                .on_hover_text(format!(
                    "{}{}",
                    conversation.name,
                    if muted { " · muted" } else { "" }
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
                                sidebar_row(ui, &workspace, conversation, None);
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
