//! Colors, fonts, and icons of the amplo design.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, ImageSource, Style, TextStyle,
    include_image, style::ScrollStyle,
};

pub const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF3, 0xF5);
pub const ACCENT: Color32 = Color32::from_rgb(0xC8, 0xF2, 0x5A);
/// The accent color at the 1.08 brightness that the design uses for hovered play buttons.
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xD8, 0xFF, 0x61);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x0E, 0x0F, 0x12);
pub const BACKGROUND: Color32 = Color32::from_rgb(0x12, 0x13, 0x17);
pub const TITLE_BAR: Color32 = Color32::from_rgb(0x0C, 0x0D, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x15, 0x17, 0x1B);
pub const LINE: Color32 = Color32::from_rgb(0x1F, 0x22, 0x27);
pub const MUTED: Color32 = Color32::from_rgb(0x8B, 0x92, 0x9D);
pub const SOFT: Color32 = Color32::from_rgb(0x9A, 0xA1, 0xAC);
pub const ICON_HOVER: Color32 = Color32::from_rgb(0x26, 0x2A, 0x31);
pub const NAV_HOVER: Color32 = Color32::from_rgb(0x1E, 0x21, 0x26);
pub const NAV_ACTIVE: Color32 = Color32::from_rgb(0x23, 0x26, 0x2C);
pub const ROW_CURRENT: Color32 = Color32::from_rgb(0x1A, 0x1D, 0x22);
pub const ROW_HOVER: Color32 = Color32::from_rgb(0x1C, 0x1F, 0x24);
pub const ROW_SELECTED: Color32 = Color32::from_rgb(0x27, 0x2C, 0x33);
pub const MINI: Color32 = Color32::from_rgb(0x1C, 0x1F, 0x24);
pub const MINI_LINE: Color32 = Color32::from_rgb(0x2A, 0x2E, 0x35);
pub const CHEVRON: Color32 = Color32::from_rgb(0xC9, 0xCD, 0xD3);
pub const WARNING: Color32 = Color32::from_rgb(0xFF, 0x9A, 0x5C);
/// A Spotify song that the local library has, and one that it lacks.
pub const AVAILABLE: Color32 = Color32::from_rgb(0x5E, 0xE0, 0x8C);
pub const MISSING: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);

/// Colors that only the full layout uses.
pub const FULL_BACKGROUND: Color32 = Color32::from_rgb(0x0E, 0x0F, 0x12);
pub const FULL_ROW_HOVER: Color32 = Color32::from_rgb(0x18, 0x1B, 0x20);
pub const SIDEBAR_LINE: Color32 = Color32::from_rgb(0x23, 0x26, 0x2C);
pub const TITLE_LINE: Color32 = Color32::from_rgb(0x1E, 0x21, 0x26);
pub const TABLE_LINE: Color32 = Color32::from_rgb(0x22, 0x25, 0x2B);
pub const PILL_LINE: Color32 = Color32::from_rgb(0x2C, 0x30, 0x38);
pub const FIELD_LINE: Color32 = Color32::from_rgb(0x26, 0x2A, 0x31);
pub const FULL_MINI: Color32 = Color32::from_rgb(0x1A, 0x1D, 0x22);

pub const LOGO: ImageSource<'static> = include_image!("../assets/icons/logo.svg");
pub const MINIMIZE: ImageSource<'static> = include_image!("../assets/icons/minimize.svg");
pub const CLOSE: ImageSource<'static> = include_image!("../assets/icons/close.svg");
pub const PLUS: ImageSource<'static> = include_image!("../assets/icons/plus.svg");
pub const PLAY: ImageSource<'static> = include_image!("../assets/icons/play.svg");
pub const PAUSE: ImageSource<'static> = include_image!("../assets/icons/pause.svg");
pub const PREVIOUS: ImageSource<'static> = include_image!("../assets/icons/prev.svg");
pub const NEXT: ImageSource<'static> = include_image!("../assets/icons/next.svg");
pub const SHUFFLE: ImageSource<'static> = include_image!("../assets/icons/shuffle.svg");
pub const SORT: ImageSource<'static> = include_image!("../assets/icons/sort.svg");
pub const REPEAT: ImageSource<'static> = include_image!("../assets/icons/repeat.svg");
pub const HEART: ImageSource<'static> = include_image!("../assets/icons/heart.svg");
pub const HEART_FILLED: ImageSource<'static> = include_image!("../assets/icons/heart-filled.svg");
pub const VOLUME: ImageSource<'static> = include_image!("../assets/icons/volume.svg");
pub const CHEVRON_UP: ImageSource<'static> = include_image!("../assets/icons/chevron-up.svg");
pub const CHEVRON_DOWN: ImageSource<'static> = include_image!("../assets/icons/chevron-down.svg");
pub const NOTE: ImageSource<'static> = include_image!("../assets/icons/note.svg");
pub const MAXIMIZE: ImageSource<'static> = include_image!("../assets/icons/maximize.svg");
pub const RESTORE: ImageSource<'static> = include_image!("../assets/icons/restore.svg");
pub const SEARCH: ImageSource<'static> = include_image!("../assets/icons/search.svg");
pub const DISC: ImageSource<'static> = include_image!("../assets/icons/disc.svg");
pub const FOLDER: ImageSource<'static> = include_image!("../assets/icons/folder.svg");
pub const CLOUD: ImageSource<'static> = include_image!("../assets/icons/cloud.svg");
pub const CHECK: ImageSource<'static> = include_image!("../assets/icons/check.svg");

const FONTS: [(&str, &[u8]); 6] = [
    ("inter", include_bytes!("../assets/fonts/Inter-Regular.ttf")),
    ("inter-semibold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
    ("grotesk-semibold", include_bytes!("../assets/fonts/SpaceGrotesk-SemiBold.ttf")),
    ("grotesk-bold", include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf")),
    ("mono", include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf")),
    ("mono-semibold", include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf")),
];

fn font(name: &str, size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(name.into()))
}

pub fn inter(size: f32) -> FontId {
    font("inter", size)
}

pub fn inter_semibold(size: f32) -> FontId {
    font("inter-semibold", size)
}

pub fn grotesk_semibold(size: f32) -> FontId {
    font("grotesk-semibold", size)
}

pub fn grotesk_bold(size: f32) -> FontId {
    font("grotesk-bold", size)
}

pub fn mono(size: f32) -> FontId {
    font("mono", size)
}

pub fn mono_semibold(size: f32) -> FontId {
    font("mono-semibold", size)
}

fn apply_style(style: &mut Style) {
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = egui::Color32::from_rgb(0x1C, 0x1F, 0x24);
    style.visuals.override_text_color = Some(TEXT);
    style.spacing.scroll = ScrollStyle::floating();
    style.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
    // Menus set their own button padding. See `ui::roomy_menu`.
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.item_spacing.y = 2.0;
    style.spacing.menu_width = 180.0;
    for widget in [&mut style.visuals.widgets.hovered, &mut style.visuals.widgets.active] {
        widget.corner_radius = egui::CornerRadius::same(6);
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for (name, bytes) in FONTS {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
        let mut family = vec![name.to_owned()];
        family.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".to_owned());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".to_owned());
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(apply_style);
    egui_extras::install_image_loaders(ctx);
}
