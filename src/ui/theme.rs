use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Ubuntu-Medium".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/Ubuntu-Medium.ttf"
        ))),
    );
    let mut bold_fonts = vec!["Ubuntu-Medium".into()];
    bold_fonts.extend(fonts.families[&FontFamily::Proportional].clone());
    fonts.families.insert(bold_family(), bold_fonts);
    ctx.set_fonts(fonts);
    ctx.set_visuals(visuals());
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(15.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(10.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.indent = 16.0;
        style.spacing.scroll.floating = true;
    });
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name("message-bold".into())
}

pub const GUTTER: i8 = 16;
pub const HEADER_HEIGHT: f32 = 52.0;

pub fn header_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(GUTTER, 12))
}

pub fn composer_panel_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin {
        left: GUTTER,
        right: GUTTER,
        top: 12,
        bottom: GUTTER,
    })
}

pub fn timeline_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(GUTTER, 8))
}

pub fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(4)
        .inner_margin(20)
}

fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = INPUT_BACKGROUND;
    visuals.faint_bg_color = HOVER;
    visuals.code_bg_color = CODE_BACKGROUND;
    visuals.hyperlink_color = LINK;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.window_corner_radius = CornerRadius::same(2);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.hovered.weak_bg_fill = HOVER;
    visuals.widgets.hovered.bg_fill = HOVER;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
    ] {
        widget.corner_radius = CornerRadius::same(2);
    }
    visuals
}

pub const BACKGROUND: Color32 = Color32::from_rgb(26, 29, 33);
pub const SIDEBAR: Color32 = Color32::from_rgb(24, 26, 29);
pub const SURFACE: Color32 = Color32::from_rgb(34, 37, 41);
pub const HOVER: Color32 = Color32::from_rgb(34, 37, 42);
pub const INPUT_BACKGROUND: Color32 = Color32::from_rgb(34, 37, 41);
pub const CODE_BACKGROUND: Color32 = Color32::from_rgb(40, 42, 46);
pub const BORDER: Color32 = Color32::from_rgb(60, 63, 68);
pub const TEXT: Color32 = Color32::from_rgb(209, 210, 211);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(248, 248, 248);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(140, 142, 146);
pub const LINK: Color32 = Color32::from_rgb(130, 170, 190);
pub const ACCENT: Color32 = Color32::from_rgb(54, 73, 82);
pub const MENTION_BACKGROUND: Color32 = Color32::from_rgb(31, 59, 79);
pub const SELF_MENTION_BACKGROUND: Color32 = Color32::from_rgb(84, 70, 25);
pub const SELF_MENTION_TEXT: Color32 = Color32::from_rgb(242, 199, 68);
pub const UNREAD_DOT: Color32 = Color32::from_rgb(235, 91, 100);
/// Buttons that destroy something: Leave, Delete.
pub const DANGER: Color32 = Color32::from_rgb(196, 43, 28);
pub const SPELLING: Color32 = Color32::from_rgb(235, 91, 100);
pub const BADGE: Color32 = Color32::from_rgb(87, 105, 112);
pub const QUOTE_BAR: Color32 = Color32::from_rgb(90, 93, 98);
pub const REACTION_MINE: Color32 = Color32::from_rgb(25, 55, 80);
pub const REACTION_MINE_BORDER: Color32 = Color32::from_rgb(29, 155, 209);
