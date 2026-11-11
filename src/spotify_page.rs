//! The Spotify page of the full layout: the login, the user's playlists, and for each song
//! whether the local library already has it.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver, Sender},
    },
};

use amplo::spotify::{
    self, Event, LocalIndex, PendingLogin, Playlist, REDIRECT_URI, Session, Track, User, Wake,
};
use eframe::egui::{
    self, Align2, Color32, Margin, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2, containers::scroll_area::ScrollSource, pos2, vec2,
};

use crate::{
    app::{Action, App},
    covers::WebCovers,
    theme,
    ui::paint_empty_cover,
    ui_full::MINI_HEIGHT,
    widgets::{
        at, button_width, click_area, line, paint_icon, paint_text, paint_texture, paint_two_lines, spaced_line,
        text_button,
    },
};

const PAD_X: f32 = 40.0;
const HEADER_HEIGHT: f32 = 132.0;
const COLUMN_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 24.0;
const CARD_HEIGHT: f32 = 72.0;
const CARD_STEP: f32 = 80.0;
const PANEL_HEADER: f32 = 92.0;
const ROW_HEIGHT: f32 = 60.0;
const LOGIN_WIDTH: f32 = 540.0;

pub enum SpotifyAction {
    Open,
    LogIn,
    /// Shows the login page in the browser again, in case its tab was closed.
    ShowLoginPage,
    CancelLogin,
    LogOut,
    Refresh,
    OpenPlaylist(String),
    ShowMissingOnly(bool),
    /// Saves the songs of the open playlist that the folder has as a playlist file.
    Import,
}

pub enum Login {
    LoggedOut,
    Waiting(PendingLogin),
    LoggedIn(Session),
}

pub struct SpotifyPage {
    /// The text in the Client ID field.
    pub client_id: String,
    pub login: Login,
    sender: Sender<Event>,
    events: Receiver<Event>,
    wake: Wake,
    pub user: Option<User>,
    /// `None` until the playlists have loaded.
    pub playlists: Option<Result<Vec<Playlist>, String>>,
    requested: bool,
    /// The playlist shown on the right, by its Spotify ID.
    pub open: Option<String>,
    pub tracks: HashMap<String, Result<Vec<Track>, String>>,
    loading: HashSet<String>,
    pub missing_only: bool,
    pub error: Option<String>,
    pub covers: WebCovers,
    /// Rebuilt when the library changes, which `library_revision` counts.
    index: Option<(u64, LocalIndex)>,
}

impl SpotifyPage {
    pub fn new(ctx: &egui::Context, client_id: Option<&str>, token: Option<&str>) -> Self {
        let (sender, events) = mpsc::channel();
        let repaint = ctx.clone();
        let wake: Wake = Arc::new(move || repaint.request_repaint());
        let login = match (client_id, token) {
            (Some(client_id), Some(token)) => {
                Login::LoggedIn(Session::resume(client_id, token, sender.clone(), Arc::clone(&wake)))
            }
            _ => Login::LoggedOut,
        };
        Self {
            client_id: client_id.unwrap_or_default().to_owned(),
            login,
            sender,
            events,
            wake,
            user: None,
            playlists: None,
            requested: false,
            open: None,
            tracks: HashMap::new(),
            loading: HashSet::new(),
            missing_only: false,
            error: None,
            covers: WebCovers::new(ctx),
            index: None,
        }
    }

    fn forget(&mut self) {
        self.user = None;
        self.playlists = None;
        self.requested = false;
        self.open = None;
        self.tracks.clear();
        self.loading.clear();
    }

    fn open_playlist(&mut self, id: String) {
        if let Login::LoggedIn(session) = &self.login
            && !self.tracks.contains_key(&id)
            && self.loading.insert(id.clone())
        {
            session.load_tracks(&id);
        }
        self.open = Some(id);
    }

    fn load_playlists(&mut self) {
        if let Login::LoggedIn(session) = &self.login {
            self.requested = true;
            self.playlists = None;
            session.load_playlists();
        }
    }

    /// For each song of the open playlist, the local song that is the same, if any.
    pub fn matches(&mut self, app_tracks: &[amplo::library::Track], revision: u64) -> Option<Vec<Option<usize>>> {
        let id = self.open.as_ref()?;
        let Some(Ok(tracks)) = self.tracks.get(id) else { return None };
        if self.index.as_ref().is_none_or(|(built, _)| *built != revision) {
            let songs = app_tracks.iter().map(|track| (track.info.title.as_str(), track.info.artist.as_str()));
            self.index = Some((revision, LocalIndex::new(songs)));
        }
        let (_, index) = self.index.as_ref()?;
        Some(tracks.iter().map(|track| index.find(track)).collect())
    }
}

#[cfg(test)]
impl SpotifyPage {
    /// Shows these playlists as if Spotify had sent them. Nothing goes over the network.
    pub fn show_sample(&mut self, playlists: Vec<(Playlist, Vec<Track>)>) {
        self.login = Login::LoggedIn(Session::resume("test", "test", self.sender.clone(), Arc::clone(&self.wake)));
        self.requested = true;
        self.user = Some(User { id: "tester".to_owned(), name: "Tester".to_owned() });
        self.open = playlists.first().map(|(playlist, _)| playlist.id.clone());
        for (playlist, tracks) in &playlists {
            self.tracks.insert(playlist.id.clone(), Ok(tracks.clone()));
        }
        self.playlists = Some(Ok(playlists.into_iter().map(|(playlist, _)| playlist).collect()));
    }
}

/// Shows a page in the default browser.
fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::{
            Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
            core::{HSTRING, PCWSTR, w},
        };
        // SAFETY: Every string lives until the call returns.
        let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        // Values above 32 mean success.
        if result.0 as isize > 32 {
            return Ok(());
        }
    }
    Err(format!("Could not open the browser. Open this page yourself: {url}"))
}

/// Text that wraps onto more lines. Returns its height.
fn paragraph(ui: &Ui, left: f32, top: f32, width: f32, text: &str, font: egui::FontId, color: Color32) -> f32 {
    let galley = ui.painter().layout(text.to_owned(), font, color, width);
    let height = galley.size().y;
    ui.painter().galley(pos2(left, top), galley, Color32::PLACEHOLDER);
    height
}

/// The green or red label beside a song, ending at `right`.
fn paint_status(ui: &Ui, right: f32, center_y: f32, available: bool) {
    let (text, icon, color) =
        if available { ("Available", theme::CHECK, theme::AVAILABLE) } else { ("Missing", theme::CLOSE, theme::MISSING) };
    let galley = line(ui, text, theme::inter_semibold(12.0), color, 120.0);
    let width = 10.0 + 12.0 + 6.0 + galley.size().x + 12.0;
    let pill = Rect::from_min_max(pos2(right - width, center_y - 13.0), pos2(right, center_y + 13.0));
    ui.painter().rect(pill, 13.0, color.gamma_multiply(0.14), Stroke::new(1.0, color.gamma_multiply(0.45)), StrokeKind::Inside);
    let icon_rect = Rect::from_center_size(pos2(pill.left() + 16.0, center_y), Vec2::splat(12.0));
    paint_icon(ui, icon_rect, icon, color);
    paint_text(ui, Align2::LEFT_CENTER, pos2(pill.left() + 28.0, center_y), galley);
}

fn plural_count(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

impl App {
    pub fn spotify_action(&mut self, action: SpotifyAction) {
        let page = &mut self.spotify;
        match action {
            SpotifyAction::Open => {
                self.spotify_open = true;
                self.minimized = true;
                self.selected.clear();
            }
            SpotifyAction::LogIn => {
                page.error = None;
                match spotify::start_login(&page.client_id, page.sender.clone(), Arc::clone(&page.wake)) {
                    Ok(pending) => {
                        if let Err(err) = open_in_browser(&pending.url) {
                            page.error = Some(err);
                        }
                        page.login = Login::Waiting(pending);
                    }
                    Err(err) => page.error = Some(err),
                }
            }
            SpotifyAction::ShowLoginPage => {
                if let Login::Waiting(pending) = &page.login
                    && let Err(err) = open_in_browser(&pending.url)
                {
                    page.error = Some(err);
                }
            }
            SpotifyAction::CancelLogin => {
                if let Login::Waiting(pending) = &page.login {
                    pending.cancel.store(true, Ordering::Relaxed);
                }
                page.login = Login::LoggedOut;
                page.error = None;
            }
            SpotifyAction::LogOut => {
                page.login = Login::LoggedOut;
                page.forget();
                self.settings.spotify_token = None;
                self.apply(Action::SaveSettings);
            }
            SpotifyAction::Refresh => {
                page.tracks.clear();
                page.loading.clear();
                page.load_playlists();
                if let Some(id) = page.open.clone() {
                    page.open_playlist(id);
                }
            }
            SpotifyAction::OpenPlaylist(id) => page.open_playlist(id),
            SpotifyAction::ShowMissingOnly(missing_only) => page.missing_only = missing_only,
            SpotifyAction::Import => self.import_spotify_playlist(),
        }
    }

    fn import_spotify_playlist(&mut self) {
        let revision = self.library_revision;
        let Some(matches) = self.spotify.matches(&self.library.tracks, revision) else { return };
        let page = &self.spotify;
        let Some(name) = page
            .open
            .as_ref()
            .and_then(|id| page.playlists.as_ref()?.as_ref().ok()?.iter().find(|playlist| playlist.id == *id))
            .map(|playlist| playlist.name.clone())
        else {
            return;
        };
        // The Spotify order, with each song once, as a playlist can hold a song twice.
        let mut seen = HashSet::new();
        let songs: Vec<_> = matches
            .iter()
            .flatten()
            .filter(|&&track| seen.insert(track))
            .map(|&track| self.library.tracks[track].path.clone())
            .collect();
        let missing = matches.iter().filter(|found| found.is_none()).count();
        self.import_playlist(&name, &songs, missing);
    }

    /// Takes in what the background requests found.
    pub fn poll_spotify(&mut self, ctx: &egui::Context) {
        self.spotify.covers.poll(ctx);
        while let Ok(event) = self.spotify.events.try_recv() {
            let page = &mut self.spotify;
            match event {
                Event::LoggedIn { session, refresh_token } => {
                    if !matches!(page.login, Login::Waiting(_)) {
                        continue;
                    }
                    page.login = Login::LoggedIn(session);
                    page.forget();
                    page.load_playlists();
                    self.settings.spotify_client = Some(page.client_id.trim().to_owned());
                    self.settings.spotify_token = Some(refresh_token);
                    self.apply(Action::SaveSettings);
                }
                Event::LoginFailed(message) => {
                    if matches!(page.login, Login::Waiting(_)) {
                        page.login = Login::LoggedOut;
                        page.error = Some(message);
                    }
                }
                Event::TokenChanged(token) => {
                    if matches!(page.login, Login::LoggedIn(_)) {
                        self.settings.spotify_token = Some(token);
                        self.apply(Action::SaveSettings);
                    }
                }
                Event::LoggedOut(message) => {
                    if matches!(page.login, Login::LoggedIn(_)) {
                        page.login = Login::LoggedOut;
                        page.forget();
                        page.error = Some(message);
                        self.settings.spotify_token = None;
                        self.apply(Action::SaveSettings);
                    }
                }
                Event::Playlists(result) => {
                    if !matches!(page.login, Login::LoggedIn(_)) {
                        continue;
                    }
                    let result = result.map(|(user, playlists)| {
                        page.user = Some(user);
                        playlists
                    });
                    let first = result.as_ref().ok().and_then(|playlists| playlists.first()).map(|p| p.id.clone());
                    page.playlists = Some(result);
                    let still_there = page.playlists.as_ref().and_then(|result| result.as_ref().ok()).is_some_and(|list| {
                        page.open.as_ref().is_some_and(|open| list.iter().any(|playlist| playlist.id == *open))
                    });
                    if !still_there && let Some(first) = first {
                        page.open_playlist(first);
                    }
                }
                Event::Tracks { playlist, result } => {
                    page.loading.remove(&playlist);
                    if matches!(page.login, Login::LoggedIn(_)) {
                        page.tracks.insert(playlist, result);
                    }
                }
            }
        }
    }

    pub fn draw_spotify(&mut self, ui: &mut Ui, stage: Rect, actions: &mut Vec<Action>) {
        let bottom_space = if self.current_track().is_some() { MINI_HEIGHT + 32.0 } else { 24.0 };
        let content = Rect::from_min_max(stage.min, pos2(stage.right(), stage.bottom() - bottom_space));
        match self.spotify.login {
            Login::LoggedOut => self.draw_spotify_login(ui, content, actions),
            Login::Waiting(_) => self.draw_spotify_waiting(ui, content, actions),
            Login::LoggedIn(_) => {
                if !self.spotify.requested {
                    self.spotify.load_playlists();
                }
                self.draw_spotify_library(ui, content, actions);
            }
        }
    }

    fn draw_spotify_header(&self, ui: &Ui, content: Rect, title: &str, details: &str) {
        let left = content.left() + PAD_X;
        let width = content.width() - 2.0 * PAD_X;
        let label = spaced_line(ui, "SPOTIFY", theme::inter_semibold(12.0), theme::MUTED, width, 0.96);
        ui.painter().galley(pos2(left, content.top() + 30.0), label, Color32::PLACEHOLDER);
        let title = spaced_line(ui, title, theme::grotesk_bold(36.0), theme::TEXT, width * 0.6, -1.0);
        ui.painter().galley(pos2(left, content.top() + 48.0), title, Color32::PLACEHOLDER);
        let details = line(ui, details, theme::inter(13.0), theme::SOFT, width * 0.6);
        ui.painter().galley(pos2(left, content.top() + 98.0), details, Color32::PLACEHOLDER);
    }

    fn draw_spotify_login(&mut self, ui: &mut Ui, content: Rect, actions: &mut Vec<Action>) {
        let width = LOGIN_WIDTH.min(content.width() - 2.0 * PAD_X);
        let left = content.center().x - width / 2.0;
        let mut top = content.top() + (content.height() - 470.0).max(0.0) / 3.0 + 24.0;

        let icon = Rect::from_min_size(pos2(left, top), Vec2::splat(44.0));
        ui.painter().rect_filled(icon, 12.0, theme::NAV_ACTIVE);
        paint_icon(ui, icon.shrink(11.0), theme::CLOUD, theme::ACCENT);
        top += 60.0;
        let title = spaced_line(ui, "Connect Spotify", theme::grotesk_bold(30.0), theme::TEXT, width, -0.6);
        top += title.size().y + 8.0;
        ui.painter().galley(pos2(left, top - title.size().y - 8.0), title, Color32::PLACEHOLDER);
        let intro = "See your Spotify playlists, and which of their songs are already in your music folder.";
        top += paragraph(ui, left, top, width, intro, theme::inter(14.0), theme::SOFT) + 20.0;

        let steps = [
            "1. Create an app on developer.spotify.com/dashboard, with Web API selected.",
            "2. Add this redirect URI to the app:",
        ];
        for step in steps {
            top += paragraph(ui, left, top, width, step, theme::inter(13.0), theme::TEXT) + 6.0;
        }
        let uri_field = Rect::from_min_size(pos2(left + 16.0, top), vec2(width - 16.0, 34.0));
        ui.painter().rect(uri_field, 8.0, theme::PANEL, Stroke::new(1.0, theme::FIELD_LINE), StrokeKind::Inside);
        let uri = line(ui, REDIRECT_URI, theme::mono(12.0), theme::ACCENT, uri_field.width() - 90.0);
        paint_text(ui, Align2::LEFT_CENTER, pos2(uri_field.left() + 12.0, uri_field.center().y), uri);
        let copy = Rect::from_min_size(pos2(uri_field.right() - 66.0, uri_field.top() + 5.0), vec2(60.0, 24.0));
        if text_button(ui, copy, "Copy", false).clicked() {
            ui.ctx().copy_text(REDIRECT_URI.to_owned());
        }
        top += 34.0 + 10.0;
        top += paragraph(ui, left, top, width, "3. Paste the app's Client ID here:", theme::inter(13.0), theme::TEXT) + 8.0;

        let field = Rect::from_min_size(pos2(left, top), vec2(width, 40.0));
        ui.painter().rect(field, 10.0, theme::PANEL, Stroke::new(1.0, theme::FIELD_LINE), StrokeKind::Inside);
        let text = Rect::from_min_max(pos2(field.left() + 14.0, field.top()), pos2(field.right() - 14.0, field.bottom()));
        let hint = RichText::new("Client ID").font(theme::mono(13.0)).color(theme::MUTED);
        let edit = egui::TextEdit::singleline(&mut self.spotify.client_id)
            .id_salt("spotify-client")
            .hint_text(hint)
            .font(theme::mono(13.0))
            .text_color(theme::TEXT)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .align(Align2::LEFT_CENTER)
            .desired_width(text.width());
        let response = ui.put(text, edit);
        let entered = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        top += 40.0 + 16.0;

        let label = "Log in with Spotify";
        let button = Rect::from_min_size(pos2(left, top), vec2(button_width(ui, label) + 8.0, 42.0));
        if text_button(ui, button, label, true).clicked() || entered {
            actions.push(Action::Spotify(SpotifyAction::LogIn));
        }
        top += 42.0 + 14.0;
        if let Some(error) = &self.spotify.error {
            top += paragraph(ui, left, top, width, error, theme::inter(13.0), theme::WARNING) + 10.0;
        }
        let note = "Spotify allows this only when the app's owner has Premium. An app in development mode \
                    works for up to 5 people, which you add in its User Management.";
        paragraph(ui, left, top, width, note, theme::inter(12.0), theme::MUTED);
    }

    fn draw_spotify_waiting(&self, ui: &mut Ui, content: Rect, actions: &mut Vec<Action>) {
        let center = content.center();
        let width = (content.width() - 2.0 * PAD_X).min(LOGIN_WIDTH);
        let icon = Rect::from_center_size(center - vec2(0.0, 64.0), Vec2::splat(44.0));
        ui.painter().rect_filled(icon, 12.0, theme::NAV_ACTIVE);
        paint_icon(ui, icon.shrink(11.0), theme::CLOUD, theme::ACCENT);
        let title = line(ui, "Waiting for Spotify…", theme::grotesk_semibold(20.0), theme::TEXT, width);
        paint_text(ui, Align2::CENTER_CENTER, center - vec2(0.0, 18.0), title);
        let text = line(ui, "Finish the login in your browser.", theme::inter(13.0), theme::SOFT, width);
        paint_text(ui, Align2::CENTER_CENTER, center + vec2(0.0, 8.0), text);

        let again = "Open the login page again";
        let again_width = button_width(ui, again);
        let cancel_width = button_width(ui, "Cancel");
        let total = again_width + 12.0 + cancel_width;
        let again_rect = Rect::from_min_size(pos2(center.x - total / 2.0, center.y + 36.0), vec2(again_width, 38.0));
        let cancel_rect = Rect::from_min_size(pos2(again_rect.right() + 12.0, again_rect.top()), vec2(cancel_width, 38.0));
        if text_button(ui, again_rect, again, false).clicked() {
            actions.push(Action::Spotify(SpotifyAction::ShowLoginPage));
        }
        if text_button(ui, cancel_rect, "Cancel", false).clicked() {
            actions.push(Action::Spotify(SpotifyAction::CancelLogin));
        }
        if let Some(error) = &self.spotify.error {
            let error = line(ui, error, theme::inter(12.0), theme::WARNING, width);
            paint_text(ui, Align2::CENTER_CENTER, center + vec2(0.0, 98.0), error);
        }
    }

    fn draw_spotify_library(&mut self, ui: &mut Ui, content: Rect, actions: &mut Vec<Action>) {
        let count = match &self.spotify.playlists {
            Some(Ok(playlists)) => plural_count(playlists.len(), "playlist"),
            _ => String::new(),
        };
        let details = match (&self.spotify.user, count.is_empty()) {
            (Some(user), false) => format!("Logged in as {} · {count}", user.name),
            (Some(user), true) => format!("Logged in as {}", user.name),
            (None, _) => "Logged in".to_owned(),
        };
        self.draw_spotify_header(ui, content, "Your playlists", &details);

        let right = content.right() - PAD_X;
        let log_out = Rect::from_min_size(pos2(right - button_width(ui, "Log out"), content.top() + 56.0), vec2(button_width(ui, "Log out"), 36.0));
        let refresh = Rect::from_min_size(
            pos2(log_out.left() - 10.0 - button_width(ui, "Refresh"), log_out.top()),
            vec2(button_width(ui, "Refresh"), 36.0),
        );
        if text_button(ui, refresh, "Refresh", false).clicked() {
            actions.push(Action::Spotify(SpotifyAction::Refresh));
        }
        if text_button(ui, log_out, "Log out", false).clicked() {
            actions.push(Action::Spotify(SpotifyAction::LogOut));
        }

        let body = Rect::from_min_max(pos2(content.left() + PAD_X, content.top() + HEADER_HEIGHT), pos2(right, content.bottom()));
        let playlists = match &self.spotify.playlists {
            None => {
                let text = line(ui, "Loading your playlists…", theme::inter(14.0), theme::SOFT, body.width());
                paint_text(ui, Align2::LEFT_TOP, body.min + vec2(0.0, 8.0), text);
                return;
            }
            Some(Err(error)) => {
                paragraph(ui, body.left(), body.top() + 8.0, body.width().min(640.0), error, theme::inter(14.0), theme::WARNING);
                return;
            }
            Some(Ok(playlists)) if playlists.is_empty() => {
                let text = line(ui, "Your Spotify account has no playlists.", theme::inter(14.0), theme::SOFT, body.width());
                paint_text(ui, Align2::LEFT_TOP, body.min + vec2(0.0, 8.0), text);
                return;
            }
            Some(Ok(playlists)) => playlists.clone(),
        };

        let column = Rect::from_min_size(body.min, vec2(COLUMN_WIDTH.min(body.width() * 0.4), body.height()));
        ui.scope_builder(UiBuilder::new().id_salt("spotify-playlists").max_rect(column), |ui| {
            ui.set_clip_rect(column.intersect(ui.clip_rect()));
            ScrollArea::vertical()
                .id_salt("spotify-playlists-scroll")
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    ui.set_min_size(vec2(column.width(), playlists.len() as f32 * CARD_STEP));
                    for (row, playlist) in playlists.iter().enumerate() {
                        let top = row as f32 * CARD_STEP;
                        if top + CARD_STEP < viewport.top() || top > viewport.bottom() {
                            continue;
                        }
                        let card = at(origin, 0.0, top, column.width() - 8.0, CARD_HEIGHT);
                        self.draw_spotify_card(ui, card, playlist, actions);
                    }
                });
        });

        let panel = Rect::from_min_max(pos2(column.right() + COLUMN_GAP, body.top()), body.max);
        let open = self.spotify.open.as_ref().and_then(|id| playlists.iter().find(|playlist| playlist.id == *id)).cloned();
        if let Some(open) = open {
            self.draw_spotify_tracks(ui, panel, &open, actions);
        }
    }

    fn draw_spotify_card(&mut self, ui: &Ui, card: Rect, playlist: &Playlist, actions: &mut Vec<Action>) {
        let active = self.spotify.open.as_deref() == Some(playlist.id.as_str());
        let response = click_area(ui, card, &playlist.name);
        let fill = if active {
            theme::NAV_ACTIVE
        } else if response.hovered() {
            theme::NAV_HOVER
        } else {
            theme::PANEL
        };
        let stroke = if active { theme::ACCENT.gamma_multiply(0.5) } else { theme::LINE };
        ui.painter().rect(card, 12.0, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        if response.clicked() {
            actions.push(Action::Spotify(SpotifyAction::OpenPlaylist(playlist.id.clone())));
        }

        let cover = Rect::from_min_size(pos2(card.left() + 12.0, card.center().y - 24.0), Vec2::splat(48.0));
        match playlist.image.as_deref().and_then(|url| self.spotify.covers.texture(url)) {
            Some(texture) => paint_texture(ui, cover, texture, 8),
            None => paint_empty_cover(ui, cover, 8.0),
        }
        let left = cover.right() + 12.0;
        let width = (card.right() - 12.0 - left).max(0.0);
        let color = if active { theme::ACCENT } else { theme::TEXT };
        let name = line(ui, &playlist.name, theme::inter_semibold(14.0), color, width);
        let tracks = line(ui, &plural_count(playlist.tracks, "track"), theme::inter(12.0), theme::SOFT, width);
        let owner = line(ui, &playlist.owner, theme::inter(12.0), theme::MUTED, width);
        let height = name.size().y + tracks.size().y + owner.size().y + 4.0;
        let mut top = card.center().y - height / 2.0;
        for galley in [name, tracks, owner] {
            let step = galley.size().y + 2.0;
            ui.painter().galley(pos2(left, top), galley, Color32::PLACEHOLDER);
            top += step;
        }
    }

    fn draw_spotify_tracks(&mut self, ui: &mut Ui, panel: Rect, playlist: &Playlist, actions: &mut Vec<Action>) {
        ui.painter().rect(panel, 16.0, theme::PANEL, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
        let inner_left = panel.left() + 24.0;
        let inner_right = panel.right() - 24.0;
        // The import and filter buttons take the right side.
        let text_width = (inner_right - inner_left - 380.0).max(160.0);
        let title = line(ui, &playlist.name, theme::grotesk_semibold(22.0), theme::TEXT, text_width);
        ui.painter().galley(pos2(inner_left, panel.top() + 22.0), title, Color32::PLACEHOLDER);

        let revision = self.library_revision;
        let matches = self.spotify.matches(&self.library.tracks, revision);
        let tracks = match self.spotify.tracks.get(&playlist.id) {
            None => {
                let text = line(ui, "Loading songs…", theme::inter(13.0), theme::SOFT, inner_right - inner_left);
                paint_text(ui, Align2::LEFT_TOP, pos2(inner_left, panel.top() + 58.0), text);
                return;
            }
            Some(Err(error)) => {
                paragraph(ui, inner_left, panel.top() + 58.0, inner_right - inner_left, error, theme::inter(13.0), theme::WARNING);
                return;
            }
            Some(Ok(tracks)) => tracks.clone(),
        };
        let matches = matches.unwrap_or_else(|| vec![None; tracks.len()]);
        let available = matches.iter().filter(|found| found.is_some()).count();
        let summary = format!("{available} of {} available locally", tracks.len());
        let summary = line(ui, &summary, theme::inter(13.0), theme::SOFT, text_width);
        ui.painter().galley(pos2(inner_left, panel.top() + 56.0), summary, Color32::PLACEHOLDER);

        let missing = tracks.len() - available;
        let filters = [("All".to_owned(), false), (format!("Missing {missing}"), true)];
        let mut right = inner_right;
        for (label, missing_only) in filters.iter().rev() {
            let width = button_width(ui, label);
            let rect = Rect::from_min_size(pos2(right - width, panel.top() + 26.0), vec2(width, 32.0));
            let chosen = self.spotify.missing_only == *missing_only;
            if text_button(ui, rect, label, chosen).clicked() {
                actions.push(Action::Spotify(SpotifyAction::ShowMissingOnly(*missing_only)));
            }
            right = rect.left() - 8.0;
        }
        let import = "Import as playlist";
        let width = button_width(ui, import);
        let rect = Rect::from_min_size(pos2(right - 8.0 - width, panel.top() + 26.0), vec2(width, 32.0));
        if text_button(ui, rect, import, false).on_hover_text("Saves the available songs as an .m3u playlist").clicked() {
            actions.push(Action::Spotify(SpotifyAction::Import));
        }

        let divider = panel.top() + PANEL_HEADER;
        ui.painter().hline(panel.left() + 1.0..=panel.right() - 1.0, divider, Stroke::new(1.0, theme::LINE));
        let rows: Vec<usize> =
            (0..tracks.len()).filter(|&row| !self.spotify.missing_only || matches[row].is_none()).collect();
        let list = Rect::from_min_max(pos2(panel.left() + 8.0, divider + 6.0), pos2(panel.right() - 8.0, panel.bottom() - 8.0));
        if rows.is_empty() {
            let text = if tracks.is_empty() { "This playlist has no songs." } else { "Every song is in your music folder." };
            let text = line(ui, text, theme::inter(13.0), theme::SOFT, list.width());
            paint_text(ui, Align2::LEFT_TOP, list.min + vec2(16.0, 12.0), text);
            return;
        }
        ui.scope_builder(UiBuilder::new().id_salt("spotify-tracks").max_rect(list), |ui| {
            ui.set_clip_rect(list.intersect(ui.clip_rect()));
            ScrollArea::vertical()
                .id_salt(("spotify-tracks-scroll", &playlist.id))
                .auto_shrink(false)
                .scroll_source(ScrollSource::MOUSE_WHEEL | ScrollSource::SCROLL_BAR)
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    ui.set_min_size(vec2(list.width(), rows.len() as f32 * ROW_HEIGHT));
                    let first = (viewport.top() / ROW_HEIGHT).floor().max(0.0) as usize;
                    let last = ((viewport.bottom() / ROW_HEIGHT).ceil().max(0.0) as usize).min(rows.len());
                    for (shown, &row) in rows.iter().enumerate().take(last).skip(first) {
                        let rect = at(origin, 0.0, shown as f32 * ROW_HEIGHT, list.width() - 8.0, ROW_HEIGHT);
                        self.paint_spotify_track(ui, rect, &tracks[row], matches[row].is_some());
                    }
                });
        });
    }

    fn paint_spotify_track(&mut self, ui: &Ui, rect: Rect, track: &Track, available: bool) {
        let hover = ui.interact(rect, ui.scope_id().with(("spotify-row", rect.top() as i32)), Sense::hover());
        if hover.hovered() {
            ui.painter().rect_filled(rect, 10.0, theme::FULL_ROW_HOVER);
        }
        let center_y = rect.center().y;
        let cover = Rect::from_min_size(pos2(rect.left() + 12.0, center_y - 20.0), Vec2::splat(40.0));
        match track.image.as_deref().and_then(|url| self.spotify.covers.texture(url)) {
            Some(texture) => paint_texture(ui, cover, texture, 6),
            None => paint_empty_cover(ui, cover, 6.0),
        }
        let left = cover.right() + 12.0;
        let width = (rect.right() - 130.0 - left).max(0.0);
        let artists = if track.artists.is_empty() { "Unknown artist".to_owned() } else { track.artists.join(", ") };
        let details = if track.album.is_empty() { artists } else { format!("{artists} • {}", track.album) };
        let title = line(ui, &track.name, theme::inter_semibold(14.0), theme::TEXT, width);
        let details = line(ui, &details, theme::inter(12.0), theme::SOFT, width);
        paint_two_lines(ui, left, center_y, 3.0, title, details);
        paint_status(ui, rect.right() - 12.0, center_y, available);
    }
}
