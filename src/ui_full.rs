//! Draws the full layout, shown while the window is maximized. Sizes follow the design's
//! 1280 by 780 frame, and the parts stretch with the window.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, Vec2, ViewportCommand, containers::scroll_area::ScrollSource, pos2, text::LayoutJob, vec2,
};
use amplo::library::{Collection, LIKED_SONGS, Playlist, split_names};

use crate::{
    app::{Action, App, CardTab, format_time},
    covers::LargeCover,
    spotify_page::SpotifyAction,
    theme,
    ui::{
        LyricStyle, UNKNOWN_TIME, artist_links, artist_name, draw_empty_playlist, row_artist_links, paint_empty_cover, plural_songs,
        title_bar_drag,
    },
    widgets::{
        IconButton, MarqueeText, accent_button, at, click_area, line, marquee, paint_equalizer, paint_icon,
        paint_shadow, paint_text, paint_texture, paint_two_lines, pill_button, spaced_line, text_rects,
    },
};

const SIDEBAR_WIDTH: f32 = 248.0;
const TITLE_HEIGHT: f32 = 56.0;
pub const MINI_HEIGHT: f32 = 75.0;
const PAD_X: f32 = 40.0;
const HEADER_TOP: f32 = 32.0;
const MOSAIC: f32 = 188.0;
const TABLE_TOP: f32 = HEADER_TOP + MOSAIC + 32.0;
const TABLE_HEADING: f32 = 26.0;
const ROWS_TOP: f32 = TABLE_TOP + TABLE_HEADING + 8.0;
const ROW_HEIGHT: f32 = 60.0;
const LIST_BOTTOM_SPACE: f32 = 120.0;
const NAV_ITEM: f32 = 40.0;
const PLAYLIST_ITEM: f32 = 52.0;
const PLAYLIST_STEP: f32 = 54.0;
const TRANSPORT_HEIGHT: f32 = 94.0;
/// The least room beside the cover for the album, the genre, the time, and the like button.
const INFO_WIDTH: f32 = 200.0;

const FULL_LYRICS: LyricStyle = LyricStyle {
    font_size: 28.0,
    line_height: 35.0,
    min_slot: 62.0,
    plain_font_size: 24.0,
    empty_icon: 34.0,
    empty_title: 20.0,
    empty_text: 13.0,
};

/// Splits the window into the sidebar, the title bar beside it, and the stage below that.
pub fn split(window: Rect) -> (Rect, Rect, Rect) {
    let sidebar = Rect::from_min_max(window.min, pos2(window.left() + SIDEBAR_WIDTH, window.bottom()));
    let title = Rect::from_min_max(pos2(sidebar.right(), window.top()), pos2(window.right(), window.top() + TITLE_HEIGHT));
    let stage = Rect::from_min_max(pos2(sidebar.right(), title.bottom()), window.max);
    (sidebar, title, stage)
}

/// The left edges and widths of the columns of the song table.
struct Columns {
    number: f32,
    title: f32,
    title_width: f32,
    album: f32,
    album_width: f32,
    time_right: f32,
}

/// The design's grid: `44px 2fr 1.3fr 64px`, with 16 points between columns.
fn columns(left: f32, width: f32) -> Columns {
    let inner_left = left + 16.0;
    let inner_right = left + width - 16.0;
    let fraction = ((inner_right - inner_left) - 44.0 - 64.0 - 3.0 * 16.0).max(0.0) / 3.3;
    let title = inner_left + 44.0 + 16.0;
    let album = title + 2.0 * fraction + 16.0;
    Columns {
        number: inner_left,
        title,
        title_width: 2.0 * fraction,
        album,
        album_width: 1.3 * fraction,
        time_right: inner_right,
    }
}

/// The column names over a song table, with a line below them.
fn paint_table_heading(ui: &Ui, left: f32, width: f32, top: f32) {
    let cols = columns(left, width);
    let center_y = top + 8.0;
    let heading = |text: &str, anchor: Align2, x: f32| {
        let galley = spaced_line(ui, text, theme::inter_semibold(12.0), theme::MUTED, 200.0, 0.72);
        paint_text(ui, anchor, pos2(x, center_y), galley);
    };
    heading("#", Align2::LEFT_CENTER, cols.number);
    heading("TITLE", Align2::LEFT_CENTER, cols.title);
    heading("ALBUM", Align2::LEFT_CENTER, cols.album);
    heading("TIME", Align2::RIGHT_CENTER, cols.time_right);
    let border = top + TABLE_HEADING - 0.5;
    ui.painter().hline(left..=left + width, border, Stroke::new(1.0, theme::TABLE_LINE));
}

fn total_seconds(app: &App, playlist: &Playlist) -> u64 {
    playlist
        .tracks
        .iter()
        .filter_map(|&track| app.library.tracks[track].info.duration)
        .map(|duration| duration.as_secs())
        .sum()
}

impl App {
    pub fn draw_full_title_bar(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme::TITLE_LINE));
        title_bar_drag(ui, rect, actions);
        let center_y = rect.center().y;

        let close = Rect::from_min_size(pos2(rect.right() - 16.0 - 36.0, center_y - 16.0), vec2(36.0, 32.0));
        let restore = close.translate(vec2(-40.0, 0.0));
        let minimize = restore.translate(vec2(-40.0, 0.0));
        let button = |icon, size, label: &str, target: Rect| {
            IconButton { icon, icon_size: size, color: theme::SOFT, radius: 8.0 }.show(ui, target, label)
        };
        if button(theme::MINIMIZE, 14.0, "Minimize", minimize).clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
        }
        if button(theme::RESTORE, 14.0, "Restore", restore).clicked() {
            actions.push(Action::ToggleFull);
        }
        if button(theme::CLOSE, 14.0, "Close", close).clicked() {
            actions.push(Action::HideToTray);
        }

        let field_left = rect.left() + 32.0;
        let field_width = 340.0_f32.min(minimize.left() - 24.0 - field_left).max(0.0);
        let field = Rect::from_min_size(pos2(field_left, center_y - 18.0), vec2(field_width, 36.0));
        self.draw_search(ui, field, actions);

        let message = match (&self.error, &self.notice) {
            (Some(error), _) => Some((error, theme::WARNING)),
            (None, Some((notice, _))) => Some((notice, theme::ACCENT)),
            (None, None) => None,
        };
        if let Some((message, color)) = message {
            let left = field.right() + 16.0;
            let width = (minimize.left() - 16.0 - left).max(0.0);
            let galley = line(ui, message, theme::inter(12.0), color, width);
            let text = Align2::LEFT_CENTER.anchor_size(pos2(left, center_y), galley.size());
            paint_text(ui, Align2::LEFT_CENTER, pos2(left, center_y), galley);
            // A long message is cut short, so it needs another way to be read in full.
            ui.interact(text, ui.scope_id().with("error"), Sense::hover()).on_hover_text(message);
        }
    }

    fn draw_search(&mut self, ui: &mut Ui, field: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect(field, 10.0, theme::PANEL, Stroke::new(1.0, theme::FIELD_LINE), egui::StrokeKind::Inside);
        let icon = Rect::from_min_size(pos2(field.left() + 12.0, field.top() + 10.0), Vec2::splat(16.0));
        paint_icon(ui, icon, theme::SEARCH, theme::MUTED);

        let text = Rect::from_min_max(pos2(field.left() + 36.0, field.top()), pos2(field.right() - 12.0, field.bottom()));
        let hint = RichText::new("Search songs, artists, albums").font(theme::inter(13.0)).color(theme::MUTED);
        let edit = egui::TextEdit::singleline(&mut self.search)
            .id_salt("search")
            .hint_text(hint)
            .font(theme::inter(13.0))
            .text_color(theme::TEXT)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .align(Align2::LEFT_CENTER)
            .desired_width(text.width());
        let response = ui.put(text, edit);
        // The results show in the song list, which Now Playing covers.
        if response.changed() {
            // Other rows show now, and the selection could hold hidden ones.
            actions.push(Action::ClearSelection);
            if !self.search.trim().is_empty() {
                actions.push(Action::Minimize);
                self.spotify_open = false;
            }
        }
        if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
            self.search.clear();
        }
    }

    pub fn draw_full_sidebar(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(rect, 0.0, theme::PANEL);
        ui.painter().vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, theme::SIDEBAR_LINE));

        let logo = Rect::from_min_size(pos2(rect.left() + 22.0, rect.top() + 20.0), Vec2::splat(30.0));
        ui.painter().rect_filled(logo, 9.0, theme::ACCENT);
        paint_icon(ui, logo.shrink(6.0), theme::LOGO, theme::ON_ACCENT);
        let name = spaced_line(ui, "amplo", theme::grotesk_bold(19.0), theme::TEXT, 160.0, -0.38);
        paint_text(ui, Align2::LEFT_CENTER, pos2(logo.right() + 10.0, logo.center().y), name);

        let left = rect.left() + 14.0;
        let width = rect.width() - 28.0;
        let heading = |ui: &Ui, text: &str, center_y: f32| {
            let galley = spaced_line(ui, text, theme::inter_semibold(11.0), theme::MUTED, width, 0.88);
            paint_text(ui, Align2::LEFT_CENTER, pos2(left + 10.0, center_y), galley);
        };

        let library_top = rect.top() + 74.0;
        heading(ui, "LIBRARY", library_top + 6.5);
        let now_rect = Rect::from_min_size(pos2(left, library_top + 21.0), vec2(width, NAV_ITEM));
        let has_track = self.current_track().is_some();
        if self.nav_item(ui, now_rect, theme::DISC, "Now Playing", None, has_track && !self.minimized, has_track) {
            actions.push(Action::Expand);
        }
        let spotify_rect = now_rect.translate(vec2(0.0, NAV_ITEM + 2.0));
        if self.nav_item(ui, spotify_rect, theme::CLOUD, "Spotify", None, self.minimized && self.spotify_open, true) {
            actions.push(Action::Spotify(SpotifyAction::Open));
        }
        let liked_rect = spotify_rect.translate(vec2(0.0, NAV_ITEM + 2.0));
        let liked = self.library.playlists.get(LIKED_SONGS).map_or(0, |playlist| playlist.tracks.len());
        let liked_active = self.minimized && !self.spotify_open && self.view == LIKED_SONGS;
        if self.nav_item(ui, liked_rect, theme::HEART, "Liked Songs", Some(liked), liked_active, liked > 0) {
            actions.push(Action::OpenPlaylist(LIKED_SONGS));
        }

        let playlists_top = liked_rect.bottom() + 24.0;
        heading(ui, "PLAYLISTS", playlists_top + 14.0);
        let add = Rect::from_min_size(pos2(left + width - 4.0 - 28.0, playlists_top), Vec2::splat(28.0));
        let open = add.translate(vec2(-32.0, 0.0));
        let button = |icon| IconButton { icon, icon_size: 16.0, color: theme::SOFT, radius: 8.0 };
        if button(theme::FOLDER).show(ui, open, "Open music folder").clicked() {
            actions.push(Action::OpenFolder);
        }
        if button(theme::PLUS).show(ui, add, "New playlist").clicked() {
            actions.push(Action::NewPlaylist);
        }

        let list = Rect::from_min_max(pos2(left, playlists_top + 36.0), pos2(left + width, rect.bottom() - 14.0));
        let playlists: Vec<usize> =
            self.visible_playlists().into_iter().filter(|&index| index != LIKED_SONGS).collect();
        ui.scope_builder(UiBuilder::new().id_salt("full-sidebar").max_rect(list), |ui| {
            ui.set_clip_rect(list);
            ScrollArea::vertical()
                .id_salt("full-sidebar-scroll")
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    ui.set_min_size(vec2(list.width(), playlists.len() as f32 * PLAYLIST_STEP));
                    for (row, &index) in playlists.iter().enumerate() {
                        let top = row as f32 * PLAYLIST_STEP;
                        if top + PLAYLIST_STEP < viewport.top() || top > viewport.bottom() {
                            continue;
                        }
                        let item = at(origin, 0.0, top, list.width(), PLAYLIST_ITEM);
                        self.draw_full_playlist_item(ui, item, index, actions);
                    }
                });
        });
    }

    /// One entry of the Library list. Returns whether it was clicked while enabled.
    fn nav_item(
        &self,
        ui: &Ui,
        rect: Rect,
        icon: egui::ImageSource<'static>,
        label: &str,
        count: Option<usize>,
        active: bool,
        enabled: bool,
    ) -> bool {
        let response = click_area(ui, rect, label);
        if active {
            ui.painter().rect_filled(rect, 10.0, theme::NAV_ACTIVE);
        } else if enabled && response.hovered() {
            ui.painter().rect_filled(rect, 10.0, theme::NAV_HOVER);
        }
        let color = match (active, enabled) {
            (true, _) => theme::ACCENT,
            (false, true) => theme::CHEVRON,
            (false, false) => theme::MUTED,
        };
        let center_y = rect.center().y;
        paint_icon(ui, Rect::from_center_size(pos2(rect.left() + 19.0, center_y), Vec2::splat(18.0)), icon, color);
        let text = line(ui, label, theme::inter(14.0), color, rect.width() - 80.0);
        paint_text(ui, Align2::LEFT_CENTER, pos2(rect.left() + 40.0, center_y), text);
        if let Some(count) = count {
            let count = line(ui, &count.to_string(), theme::inter(12.0), theme::MUTED, 40.0);
            paint_text(ui, Align2::RIGHT_CENTER, pos2(rect.right() - 10.0, center_y), count);
        }
        enabled && response.clicked()
    }

    fn draw_full_playlist_item(&mut self, ui: &mut Ui, rect: Rect, index: usize, actions: &mut Vec<Action>) {
        let active = self.minimized && !self.spotify_open && self.view == index;
        let is_playing = self.is_playing() && self.queue.as_ref().is_some_and(|queue| queue.playlist == Some(index));
        let playlist = self.library.playlists[index].clone();
        let response = click_area(ui, rect, &playlist.name);
        if active {
            ui.painter().rect_filled(rect, 10.0, theme::NAV_ACTIVE);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 10.0, theme::NAV_HOVER);
        }
        if response.clicked() {
            actions.push(Action::OpenPlaylist(index));
        }

        let center_y = rect.center().y;
        let cover = Rect::from_center_size(pos2(rect.left() + 29.0, center_y), Vec2::splat(38.0));
        match playlist.tracks.first() {
            Some(&track) => {
                let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
                paint_texture(ui, cover, texture, 8);
            }
            None => paint_empty_cover(ui, cover, 8.0),
        }

        let text_left = cover.right() + 12.0;
        let reserved = if is_playing { 24.0 } else { 0.0 };
        let width = (rect.right() - 10.0 - text_left - reserved).max(0.0);
        let color = if active { theme::ACCENT } else { theme::TEXT };
        let minutes = (total_seconds(self, &playlist) as f32 / 60.0).round() as u64;
        let meta = format!("{} · {minutes} min", plural_songs(playlist.tracks.len()));
        let title = line(ui, &playlist.name, theme::inter_semibold(14.0), color, width);
        let meta = line(ui, &meta, theme::inter(12.0), theme::MUTED, width);
        paint_two_lines(ui, text_left, center_y, 2.0, title, meta);
        if is_playing {
            paint_equalizer(ui, rect.right() - 20.0, center_y, true, theme::ACCENT);
        }
    }

    pub fn draw_full_list(&mut self, ui: &mut Ui, stage: Rect, actions: &mut Vec<Action>) {
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

        let rows = self.visible_rows();
        let shown = rows.len().max(1);
        let total = ROWS_TOP + shown as f32 * ROW_HEIGHT + LIST_BOTTOM_SPACE;
        ScrollArea::vertical()
            .id_salt(("full-list-scroll", &playlist.name))
            .auto_shrink(false)
            .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
            .show_viewport(ui, |ui, viewport| {
                let origin = ui.max_rect().min;
                let width = stage.width();
                ui.set_min_size(vec2(width, total));
                if viewport.top() < ROWS_TOP {
                    self.draw_full_list_header(ui, origin, width, &playlist, actions);
                }
                if rows.is_empty() {
                    let text = format!("No songs match “{}”", self.search.trim());
                    let galley = line(ui, &text, theme::inter(14.0), theme::SOFT, width - 2.0 * PAD_X);
                    paint_text(ui, Align2::LEFT_CENTER, origin + vec2(PAD_X + 16.0, ROWS_TOP + ROW_HEIGHT / 2.0), galley);
                    return;
                }
                let first = ((viewport.top() - ROWS_TOP) / ROW_HEIGHT).floor().max(0.0) as usize;
                let last = (((viewport.bottom() - ROWS_TOP) / ROW_HEIGHT).ceil().max(0.0) as usize).min(rows.len());
                for (shown_row, &row) in rows.iter().enumerate().take(last).skip(first) {
                    let top = ROWS_TOP + shown_row as f32 * ROW_HEIGHT;
                    let rect = at(origin, PAD_X, top, width - 2.0 * PAD_X, ROW_HEIGHT);
                    self.draw_full_row(ui, rect, row, playlist.tracks[row], actions);
                }
            });
    }

    fn draw_full_list_header(&mut self, ui: &mut Ui, origin: Pos2, width: f32, playlist: &Playlist, actions: &mut Vec<Action>) {
        let mosaic = at(origin, PAD_X, HEADER_TOP, MOSAIC, MOSAIC);
        paint_shadow(ui, mosaic, 16, 20, 50, 115);
        let half = MOSAIC / 2.0;
        // An album has one cover, which fills the whole square. An artist or a genre shows the
        // covers of its first albums.
        let covers: Vec<usize> = match &playlist.collection {
            Some(Collection::Album { .. }) => {
                let texture = self.covers.texture(ui.ctx(), &self.library.tracks[playlist.tracks[0]].path);
                paint_texture(ui, mosaic, texture, 16);
                Vec::new()
            }
            Some(_) => {
                let mut albums: Vec<&str> = Vec::new();
                let mut firsts = Vec::new();
                for &track in &playlist.tracks {
                    let album = self.library.tracks[track].info.album.as_str();
                    if firsts.len() < 4 && !albums.iter().any(|seen| seen.eq_ignore_ascii_case(album)) {
                        albums.push(album);
                        firsts.push(track);
                    }
                }
                firsts
            }
            None => playlist.tracks.clone(),
        };
        let tiles = if covers.is_empty() { 0 } else { 4 };
        for index in 0..tiles {
            let track = covers[index % covers.len()];
            let radius = match index {
                0 => CornerRadius { nw: 16, ..CornerRadius::ZERO },
                1 => CornerRadius { ne: 16, ..CornerRadius::ZERO },
                2 => CornerRadius { sw: 16, ..CornerRadius::ZERO },
                _ => CornerRadius { se: 16, ..CornerRadius::ZERO },
            };
            let tile = Rect::from_min_size(
                mosaic.min + vec2((index % 2) as f32 * half, (index / 2) as f32 * half),
                Vec2::splat(half),
            );
            let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
            paint_texture(ui, tile, texture, radius);
        }

        // The text sits on the bottom of the covers, as the design's `align-items: flex-end`.
        let text_left = mosaic.right() + 28.0;
        let text_width = (origin.x + width - PAD_X - text_left).max(0.0);
        let buttons_top = mosaic.bottom() - 44.0;

        let seconds = total_seconds(self, playlist);
        let mut meta = format!("{} · {} min {} sec", plural_songs(playlist.tracks.len()), seconds / 60, seconds % 60);
        let kind = playlist.collection.as_ref().map_or("PLAYLIST", Collection::kind);
        if let Some(Collection::Album { artist, .. }) = &playlist.collection {
            let first = &self.library.tracks[playlist.tracks[0]].info;
            let artist = if artist.is_empty() { &first.artist } else { artist };
            if !artist.is_empty() {
                meta = format!("{artist} · {meta}");
            }
        }
        let meta = line(ui, &meta, theme::inter(14.0), theme::SOFT, text_width);
        let meta_top = buttons_top - 18.0 - meta.size().y;
        let title = spaced_line(ui, &playlist.name, theme::grotesk_bold(52.0), theme::TEXT, text_width, -1.56);
        let title_top = meta_top - 10.0 - title.size().y;
        let label = spaced_line(ui, kind, theme::inter_semibold(12.0), theme::MUTED, text_width, 0.96);
        let label_top = title_top - 10.0 - label.size().y;
        ui.painter().galley(pos2(text_left, label_top), label, Color32::PLACEHOLDER);
        ui.painter().galley(pos2(text_left, title_top), title, Color32::PLACEHOLDER);
        ui.painter().galley(pos2(text_left, meta_top), meta, Color32::PLACEHOLDER);

        let play_text = line(ui, "Play", theme::inter_semibold(14.0), Color32::PLACEHOLDER, 100.0);
        let play = Rect::from_min_size(pos2(text_left, buttons_top), vec2(16.0 + 18.0 + 8.0 + play_text.size().x + 22.0, 44.0));
        if pill_button(ui, play, theme::PLAY, 18.0, play_text, true, "Play playlist").clicked() {
            actions.push(Action::PlayView);
        }
        let shuffle_text = line(ui, "Shuffle", theme::inter(14.0), Color32::PLACEHOLDER, 100.0);
        let shuffle_width = 18.0 + 17.0 + 8.0 + shuffle_text.size().x + 18.0;
        let shuffle = Rect::from_min_size(pos2(play.right() + 12.0, buttons_top), vec2(shuffle_width, 44.0));
        if pill_button(ui, shuffle, theme::SHUFFLE, 17.0, shuffle_text, false, "Shuffle play").clicked() {
            actions.push(Action::ShufflePlayView);
        }

        paint_table_heading(ui, origin.x + PAD_X, width - 2.0 * PAD_X, origin.y + TABLE_TOP);
    }

    fn draw_full_row(&mut self, ui: &mut Ui, rect: Rect, row: usize, track: usize, actions: &mut Vec<Action>) {
        let is_current = self.current_track() == Some(track)
            && self.queue.as_ref().is_some_and(|queue| queue.playlist == Some(self.view));
        let number = row + 1;
        let title = self.library.tracks[track].info.title.clone();
        let response = click_area(ui, rect, &format!("{number}. {title}"));
        if self.selected.contains(&row) {
            ui.painter().rect_filled(rect, 12.0, theme::ROW_SELECTED);
        } else if is_current {
            ui.painter().rect_filled(rect, 12.0, theme::ROW_CURRENT);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 12.0, theme::FULL_ROW_HOVER);
        }
        self.song_row_input(ui, &response, row, actions);
        let (artist, artist_origin) = self.paint_full_track(ui, rect, number, track, is_current);

        // Each artist opens their songs, and the album name opens the album, unless it is
        // the album shown.
        let info = &self.library.tracks[track].info;
        row_artist_links(ui, row, &artist, artist_origin, 0, &info.artist, actions);
        let showing_album = self
            .library
            .playlists
            .get(self.view)
            .is_some_and(|playlist| matches!(playlist.collection, Some(Collection::Album { .. })));
        if info.album.is_empty() || showing_album {
            return;
        }
        let cols = columns(rect.left(), rect.width());
        let album = line(ui, &info.album, theme::inter(13.0), theme::SOFT, cols.album_width);
        let text = Align2::LEFT_CENTER.anchor_size(pos2(cols.album, rect.center().y), album.size());
        let link = ui
            .interact(text.expand2(vec2(2.0, 4.0)), ui.scope_id().with(("album-link", row)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let name = info.album.clone();
        link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, format!("Album: {name}")));
        if link.hovered() {
            ui.painter().hline(text.x_range(), text.bottom(), Stroke::new(1.0, theme::SOFT));
        }
        if link.clicked()
            && let Some(album) = Collection::album_of(info)
        {
            actions.push(Action::OpenCollection(album));
        }
    }

    /// One song in the table of a playlist or of the queue: its number, or an equalizer
    /// while it plays, then cover, title, artist, album, and length. Returns the artist line
    /// and where it is.
    fn paint_full_track(
        &mut self,
        ui: &Ui,
        rect: Rect,
        number: usize,
        track: usize,
        is_current: bool,
    ) -> (std::sync::Arc<egui::Galley>, Pos2) {
        let cols = columns(rect.left(), rect.width());
        let center_y = rect.center().y;
        if is_current {
            paint_equalizer(ui, cols.number, center_y, self.is_playing(), theme::ACCENT);
        } else {
            let text = line(ui, &number.to_string(), theme::mono(13.0), theme::MUTED, 44.0);
            paint_text(ui, Align2::LEFT_CENTER, pos2(cols.number, center_y), text);
        }

        let cover = Rect::from_min_size(pos2(cols.title, center_y - 20.0), Vec2::splat(40.0));
        let texture = self.covers.texture(ui.ctx(), &self.library.tracks[track].path);
        paint_texture(ui, cover, texture, 8);

        let info = &self.library.tracks[track].info;
        let text_width = (cols.title_width - 52.0).max(0.0);
        let color = if is_current { theme::ACCENT } else { theme::TEXT };
        let title = line(ui, &info.title, theme::inter_semibold(15.0), color, text_width);
        let artist = line(ui, artist_name(&info.artist), theme::inter(13.0), theme::SOFT, text_width);
        // Where `paint_two_lines` puts the artist.
        let artist_origin = pos2(cover.right() + 12.0, center_y + (title.size().y + 3.0 - artist.size().y) / 2.0);
        paint_two_lines(ui, cover.right() + 12.0, center_y, 3.0, title, artist.clone());

        let album = line(ui, &info.album, theme::inter(13.0), theme::SOFT, cols.album_width);
        paint_text(ui, Align2::LEFT_CENTER, pos2(cols.album, center_y), album);

        let time = info.duration.map_or_else(|| UNKNOWN_TIME.to_owned(), |duration| format_time(duration.as_secs_f32()));
        let time = line(ui, &time, theme::mono(13.0), theme::SOFT, 64.0);
        paint_text(ui, Align2::RIGHT_CENTER, pos2(cols.time_right, center_y), time);
        (artist, artist_origin)
    }

    pub fn draw_full_now_playing(&mut self, ui: &mut Ui, panel: Rect, dt: f32, actions: &mut Vec<Action>) {
        ui.painter().rect_filled(panel, 0.0, theme::FULL_BACKGROUND);
        // Stops the track list underneath from reacting while this panel slides over it.
        ui.interact(panel, ui.scope_id().with("cover-list"), Sense::click());
        self.update_visualizer(dt);

        let Some(track) = self.current_track() else { return };
        let content = Rect::from_min_max(panel.min + vec2(PAD_X, 28.0), panel.max - vec2(PAD_X, 32.0));

        let source = self.queue.as_ref().and_then(|queue| queue.playlist).and_then(|index| self.library.playlists.get(index));
        let kind = source.and_then(|playlist| playlist.collection.as_ref()).map_or("PLAYLIST", Collection::kind);
        let from = format!("PLAYING FROM {kind}");
        let source = source.map_or_else(|| "Queue".to_owned(), |playlist| playlist.name.clone());
        let back_text = line(ui, "Show playlist", theme::inter(13.0), Color32::PLACEHOLDER, 200.0);
        let back_width = 10.0 + 18.0 + 6.0 + back_text.size().x + 14.0;
        let back = Rect::from_min_size(pos2(content.right() - back_width, content.top()), vec2(back_width, 38.0));
        let label = spaced_line(ui, &from, theme::inter_semibold(11.0), theme::MUTED, 300.0, 0.88);
        let name_width = (back.left() - 16.0 - content.left()).max(0.0);
        let name = line(ui, &source, theme::inter_semibold(15.0), theme::TEXT, name_width);
        paint_two_lines(ui, content.left(), back.center().y, 2.0, label, name);
        let response = click_area(ui, back, "Back to song list");
        let fill = if response.hovered() { theme::ICON_HOVER } else { Color32::TRANSPARENT };
        ui.painter().rect(back, 19.0, fill, Stroke::new(1.0, theme::PILL_LINE), egui::StrokeKind::Inside);
        let chevron = Rect::from_center_size(pos2(back.left() + 10.0 + 9.0, back.center().y), Vec2::splat(18.0));
        paint_icon(ui, chevron, theme::CHEVRON_DOWN, theme::CHEVRON);
        ui.painter().galley(
            pos2(chevron.right() + 6.0, back.center().y - back_text.size().y / 2.0),
            back_text,
            theme::CHEVRON,
        );
        if response.clicked() {
            actions.push(Action::Minimize);
        }

        let transport_top = content.bottom() - TRANSPORT_HEIGHT;
        let body = Rect::from_min_max(pos2(content.left(), back.bottom() + 24.0), pos2(content.right(), transport_top - 24.0));
        // The song and the lyrics share the width equally, as in the compact layout.
        let column = (body.width() - 40.0) / 2.0;
        let left = body.left();
        let info_right = left + column;
        // Beside the cover stand the time, the audio details, and the like button, at the
        // bottom right. The widest time this track can show sets their room, so the cover
        // does not change size while the track plays.
        let time_font = theme::mono_semibold(42.0);
        let tech_font = theme::mono(13.0);
        let widest_time = ["00:00".to_owned(), self.total_time()]
            .iter()
            .map(|text| spaced_line(ui, text, time_font.clone(), theme::ACCENT, f32::INFINITY, -0.84).size().x)
            .chain(self.tech.iter().map(|text| line(ui, text, tech_font.clone(), theme::MUTED, f32::INFINITY).size().x))
            .fold(40.0, f32::max);
        let info_width = widest_time.max(INFO_WIDTH);
        // A cover larger than its own pixels would look blurred, so that is its limit.
        let path = self.library.tracks[track].path.clone();
        let pixels_per_point = ui.ctx().pixels_per_point();
        let (texture, sharp_size) = match self.covers.large(&path) {
            LargeCover::Ready(texture, pixels) => (texture, pixels.min_elem() / pixels_per_point),
            // The small cover stands in while the large one loads, or for generated art.
            LargeCover::Loading | LargeCover::Missing => (self.covers.texture(ui.ctx(), &path), f32::INFINITY),
        };
        // The title, the artist, and the bars take about 130 points below the cover.
        let cover_size = (body.height() - 130.0).min(column - info_width - 24.0).max(160.0).min(sharp_size);

        let cover = Rect::from_min_size(body.min, Vec2::splat(cover_size));
        paint_shadow(ui, cover, 20, 30, 70, 140);
        paint_texture(ui, cover, texture, 20);

        // The album and the genre stand at the top right of the cover's row.
        // Beside the cover, from the top: the time, the audio details, then the album, the
        // genre, and the file format, and the like button last. They sit at the bottom of the
        // cover's row, and start at its top when they need more room than that.
        let info = &self.library.tracks[track].info;
        let details_width = (info_right - cover.right() - 24.0).max(0.0);
        let format = path.extension().map(|extension| extension.to_string_lossy().to_uppercase()).unwrap_or_default();
        let time = spaced_line(ui, &format_time(self.position()), time_font, theme::ACCENT, f32::INFINITY, -0.84);
        let tech: Vec<_> =
            self.tech.iter().map(|text| line(ui, text, tech_font.clone(), theme::MUTED, f32::INFINITY)).collect();
        let details: Vec<_> = [("ALBUM", &info.album), ("GENRE", &info.genre), ("FORMAT", &format)]
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| {
                let label = spaced_line(ui, name, theme::inter_semibold(10.0), theme::MUTED, details_width, 1.0);
                let mut job = LayoutJob::simple(value.clone(), theme::inter(14.0), theme::SOFT, details_width);
                job.halign = egui::Align::RIGHT;
                job.wrap.max_rows = 4;
                (name, label, ui.painter().layout_job(job))
            })
            .collect();

        const TECH_STEP: f32 = 19.0;
        const DETAIL_GAP: f32 = 12.0;
        const LIKE: f32 = 40.0;
        let details_height: f32 =
            details.iter().map(|(_, label, value)| label.size().y + 4.0 + value.size().y + DETAIL_GAP).sum();
        let height = time.size().y + 4.0 + tech.len() as f32 * TECH_STEP + 16.0 + details_height + LIKE - 6.0;
        let mut y = (cover.bottom() + 6.0 - height).max(cover.top());

        let time_height = time.size().y;
        paint_text(ui, Align2::RIGHT_TOP, pos2(info_right, y), time);
        y += time_height + 4.0;
        for text in tech {
            paint_text(ui, Align2::RIGHT_TOP, pos2(info_right, y), text);
            y += TECH_STEP;
        }
        y += 16.0;
        for (name, label, value) in details {
            let label_height = label.size().y;
            paint_text(ui, Align2::RIGHT_TOP, pos2(info_right, y), label);
            y += label_height + 4.0;
            let value_height = value.size().y;
            // Right-aligned text is laid out to the left of its position.
            let value_rect = value.rect.translate(vec2(info_right, y));
            if name == "ALBUM" {
                let link = click_area(ui, value_rect.expand(3.0), &format!("Album: {}", info.album));
                if link.hovered() {
                    ui.painter().hline(value_rect.x_range(), value_rect.bottom(), Stroke::new(1.0, theme::SOFT));
                }
                if link.clicked()
                    && let Some(album) = Collection::album_of(info)
                {
                    actions.push(Action::OpenCollection(album));
                }
            }
            // Each genre of the song opens on its own.
            if name == "GENRE" {
                for range in split_names(&info.genre) {
                    let genre = &info.genre[range.clone()];
                    let chars = info.genre[..range.start].chars().count()..info.genre[..range.end].chars().count();
                    let rects = text_rects(&value, chars);
                    let mut hovered = false;
                    let mut clicked = false;
                    for (part, rect) in rects.iter().enumerate() {
                        let rect = rect.translate(vec2(info_right, y));
                        let link = ui
                            .interact(rect.expand(2.0), ui.scope_id().with(("genre-link", genre, part)), Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        link.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, format!("Genre: {genre}")));
                        hovered |= link.hovered();
                        clicked |= link.clicked();
                    }
                    if hovered {
                        for rect in &rects {
                            let rect = rect.translate(vec2(info_right, y));
                            ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme::SOFT));
                        }
                    }
                    if clicked {
                        actions.push(Action::OpenCollection(Collection::Genre(genre.to_owned())));
                    }
                }
            }
            ui.painter().galley(pos2(info_right, y), value, Color32::PLACEHOLDER);
            y += value_height + DETAIL_GAP;
        }

        let liked = self.is_liked(track);
        let (heart, heart_color, like_label) =
            if liked { (theme::HEART_FILLED, theme::ACCENT, "Unlike") } else { (theme::HEART, theme::SOFT, "Like") };
        let like = Rect::from_min_size(pos2(info_right + 6.0 - LIKE, y - 6.0), Vec2::splat(LIKE));
        if IconButton::round(heart, 22.0, heart_color).show(ui, like, like_label).clicked() {
            actions.push(Action::ToggleLike);
        }

        let title_top = cover.bottom() + 18.0;
        let elapsed = ui.input(|input| input.time) - self.marquee_started;
        let text = |text, font, color| MarqueeText { text, font, color, background: theme::FULL_BACKGROUND };
        let text_width = column;
        let title = text(&info.title, theme::grotesk_bold(28.0), theme::TEXT);
        let title_height = marquee(ui, pos2(left, title_top), text_width, title, elapsed);
        let artist_top = title_top + title_height + 4.0;
        let artist = text(artist_name(&info.artist), theme::inter(15.0), theme::SOFT);
        let artist_height = artist_links(ui, pos2(left, artist_top), text_width, artist, elapsed, actions);
        let bars = Rect::from_min_size(pos2(left, artist_top + artist_height + 18.0), vec2(column, 36.0));
        self.draw_visualizer(ui, bars, 3.0);

        let card = Rect::from_min_max(pos2(info_right + 40.0, body.top()), body.max);
        self.draw_full_card(ui, card, dt, actions);
        self.draw_full_transport(ui, content, transport_top, actions);
    }

    fn draw_full_card(&mut self, ui: &mut Ui, card: Rect, dt: f32, actions: &mut Vec<Action>) {
        ui.painter().rect(card, 20.0, theme::PANEL, Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
        let inner = card.shrink2(vec2(28.0, 24.0));
        self.draw_card_tabs(ui, inner.left(), inner.top(), theme::grotesk_semibold(17.0), false, actions);

        let header_bottom = inner.top() + 24.0;
        match self.card_tab {
            CardTab::Lyrics => {
                let mut edit_right = inner.right();
                if self.lyrics.as_ref().is_some_and(|lyrics| lyrics.synced) {
                    let badge = spaced_line(ui, "SYNCED", theme::inter_semibold(11.0), theme::ACCENT, 100.0, 0.66);
                    let size = badge.size() + vec2(20.0, 8.0);
                    let pill = Rect::from_min_size(pos2(inner.right() - size.x, inner.top()), size);
                    ui.painter().rect_stroke(pill, size.y / 2.0, Stroke::new(1.0, theme::PILL_LINE), egui::StrokeKind::Inside);
                    paint_text(ui, Align2::CENTER_CENTER, pill.center(), badge);
                    edit_right = pill.left() - 16.0;
                }
                if self.lyrics.is_some() {
                    self.draw_lyrics_links(ui, edit_right, inner.top() + 11.0, theme::inter_semibold(13.0), false, actions);
                }
                let view = Rect::from_min_max(pos2(inner.left(), header_bottom + 12.0), inner.max);
                self.draw_lyrics(ui, view, dt, &FULL_LYRICS, actions);
            }
            CardTab::Queue => {
                // The lyrics do not animate while hidden, so nothing is left to wait for.
                self.lyric_motion.moving = false;
                let view = Rect::from_min_max(
                    pos2(card.left() + 12.0, header_bottom + 12.0),
                    pos2(card.right() - 12.0, card.bottom() - 12.0),
                );
                self.draw_full_queue(ui, view, actions);
            }
        }
    }

    /// The queue as a song table, like a playlist: the song that plays, then what comes next.
    fn draw_full_queue(&mut self, ui: &mut Ui, view: Rect, actions: &mut Vec<Action>) {
        const SECTION: f32 = 36.0;
        let Some(queue) = &self.queue else { return };
        let (position, count) = (queue.position, queue.tracks.len());
        let source = queue.playlist.and_then(|index| self.library.playlists.get(index));
        let next_heading = source.map_or_else(|| "Next in queue".to_owned(), |playlist| format!("Next from: {}", playlist.name));
        let now_top = TABLE_HEADING + 4.0;
        let next_top = now_top + SECTION + ROW_HEIGHT + 12.0;
        let rows_top = next_top + SECTION;
        let upcoming = count.saturating_sub(1);
        let total = rows_top + upcoming as f32 * ROW_HEIGHT + 8.0;

        ui.scope_builder(UiBuilder::new().id_salt("full-queue").max_rect(view), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(view));
            // A new position starts at the top, where the song that plays now is.
            ScrollArea::vertical()
                .id_salt(("full-queue-scroll", position))
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    let width = view.width();
                    ui.set_min_size(vec2(width, total));
                    paint_table_heading(ui, origin.x, width, origin.y);

                    let heading = |ui: &Ui, top: f32, text: &str| {
                        let galley = line(ui, text, theme::grotesk_semibold(15.0), theme::TEXT, width - 32.0);
                        paint_text(ui, Align2::LEFT_CENTER, origin + vec2(16.0, top + SECTION / 2.0), galley);
                    };
                    heading(ui, now_top, "Now playing");
                    if let Some(current) = self.current_track() {
                        let rect = at(origin, 0.0, now_top + SECTION, width, ROW_HEIGHT);
                        ui.painter().rect_filled(rect, 12.0, theme::ROW_CURRENT);
                        self.paint_full_track(ui, rect, 0, current, true);
                    }
                    if upcoming == 0 {
                        return;
                    }
                    heading(ui, next_top, &next_heading);

                    let first = ((viewport.top() - rows_top) / ROW_HEIGHT).floor().max(0.0) as usize;
                    let last = (((viewport.bottom() - rows_top) / ROW_HEIGHT).ceil().max(0.0) as usize).min(upcoming);
                    for row in first..last {
                        // The queue wraps around, so the songs before this one follow at the end.
                        let target = (position + 1 + row) % count;
                        let Some(track) = self.queue.as_ref().map(|queue| queue.tracks[target]) else { return };
                        let rect = at(origin, 0.0, rows_top + row as f32 * ROW_HEIGHT, width, ROW_HEIGHT);
                        let label = format!("Queue: {}", self.library.tracks[track].info.title);
                        let response = click_area(ui, rect, &label);
                        if response.hovered() {
                            ui.painter().rect_filled(rect, 12.0, theme::FULL_ROW_HOVER);
                        }
                        if response.clicked() {
                            actions.push(Action::PlayQueue(target));
                        }
                        self.paint_full_track(ui, rect, row + 1, track, false);
                    }
                });
        });
    }

    fn draw_full_transport(&mut self, ui: &mut Ui, content: Rect, top: f32, actions: &mut Vec<Action>) {
        let left = content.left();
        let right = content.right();
        let seek_y = top + 8.0;
        let elapsed = line(ui, &format_time(self.position()), theme::mono(12.0), theme::SOFT, 60.0);
        paint_text(ui, Align2::LEFT_CENTER, pos2(left, seek_y), elapsed);
        let total = line(ui, &self.total_time(), theme::mono(12.0), theme::SOFT, 60.0);
        paint_text(ui, Align2::RIGHT_CENTER, pos2(right, seek_y), total);
        let seek = Rect::from_min_max(pos2(left + 54.0, seek_y - 8.0), pos2(right - 54.0, seek_y + 8.0));
        self.seek_slider(ui, seek, actions);

        let center_y = top + 16.0 + 14.0 + 32.0;
        let center_x = content.center().x;
        let button_at = |x: f32, size: f32| Rect::from_center_size(pos2(center_x + x, center_y), Vec2::splat(size));
        let toggle_color = |on: bool| if on { theme::ACCENT } else { theme::SOFT };

        let shuffle = IconButton::round(theme::SHUFFLE, 20.0, toggle_color(self.shuffle));
        if shuffle.show(ui, button_at(-122.0, 44.0), "Shuffle").clicked() {
            actions.push(Action::ToggleShuffle);
        }
        if IconButton::round(theme::PREVIOUS, 22.0, theme::TEXT).show(ui, button_at(-66.0, 48.0), "Previous").clicked() {
            actions.push(Action::Previous);
        }
        let play = button_at(0.0, 64.0);
        paint_shadow(ui, play, 32, 10, 30, 102);
        let (icon, size, label) = if self.is_playing() { (theme::PAUSE, 24.0, "Pause") } else { (theme::PLAY, 26.0, "Play") };
        if accent_button(ui, play, icon, size, label).clicked() {
            actions.push(Action::Toggle);
        }
        if IconButton::round(theme::NEXT, 22.0, theme::TEXT).show(ui, button_at(66.0, 48.0), "Next").clicked() {
            actions.push(Action::Next);
        }
        let repeat = IconButton::round(theme::REPEAT, 20.0, toggle_color(self.repeat));
        if repeat.show(ui, button_at(122.0, 44.0), "Repeat").clicked() {
            actions.push(Action::ToggleRepeat);
        }

        let volume = Rect::from_min_max(pos2(right - 120.0, center_y - 8.0), pos2(right, center_y + 8.0));
        let icon = Rect::from_center_size(pos2(volume.left() - 20.0, center_y), Vec2::splat(20.0));
        paint_icon(ui, icon, theme::VOLUME, theme::SOFT);
        self.volume_slider(ui, volume, actions);
    }

    pub fn draw_full_mini(&mut self, ui: &mut Ui, rect: Rect, actions: &mut Vec<Action>) {
        let Some(track) = self.current_track() else { return };
        // Stops the rows under the card from taking its clicks.
        ui.interact(rect, ui.scope_id().with("cover-rows"), Sense::click());
        paint_shadow(ui, rect, 18, 18, 50, 140);
        ui.painter().rect(rect, 18.0, theme::FULL_MINI, Stroke::new(1.0, theme::MINI_LINE), egui::StrokeKind::Inside);

        // The bar stops where the card's rounded corners would cut it off.
        let bar_left = rect.left() + 10.0;
        let bar_right = rect.right() - 10.0;
        let bar_y = rect.top() + 1.0..=rect.top() + 4.0;
        let position = self.position();
        let total = self.duration.map_or(0.0, |duration| duration.as_secs_f32());
        let progress = if total > 0.0 { (position / total).clamp(0.0, 1.0) } else { 0.0 };
        ui.painter().rect_filled(Rect::from_x_y_ranges(bar_left..=bar_right, bar_y.clone()), 1.5, theme::MINI_LINE);
        let filled_right = bar_left + progress * (bar_right - bar_left);
        ui.painter().rect_filled(Rect::from_x_y_ranges(bar_left..=filled_right, bar_y), 1.5, theme::ACCENT);

        let center_y = rect.top() + 3.0 + 10.0 + 26.0;
        let time = format!("{} / {}", format_time(position), self.total_time());
        let time = line(ui, &time, theme::mono(12.0), theme::SOFT, 200.0);
        let group_width = 40.0 + 6.0 + 44.0 + 6.0 + 40.0 + 8.0 + time.size().x;
        let group_left = rect.center().x - group_width / 2.0;
        let previous = Rect::from_center_size(pos2(group_left + 20.0, center_y), Vec2::splat(40.0));
        let play = Rect::from_center_size(pos2(previous.right() + 6.0 + 22.0, center_y), Vec2::splat(44.0));
        let next = Rect::from_center_size(pos2(play.right() + 6.0 + 20.0, center_y), Vec2::splat(40.0));
        paint_text(ui, Align2::LEFT_CENTER, pos2(next.right() + 8.0, center_y), time);

        let expand = Rect::from_center_size(pos2(rect.right() - 14.0 - 20.0, center_y), Vec2::splat(40.0));
        let volume = Rect::from_min_max(pos2(expand.left() - 10.0 - 96.0, center_y - 8.0), pos2(expand.left() - 10.0, center_y + 8.0));
        let volume_icon = Rect::from_center_size(pos2(volume.left() - 19.0, center_y), Vec2::splat(18.0));

        let info = &self.library.tracks[track].info;
        let open = Rect::from_min_max(pos2(rect.left() + 14.0, center_y - 26.0), pos2(group_left - 16.0, center_y + 26.0));
        let title_label = format!("Open Now Playing: {}", info.title);
        let text_width = (open.width() - 64.0).max(0.0);
        let title = line(ui, &info.title, theme::inter_semibold(14.0), theme::TEXT, text_width);
        let artist = line(ui, artist_name(&info.artist), theme::inter(12.0), theme::SOFT, text_width);
        if click_area(ui, open, &title_label).clicked() {
            actions.push(Action::Expand);
        }
        let path = self.library.tracks[track].path.clone();
        let texture = self.covers.texture(ui.ctx(), &path);
        let cover = Rect::from_min_size(open.min, Vec2::splat(52.0));
        paint_texture(ui, cover, texture, 10);
        paint_two_lines(ui, cover.right() + 12.0, center_y, 3.0, title, artist);

        if IconButton::round(theme::PREVIOUS, 18.0, theme::TEXT).show(ui, previous, "Previous").clicked() {
            actions.push(Action::Previous);
        }
        let (icon, size, label) = if self.is_playing() { (theme::PAUSE, 18.0, "Pause") } else { (theme::PLAY, 20.0, "Play") };
        if accent_button(ui, play, icon, size, label).clicked() {
            actions.push(Action::Toggle);
        }
        if IconButton::round(theme::NEXT, 18.0, theme::TEXT).show(ui, next, "Next").clicked() {
            actions.push(Action::Next);
        }
        paint_icon(ui, volume_icon, theme::VOLUME, theme::SOFT);
        self.volume_slider(ui, volume, actions);
        if IconButton::round(theme::CHEVRON_UP, 20.0, theme::CHEVRON).show(ui, expand, "Expand").clicked() {
            actions.push(Action::Expand);
        }
    }
}
