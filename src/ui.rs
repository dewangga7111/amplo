//! Draws the compact layout: 780 by 460 points, positioned like the design. The parts that
//! both layouts share are here too. The full layout is in `ui_full.rs`.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Galley, Pos2, Rect, ScrollArea, Sense, Stroke, TextFormat, Ui, UiBuilder,
    Vec2, ViewportCommand, containers::scroll_area::ScrollSource, pos2, text::{LayoutJob, TextWrapping},
    vec2,
};
use amplo::{
    library::{ALL_SONGS, Collection, LIKED_SONGS, Playlist, Sort, split_names},
    lyrics::Extra,
};

use crate::{
    app::{APP_NAME, Action, App, CardTab, Selection, VISUALIZER_BARS, format_time},
    theme,
    ui_full,
    widgets::{
        IconButton, MarqueeText, accent_button, at, button_width, click_area, line, marquee, marquee_links, paint_edge_fades,
        paint_equalizer, paint_icon, paint_shadow, paint_text, paint_texture, paint_two_lines, slider, spaced_line,
        text_button, text_rects,
    },
};

pub const WINDOW_SIZE: Vec2 = vec2(780.0, 460.0);
const TITLE_HEIGHT: f32 = 36.0;
const SIDEBAR_WIDTH: f32 = 200.0;
const MINI_HEIGHT: f32 = 54.0;
const ROW_HEIGHT: f32 = 40.0;
const ROWS_TOP: f32 = 77.0;
const LIST_BOTTOM_SPACE: f32 = 80.0;
const NAV_STEP: f32 = 46.0;
const QUEUE_HEADING: f32 = 24.0;
const QUEUE_ROW: f32 = 46.0;
/// Roughly how long, in seconds, the lyrics take to scroll most of the way to a new line.
const LYRIC_GLIDE_TIME: f32 = 0.3;
pub const UNKNOWN_TIME: &str = "-:--";

/// Sizes of the lyrics in one layout. One font size serves all lines, and only the color
/// changes, so text never resizes or rewraps.
pub struct LyricStyle {
    pub font_size: f32,
    pub line_height: f32,
    /// The height of a line in the list. Long lines get all the rows they need.
    pub min_slot: f32,
    pub plain_font_size: f32,
    pub empty_icon: f32,
    pub empty_title: f32,
    pub empty_text: f32,
}

const COMPACT_LYRICS: LyricStyle = LyricStyle {
    font_size: 17.0,
    line_height: 21.25,
    min_slot: 46.0,
    plain_font_size: 15.0,
    empty_icon: 28.0,
    empty_title: 16.0,
    empty_text: 12.0,
};

/// Removes Space from this frame's input and tells whether it was pressed. Removed, so a
/// focused button does not also react to it. Holding the key down does not repeat.
fn take_space_press(ui: &Ui) -> bool {
    ui.input_mut(|input| {
        let mut pressed = false;
        input.events.retain(|event| match event {
            egui::Event::Key { key: egui::Key::Space, pressed: down, repeat, modifiers, .. } if modifiers.is_none() => {
                pressed |= *down && !*repeat;
                false
            }
            _ => true,
        });
        pressed
    })
}
/// Lets the empty part of a title bar move the window, and a double click maximize it.
pub fn title_bar_drag(ui: &Ui, rect: Rect, actions: &mut Vec<Action>) {
    let drag = ui.interact(rect, ui.scope_id().with("title-bar"), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        actions.push(Action::ToggleFull);
    }
}

/// Repaint rates while audio plays. Moving bars need more than the slow equalizers.
const VISUALIZER_FRAME: f32 = 1.0 / 30.0;
const LIST_FRAME: f32 = 1.0 / 15.0;

/// Moves `value` toward `target` with a fast start and slow end, close to the design's
/// `cubic-bezier(.2,.8,.2,1)`.
pub fn ease(value: &mut f32, target: f32, rate: f32, dt: f32) {
    *value += (target - *value) * (1.0 - (-rate * dt).exp());
    if (target - *value).abs() < 0.002 {
        *value = target;
    }
}

/// egui packs menu items tightly, with almost no padding. This gives each item a row that is
/// easy to hit, with the hover filling it.
fn roomy_menu(ui: &mut Ui) {
    ui.spacing_mut().button_padding = vec2(12.0, 7.0);
    ui.spacing_mut().item_spacing.y = 2.0;
}

/// Stands in for the cover of a playlist without songs.
pub fn paint_empty_cover(ui: &Ui, rect: Rect, radius: f32) {
    ui.painter().rect_filled(rect, radius, theme::MINI_LINE);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.5));
    paint_icon(ui, icon, theme::NOTE, theme::MUTED);
}

/// A playlist without songs, such as one just created.
pub fn draw_empty_playlist(ui: &Ui, stage: Rect, name: &str) {
    let center = stage.center();
    let icon = Rect::from_center_size(center - vec2(0.0, 40.0), Vec2::splat(28.0));
    paint_icon(ui, icon, theme::NOTE, theme::SOFT);
    let heading = line(ui, name, theme::grotesk_semibold(16.0), theme::TEXT, stage.width() - 40.0);
    paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 10.0), heading);
    let details = line(ui, "This playlist has no songs yet.", theme::inter(12.0), theme::SOFT, stage.width() - 40.0);
    paint_text(ui, Align2::CENTER_CENTER, center + vec2(0.0, 12.0), details);
}

/// The lyrics card while a file is dragged over the window.
fn paint_drop_target(ui: &Ui, view: Rect, style: &LyricStyle) {
    let area = view.shrink(4.0);
    ui.painter().rect(area, 12.0, theme::ACCENT.gamma_multiply(0.06), Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
    let center = area.center();
    let icon = Rect::from_center_size(center - vec2(0.0, style.empty_icon), Vec2::splat(style.empty_icon));
    paint_icon(ui, icon, theme::NOTE, theme::ACCENT);
    let title = line(ui, "Drop to add lyrics", theme::grotesk_semibold(style.empty_title), theme::TEXT, area.width());
    paint_text(ui, Align2::CENTER_CENTER, center + vec2(0.0, 6.0), title);
    let hint = line(ui, "An .lrc or .txt file", theme::inter(style.empty_text), theme::SOFT, area.width());
    paint_text(ui, Align2::CENTER_TOP, center + vec2(0.0, 8.0 + style.empty_title * 0.75), hint);
}

/// A small text link that ends at `right`. Returns whether it was clicked, and its left edge.
fn header_link(ui: &Ui, right: f32, center_y: f32, font: egui::FontId, text: &str, label: &str, on: bool) -> (bool, f32) {
    let galley = line(ui, text, font, Color32::PLACEHOLDER, 200.0);
    let rect = Align2::RIGHT_CENTER.anchor_size(pos2(right, center_y), galley.size());
    let response = click_area(ui, rect.expand2(vec2(6.0, 5.0)), label).on_hover_text(label);
    let color = match (on, response.hovered()) {
        (true, _) => theme::ACCENT,
        (false, true) => theme::TEXT,
        (false, false) => theme::MUTED,
    };
    ui.painter().galley(rect.min, galley, color);
    (response.clicked(), rect.left())
}

impl App {
    /// The links in the lyrics card's header, ending at `right`: what shows under each line,
    /// when the lyrics have romaji or a translation, then the one that edits the lyrics.
    pub fn draw_lyrics_links(&self, ui: &Ui, right: f32, center_y: f32, font: egui::FontId, short: bool, actions: &mut Vec<Action>) {
        let edit = if short { "Edit" } else { "Edit lyrics" };
        let (clicked, left) = header_link(ui, right, center_y, font.clone(), edit, "Edit lyrics", false);
        if clicked {
            actions.push(Action::EditLyrics);
        }
        if self.lyric_extra_choices().len() < 2 {
            return;
        }
        let extra = self.lyric_extra();
        let text = match extra {
            Some(Extra::Romaji) => "Romaji",
            Some(Extra::Translation) => "Translation",
            None => "Original",
        };
        let label = "Change what shows under each line";
        if header_link(ui, left - 14.0, center_y, font, text, label, extra.is_some()).0 {
            actions.push(Action::CycleLyricsExtra);
        }
    }
}

/// Lyrics from the clipboard after Paste lyrics, and lyrics files dropped on the window.
fn receive_lyrics(app: &mut App, ui: &Ui) {
    if let Some(until) = app.paste_lyrics_until {
        let pasted = ui.input_mut(|input| {
            let index = input.events.iter().position(|event| matches!(event, egui::Event::Paste(_)))?;
            match input.events.remove(index) {
                egui::Event::Paste(text) => Some(text),
                _ => None,
            }
        });
        if let Some(text) = pasted {
            app.paste_lyrics_until = None;
            app.add_lyrics(&text);
        } else if std::time::Instant::now() >= until {
            app.paste_lyrics_until = None;
            app.error = Some("The clipboard has no text to paste.".to_owned());
        } else {
            ui.ctx().request_repaint();
        }
    }

    let dropped = ui.input(|input| input.raw.dropped_files.clone());
    if dropped.is_empty() {
        return;
    }
    let is_lyrics = |path: &std::path::Path| {
        path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("lrc") || ext.eq_ignore_ascii_case("txt"))
    };
    let Some(file) = dropped.iter().find(|file| is_lyrics(file.path())) else {
        app.error = Some("Drop an .lrc or .txt file to add lyrics.".to_owned());
        return;
    };
    let text = match file.bytes() {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => {
            app.error = Some(format!("Could not read the lyrics file: {err}"));
            return;
        }
    };
    // With the form open, the file fills it, to be checked before it is saved.
    match &mut app.lyrics_editor {
        Some(editor) => {
            editor.text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
            editor.error = None;
        }
        None => app.add_lyrics(&text),
    }
}

/// The form that writes or changes lyrics. Escape or a click outside cancels.
fn draw_lyrics_editor(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(editor) = &mut app.lyrics_editor else { return };
    let frame = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::PILL_LINE))
        .corner_radius(14)
        .inner_margin(22);
    let width = (ctx.content_rect().width() - 120.0).clamp(280.0, 560.0);
    let height = (ctx.content_rect().height() - 300.0).clamp(120.0, 420.0);
    let modal = egui::Modal::new(egui::Id::unique("lyrics-editor"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.label(egui::RichText::new("Lyrics").font(theme::grotesk_semibold(18.0)).color(theme::TEXT));
            ui.label(egui::RichText::new(&editor.title).font(theme::inter(13.0)).color(theme::SOFT));
            ui.add_space(12.0);
            let hint = egui::RichText::new("Paste or type the lyrics here.\n\nLines that start with a time such as [01:23.45] follow the song.")
                .font(theme::inter(13.0))
                .color(theme::MUTED);
            let response = egui::ScrollArea::vertical()
                .max_height(height)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut editor.text)
                            .id_salt("lyrics-text")
                            .hint_text(hint)
                            .font(theme::inter(13.0))
                            .text_color(theme::TEXT)
                            .background_color(theme::BACKGROUND)
                            .margin(egui::Margin::symmetric(10, 8))
                            .desired_width(f32::INFINITY)
                            // Fills the room the window has, so the box ends in view.
                            .desired_rows(((height - 18.0) / 18.0).max(3.0) as usize),
                    )
                })
                .inner;
            if editor.wants_focus {
                response.request_focus();
                editor.wants_focus = false;
            }
            if response.changed() {
                editor.error = None;
            }
            if let Some(error) = &editor.error {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(error).font(theme::inter(12.0)).color(theme::WARNING));
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("You can also drop an .lrc or .txt file here.").font(theme::inter(12.0)).color(theme::MUTED));

            ui.add_space(14.0);
            let can_save = !editor.text.trim().is_empty();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = egui::Button::new(
                    egui::RichText::new("Save").font(theme::inter_semibold(13.0)).color(theme::ON_ACCENT),
                )
                .fill(theme::ACCENT)
                .corner_radius(8)
                .min_size(vec2(76.0, 34.0));
                let saved = ui.add_enabled(can_save, save).clicked();
                ui.add_space(4.0);
                let cancel = egui::Button::new(
                    egui::RichText::new("Cancel").font(theme::inter_semibold(13.0)).color(theme::SOFT),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.0, theme::PILL_LINE))
                .corner_radius(8)
                .min_size(vec2(76.0, 34.0));
                let cancelled = ui.add(cancel).clicked();
                (saved, cancelled)
            })
            .inner
        });
    let (save, cancel) = modal.inner;
    if save {
        actions.push(Action::SaveLyrics);
    } else if cancel || modal.should_close() {
        actions.push(Action::CloseLyricsEditor);
    }
}

/// The form that names a new playlist. Enter saves, and Escape or a click outside cancels.
fn draw_new_playlist(app: &mut App, ctx: &egui::Context, actions: &mut Vec<Action>) {
    let Some(form) = &mut app.new_playlist else { return };
    let frame = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::PILL_LINE))
        .corner_radius(14)
        .inner_margin(22);
    let modal = egui::Modal::new(egui::Id::unique("new-playlist"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(300.0);
            ui.label(egui::RichText::new("New playlist").font(theme::grotesk_semibold(18.0)).color(theme::TEXT));
            ui.add_space(14.0);
            let hint = egui::RichText::new("Playlist name").font(theme::inter(14.0)).color(theme::MUTED);
            let field = egui::TextEdit::singleline(&mut form.name)
                .id_salt("new-playlist-name")
                .hint_text(hint)
                .font(theme::inter(14.0))
                .text_color(theme::TEXT)
                .background_color(theme::BACKGROUND)
                .margin(egui::Margin::symmetric(10, 8))
                .desired_width(f32::INFINITY);
            let response = ui.add(field);
            if form.wants_focus {
                response.request_focus();
                form.wants_focus = false;
            }
            if response.changed() {
                form.error = None;
            }
            let entered = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if let Some(error) = &form.error {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(error).font(theme::inter(12.0)).color(theme::WARNING));
            }

            ui.add_space(18.0);
            let can_save = !form.name.trim().is_empty();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = egui::Button::new(
                    egui::RichText::new("Save").font(theme::inter_semibold(13.0)).color(theme::ON_ACCENT),
                )
                .fill(theme::ACCENT)
                .corner_radius(8)
                .min_size(vec2(76.0, 34.0));
                let saved = ui.add_enabled(can_save, save).clicked();
                ui.add_space(4.0);
                let cancel = egui::Button::new(
                    egui::RichText::new("Cancel").font(theme::inter_semibold(13.0)).color(theme::SOFT),
                )
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.0, theme::PILL_LINE))
                .corner_radius(8)
                .min_size(vec2(76.0, 34.0));
                let cancelled = ui.add(cancel).clicked();
                ((saved || entered) && can_save, cancelled)
            })
            .inner
        });
    let (save, cancel) = modal.inner;
    if save {
        actions.push(Action::CreatePlaylist);
    } else if cancel || modal.should_close() {
        app.new_playlist = None;
    }
}

pub fn plural_songs(count: usize) -> String {
    if count == 1 { "1 song".to_owned() } else { format!("{count} songs") }
}

impl App {
    pub fn draw(&mut self, ui: &mut Ui) {
        let dt = ui.input(|input| input.stable_dt).min(0.25);
        let has_track = self.current_track().is_some();
        let target = if has_track && !self.minimized { 1.0 } else { 0.0 };
        ease(&mut self.slide, target, 10.0, dt);
        // The mini player starts rising once Now Playing has moved out of its way.
        let mini_target = if has_track && target == 0.0 && self.slide < 0.8 { 1.0 } else { 0.0 };
        ease(&mut self.mini, mini_target, 12.0, dt);

        // Windows reports the state, so maximizing by any means shows the full layout.
        if let Some(maximized) = ui.input(|input| input.viewport().maximized) {
            self.full = maximized;
        }

        let window = ui.max_rect();
        let background = if self.full { theme::FULL_BACKGROUND } else { theme::BACKGROUND };
        ui.painter().rect_filled(window, 0.0, background);
        let mut actions = Vec::new();
        // Space types into the search box while it has the focus.
        if !ui.ctx().egui_wants_keyboard_input() && take_space_press(ui) {
            actions.push(Action::Toggle);
        }
        receive_lyrics(self, ui);
        if !ui.ctx().egui_wants_keyboard_input() && self.new_playlist.is_none() && self.lyrics_editor.is_none() {
            let list_shown = (self.minimized || !has_track) && !(self.full && self.spotify_open);
            let select_all = ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::A));
            if list_shown && select_all {
                actions.push(Action::SelectAll);
            }
            // An open menu takes Escape first, to close.
            if !self.selected.is_empty()
                && !egui::Popup::is_any_open(ui.ctx())
                && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                actions.push(Action::ClearSelection);
            }
        }
        let now = ui.input(|input| input.time);
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
        }
        if self.full {
            let (sidebar, title, stage) = ui_full::split(window);
            self.draw_full_title_bar(ui, title, &mut actions);
            self.draw_full_sidebar(ui, sidebar, &mut actions);
            self.draw_stage(ui, stage, dt, &mut actions);
        } else {
            let title = Rect::from_min_size(window.min, vec2(window.width(), TITLE_HEIGHT));
            let sidebar = Rect::from_min_max(
                pos2(window.left(), title.bottom()),
                pos2(window.left() + SIDEBAR_WIDTH, window.bottom()),
            );
            let stage = Rect::from_min_max(pos2(sidebar.right(), title.bottom()), window.max);
            self.draw_title_bar(ui, title, &mut actions);
            self.draw_sidebar(ui, sidebar, &mut actions);
            self.draw_stage(ui, stage, dt, &mut actions);
        }
        draw_new_playlist(self, ui.ctx(), &mut actions);
        draw_lyrics_editor(self, ui.ctx(), &mut actions);

        for action in actions {
            self.apply(action);
        }

        let lyrics_moving = self.slide > 0.0 && self.lyric_motion.moving;
        if self.slide != target || self.mini != mini_target || lyrics_moving {
            ui.ctx().request_repaint();
        } else if self.is_playing() {
            let frame = if self.slide > 0.0 { VISUALIZER_FRAME } else { LIST_FRAME };
            ui.ctx().request_repaint_after_secs(frame);
        } else if self.slide > 0.0 && self.analyzer.bars().iter().any(|&bar| bar > 0.001) {
            ui.ctx().request_repaint_after_secs(VISUALIZER_FRAME);
        }
    }

    fn draw_title_bar(&self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, theme::TITLE_BAR);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
        title_bar_drag(ui, rect, actions);

        let center_y = rect.center().y;
        let logo = Rect::from_center_size(pos2(rect.left() + 23.0, center_y), Vec2::splat(18.0));
        painter.rect_filled(logo, 5.0, theme::ACCENT);
        paint_icon(ui, logo.shrink(2.0), theme::LOGO, theme::ON_ACCENT);

        let name = line(ui, APP_NAME, theme::grotesk_bold(13.0), theme::TEXT, f32::INFINITY);
        let name_right = logo.right() + 10.0 + name.size().x;
        paint_text(ui, Align2::LEFT_CENTER, pos2(logo.right() + 10.0, center_y), name);

        let minimize = Rect::from_center_size(pos2(rect.right() - 85.0, center_y), vec2(30.0, 26.0));
        let maximize = Rect::from_center_size(pos2(rect.right() - 53.0, center_y), vec2(30.0, 26.0));
        let close = Rect::from_center_size(pos2(rect.right() - 21.0, center_y), vec2(30.0, 26.0));

        let status_left = name_right + 10.0;
        let status_width = (minimize.left() - 10.0 - status_left).max(0.0);
        let notice = self.notice.as_ref().map(|(notice, _)| notice);
        let (status, color) = match (&self.error, notice, self.current_track()) {
            (Some(error), _, _) => (error.clone(), theme::WARNING),
            (None, Some(notice), _) => (notice.clone(), theme::ACCENT),
            (None, None, Some(track)) => {
                let info = &self.library.tracks[track].info;
                let text = if info.artist.is_empty() {
                    format!("· {}", info.title)
                } else {
                    format!("· {} — {}", info.artist, info.title)
                };
                (text, theme::MUTED)
            }
            (None, None, None) => (String::new(), theme::MUTED),
        };
        let galley = line(ui, &status, theme::mono(11.0), color, status_width);
        let status_rect = Align2::LEFT_CENTER.anchor_size(pos2(status_left, center_y), galley.size());
        paint_text(ui, Align2::LEFT_CENTER, pos2(status_left, center_y), galley);
        if self.error.is_some() || self.notice.is_some() {
            // The bar is short, so a long message needs another way to be read in full.
            let hover = ui.interact(status_rect, ui.scope_id().with("status"), Sense::hover());
            hover.on_hover_text(status);
        }

        let button = |icon, size, label: &str, target: Rect| {
            IconButton { icon, icon_size: size, color: theme::SOFT, radius: 6.0 }.show(ui, target, label)
        };
        if button(theme::MINIMIZE, 14.0, "Minimize", minimize).clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if button(theme::MAXIMIZE, 13.0, "Maximize", maximize).clicked() {
            actions.push(Action::ToggleFull);
        }
        if button(theme::CLOSE, 14.0, "Close", close).clicked() {
            actions.push(Action::HideToTray);
        }
    }

    pub fn visible_playlists(&self) -> Vec<usize> {
        (0..self.library.playlists.len())
            .filter(|&index| match index {
                ALL_SONGS => !self.library.tracks.is_empty(),
                LIKED_SONGS => !self.library.playlists[index].tracks.is_empty(),
                // An album, an artist, or a genre opens from its songs, not from the sidebar.
                _ => self.library.playlists[index].collection.is_none(),
            })
            .collect()
    }

    fn draw_sidebar(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(rect, 0.0, theme::PANEL);
        ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, theme::LINE));

        let heading = spaced_line(ui, "PLAYLISTS", theme::inter_semibold(10.0), theme::MUTED, 120.0, 1.0);
        paint_text(ui, Align2::LEFT_CENTER, pos2(rect.left() + 16.0, rect.top() + 24.0), heading);
        let add = at(rect.min, 165.0, 12.0, 24.0, 24.0);
        let open = add.translate(vec2(-28.0, 0.0));
        let button = |icon| IconButton { icon, icon_size: 14.0, color: theme::SOFT, radius: 6.0 };
        if button(theme::FOLDER).show(ui, open, "Open music folder").clicked() {
            actions.push(Action::OpenFolder);
        }
        if button(theme::PLUS).show(ui, add, "New playlist").clicked() {
            actions.push(Action::NewPlaylist);
        }

        let list = Rect::from_min_max(
            pos2(rect.left() + 8.0, rect.top() + 44.0),
            pos2(rect.right() - 9.0, rect.bottom() - 12.0),
        );
        let playlists = self.visible_playlists();
        ui.scope_builder(UiBuilder::new().id_salt("sidebar").max_rect(list), |ui| {
            ui.set_clip_rect(list);
            ScrollArea::vertical()
                .id_salt("sidebar-scroll")
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    let total = playlists.len() as f32 * NAV_STEP;
                    ui.set_min_size(vec2(list.width(), total));
                    for (row, &index) in playlists.iter().enumerate() {
                        let top = row as f32 * NAV_STEP;
                        if top + NAV_STEP < viewport.top() || top > viewport.bottom() {
                            continue;
                        }
                        self.draw_playlist_item(ui, at(origin, 0.0, top, list.width(), 44.0), index, actions);
                    }
                });
        });
    }

    fn draw_playlist_item(&mut self, ui: &mut Ui, rect: Rect, index: usize, actions: &mut Vec<Action>) {
        let active = self.minimized && self.view == index;
        let is_playing = self.is_playing() && self.queue.as_ref().is_some_and(|queue| queue.playlist == Some(index));
        let name = self.library.playlists[index].name.clone();
        let response = click_area(ui, rect, &name);
        if active {
            ui.painter().rect_filled(rect, 8.0, theme::NAV_ACTIVE);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 8.0, theme::NAV_HOVER);
        }
        if response.clicked() {
            actions.push(Action::OpenPlaylist(index));
        }

        let center_y = rect.center().y;
        let cover = Rect::from_center_size(pos2(rect.left() + 22.0, center_y), Vec2::splat(28.0));
        let first_track = self.library.playlists[index].tracks.first().copied();
        match first_track {
            Some(track) => {
                let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
                paint_texture(ui, cover, texture, 6);
            }
            None => paint_empty_cover(ui, cover, 6.0),
        }

        let text_left = cover.right() + 10.0;
        let reserved = if is_playing { 20.0 } else { 0.0 };
        let width = (rect.right() - 8.0 - text_left - reserved).max(0.0);
        let color = if active { theme::ACCENT } else { theme::TEXT };
        let count = self.library.playlists[index].tracks.len();
        let title = line(ui, &name, theme::inter_semibold(13.0), color, width);
        let meta = line(ui, &plural_songs(count), theme::inter(11.0), theme::MUTED, width);
        paint_two_lines(ui, text_left, center_y, 1.0, title, meta);
        if is_playing {
            paint_equalizer(ui, rect.right() - 18.0, center_y, true, theme::ACCENT);
        }
    }

    fn draw_stage(&mut self, ui: &mut Ui, stage: Rect, dt: f32, actions: &mut Vec<Action>) {
        ui.scope_builder(UiBuilder::new().id_salt("stage").max_rect(stage), |ui| {
            ui.set_clip_rect(stage);

            if self.slide < 1.0 {
                ui.scope_builder(UiBuilder::new().id_salt("list").max_rect(stage), |ui| {
                    if self.slide > 0.0 {
                        ui.disable();
                        ui.style_mut().visuals.disabled_alpha = 1.0;
                    }
                    if self.full && self.spotify_open {
                        self.draw_spotify(ui, stage, actions);
                    } else if self.full {
                        self.draw_full_list(ui, stage, actions);
                    } else {
                        self.draw_list_section(ui, stage, actions);
                    }
                });
            }

            if self.slide > 0.0 {
                let offset = (1.0 - self.slide) * stage.height();
                let panel = stage.translate(vec2(0.0, offset));
                let builder = UiBuilder::new().id_salt("now-playing").max_rect(panel);
                ui.scope_builder(builder, |ui| {
                    ui.multiply_opacity((self.slide / 0.6).min(1.0));
                    if self.full {
                        self.draw_full_now_playing(ui, panel, dt, actions);
                    } else {
                        self.draw_now_playing(ui, panel, dt, actions);
                    }
                });
            }

            if self.mini > 0.0 {
                let (height, margin) = if self.full { (ui_full::MINI_HEIGHT, 16.0) } else { (MINI_HEIGHT, 10.0) };
                let offset = (1.0 - self.mini) * height * 1.2;
                let rect = Rect::from_min_max(
                    pos2(stage.left() + margin, stage.bottom() - margin - height + offset),
                    pos2(stage.right() - margin, stage.bottom() - margin + offset),
                );
                let builder = UiBuilder::new().id_salt("mini").max_rect(rect);
                ui.scope_builder(builder, |ui| {
                    if self.full {
                        self.draw_full_mini(ui, rect, actions);
                    } else {
                        self.draw_mini(ui, rect, actions);
                    }
                });
            }
        });
    }

    pub fn draw_empty_state(&self, ui: &mut Ui, stage: Rect, actions: &mut Vec<Action>) {
        let center = stage.center();
        let icon = Rect::from_center_size(center - vec2(0.0, 58.0), Vec2::splat(28.0));
        paint_icon(ui, icon, theme::NOTE, theme::SOFT);
        let (title, subtitle) = if self.loading {
            ("Scanning your music…", "This takes a moment on a large folder.")
        } else {
            ("No music yet", "Open a folder and amplo will find the songs inside.")
        };
        let heading = line(ui, title, theme::grotesk_semibold(16.0), theme::TEXT, stage.width() - 40.0);
        paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 28.0), heading);
        let details = line(ui, subtitle, theme::inter(12.0), theme::SOFT, stage.width() - 40.0);
        paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 6.0), details);

        if self.loading {
            return;
        }
        let button = Rect::from_center_size(center + vec2(0.0, 32.0), vec2(116.0, 36.0));
        let response = click_area(ui, button, "Open folder");
        let fill = if response.hovered() { theme::ACCENT_HOVER } else { theme::ACCENT };
        ui.painter().rect_filled(button, 18.0, fill);
        let label = line(ui, "Open folder", theme::inter_semibold(13.0), theme::ON_ACCENT, button.width());
        paint_text(ui, Align2::CENTER_CENTER, button.center(), label);
        if response.clicked() {
            actions.push(Action::OpenFolder);
        }
    }

    fn draw_list_section(&mut self, ui: &mut Ui, stage: Rect, actions: &mut Vec<Action>) {
        let Some(playlist) = self.library.playlists.get(self.view).cloned() else {
            self.draw_empty_state(ui, stage, actions);
            return;
        };
        if self.library.tracks.is_empty() {
            self.draw_empty_state(ui, stage, actions);
            return;
        }
        if playlist.tracks.is_empty() {
            draw_empty_playlist(ui, stage, &playlist.name);
            return;
        }

        let rows = playlist.tracks.len();
        let total = ROWS_TOP + rows as f32 * ROW_HEIGHT + LIST_BOTTOM_SPACE;
        ScrollArea::vertical()
            .id_salt(("list-scroll", &playlist.name))
            .auto_shrink(false)
            .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
            .show_viewport(ui, |ui, viewport| {
                let origin = ui.max_rect().min;
                ui.set_min_size(vec2(stage.width(), total));
                if viewport.top() < ROWS_TOP {
                    self.draw_list_header(ui, origin, &playlist, actions);
                }
                let first = ((viewport.top() - ROWS_TOP) / ROW_HEIGHT).floor().max(0.0) as usize;
                let last = (((viewport.bottom() - ROWS_TOP) / ROW_HEIGHT).ceil().max(0.0) as usize).min(rows);
                for row in first..last {
                    let rect = at(origin, 14.0, ROWS_TOP + row as f32 * ROW_HEIGHT, stage.width() - 28.0, ROW_HEIGHT);
                    self.draw_row(ui, rect, row, playlist.tracks[row], actions);
                }
            });
    }

    fn draw_list_header(&mut self, ui: &mut Ui, origin: Pos2, playlist: &Playlist, actions: &mut Vec<Action>) {
        // Four covers in a square with rounded outer corners. Short playlists repeat theirs.
        for index in 0..4 {
            let track = playlist.tracks[index % playlist.tracks.len()];
            let x = 18.0 + (index % 2) as f32 * 22.0;
            let y = 14.0 + (index / 2) as f32 * 22.0;
            let radius = match index {
                0 => CornerRadius { nw: 8, ..CornerRadius::ZERO },
                1 => CornerRadius { ne: 8, ..CornerRadius::ZERO },
                2 => CornerRadius { sw: 8, ..CornerRadius::ZERO },
                _ => CornerRadius { se: 8, ..CornerRadius::ZERO },
            };
            let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
            paint_texture(ui, at(origin, x, y, 22.0, 22.0), texture, radius);
        }

        let seconds: f32 = playlist
            .tracks
            .iter()
            .filter_map(|&track| self.library.tracks[track].info.duration)
            .map(|duration| duration.as_secs_f32())
            .sum();
        let songs = plural_songs(playlist.tracks.len());
        let meta = format!("{songs} · {} min", (seconds / 60.0) as u32);
        let section_width = ui.max_rect().width();
        let play = at(origin, section_width - 54.0, 18.0, 36.0, 36.0);
        let random = at(origin, section_width - 98.0, 18.0, 36.0, 36.0);
        let left = origin.x + 74.0;
        let mut text_right = random.left();
        // An icon here, as the window is small. The menu marks the order chosen.
        if self.view_is_sorted() {
            let sort = at(origin, section_width - 142.0, 18.0, 36.0, 36.0);
            let label = format!("Sort: {}", self.library.sort.label());
            let response = IconButton::round(theme::SORT, 16.0, theme::TEXT).show(ui, sort, &label);
            self.sort_menu(&response, actions);
            text_right = sort.left();
        }
        let width = text_right - 12.0 - left;
        let title = spaced_line(ui, &playlist.name, theme::grotesk_bold(18.0), theme::TEXT, width, -0.18);
        let subtitle = line(ui, &meta, theme::inter(12.0), theme::SOFT, width);
        paint_two_lines(ui, left, origin.y + 36.0, 2.0, title, subtitle);

        let shuffle = IconButton::round(theme::SHUFFLE, 16.0, theme::TEXT);
        if shuffle.show(ui, random, "Shuffle play").clicked() {
            actions.push(Action::ShufflePlayView);
        }
        if accent_button(ui, play, theme::PLAY, 16.0, "Play playlist").clicked() {
            actions.push(Action::PlayView);
        }
        let border = origin.y + 70.5;
        let stroke = Stroke::new(1.0, theme::LINE);
        ui.painter().hline(origin.x + 14.0..=origin.x + section_width - 14.0, border, stroke);
    }

    /// Whether the shown list follows the sort menu. Albums keep their own order.
    pub fn view_is_sorted(&self) -> bool {
        self.library.playlists.get(self.view).is_some_and(Playlist::follows_sort)
    }

    /// The order of the song lists, ending at `right_center`. A click lists the other
    /// orders. Returns where the button is.
    pub fn draw_sort_button(&self, ui: &Ui, right_center: Pos2, height: f32, actions: &mut Vec<Action>) -> Rect {
        let sort = self.library.sort;
        let mut job = LayoutJob::default();
        job.append("Sort by ", 0.0, TextFormat { font_id: theme::inter(13.0), color: theme::SOFT, ..TextFormat::default() });
        job.append(sort.label(), 0.0, TextFormat { font_id: theme::inter(13.0), color: theme::TEXT, ..TextFormat::default() });
        let text = ui.painter().layout_job(job);
        const CHEVRON: f32 = 14.0;
        let width = 14.0 + text.size().x + 6.0 + CHEVRON + 10.0;
        let rect = Rect::from_min_size(pos2(right_center.x - width, right_center.y - height / 2.0), vec2(width, height));
        let response = click_area(ui, rect, &format!("Sort: {}", sort.label()));
        let fill = if response.hovered() { theme::ICON_HOVER } else { Color32::TRANSPARENT };
        ui.painter().rect(rect, height / 2.0, fill, Stroke::new(1.0, theme::PILL_LINE), egui::StrokeKind::Inside);
        let text_width = text.size().x;
        paint_text(ui, Align2::LEFT_CENTER, pos2(rect.left() + 14.0, rect.center().y), text);
        let chevron = Rect::from_center_size(
            pos2(rect.left() + 14.0 + text_width + 6.0 + CHEVRON / 2.0, rect.center().y),
            Vec2::splat(CHEVRON),
        );
        paint_icon(ui, chevron, theme::CHEVRON_DOWN, theme::CHEVRON);
        self.sort_menu(&response, actions);
        rect
    }

    /// The menu of orders, which opens from a click on `button`. The chosen one is marked.
    fn sort_menu(&self, button: &egui::Response, actions: &mut Vec<Action>) {
        let sort = self.library.sort;
        egui::Popup::menu(button).show(|ui| {
            roomy_menu(ui);
            for choice in Sort::ALL {
                if ui.add(egui::Button::selectable(choice == sort, choice.label())).clicked() {
                    actions.push(Action::SetSort(choice));
                }
            }
        });
    }

    /// Clicks on a song. A plain click plays it, Ctrl and Shift select as in a file manager,
    /// and a right click opens the menu for the selected songs.
    pub fn song_row_input(&self, ui: &Ui, response: &egui::Response, row: usize, actions: &mut Vec<Action>) {
        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            if modifiers.shift {
                actions.push(Action::Select(row, Selection::Range));
            } else if modifiers.command {
                actions.push(Action::Select(row, Selection::Toggle));
            } else {
                actions.push(Action::PlayRow(row));
            }
        }
        // A right click outside the selection acts on that song alone, as in Spotify.
        if response.secondary_clicked() && !self.selected.contains(&row) {
            actions.push(Action::Select(row, Selection::Only));
        }
        response.context_menu(|ui| self.song_menu(ui, row, actions));
    }

    fn song_menu(&self, ui: &mut Ui, row: usize, actions: &mut Vec<Action>) {
        roomy_menu(ui);
        let count = if self.selected.contains(&row) { self.selected.len() } else { 1 };
        let label = if count > 1 { format!("Add {count} songs to playlist") } else { "Add to playlist".to_owned() };
        ui.menu_button(label, |ui| {
            roomy_menu(ui);
            if ui.button("New playlist…").clicked() {
                actions.push(Action::NewPlaylistFromSelection);
            }
            let playlists: Vec<(usize, &Playlist)> =
                self.library.playlists.iter().enumerate().filter(|(_, playlist)| playlist.path.is_some()).collect();
            if playlists.is_empty() {
                return;
            }
            ui.separator();
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (index, playlist) in playlists {
                    if ui.button(&playlist.name).clicked() {
                        actions.push(Action::AddToPlaylist(index));
                    }
                }
            });
        });
        let Some(playlist) = self.library.playlists.get(self.view) else { return };
        // Only for one song, as the selection can span albums.
        if count == 1
            && let Some(&track) = playlist.tracks.get(row)
            && let Some(album) = Collection::album_of(&self.library.tracks[track].info)
            && playlist.collection.as_ref() != Some(&album)
            && ui.button("Go to album").clicked()
        {
            actions.push(Action::OpenCollection(album));
        }
        // All Songs shows the folder, so it has nothing to remove from.
        if self.view == LIKED_SONGS || playlist.path.is_some() {
            let label = if count > 1 {
                format!("Remove {count} songs from {}", playlist.name)
            } else {
                format!("Remove from {}", playlist.name)
            };
            if ui.button(label).clicked() {
                actions.push(Action::RemoveFromPlaylist);
            }
        }
    }

    fn draw_row(&self, ui: &mut Ui, rect: Rect, row: usize, track: usize, actions: &mut Vec<Action>) {
        let info = &self.library.tracks[track].info;
        let is_current = self.current_track() == Some(track)
            && self.queue.as_ref().is_some_and(|queue| queue.playlist == Some(self.view));
        let number = row + 1;
        let response = click_area(ui, rect, &format!("{number}. {}", info.title));
        if self.selected.contains(&row) {
            ui.painter().rect_filled(rect, 8.0, theme::ROW_SELECTED);
        } else if is_current {
            ui.painter().rect_filled(rect, 8.0, theme::ROW_CURRENT);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 8.0, theme::ROW_HOVER);
        }
        self.song_row_input(ui, &response, row, actions);

        let center_y = rect.center().y;
        let number_left = rect.left() + 8.0;
        if is_current {
            paint_equalizer(ui, number_left, center_y, self.is_playing(), theme::ACCENT);
        } else {
            let text = line(ui, &format!("{number:02}"), theme::mono(12.0), theme::MUTED, f32::INFINITY);
            paint_text(ui, Align2::LEFT_CENTER, pos2(number_left, center_y), text);
        }

        let title_left = number_left + 36.0;
        let time_right = rect.right() - 8.0;
        let title_width = time_right - 54.0 - title_left;
        let color = if is_current { theme::ACCENT } else { theme::TEXT };
        let mut job = LayoutJob::default();
        job.append(
            &info.title,
            0.0,
            TextFormat { font_id: theme::inter_semibold(13.0), color, ..TextFormat::default() },
        );
        if !info.artist.is_empty() {
            let format = TextFormat { font_id: theme::inter(13.0), color: theme::SOFT, ..TextFormat::default() };
            job.append(&format!(" · {}", info.artist), 0.0, format);
        }
        job.wrap = TextWrapping::truncate_at_width(title_width);
        let title = ui.painter().layout_job(job);
        let origin = Align2::LEFT_CENTER.anchor_size(pos2(title_left, center_y), title.size()).min;
        // The artists come after the title and " · ".
        let first_char = info.title.chars().count() + 3;
        row_artist_links(ui, row, &title, origin, first_char, &info.artist, actions);
        paint_text(ui, Align2::LEFT_CENTER, pos2(title_left, center_y), title);

        let time = info
            .duration
            .map_or_else(|| UNKNOWN_TIME.to_owned(), |duration| format_time(duration.as_secs_f32()));
        let time = line(ui, &time, theme::mono(12.0), theme::SOFT, f32::INFINITY);
        paint_text(ui, Align2::RIGHT_CENTER, pos2(time_right, center_y), time);
    }

    fn draw_now_playing(&mut self, ui: &mut Ui, panel: Rect, dt: f32, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(panel, 0.0, theme::BACKGROUND);
        // Stops the track list underneath from reacting while this panel slides over it.
        ui.interact(panel, ui.scope_id().with("cover-list"), Sense::click());
        self.update_visualizer(dt);

        let Some(track) = self.current_track() else { return };
        let content = panel.shrink(16.0);
        let column = (content.width() - 14.0) / 2.0;
        let transport_top = content.bottom() - 77.0;
        let cards_bottom = transport_top - 12.0;
        let left = content.left();
        let top = content.top();

        let cover = Rect::from_min_size(content.min, Vec2::splat(150.0));
        paint_shadow(ui, cover, 12, 14, 34, 128);
        let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
        paint_texture(ui, cover, texture, 12);

        let info_right = left + column;
        let minimize = Rect::from_min_size(pos2(info_right - 32.0, top), Vec2::splat(32.0));
        let button = IconButton::round(theme::CHEVRON_DOWN, 16.0, theme::CHEVRON);
        if button.show(ui, minimize, "Back to song list").clicked() {
            actions.push(Action::Minimize);
        }

        let info_bottom = cover.bottom() - 2.0;
        let mut y = info_bottom;
        for tech in self.tech.iter().rev() {
            let text = line(ui, tech, theme::mono(10.0), theme::MUTED, 110.0);
            paint_text(ui, Align2::RIGHT_BOTTOM, pos2(info_right, y), text);
            y -= 15.0;
        }
        let time = format_time(self.position());
        let time = spaced_line(ui, &time, theme::mono_semibold(26.0), theme::ACCENT, f32::INFINITY, -0.52);
        paint_text(ui, Align2::RIGHT_CENTER, pos2(info_right, info_bottom - 45.0 - 4.0 - 13.0), time);

        let elapsed = ui.input(|input| input.time) - self.marquee_started;
        let info = &self.library.tracks[track].info;
        let title_top = cover.bottom() + 10.0;
        let text = |text, font, color| MarqueeText { text, font, color, background: theme::BACKGROUND };
        let title = text(&info.title, theme::grotesk_bold(19.0), theme::TEXT);
        let title_height = marquee(ui, pos2(left, title_top), column, title, elapsed);
        let artist = text(artist_name(&info.artist), theme::inter(13.0), theme::SOFT);
        let artist_top = title_top + title_height + 2.0;
        let artist_height = artist_links(ui, pos2(left, artist_top), column, artist, elapsed, actions);

        let bars_top = artist_top + artist_height + 10.0;
        self.draw_visualizer(ui, Rect::from_min_size(pos2(left, bars_top), vec2(column, 26.0)), 2.0);

        let lyrics = Rect::from_min_max(pos2(left + column + 14.0, top), pos2(content.right(), cards_bottom));
        self.draw_card(ui, lyrics, dt, actions);
        self.draw_transport(ui, content, transport_top, track, actions);
    }

    pub fn draw_visualizer(&self, ui: &Ui, rect: Rect, gap: f32) {
        let count = VISUALIZER_BARS as f32;
        let width = (rect.width() - gap * (count - 1.0)) / count;
        let color = theme::ACCENT.gamma_multiply(0.85);
        for (index, &level) in self.analyzer.bars().iter().enumerate() {
            let height = 2.0 + level * (rect.height() - 2.0);
            let x = rect.left() + index as f32 * (width + gap);
            let bar = Rect::from_min_max(pos2(x, rect.bottom() - height), pos2(x + width, rect.bottom()));
            ui.painter().rect_filled(bar, 1.0, color);
        }
    }

    /// The Lyrics and Queue tabs of the card beside the cover, starting at `left` and `top`.
    pub fn draw_card_tabs(&self, ui: &Ui, left: f32, top: f32, font: egui::FontId, uppercase: bool, actions: &mut Vec<Action>) {
        let mut tab_left = left;
        for (tab, label) in [(CardTab::Lyrics, "Lyrics"), (CardTab::Queue, "Queue")] {
            let title = if uppercase { label.to_uppercase() } else { label.to_owned() };
            let spacing = if uppercase { 1.0 } else { 0.0 };
            let galley = spaced_line(ui, &title, font.clone(), Color32::PLACEHOLDER, 200.0, spacing);
            let text = Rect::from_min_size(pos2(tab_left, top), galley.size());
            let response = click_area(ui, text.expand2(vec2(5.0, 7.0)), label);
            let active = self.card_tab == tab;
            let color = if active {
                theme::TEXT
            } else if response.hovered() {
                theme::SOFT
            } else {
                theme::MUTED
            };
            ui.painter().galley(text.min, galley, color);
            if active {
                let line = Rect::from_x_y_ranges(text.x_range(), text.bottom() + 2.0..=text.bottom() + 4.0);
                ui.painter().rect_filled(line, 1.0, theme::ACCENT);
            }
            if response.clicked() {
                actions.push(Action::ShowCardTab(tab));
            }
            tab_left = text.right() + font.size * 1.4;
        }
    }

    fn update_lyric_motion(&mut self, height: f32, slots: &[f32], dt: f32) {
        let position = std::time::Duration::from_secs_f32(self.position());
        let active = self.lyrics.as_ref().and_then(|lyrics| lyrics.active_line(position));
        let motion = &mut self.lyric_motion;
        if active != motion.active {
            motion.previous = motion.active;
            motion.active = active;
            motion.mix = 0.0;
        }
        ease(&mut motion.mix, 1.0, 5.0, dt);

        let centered = active.unwrap_or(0);
        let above: f32 = slots[..centered].iter().sum();
        let target = height / 2.0 - above - slots[centered] / 2.0;
        let scroll = motion.scroll.get_or_insert(target);
        glide(scroll, &mut motion.scroll_velocity, target, dt);
        motion.moving = motion.mix < 1.0 || *scroll != target;
    }

    /// The card beside the cover, with its tabs for lyrics and the queue.
    fn draw_card(&mut self, ui: &mut Ui, card: Rect, dt: f32, actions: &mut Vec<Action>) {
        ui.painter().rect(card, 12.0, theme::PANEL, Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
        let inner = card.shrink2(vec2(17.0, 15.0));
        self.draw_card_tabs(ui, inner.left(), inner.top(), theme::inter_semibold(10.0), true, actions);

        match self.card_tab {
            CardTab::Lyrics => {
                let mut links_right = inner.right();
                // The card is narrow, so the badge gives its room to the romaji switch.
                let has_extras = self.lyric_extra_choices().len() > 1;
                if !has_extras && self.lyrics.as_ref().is_some_and(|lyrics| lyrics.synced) {
                    let badge = spaced_line(ui, "SYNCED", theme::inter_semibold(10.0), theme::ACCENT, 100.0, 0.6);
                    links_right -= badge.size().x + 12.0;
                    paint_text(ui, Align2::RIGHT_TOP, inner.right_top(), badge);
                }
                if self.lyrics.is_some() {
                    self.draw_lyrics_links(ui, links_right, inner.top() + 6.0, theme::inter_semibold(10.0), true, actions);
                }
                let view = Rect::from_min_max(pos2(inner.left(), inner.top() + 20.0), inner.max);
                self.draw_lyrics(ui, view, dt, &COMPACT_LYRICS, actions);
            }
            CardTab::Queue => {
                // The lyrics do not animate while hidden, so nothing is left to wait for.
                self.lyric_motion.moving = false;
                let view = Rect::from_min_max(
                    pos2(card.left() + 8.0, inner.top() + 22.0),
                    pos2(card.right() - 8.0, card.bottom() - 8.0),
                );
                self.draw_queue(ui, view, actions);
            }
        }
    }

    pub fn draw_queue(&mut self, ui: &mut Ui, view: Rect, actions: &mut Vec<Action>) {
        let Some(queue) = &self.queue else { return };
        let (position, count) = (queue.position, queue.tracks.len());
        let source = queue.playlist.and_then(|index| self.library.playlists.get(index));
        let next_heading = source.map_or_else(|| "Next in queue".to_owned(), |playlist| format!("Next from: {}", playlist.name));
        let next_top = QUEUE_HEADING + QUEUE_ROW + 8.0 + QUEUE_HEADING;
        let upcoming = count.saturating_sub(1);
        let total = next_top + upcoming as f32 * QUEUE_ROW + 8.0;

        ui.scope_builder(UiBuilder::new().id_salt("queue").max_rect(view), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(view));
            // A new position starts at the top, where the song that plays now is.
            ScrollArea::vertical()
                .id_salt(("queue-scroll", position))
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    let width = view.width();
                    ui.set_min_size(vec2(width, total));

                    let heading = |ui: &Ui, top: f32, text: &str| {
                        let galley = line(ui, text, theme::grotesk_semibold(13.0), theme::TEXT, width - 18.0);
                        paint_text(ui, Align2::LEFT_CENTER, origin + vec2(9.0, top + QUEUE_HEADING / 2.0), galley);
                    };
                    heading(ui, 0.0, "Now playing");
                    if let Some(current) = self.current_track() {
                        let rect = at(origin, 0.0, QUEUE_HEADING, width, QUEUE_ROW);
                        self.paint_queue_track(ui, rect, current, theme::ACCENT);
                    }
                    if upcoming == 0 {
                        return;
                    }
                    heading(ui, QUEUE_HEADING + QUEUE_ROW + 8.0, &next_heading);

                    let first = ((viewport.top() - next_top) / QUEUE_ROW).floor().max(0.0) as usize;
                    let last = (((viewport.bottom() - next_top) / QUEUE_ROW).ceil().max(0.0) as usize).min(upcoming);
                    for row in first..last {
                        // The queue wraps around, so the songs before this one follow at the end.
                        let target = (position + 1 + row) % count;
                        let Some(track) = self.queue.as_ref().map(|queue| queue.tracks[target]) else { return };
                        let rect = at(origin, 0.0, next_top + row as f32 * QUEUE_ROW, width, QUEUE_ROW);
                        let label = format!("Queue: {}", self.library.tracks[track].info.title);
                        let response = click_area(ui, rect, &label);
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 8.0, theme::NAV_HOVER);
                        }
                        if response.clicked() {
                            actions.push(Action::PlayQueue(target));
                        }
                        self.paint_queue_track(ui, rect, track, theme::TEXT);
                    }
                });
        });
    }

    fn paint_queue_track(&mut self, ui: &Ui, rect: Rect, track: usize, title_color: Color32) {
        let center_y = rect.center().y;
        let cover = Rect::from_center_size(pos2(rect.left() + 27.0, center_y), Vec2::splat(36.0));
        let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
        paint_texture(ui, cover, texture, 7);

        let info = &self.library.tracks[track].info;
        let left = cover.right() + 10.0;
        let width = (rect.right() - 9.0 - left).max(0.0);
        let title = line(ui, &info.title, theme::inter_semibold(13.0), title_color, width);
        let artist = line(ui, &info.artist, theme::inter(11.0), theme::SOFT, width);
        paint_two_lines(ui, left, center_y, 1.0, title, artist);
    }

    /// The lyrics, or a note that there are none, inside `view`.
    pub fn draw_lyrics(&mut self, ui: &mut Ui, view: Rect, dt: f32, style: &LyricStyle, actions: &mut Vec<Action>) {
        if ui.input(|input| !input.raw.hovered_files.is_empty()) {
            paint_drop_target(ui, view, style);
            return;
        }
        let Some(lyrics) = &self.lyrics else {
            // The text block, the buttons, and the hint below them, centered as one.
            let center = view.center() - vec2(0.0, 10.0 + style.empty_title * 1.6);
            let icon = Rect::from_center_size(center - vec2(0.0, style.empty_icon + 8.0), Vec2::splat(style.empty_icon));
            paint_icon(ui, icon, theme::NOTE, theme::SOFT);
            let title =
                line(ui, "Lyrics not available", theme::grotesk_semibold(style.empty_title), theme::TEXT, view.width());
            paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 4.0), title);
            let mut job = LayoutJob::simple(
                "Lyrics for this song haven’t been added yet.".to_owned(),
                theme::inter(style.empty_text),
                theme::SOFT,
                view.width(),
            );
            job.halign = egui::Align::Center;
            let sub = ui.painter().layout_job(job);
            let sub_top = center.y + style.empty_title * 0.75;
            let buttons_top = sub_top + sub.size().y + style.empty_title;
            // Centered text is laid out around its position, so no anchor is needed.
            ui.painter().galley(pos2(center.x, sub_top), sub, Color32::PLACEHOLDER);

            let height = (style.empty_title * 2.0).max(30.0);
            let (paste, write) = ("Paste lyrics", "Write lyrics");
            let (paste_width, write_width) = (button_width(ui, paste), button_width(ui, write));
            let left = center.x - (paste_width + 10.0 + write_width) / 2.0;
            let paste_rect = Rect::from_min_size(pos2(left, buttons_top), vec2(paste_width, height));
            let write_rect = Rect::from_min_size(pos2(paste_rect.right() + 10.0, buttons_top), vec2(write_width, height));
            if text_button(ui, paste_rect, paste, true).on_hover_text("Saves the copied lyrics as an .lrc file").clicked() {
                actions.push(Action::PasteLyrics);
            }
            if text_button(ui, write_rect, write, false).clicked() {
                actions.push(Action::EditLyrics);
            }
            let hint = line(ui, "or drop an .lrc or .txt file here", theme::inter(style.empty_text), theme::MUTED, view.width());
            paint_text(ui, Align2::CENTER_TOP, pos2(center.x, paste_rect.bottom() + 10.0), hint);
            return;
        };
        if lyrics.synced {
            self.draw_synced_lines(ui, view, dt, style);
        } else {
            self.draw_plain_lines(ui, view, style);
        }
        paint_edge_fades(ui, view, view.height() * 0.14, theme::PANEL);
    }

    fn draw_synced_lines(&mut self, ui: &mut Ui, view: Rect, dt: f32, style: &LyricStyle) {
        let extra = self.lyric_extra();
        let Some(lyrics) = &self.lyrics else { return };
        // Laid out without a color, so a color fade does not lay the text out again.
        let galleys: Vec<_> = lyrics
            .lines
            .iter()
            .map(|line| {
                let font = theme::grotesk_semibold(style.font_size);
                let text = wrapped(ui, &line.text, font, Color32::PLACEHOLDER, view.width(), style.line_height);
                let under = extra.and_then(|extra| line.extra(extra)).map(|under| {
                    let size = extra_font_size(style.font_size);
                    wrapped(ui, under, theme::inter(size), Color32::PLACEHOLDER, view.width(), size * 1.3)
                });
                (text, under)
            })
            .collect();
        let slots: Vec<f32> = galleys
            .iter()
            .map(|(text, under)| {
                let under = under.as_ref().map_or(0.0, |under| EXTRA_GAP + under.size().y);
                lyric_slot_height(text.size().y + under, style.min_slot)
            })
            .collect();
        self.update_lyric_motion(view.height(), &slots, dt);

        let motion = &self.lyric_motion;
        let scroll = motion.scroll.unwrap_or(0.0);
        let active = motion.active.map_or(-1, |index| index as i64);

        let clip = ui.clip_rect().intersect(view);
        let mut text_ui = ui.new_child(UiBuilder::new().max_rect(view));
        text_ui.set_clip_rect(clip);
        let mut top = view.top() + scroll;
        for (index, ((galley, under), &slot)) in galleys.into_iter().zip(&slots).enumerate() {
            let center_y = top + slot / 2.0;
            top += slot;
            if top < view.top() || top - slot > view.bottom() {
                continue;
            }
            let highlight = if motion.active == Some(index) {
                motion.mix
            } else if motion.previous == Some(index) {
                1.0 - motion.mix
            } else {
                0.0
            };
            let rest_alpha = if (index as i64) < active { 0.45 } else { 0.8 };
            let color = theme::SOFT.gamma_multiply(rest_alpha).lerp_to_gamma(theme::TEXT, highlight);
            let under_height = under.as_ref().map_or(0.0, |under| EXTRA_GAP + under.size().y);
            let size = galley.size() + vec2(0.0, under_height);
            let rect = Align2::LEFT_CENTER.anchor_size(pos2(view.left(), center_y), size);
            let under_top = rect.top() + galley.size().y + EXTRA_GAP;
            text_ui.painter().galley(rect.min, galley, color);
            if let Some(under) = under {
                let under_color = theme::SOFT.gamma_multiply(rest_alpha * 0.85).lerp_to_gamma(theme::ACCENT, highlight * 0.8);
                text_ui.painter().galley(pos2(rect.left(), under_top), under, under_color);
            }
        }
    }

    fn draw_plain_lines(&self, ui: &mut Ui, view: Rect, style: &LyricStyle) {
        let extra = self.lyric_extra();
        let Some(lyrics) = &self.lyrics else { return };
        let size = style.plain_font_size;
        ui.scope_builder(UiBuilder::new().id_salt("plain-lyrics").max_rect(view), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(view));
            ScrollArea::vertical().id_salt("plain-lyrics-scroll").auto_shrink(false).show(ui, |ui| {
                ui.add_space(view.height() * 0.14);
                for text in &lyrics.lines {
                    let font = theme::grotesk_semibold(size);
                    let galley = wrapped(ui, &text.text, font, theme::SOFT, view.width() - 8.0, size * 1.25);
                    ui.add(egui::Label::new(galley));
                    if let Some(under) = extra.and_then(|extra| text.extra(extra)) {
                        let under_size = extra_font_size(size);
                        let color = theme::SOFT.gamma_multiply(0.7);
                        let galley = wrapped(ui, under, theme::inter(under_size), color, view.width() - 8.0, under_size * 1.3);
                        ui.add_space(EXTRA_GAP - ui.spacing().item_spacing.y);
                        ui.add(egui::Label::new(galley));
                    }
                    ui.add_space(6.0);
                }
                ui.add_space(view.height() * 0.14);
            });
        });
    }

    fn draw_transport(&mut self, ui: &mut Ui, content: Rect, top: f32, track: usize, actions: &mut Vec<Action>) {
        let left = content.left();
        let right = content.right();
        ui.painter().hline(left..=right, top + 0.5, Stroke::new(1.0, theme::LINE));

        let seek_y = top + 19.0;
        let time = line(ui, &self.total_time(), theme::mono(11.0), theme::SOFT, f32::INFINITY);
        let time_left = right - time.size().x;
        paint_text(ui, Align2::RIGHT_CENTER, pos2(right, seek_y), time);

        let seek = Rect::from_min_max(pos2(left, seek_y - 8.0), pos2(time_left - 10.0, seek_y + 8.0));
        self.seek_slider(ui, seek, actions);

        let center_y = top + 55.0;
        let center_x = content.center().x;
        let button_at = |x: f32, size: f32| Rect::from_center_size(pos2(x, center_y), Vec2::splat(size));
        let toggle_color = |on: bool| if on { theme::ACCENT } else { theme::SOFT };

        let liked = self.is_liked(track);
        let (heart, heart_color) = if liked {
            (theme::HEART_FILLED, theme::ACCENT)
        } else {
            (theme::HEART, theme::SOFT)
        };
        let like_label = if liked { "Unlike" } else { "Like" };
        if IconButton::round(heart, 17.0, heart_color).show(ui, button_at(left + 18.0, 36.0), like_label).clicked() {
            actions.push(Action::ToggleLike);
        }

        let shuffle = IconButton::round(theme::SHUFFLE, 16.0, toggle_color(self.shuffle));
        if shuffle.show(ui, button_at(center_x - 92.0, 36.0), "Shuffle").clicked() {
            actions.push(Action::ToggleShuffle);
        }
        if IconButton::round(theme::PREVIOUS, 18.0, theme::TEXT).show(ui, button_at(center_x - 48.0, 40.0), "Previous").clicked() {
            actions.push(Action::Previous);
        }
        let (icon, size, label) = if self.is_playing() {
            (theme::PAUSE, 18.0, "Pause")
        } else {
            (theme::PLAY, 20.0, "Play")
        };
        if accent_button(ui, button_at(center_x, 44.0), icon, size, label).clicked() {
            actions.push(Action::Toggle);
        }
        if IconButton::round(theme::NEXT, 18.0, theme::TEXT).show(ui, button_at(center_x + 48.0, 40.0), "Next").clicked() {
            actions.push(Action::Next);
        }
        let repeat = IconButton::round(theme::REPEAT, 16.0, toggle_color(self.repeat));
        if repeat.show(ui, button_at(center_x + 92.0, 36.0), "Repeat").clicked() {
            actions.push(Action::ToggleRepeat);
        }

        let volume = Rect::from_min_max(pos2(right - 110.0, center_y - 8.0), pos2(right, center_y + 8.0));
        let icon = Rect::from_center_size(pos2(volume.left() - 16.0, center_y), Vec2::splat(16.0));
        paint_icon(ui, icon, theme::VOLUME, theme::SOFT);
        self.volume_slider(ui, volume, actions);
    }

    /// The length of the playing track, as the end of the seek bar shows it.
    pub fn total_time(&self) -> String {
        match self.duration {
            Some(duration) if !duration.is_zero() => format_time(duration.as_secs_f32()),
            _ => UNKNOWN_TIME.to_owned(),
        }
    }

    /// Moves through the track. The seek happens when the slider is let go.
    pub fn seek_slider(&mut self, ui: &Ui, rect: Rect, actions: &mut Vec<Action>) {
        let total = self.duration.map_or(0.0, |duration| duration.as_secs_f32());
        let mut fraction = if total > 0.0 { self.position() / total } else { 0.0 };
        let response = slider(ui, rect, "Position", &mut fraction);
        if total > 0.0 {
            if response.changed() {
                self.seek_preview = Some(fraction * total);
            }
            if !response.is_pointer_button_down_on()
                && let Some(target) = self.seek_preview.take()
            {
                actions.push(Action::Seek(target));
            }
        }
    }

    pub fn volume_slider(&mut self, ui: &Ui, rect: Rect, actions: &mut Vec<Action>) {
        let mut level = self.settings.volume;
        let response = slider(ui, rect, "Volume", &mut level);
        if response.changed() {
            self.set_volume(level);
        }
        if response.drag_stopped() || response.clicked() {
            actions.push(Action::SaveSettings);
        }
    }

    fn draw_mini(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let Some(track) = self.current_track() else { return };
        // Stops the rows under the card from taking its clicks.
        ui.interact(rect, ui.scope_id().with("cover-rows"), Sense::click());
        paint_shadow(ui, rect, 12, 12, 30, 128);
        ui.painter().rect(rect, 12.0, theme::MINI, Stroke::new(1.0, theme::MINI_LINE), egui::StrokeKind::Inside);

        // The bar stops where the card's rounded corners would cut it off.
        let bar_left = rect.left() + 7.0;
        let bar_right = rect.right() - 7.0;
        let bar_y = rect.top() + 1.0..=rect.top() + 3.0;
        let position = self.position();
        let total = self.duration.map_or(0.0, |duration| duration.as_secs_f32());
        let progress = if total > 0.0 { (position / total).clamp(0.0, 1.0) } else { 0.0 };
        ui.painter().rect_filled(Rect::from_x_y_ranges(bar_left..=bar_right, bar_y.clone()), 1.0, theme::MINI_LINE);
        let filled_right = bar_left + progress * (bar_right - bar_left);
        ui.painter().rect_filled(Rect::from_x_y_ranges(bar_left..=filled_right, bar_y), 1.0, theme::ACCENT);

        let center_y = rect.top() + 3.0 + 7.0 + 18.0;
        let button = |right: f32, size: f32| Rect::from_center_size(pos2(right - size / 2.0, center_y), Vec2::splat(size));

        let chevron = button(rect.right() - 9.0, 32.0);
        let next = button(chevron.left() - 8.0, 32.0);
        let play = button(next.left() - 8.0, 34.0);
        let previous = button(play.left() - 8.0, 32.0);
        let time = line(ui, &format_time(position), theme::mono(11.0), theme::ACCENT, f32::INFINITY);
        let time_left = previous.left() - 8.0 - time.size().x;
        paint_text(ui, Align2::RIGHT_CENTER, pos2(previous.left() - 8.0, center_y), time);

        let info = &self.library.tracks[track].info;
        let expand_area = Rect::from_min_max(pos2(rect.left() + 9.0, center_y - 18.0), pos2(time_left - 8.0, center_y + 18.0));
        let title_label = format!("Open Now Playing: {}", info.title);
        let width = expand_area.width() - 46.0;
        let title = line(ui, &info.title, theme::inter_semibold(13.0), theme::TEXT, width);
        let artist = line(ui, &info.artist, theme::inter(11.0), theme::SOFT, width);

        if click_area(ui, expand_area, &title_label).clicked() {
            actions.push(Action::Expand);
        }
        let path = self.library.tracks[track].path.clone();
        let texture = self.covers.texture(ui.ctx(), &path);
        let cover = Rect::from_min_size(expand_area.min, Vec2::splat(36.0));
        paint_texture(ui, cover, texture, 7);
        paint_two_lines(ui, cover.right() + 10.0, center_y, 1.0, title, artist);

        if IconButton::round(theme::PREVIOUS, 15.0, theme::TEXT).show(ui, previous, "Previous").clicked() {
            actions.push(Action::Previous);
        }
        let (icon, size, label) = if self.is_playing() {
            (theme::PAUSE, 14.0, "Pause")
        } else {
            (theme::PLAY, 15.0, "Play")
        };
        if accent_button(ui, play, icon, size, label).clicked() {
            actions.push(Action::Toggle);
        }
        if IconButton::round(theme::NEXT, 15.0, theme::TEXT).show(ui, next, "Next").clicked() {
            actions.push(Action::Next);
        }
        if IconButton::round(theme::CHEVRON_UP, 16.0, theme::CHEVRON).show(ui, chevron, "Expand").clicked() {
            actions.push(Action::Expand);
        }
    }
}

/// Moves `value` toward `target` with a soft start and a soft end, and without overshoot.
/// An abrupt start is what makes scrolling lyrics tiring to watch. New targets keep the
/// current speed, so lines that follow each other quickly do not jerk.
fn glide(value: &mut f32, velocity: &mut f32, target: f32, dt: f32) {
    let frequency = 2.0 / LYRIC_GLIDE_TIME;
    let step = frequency * dt;
    let decay = 1.0 / (1.0 + step + 0.48 * step * step + 0.235 * step * step * step);
    let offset = *value - target;
    let change = (*velocity + frequency * offset) * dt;
    *velocity = (*velocity - frequency * change) * decay;
    *value = target + (offset + change) * decay;
    if (*value - target).abs() < 0.05 && velocity.abs() < 1.0 {
        *value = target;
        *velocity = 0.0;
    }
}

/// The height of a lyric line in the list. Long lines get all the rows they need.
/// Space between a lyric line and its romaji or translation.
const EXTRA_GAP: f32 = 3.0;

/// Romaji and translations are smaller than the line, and stay readable in the compact card.
fn extra_font_size(line_size: f32) -> f32 {
    (line_size * 0.6).max(12.0)
}

fn lyric_slot_height(text_height: f32, min_slot: f32) -> f32 {
    (text_height + 4.0).max(min_slot)
}

/// The artist, or a note that the file does not say.
pub fn artist_name(artist: &str) -> &str {
    if artist.is_empty() { "Unknown artist" } else { artist }
}

/// The artist line of Now Playing. Each artist of the song opens their songs. Returns the
/// height of the line.
pub fn artist_links(ui: &Ui, top_left: Pos2, width: f32, text: MarqueeText<'_>, elapsed: f64, actions: &mut Vec<Action>) -> f32 {
    let artist = text.text;
    // "Unknown artist" stands for no artist, so it opens nothing.
    let names = if artist == artist_name("") { Vec::new() } else { split_names(artist) };
    let links: Vec<_> = names.into_iter().map(|range| (range.clone(), format!("Artist: {}", &artist[range]))).collect();
    let (height, clicked) = marquee_links(ui, top_left, width, text, elapsed, &links);
    if let Some(index) = clicked {
        actions.push(Action::OpenCollection(Collection::Artist(artist[links[index].0.clone()].to_owned())));
    }
    height
}

/// Makes each artist in a song row a link that opens their songs. The row's text is `galley`,
/// painted at `origin`, and the artist tag starts at its character `first_char`. Artists cut
/// off by the end of the row are not links.
pub fn row_artist_links(
    ui: &Ui,
    row: usize,
    galley: &Galley,
    origin: Pos2,
    first_char: usize,
    artist: &str,
    actions: &mut Vec<Action>,
) {
    for (index, range) in split_names(artist).into_iter().enumerate() {
        let name = &artist[range.clone()];
        let start = first_char + artist[..range.start].chars().count();
        let chars = start..first_char + artist[..range.end].chars().count();
        let rects: Vec<Rect> =
            text_rects(galley, chars).into_iter().map(|rect| rect.translate(origin.to_vec2())).collect();
        let mut hovered = false;
        for (part, rect) in rects.iter().enumerate() {
            let link = ui
                .interact(*rect, ui.scope_id().with(("artist-link", row, index, part)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, format!("Artist: {name}")));
            hovered |= link.hovered();
            if link.clicked() {
                actions.push(Action::OpenCollection(Collection::Artist(name.to_owned())));
            }
        }
        if hovered {
            for rect in &rects {
                ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, Stroke::new(1.0, theme::SOFT));
            }
        }
    }
}

/// Lays out text over as many rows as it needs, without shortening it.
fn wrapped(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
    line_height: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat { font_id: font, color, line_height: Some(line_height), ..TextFormat::default() },
    );
    job.wrap = TextWrapping::wrap_at_width(width);
    ui.painter().layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    fn long_lyric_lines_wrap_without_an_ellipsis() {
        let text = "I guess some kids are just born with tragedy in their blood and they carry it wherever they go";
        let mut harness = Harness::builder().build_ui(|ui| {
            let font = egui::FontId::proportional(18.0);
            let galley = wrapped(ui, text, font, Color32::WHITE, 233.0, 22.5);
            assert!(galley.rows.len() > 2, "expected more than 2 rows, got {}", galley.rows.len());
            assert!(!galley.elided);
            assert!(lyric_slot_height(galley.size().y, COMPACT_LYRICS.min_slot) > galley.size().y);
        });
        harness.step();
    }

    #[test]
    fn glide_starts_softly_and_arrives_without_overshoot() {
        const FRAME: f32 = 1.0 / 60.0;
        let (mut value, mut velocity) = (0.0, 0.0);
        glide(&mut value, &mut velocity, 46.0, FRAME);
        let first_step = value;
        glide(&mut value, &mut velocity, 46.0, FRAME);
        assert!(first_step < 1.0, "first frame jumped {first_step} points");
        assert!(value - first_step > first_step, "the speed must build up");

        let mut frames = 2;
        while value != 46.0 {
            glide(&mut value, &mut velocity, 46.0, FRAME);
            frames += 1;
            assert!(value <= 46.0, "overshot to {value}");
            assert!(frames < 200, "still moving after {frames} frames");
        }
    }

    #[test]
    fn glide_stays_stable_on_a_slow_frame() {
        let (mut value, mut velocity) = (0.0, 0.0);
        for _ in 0..30 {
            glide(&mut value, &mut velocity, 46.0, 0.25);
            assert!((0.0..=46.0).contains(&value), "left the range at {value}");
        }
        assert_eq!(value, 46.0);
    }

    #[test]
    fn short_lyric_lines_keep_the_design_height() {
        assert_eq!(lyric_slot_height(22.5, COMPACT_LYRICS.min_slot), COMPACT_LYRICS.min_slot);
    }
}
