//! The state of the player, and everything it does apart from drawing.

use std::{
    collections::{HashMap, HashSet, hash_map::RandomState},
    fs,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use eframe::egui;
use amplo::{
    audio::open_source,
    library::{self, ALL_SONGS, LIKED_SONGS, Library, TrackInfo},
    lyrics::Lyrics,
    settings::Settings,
    spectrum::{Analyzer, SampleBuffer, Tap},
    watch::FolderWatcher,
};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

#[cfg(windows)]
use crate::media_controls::{MediaButton, MediaControls, NowPlaying, Status};
#[cfg(windows)]
use crate::tray::{Tray, TrayCommand};
use crate::{covers::Covers, theme};

pub const APP_NAME: &str = "amplo";
pub const VISUALIZER_BARS: usize = 28;
/// How often the app checks for the end of a track when nothing is being drawn.
const TICK: Duration = Duration::from_millis(250);
/// Tags read between two repaints while the library scan runs.
const SCAN_REPAINT_EVERY: usize = 32;
/// Previous restarts the track after this much of it has played.
const RESTART_AFTER: Duration = Duration::from_secs(3);
const NO_AUDIO: &str = "No audio files found in that folder.";

pub struct Queue {
    /// `None` once a rescan finds that the playlist no longer exists.
    pub playlist: Option<usize>,
    /// The playlist's own order, which comes back when shuffle turns off.
    pub original: Vec<usize>,
    /// The play order, decided before playback starts. Next, Previous, and the end of a
    /// track all walk this list, so going back always finds the song played before.
    pub tracks: Vec<usize>,
    pub position: usize,
}

/// What the card beside the cover shows in Now Playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTab {
    Lyrics,
    Queue,
}

pub enum Action {
    Toggle,
    Next,
    Previous,
    PlayRow(usize),
    PlayView,
    /// Turns shuffle on and plays the shown playlist in a new random order.
    ShufflePlayView,
    /// Jumps to a song in the queue. Its order stays as it is.
    PlayQueue(usize),
    ShowCardTab(CardTab),
    OpenPlaylist(usize),
    /// Maximizes the window, which switches to the full layout, or restores the compact one.
    ToggleFull,
    /// The close button. The window hides in the notification area and the music keeps playing.
    HideToTray,
    Expand,
    Minimize,
    ToggleLike,
    ToggleShuffle,
    ToggleRepeat,
    OpenFolder,
    Seek(f32),
    SaveSettings,
}

/// What a finished library scan is for.
enum LoadKind {
    /// Replaces the library. `folder` is what to remember as the saved folder.
    Open { autoplay: bool, roots: Vec<PathBuf>, folder: Option<PathBuf> },
    /// Updates the library after files changed, and leaves playback alone.
    Refresh,
}

enum ScanEvent {
    Loaded { generation: u64, library: Library, kind: LoadKind },
    Info { generation: u64, track: usize, info: TrackInfo },
    Finished { generation: u64 },
    FolderChanged,
}

/// Movement of the lyric highlight between two lines.
#[derive(Default)]
pub struct LyricMotion {
    pub active: Option<usize>,
    pub previous: Option<usize>,
    /// 0 when the highlight has just left `previous`, 1 when it has reached `active`.
    pub mix: f32,
    pub scroll: Option<f32>,
    pub scroll_velocity: f32,
    /// True while the highlight or the scroll position still has to reach its goal.
    pub moving: bool,
}

pub struct App {
    pub ctx: egui::Context,
    pub library: Library,
    pub view: usize,
    pub queue: Option<Queue>,
    pub minimized: bool,
    pub card_tab: CardTab,
    pub shuffle: bool,
    pub repeat: bool,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    roots: Vec<PathBuf>,
    watcher: Option<FolderWatcher>,
    pub loading: bool,
    pub scanning: bool,
    generation: u64,
    scan_generation: Arc<AtomicU64>,
    events: Sender<ScanEvent>,
    scan_events: Receiver<ScanEvent>,
    /// Opened on first playback, so a missing audio device shows as an error and not a crash.
    sink: Option<MixerDeviceSink>,
    /// One `Player` per track. Dropping it stops the track.
    pub player: Option<Player>,
    pub duration: Option<Duration>,
    /// Slider position while the user holds it. The seek happens on release.
    pub seek_preview: Option<f32>,
    pub tech: Vec<String>,
    pub lyrics: Option<Lyrics>,
    pub lyric_motion: LyricMotion,
    samples: Arc<SampleBuffer>,
    pub analyzer: Analyzer,
    pub covers: Covers,
    pub error: Option<String>,
    pub slide: f32,
    pub mini: f32,
    /// When long titles in Now Playing started from their beginning, in UI time.
    pub marquee_started: f64,
    /// True while the window is maximized, which shows the full layout.
    pub full: bool,
    /// The search box of the full layout. It filters the shown playlist.
    pub search: String,
    window_title: String,
    /// Media keys and the Windows media panel. Missing in tests, which have no window.
    #[cfg(windows)]
    media: Option<MediaControls>,
    /// The notification area icon. Missing in tests, which have no window.
    #[cfg(windows)]
    tray: Option<Tray>,
}

pub fn format_time(seconds: f32) -> String {
    let total = seconds as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_sample_rate(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} KHZ", rate / 1000)
    } else {
        format!("{:.1} KHZ", f64::from(rate) / 1000.0)
    }
}

fn format_channels(channels: u16) -> String {
    match channels {
        1 => "MONO".to_owned(),
        2 => "STEREO".to_owned(),
        count => format!("{count} CH"),
    }
}

/// Uses the file's tags first. A decoded stream still tells its rate and channels, and the
/// file size and length give an average bitrate.
fn tech_lines(path: &Path, info: &TrackInfo, source: Option<(u32, u16)>, duration: Option<Duration>) -> Vec<String> {
    let bitrate = info.bitrate_kbps.or_else(|| {
        let seconds = duration?.as_secs_f64();
        let bytes = fs::metadata(path).ok()?.len();
        (seconds > 0.0).then(|| (bytes as f64 * 8.0 / seconds / 1000.0) as u32)
    });
    let rate = info.sample_rate.or(source.map(|(rate, _)| rate));
    let channels = info.channels.map(u16::from).or(source.map(|(_, channels)| channels));
    let mut lines = Vec::with_capacity(3);
    lines.extend(bitrate.map(|bitrate| format!("{bitrate} KBPS")));
    lines.extend(rate.map(format_sample_rate));
    lines.extend(channels.map(format_channels));
    lines
}

fn random_below(limit: usize) -> usize {
    (RandomState::new().build_hasher().finish() % limit.max(1) as u64) as usize
}

/// Returns `tracks` in a random order. With `first`, the song at that index goes in front.
fn shuffled(tracks: &[usize], first: Option<usize>) -> Vec<usize> {
    let mut order = tracks.to_vec();
    let start = match first {
        Some(index) if index < order.len() => {
            order.swap(0, index);
            1
        }
        _ => 0,
    };
    // Fisher-Yates: every order of the remaining songs is equally likely.
    let count = order.len();
    for index in start..count.saturating_sub(1) {
        order.swap(index, index + random_below(count - index));
    }
    order
}

impl App {
    pub fn new(
        ctx: egui::Context,
        settings: Settings,
        settings_path: Option<PathBuf>,
        startup_paths: Vec<PathBuf>,
    ) -> Self {
        theme::install(&ctx);
        let (events, scan_events) = mpsc::channel();
        let covers = Covers::new(&ctx);
        let saved_folder = settings.folder.clone();
        let mut app = Self {
            ctx,
            library: Library::default(),
            view: ALL_SONGS,
            queue: None,
            minimized: true,
            card_tab: CardTab::Lyrics,
            shuffle: false,
            repeat: false,
            settings,
            settings_path,
            roots: Vec::new(),
            watcher: None,
            loading: false,
            scanning: false,
            generation: 0,
            scan_generation: Arc::new(AtomicU64::new(0)),
            events,
            scan_events,
            sink: None,
            player: None,
            duration: None,
            seek_preview: None,
            tech: Vec::new(),
            lyrics: None,
            lyric_motion: LyricMotion::default(),
            samples: Arc::new(SampleBuffer::default()),
            analyzer: Analyzer::new(VISUALIZER_BARS),
            covers,
            error: None,
            slide: 0.0,
            mini: 0.0,
            marquee_started: 0.0,
            full: false,
            search: String::new(),
            window_title: APP_NAME.to_owned(),
            #[cfg(windows)]
            media: None,
            #[cfg(windows)]
            tray: None,
        };
        if !startup_paths.is_empty() {
            let kind = LoadKind::Open { autoplay: true, roots: startup_paths.clone(), folder: None };
            app.load_library(startup_paths, kind);
        } else if let Some(folder) = saved_folder {
            let roots = vec![folder.clone()];
            app.load_library(vec![folder], LoadKind::Open { autoplay: false, roots, folder: None });
        }
        app
    }

    /// Scans on a thread, because a large folder would freeze the window. Tags arrive one
    /// by one afterward, and until then new tracks show titles from their file names.
    fn load_library(&mut self, paths: Vec<PathBuf>, kind: LoadKind) {
        self.generation += 1;
        let generation = self.generation;
        self.scan_generation.store(generation, Ordering::Relaxed);
        self.loading = true;

        let current = Arc::clone(&self.scan_generation);
        let events = self.events.clone();
        let liked = self.settings.liked.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let library = library::load(paths, &liked);
            let files: Vec<PathBuf> = library.tracks.iter().map(|track| track.path.clone()).collect();
            if events.send(ScanEvent::Loaded { generation, library, kind }).is_err() {
                return;
            }
            ctx.request_repaint();
            for (track, path) in files.iter().enumerate() {
                if current.load(Ordering::Relaxed) != generation {
                    return;
                }
                let info = library::read_info(path);
                if events.send(ScanEvent::Info { generation, track, info }).is_err() {
                    return;
                }
                if track % SCAN_REPAINT_EVERY == 0 {
                    ctx.request_repaint();
                }
            }
            let _ = events.send(ScanEvent::Finished { generation });
            ctx.request_repaint();
        });
    }

    /// Loads a library the way a saved folder would, without the settings having one.
    #[cfg(test)]
    pub fn with_test_library(mut self, folder: PathBuf) -> Self {
        let kind = LoadKind::Open { autoplay: false, roots: vec![folder.clone()], folder: None };
        self.load_library(vec![folder], kind);
        self
    }

    fn receive_scan_events(&mut self) {
        while let Ok(event) = self.scan_events.try_recv() {
            match event {
                ScanEvent::Loaded { generation, library, kind } if generation == self.generation => {
                    self.loading = false;
                    match kind {
                        LoadKind::Open { autoplay, roots, folder } => {
                            self.install_library(library, autoplay, roots, folder);
                        }
                        LoadKind::Refresh => self.refresh_library(library),
                    }
                }
                ScanEvent::FolderChanged => self.load_library(self.roots.clone(), LoadKind::Refresh),
                ScanEvent::Info { generation, track, info } if generation == self.generation => {
                    if let Some(entry) = self.library.tracks.get_mut(track) {
                        entry.info = info;
                    }
                }
                ScanEvent::Finished { generation } if generation == self.generation => self.scanning = false,
                _ => {}
            }
        }
    }

    fn watch_roots(&mut self, roots: Vec<PathBuf>) {
        let events = self.events.clone();
        let ctx = self.ctx.clone();
        let watcher = FolderWatcher::new(&roots, move || {
            let _ = events.send(ScanEvent::FolderChanged);
            ctx.request_repaint();
        });
        self.roots = roots;
        self.watcher = match watcher {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                self.error = Some(format!("New songs appear after a restart. Could not watch the folder: {err}"));
                None
            }
        };
    }

    /// Applies a rescan. The track that plays goes on, and the queue and the shown playlist
    /// follow their new positions.
    fn refresh_library(&mut self, mut library: Library) {
        if library.tracks.is_empty() {
            // Most likely an unplugged drive, so the list stays for when it returns.
            self.error = Some(NO_AUDIO.to_owned());
            return;
        }
        if self.error.as_deref() == Some(NO_AUDIO) {
            self.error = None;
        }

        // Known tags stay in place, so titles do not flicker while the new scan reads them.
        let known: HashMap<&Path, &TrackInfo> =
            self.library.tracks.iter().map(|track| (track.path.as_path(), &track.info)).collect();
        for track in &mut library.tracks {
            if let Some(&info) = known.get(track.path.as_path()) {
                track.info = info.clone();
            }
        }
        let positions: HashMap<&Path, usize> =
            library.tracks.iter().enumerate().map(|(index, track)| (track.path.as_path(), index)).collect();
        let playlist_named = |old: usize| {
            let name = &self.library.playlists.get(old)?.name;
            library.playlists.iter().position(|playlist| playlist.name == *name)
        };

        let view = playlist_named(self.view)
            .filter(|&index| !library.playlists[index].tracks.is_empty())
            .unwrap_or(ALL_SONGS);

        let mut removed = None;
        let queue = self.queue.as_ref().and_then(|queue| {
            let path = &self.library.tracks[*queue.tracks.get(queue.position)?].path;
            let Some(&current) = positions.get(path.as_path()) else {
                removed = Some(self.library.tracks[queue.tracks[queue.position]].info.title.clone());
                return None;
            };
            let remap = |old: &[usize]| -> Vec<usize> {
                old.iter()
                    .filter_map(|&track| positions.get(self.library.tracks[track].path.as_path()).copied())
                    .collect()
            };
            let playlist = queue.playlist.and_then(playlist_named);
            let original = playlist
                .map(|index| library.playlists[index].tracks.clone())
                .filter(|tracks| tracks.contains(&current))
                .unwrap_or_else(|| remap(&queue.original));
            let tracks = if self.shuffle {
                // The order so far stays, so Previous still works. New songs join at the end.
                let wanted: HashSet<usize> = original.iter().copied().collect();
                let mut order = remap(&queue.tracks);
                order.retain(|track| wanted.contains(track));
                let known: HashSet<usize> = order.iter().copied().collect();
                let new_songs: Vec<usize> = wanted.difference(&known).copied().collect();
                order.extend(shuffled(&new_songs, None));
                order
            } else {
                original.clone()
            };
            let position = tracks.iter().position(|&track| track == current)?;
            Some(Queue { playlist, original, tracks, position })
        });

        if let Some(title) = removed {
            self.stop();
            self.error = Some(format!("{title} was removed from the folder."));
        } else {
            self.queue = queue;
        }
        self.view = view;
        self.library = library;
        self.scanning = true;
    }

    fn install_library(&mut self, library: Library, autoplay: bool, roots: Vec<PathBuf>, folder: Option<PathBuf>) {
        if library.tracks.is_empty() {
            self.error = Some(NO_AUDIO.to_owned());
            return;
        }
        self.stop();
        self.view = ALL_SONGS;
        self.error = (library.missing_entries > 0)
            .then(|| format!("{} playlist entries were not found.", library.missing_entries));
        self.library = library;
        self.scanning = true;
        self.watch_roots(roots);
        if let Some(folder) = folder {
            self.settings.folder = Some(folder);
            self.save_settings();
        }
        if autoplay {
            self.play_view(None);
            self.minimized = false;
        }
    }

    fn save_settings(&mut self) {
        let Some(path) = &self.settings_path else { return };
        if let Err(err) = self.settings.save(path) {
            self.error = Some(format!("Could not save settings: {err}"));
        }
    }

    pub fn current_track(&self) -> Option<usize> {
        let queue = self.queue.as_ref()?;
        queue.tracks.get(queue.position).copied()
    }

    pub fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(|player| !player.is_paused())
    }

    /// The rows of the shown playlist that match the search, by their position in it. The
    /// search applies to the full layout only, where its box is.
    pub fn visible_rows(&self) -> Vec<usize> {
        let Some(playlist) = self.library.playlists.get(self.view) else { return Vec::new() };
        let query = self.search.trim().to_lowercase();
        if !self.full || query.is_empty() {
            return (0..playlist.tracks.len()).collect();
        }
        (0..playlist.tracks.len())
            .filter(|&row| {
                let info = &self.library.tracks[playlist.tracks[row]].info;
                [&info.title, &info.artist, &info.album].iter().any(|text| text.to_lowercase().contains(&query))
            })
            .collect()
    }

    pub fn is_liked(&self, track: usize) -> bool {
        self.library.tracks.get(track).is_some_and(|track| self.settings.liked.contains(&track.path))
    }

    /// Where the display and sliders should stand, in seconds.
    pub fn position(&self) -> f32 {
        let played = self.player.as_ref().map_or(0.0, |player| player.get_pos().as_secs_f32());
        let played = self.duration.map_or(played, |total| played.min(total.as_secs_f32()));
        self.seek_preview.unwrap_or(played)
    }

    /// Makes the shown playlist the queue and starts it, at the given row or else from the
    /// top of the play order. On shuffle, this decides a new random order first.
    fn play_view(&mut self, first_row: Option<usize>) {
        let Some(playlist) = self.library.playlists.get(self.view) else { return };
        let original = playlist.tracks.clone();
        if original.is_empty() || first_row.is_some_and(|row| row >= original.len()) {
            return;
        }
        let (tracks, position) = if self.shuffle {
            (shuffled(&original, first_row), 0)
        } else {
            (original.clone(), first_row.unwrap_or(0))
        };
        self.queue = Some(Queue { playlist: Some(self.view), original, tracks, position });
        self.play_at(position);
    }

    /// Reorders the queue around the current song, which keeps playing.
    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        let shuffle = self.shuffle;
        let Some(queue) = &mut self.queue else { return };
        let Some(&current) = queue.tracks.get(queue.position) else { return };
        if shuffle {
            queue.tracks = shuffled(&queue.tracks, Some(queue.position));
            queue.position = 0;
        } else {
            queue.tracks.clone_from(&queue.original);
            queue.position = queue.tracks.iter().position(|&track| track == current).unwrap_or(0);
        }
    }

    fn play_at(&mut self, position: usize) {
        self.player = None;
        self.duration = None;
        self.seek_preview = None;
        self.lyric_motion = LyricMotion::default();
        self.restart_marquee();
        let Some(queue) = &mut self.queue else { return };
        queue.position = position;
        let Some(&track) = queue.tracks.get(position) else { return };

        let path = self.library.tracks[track].path.clone();
        let info = library::read_info(&path);
        self.library.tracks[track].info = info.clone();
        self.lyrics = Lyrics::for_track(&path);

        let started = self.start(&path);
        self.tech = tech_lines(&path, &info, started.as_ref().ok().copied(), self.duration);
        self.error = started.err().map(|err| format!("{}: {err}", info.title));
    }

    fn restart_marquee(&mut self) {
        self.marquee_started = self.ctx.input(|input| input.time);
    }

    /// Returns the sample rate and channel count of the decoded audio.
    fn start(&mut self, path: &Path) -> Result<(u32, u16), String> {
        let source = open_source(path)?;
        if self.sink.is_none() {
            let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|err| err.to_string())?;
            sink.log_on_drop(false);
            self.sink = Some(sink);
        }
        let Some(sink) = &self.sink else {
            unreachable!("sink was opened above");
        };
        let format = (source.sample_rate().get(), source.channels().get());
        let player = Player::connect_new(sink.mixer());
        player.set_volume(self.settings.volume);
        self.duration = source.total_duration();
        player.append(Tap::new(source, Arc::clone(&self.samples)));
        self.player = Some(player);
        Ok(format)
    }

    fn stop(&mut self) {
        self.player = None;
        self.queue = None;
        self.duration = None;
        self.seek_preview = None;
        self.lyrics = None;
        self.tech.clear();
        self.minimized = true;
    }

    fn toggle(&mut self) {
        match &self.player {
            Some(player) if player.is_paused() => player.play(),
            Some(player) => player.pause(),
            None => match &self.queue {
                Some(queue) => self.play_at(queue.position),
                None => self.play_view(None),
            },
        }
    }

    fn next(&mut self) {
        let Some(queue) = &self.queue else { return };
        self.play_at((queue.position + 1) % queue.tracks.len());
    }

    fn previous(&mut self) {
        let Some(queue) = &self.queue else { return };
        let (position, len) = (queue.position, queue.tracks.len());
        let restart = self.player.as_ref().is_some_and(|player| player.get_pos() > RESTART_AFTER);
        if restart {
            self.seek(0.0);
        } else {
            self.play_at((position + len - 1) % len);
        }
    }

    /// Moves on after a track ends. Unplayable files are skipped, so one bad file does not
    /// stop the rest of the queue. The last skip stays visible as the error message.
    fn advance(&mut self) {
        let Some(queue) = &self.queue else { return };
        let (mut position, len) = (queue.position, queue.tracks.len());
        if self.repeat {
            self.play_at(position);
            return;
        }
        let mut skipped = None;
        for _ in 0..len {
            position = (position + 1) % len;
            self.play_at(position);
            if self.player.is_some() {
                self.error = skipped;
                return;
            }
            skipped = self.error.take();
        }
        self.error = skipped;
    }

    fn seek(&mut self, seconds: f32) {
        if let Some(player) = &self.player
            && let Err(err) = player.try_seek(Duration::from_secs_f32(seconds.max(0.0)))
        {
            self.error = Some(format!("Could not seek: {err}"));
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.settings.volume = volume;
        if let Some(player) = &self.player {
            player.set_volume(volume);
        }
    }

    fn toggle_like(&mut self) {
        let Some(track) = self.current_track() else { return };
        let path = self.library.tracks[track].path.clone();
        if !self.settings.liked.remove(&path) {
            self.settings.liked.insert(path);
        }
        self.library.refresh_liked(&self.settings.liked);
        if self.view == LIKED_SONGS && self.library.playlists[LIKED_SONGS].tracks.is_empty() {
            self.view = ALL_SONGS;
        }
        self.save_settings();
    }

    fn open_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            let roots = vec![folder.clone()];
            let kind = LoadKind::Open { autoplay: false, roots: roots.clone(), folder: Some(folder) };
            self.load_library(roots, kind);
        }
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::Toggle => self.toggle(),
            Action::Next => self.next(),
            Action::Previous => self.previous(),
            Action::PlayRow(row) => {
                self.play_view(Some(row));
                // A chosen song opens Now Playing, where it shows.
                if self.current_track().is_some() {
                    self.minimized = false;
                    self.restart_marquee();
                }
            }
            Action::PlayView => self.play_view(None),
            Action::ShufflePlayView => {
                self.shuffle = true;
                self.play_view(None);
            }
            Action::PlayQueue(position) => self.play_at(position),
            Action::ShowCardTab(tab) => self.card_tab = tab,
            Action::OpenPlaylist(index) => {
                self.view = index;
                self.minimized = true;
            }
            Action::Expand => {
                self.minimized = false;
                self.restart_marquee();
            }
            Action::Minimize => self.minimized = true,
            Action::ToggleFull => self.ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!self.full)),
            Action::HideToTray => self.hide_to_tray(),
            Action::ToggleLike => self.toggle_like(),
            Action::ToggleShuffle => self.toggle_shuffle(),
            Action::ToggleRepeat => self.repeat = !self.repeat,
            Action::OpenFolder => self.open_folder(),
            Action::Seek(seconds) => self.seek(seconds),
            Action::SaveSettings => self.save_settings(),
        }
    }

    /// Without a tray icon, a hidden window could not be brought back, so it only minimizes.
    fn hide_to_tray(&mut self) {
        #[cfg(windows)]
        if let Some(tray) = &mut self.tray
            && tray.show_icon()
        {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    /// The icon stays in the notification area for as long as the app runs, as Spotify's does.
    #[cfg(windows)]
    pub fn attach_tray(&mut self, window: windows::Win32::Foundation::HWND) {
        self.tray = Tray::new(window);
        if let Some(tray) = &mut self.tray {
            tray.show_icon();
        }
    }

    /// eframe still updates a hidden window ten times a second, so a click shows within that.
    #[cfg(windows)]
    fn poll_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &mut self.tray else { return };
        match tray.take_command() {
            Some(TrayCommand::Show) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(TrayCommand::Quit) => {
                tray.hide_icon();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            None => {}
        }
    }

    /// Without the controls, the app still plays. Only media keys and the panel are missing.
    #[cfg(windows)]
    pub fn attach_media_controls(&mut self, window: windows::Win32::Foundation::HWND) {
        let ctx = self.ctx.clone();
        self.media = MediaControls::new(window, move || ctx.request_repaint()).ok();
    }

    /// Windows sends Play to a paused player and Pause to a playing one. Each is checked
    /// anyway, so a key pressed twice in quick succession cannot undo itself.
    #[cfg(windows)]
    pub fn press(&mut self, button: MediaButton) {
        match button {
            MediaButton::Play if !self.is_playing() => self.toggle(),
            MediaButton::Pause if self.is_playing() => self.toggle(),
            MediaButton::Play | MediaButton::Pause => {}
            MediaButton::Next => self.next(),
            MediaButton::Previous => self.previous(),
        }
    }

    #[cfg(windows)]
    fn sync_media_controls(&mut self) {
        let Some(media) = &self.media else { return };
        for button in media.presses() {
            self.press(button);
        }
        let now = self.current_track().map(|track| {
            let track = &self.library.tracks[track];
            NowPlaying { path: track.path.clone(), title: track.info.title.clone(), artist: track.info.artist.clone() }
        });
        let status = match &self.player {
            None => Status::Stopped,
            Some(player) if player.is_paused() => Status::Paused,
            Some(_) => Status::Playing,
        };
        if let Some(media) = &mut self.media {
            media.update(now, status);
        }
    }

    /// Follows the playing audio. Runs while the window is minimized too.
    pub fn update_playback(&mut self, ctx: &egui::Context) {
        self.receive_scan_events();
        self.covers.poll(ctx);
        if self.player.as_ref().is_some_and(Player::empty) {
            self.advance();
        }
        #[cfg(windows)]
        self.sync_media_controls();
        #[cfg(windows)]
        self.poll_tray(ctx);
        self.sync_window_title(ctx);
        if self.is_playing() {
            ctx.request_repaint_after(TICK);
        }
    }

    pub fn update_visualizer(&mut self, dt: f32) {
        if self.is_playing() {
            self.analyzer.update(&self.samples, dt);
        } else {
            self.analyzer.decay(dt);
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.current_track() {
            Some(track) => format!("{} - {APP_NAME}", self.library.tracks[track].info.title),
            None => APP_NAME.to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time_with_optional_hours() {
        assert_eq!(format_time(5.9), "0:05");
        assert_eq!(format_time(65.0), "1:05");
        assert_eq!(format_time(3725.0), "1:02:05");
    }

    #[test]
    fn shuffling_keeps_every_song_and_puts_the_chosen_one_first() {
        let tracks: Vec<usize> = (10..40).collect();
        let order = shuffled(&tracks, Some(7));
        assert_eq!(order[0], 17);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, tracks);

        let orders: HashSet<Vec<usize>> = (0..20).map(|_| shuffled(&tracks, None)).collect();
        assert!(orders.len() > 1, "the order never changed");
        assert!(shuffled(&[], None).is_empty());
        assert_eq!(shuffled(&[5], Some(0)), [5]);
    }

    #[test]
    fn formats_technical_values() {
        assert_eq!(format_sample_rate(44_100), "44.1 KHZ");
        assert_eq!(format_sample_rate(96_000), "96 KHZ");
        assert_eq!(format_channels(1), "MONO");
        assert_eq!(format_channels(2), "STEREO");
        assert_eq!(format_channels(6), "6 CH");
    }

    #[test]
    fn tech_lines_fall_back_to_the_decoded_stream() {
        let info = TrackInfo { bitrate_kbps: Some(320), sample_rate: None, channels: None, ..TrackInfo::default() };
        let lines = tech_lines(Path::new("none"), &info, Some((48_000, 2)), None);
        assert_eq!(lines, ["320 KBPS", "48 KHZ", "STEREO"]);
        assert!(tech_lines(Path::new("none"), &TrackInfo::default(), None, None).is_empty());
    }
}
