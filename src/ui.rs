//! Draws the compact layout: 780 by 460 points, positioned like the design.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, ScrollArea, Sense, Stroke, TextFormat, Ui, UiBuilder,
    Vec2, ViewportCommand, containers::scroll_area::ScrollSource, pos2, text::{LayoutJob, TextWrapping},
    vec2,
};
use amplo::library::{ALL_SONGS, LIKED_SONGS, Playlist};

use crate::{
    app::{APP_NAME, Action, App, CardTab, VISUALIZER_BARS, format_time},
    theme,
    widgets::{
        IconButton, accent_button, at, click_area, line, marquee, paint_edge_fades, paint_equalizer, paint_icon,
        paint_shadow, paint_text, paint_texture, paint_two_lines, slider, spaced_line,
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
const LYRIC_HEIGHT: f32 = 46.0;
/// One size for all lines. Only the color changes, so text never resizes or rewraps.
const LYRIC_FONT_SIZE: f32 = 17.0;
const LYRIC_ROW_HEIGHT: f32 = 21.25;
/// Roughly how long, in seconds, the lyrics take to scroll most of the way to a new line.
const LYRIC_GLIDE_TIME: f32 = 0.3;
const UNKNOWN_TIME: &str = "-:--";
/// Repaint rates while audio plays. Moving bars need more than the slow equalizers.
const VISUALIZER_FRAME: f32 = 1.0 / 30.0;
const LIST_FRAME: f32 = 1.0 / 15.0;

/// Moves `value` toward `target` with a fast start and slow end, close to the design's
/// `cubic-bezier(.2,.8,.2,1)`.
fn ease(value: &mut f32, target: f32, rate: f32, dt: f32) {
    *value += (target - *value) * (1.0 - (-rate * dt).exp());
    if (target - *value).abs() < 0.002 {
        *value = target;
    }
}

fn plural_songs(count: usize) -> String {
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

        let window = ui.max_rect();
        ui.painter().rect_filled(window, 0.0, theme::BACKGROUND);
        let mut actions = Vec::new();
        let title = Rect::from_min_size(window.min, vec2(window.width(), TITLE_HEIGHT));
        let sidebar = Rect::from_min_max(
            pos2(window.left(), title.bottom()),
            pos2(window.left() + SIDEBAR_WIDTH, window.bottom()),
        );
        let stage = Rect::from_min_max(pos2(sidebar.right(), title.bottom()), window.max);

        self.draw_title_bar(ui, title);
        self.draw_sidebar(ui, sidebar, &mut actions);
        self.draw_stage(ui, stage, dt, &mut actions);

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

    fn draw_title_bar(&self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, theme::TITLE_BAR);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::LINE));

        let drag = ui.interact(rect, ui.scope_id().with("title-bar"), Sense::click_and_drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let center_y = rect.center().y;
        let logo = Rect::from_center_size(pos2(rect.left() + 23.0, center_y), Vec2::splat(18.0));
        painter.rect_filled(logo, 5.0, theme::ACCENT);
        paint_icon(ui, logo.shrink(2.0), theme::LOGO, theme::ON_ACCENT);

        let name = line(ui, APP_NAME, theme::grotesk_bold(13.0), theme::TEXT, f32::INFINITY);
        let name_right = logo.right() + 10.0 + name.size().x;
        paint_text(ui, Align2::LEFT_CENTER, pos2(logo.right() + 10.0, center_y), name);

        let minimize = Rect::from_center_size(pos2(rect.right() - 53.0, center_y), vec2(30.0, 26.0));
        let close = Rect::from_center_size(pos2(rect.right() - 21.0, center_y), vec2(30.0, 26.0));

        let status_left = name_right + 10.0;
        let status_width = (minimize.left() - 10.0 - status_left).max(0.0);
        let (status, color) = match (&self.error, self.current_track()) {
            (Some(error), _) => (error.clone(), theme::WARNING),
            (None, Some(track)) => {
                let info = &self.library.tracks[track].info;
                let text = if info.artist.is_empty() {
                    format!("· {}", info.title)
                } else {
                    format!("· {} — {}", info.artist, info.title)
                };
                (text, theme::MUTED)
            }
            (None, None) => (String::new(), theme::MUTED),
        };
        let galley = line(ui, &status, theme::mono(11.0), color, status_width);
        let status_rect = Align2::LEFT_CENTER.anchor_size(pos2(status_left, center_y), galley.size());
        paint_text(ui, Align2::LEFT_CENTER, pos2(status_left, center_y), galley);
        if self.error.is_some() {
            // The bar is short, so a long message needs another way to be read in full.
            let hover = ui.interact(status_rect, ui.scope_id().with("status"), Sense::hover());
            hover.on_hover_text(status);
        }

        let button = |icon, label: &str, target: Rect| {
            IconButton { icon, icon_size: 14.0, color: theme::SOFT, radius: 6.0 }.show(ui, target, label)
        };
        if button(theme::MINIMIZE, "Minimize", minimize).clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if button(theme::CLOSE, "Close", close).clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn visible_playlists(&self) -> Vec<usize> {
        (0..self.library.playlists.len())
            .filter(|&index| match index {
                ALL_SONGS => !self.library.tracks.is_empty(),
                LIKED_SONGS => !self.library.playlists[index].tracks.is_empty(),
                _ => true,
            })
            .collect()
    }

    fn draw_sidebar(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(rect, 0.0, theme::PANEL);
        ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, theme::LINE));

        let heading = spaced_line(ui, "PLAYLISTS", theme::inter_semibold(10.0), theme::MUTED, 120.0, 1.0);
        paint_text(ui, Align2::LEFT_CENTER, pos2(rect.left() + 16.0, rect.top() + 24.0), heading);
        let open = at(rect.min, 165.0, 12.0, 24.0, 24.0);
        let button = IconButton { icon: theme::PLUS, icon_size: 14.0, color: theme::SOFT, radius: 6.0 };
        if button.show(ui, open, "Open music folder").clicked() {
            actions.push(Action::OpenFolder);
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
        if let Some(track) = first_track {
            let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
            paint_texture(ui, cover, texture, 6);
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
                    self.draw_list_section(ui, stage, actions);
                });
            }

            if self.slide > 0.0 {
                let offset = (1.0 - self.slide) * stage.height();
                let panel = stage.translate(vec2(0.0, offset));
                let builder = UiBuilder::new().id_salt("now-playing").max_rect(panel);
                ui.scope_builder(builder, |ui| {
                    ui.multiply_opacity((self.slide / 0.6).min(1.0));
                    self.draw_now_playing(ui, panel, dt, actions);
                });
            }

            if self.mini > 0.0 {
                let offset = (1.0 - self.mini) * MINI_HEIGHT * 1.2;
                let rect = Rect::from_min_max(
                    pos2(stage.left() + 10.0, stage.bottom() - 10.0 - MINI_HEIGHT + offset),
                    pos2(stage.right() - 10.0, stage.bottom() - 10.0 + offset),
                );
                let builder = UiBuilder::new().id_salt("mini").max_rect(rect);
                ui.scope_builder(builder, |ui| self.draw_mini(ui, rect, actions));
            }
        });
    }

    fn draw_empty_state(&self, ui: &mut Ui, stage: Rect, actions: &mut Vec<Action>) {
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
        let width = random.left() - 12.0 - left;
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

    fn draw_row(&self, ui: &mut Ui, rect: Rect, row: usize, track: usize, actions: &mut Vec<Action>) {
        let info = &self.library.tracks[track].info;
        let is_current = self.current_track() == Some(track)
            && self.queue.as_ref().is_some_and(|queue| queue.playlist == Some(self.view));
        let number = row + 1;
        let response = click_area(ui, rect, &format!("{number}. {}", info.title));
        if is_current {
            ui.painter().rect_filled(rect, 8.0, theme::ROW_CURRENT);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 8.0, theme::ROW_HOVER);
        }
        if response.clicked() {
            actions.push(Action::PlayRow(row));
        }

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
        let title_height =
            marquee(ui, pos2(left, title_top), column, &info.title, theme::grotesk_bold(19.0), theme::TEXT, elapsed);
        let artist = if info.artist.is_empty() { "Unknown artist" } else { &info.artist };
        let artist_top = title_top + title_height + 2.0;
        let artist_height = marquee(ui, pos2(left, artist_top), column, artist, theme::inter(13.0), theme::SOFT, elapsed);

        let bars_top = artist_top + artist_height + 10.0;
        self.draw_visualizer(ui, Rect::from_min_size(pos2(left, bars_top), vec2(column, 26.0)));

        let lyrics = Rect::from_min_max(pos2(left + column + 14.0, top), pos2(content.right(), cards_bottom));
        self.draw_card(ui, lyrics, dt, actions);
        self.draw_transport(ui, content, transport_top, track, actions);
    }

    fn draw_visualizer(&self, ui: &Ui, rect: Rect) {
        const GAP: f32 = 2.0;
        let count = VISUALIZER_BARS as f32;
        let width = (rect.width() - GAP * (count - 1.0)) / count;
        let color = theme::ACCENT.gamma_multiply(0.85);
        for (index, &level) in self.analyzer.bars().iter().enumerate() {
            let height = 2.0 + level * (rect.height() - 2.0);
            let x = rect.left() + index as f32 * (width + GAP);
            let bar = Rect::from_min_max(pos2(x, rect.bottom() - height), pos2(x + width, rect.bottom()));
            ui.painter().rect_filled(bar, 1.0, color);
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

        let mut tab_left = inner.left();
        for (tab, title, label) in [(CardTab::Lyrics, "LYRICS", "Lyrics"), (CardTab::Queue, "QUEUE", "Queue")] {
            let galley = spaced_line(ui, title, theme::inter_semibold(10.0), Color32::PLACEHOLDER, 100.0, 1.0);
            let text = Rect::from_min_size(pos2(tab_left, inner.top()), galley.size());
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
            tab_left = text.right() + 14.0;
        }

        match self.card_tab {
            CardTab::Lyrics => self.draw_lyrics(ui, inner, dt),
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

    fn draw_queue(&mut self, ui: &mut Ui, view: Rect, actions: &mut Vec<Action>) {
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

    fn draw_lyrics(&mut self, ui: &mut Ui, inner: Rect, dt: f32) {
        let Some(lyrics) = &self.lyrics else {
            let center = inner.center();
            paint_icon(ui, Rect::from_center_size(center - vec2(0.0, 36.0), Vec2::splat(28.0)), theme::NOTE, theme::SOFT);
            let title = line(ui, "Lyrics not available", theme::grotesk_semibold(16.0), theme::TEXT, inner.width());
            paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 4.0), title);
            let mut job = LayoutJob::simple(
                "Lyrics for this song haven’t been added yet.".to_owned(),
                theme::inter(12.0),
                theme::SOFT,
                inner.width(),
            );
            job.halign = egui::Align::Center;
            let sub = ui.painter().layout_job(job);
            // Centered text is laid out around its position, so no anchor is needed.
            ui.painter().galley(center + vec2(0.0, 12.0), sub, Color32::PLACEHOLDER);
            return;
        };
        let synced = lyrics.synced;
        if synced {
            let badge = spaced_line(ui, "SYNCED", theme::inter_semibold(10.0), theme::ACCENT, 100.0, 0.6);
            paint_text(ui, Align2::RIGHT_TOP, inner.right_top(), badge);
        }

        let view = Rect::from_min_max(pos2(inner.left(), inner.top() + 20.0), inner.max);
        if synced {
            self.draw_synced_lines(ui, view, dt);
        } else {
            self.draw_plain_lines(ui, view);
        }
        paint_edge_fades(ui, view, view.height() * 0.14, theme::PANEL);
    }

    fn draw_synced_lines(&mut self, ui: &mut Ui, view: Rect, dt: f32) {
        let Some(lyrics) = &self.lyrics else { return };
        // Laid out without a color, so a color fade does not lay the text out again.
        let galleys: Vec<_> = lyrics
            .lines
            .iter()
            .map(|line| {
                let font = theme::grotesk_semibold(LYRIC_FONT_SIZE);
                wrapped(ui, &line.text, font, Color32::PLACEHOLDER, view.width(), LYRIC_ROW_HEIGHT)
            })
            .collect();
        let slots: Vec<f32> = galleys.iter().map(|galley| lyric_slot_height(galley.size().y)).collect();
        self.update_lyric_motion(view.height(), &slots, dt);

        let motion = &self.lyric_motion;
        let scroll = motion.scroll.unwrap_or(0.0);
        let active = motion.active.map_or(-1, |index| index as i64);

        let clip = ui.clip_rect().intersect(view);
        let mut text_ui = ui.new_child(UiBuilder::new().max_rect(view));
        text_ui.set_clip_rect(clip);
        let mut top = view.top() + scroll;
        for (index, (galley, &slot)) in galleys.into_iter().zip(&slots).enumerate() {
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
            let rect = Align2::LEFT_CENTER.anchor_size(pos2(view.left(), center_y), galley.size());
            text_ui.painter().galley(rect.min, galley, color);
        }
    }

    fn draw_plain_lines(&self, ui: &mut Ui, view: Rect) {
        let Some(lyrics) = &self.lyrics else { return };
        ui.scope_builder(UiBuilder::new().id_salt("plain-lyrics").max_rect(view), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(view));
            ScrollArea::vertical().id_salt("plain-lyrics-scroll").auto_shrink(false).show(ui, |ui| {
                ui.add_space(view.height() * 0.14);
                for text in &lyrics.lines {
                    let galley = wrapped(ui, &text.text, theme::grotesk_semibold(15.0), theme::SOFT, view.width() - 8.0, 18.75);
                    ui.add(egui::Label::new(galley));
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
        let total = self.duration.map_or(0.0, |duration| duration.as_secs_f32());
        let time = if total > 0.0 { format_time(total) } else { UNKNOWN_TIME.to_owned() };
        let time = line(ui, &time, theme::mono(11.0), theme::SOFT, f32::INFINITY);
        let time_left = right - time.size().x;
        paint_text(ui, Align2::RIGHT_CENTER, pos2(right, seek_y), time);

        let seek = Rect::from_min_max(pos2(left, seek_y - 8.0), pos2(time_left - 10.0, seek_y + 8.0));
        let mut fraction = if total > 0.0 { self.position() / total } else { 0.0 };
        let response = slider(ui, seek, "Position", &mut fraction);
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
        let mut level = self.settings.volume;
        let response = slider(ui, volume, "Volume", &mut level);
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
fn lyric_slot_height(text_height: f32) -> f32 {
    (text_height + 4.0).max(LYRIC_HEIGHT)
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
            assert!(lyric_slot_height(galley.size().y) > galley.size().y);
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
        assert_eq!(lyric_slot_height(22.5), LYRIC_HEIGHT);
    }
}
