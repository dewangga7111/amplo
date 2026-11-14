//! The state of the player, and everything it does apart from drawing.

use std::{
    collections::{BTreeSet, HashMap, HashSet, hash_map::RandomState},
    fs,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use amplo::{
    audio::open_source,
    library::{self, ALL_SONGS, Collection, LIKED_SONGS, Library, Sort, TrackInfo},
    lyrics::{Extra, Lyrics},
    settings::Settings,
    spectrum::{Analyzer, SampleBuffer, Tap},
    watch::FolderWatcher,
};
use rodio::{
    DeviceSinkBuilder, DeviceSinkError, MixerDeviceSink, Player, Source,
    cpal::{
        self,
        traits::{DeviceTrait, HostTrait},
    },
};

#[cfg(windows)]
use crate::media_controls::{MediaButton, MediaControls, NowPlaying, Status};
#[cfg(windows)]
use crate::tray::{Tray, TrayCommand};
use crate::{
    covers::Covers,
    spotify_page::{SpotifyAction, SpotifyPage},
    theme,
    ui::plural_songs,
};

pub const APP_NAME: &str = "amplo";
pub const VISUALIZER_BARS: usize = 28;
/// How often the app checks for the end of a track when nothing is being drawn.
const TICK: Duration = Duration::from_millis(250);
/// Tags read between two repaints while the library scan runs.
const SCAN_REPAINT_EVERY: usize = 32;
/// Previous restarts the track after this much of it has played.
const RESTART_AFTER: Duration = Duration::from_secs(3);
const NO_AUDIO: &str = "No audio files found in that folder.";
/// Shown while the audio output cannot be opened again, for example right after sleep.
const NO_OUTPUT: &str = "No audio output";
/// How long a confirmation stays in the title bar.
const NOTICE_SECONDS: f64 = 3.0;
/// How long the app waits for the clipboard text after asking for it.
const PASTE_WAIT: Duration = Duration::from_secs(1);
/// How often a lost audio output is opened again until a device answers.
const REOPEN_INTERVAL: Duration = Duration::from_secs(1);
/// How often the output is compared with the default device of Windows.
const DEVICE_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// The default output device of Windows right now.
fn default_device_id() -> Option<String> {
    cpal::default_host().default_output_device().and_then(|device| device.id().ok()).map(|id| id.to_string())
}

/// The message with its causes, which rodio leaves out of its own text.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// How a click changes the selected rows, as in a file manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Only this row. A right click on a row outside the selection does this.
    Only,
    /// Ctrl+click adds the row, or takes it out.
    Toggle,
    /// Shift+click selects every shown row from the last clicked one to this one.
    Range,
}

/// The form that names a new playlist, while it is open.
#[derive(Default)]
pub struct NewPlaylist {
    pub name: String,
    /// Songs the playlist starts with.
    pub songs: Vec<PathBuf>,
    pub error: Option<String>,
    /// The text box takes the keyboard when the form opens, and again after an error.
    pub wants_focus: bool,
}

/// The form that writes or changes the lyrics of a song, while it is open.
pub struct LyricsEditor {
    pub track: PathBuf,
    pub title: String,
    pub text: String,
    pub error: Option<String>,
    pub wants_focus: bool,
}

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
    /// Opens the form that names a new playlist.
    NewPlaylist,
    /// Opens the same form, for a playlist that starts with the selected songs.
    NewPlaylistFromSelection,
    /// Saves the playlist named in the form.
    CreatePlaylist,
    /// Changes which rows of the shown playlist are selected.
    Select(usize, Selection),
    SelectAll,
    ClearSelection,
    /// Appends the selected songs to the playlist at this index.
    AddToPlaylist(usize),
    /// Takes the selected songs out of the shown playlist.
    RemoveFromPlaylist,
    /// Saves the text on the clipboard as the lyrics of the song that plays.
    PasteLyrics,
    /// Opens the form that writes or changes the lyrics of the song that plays.
    EditLyrics,
    SaveLyrics,
    CloseLyricsEditor,
    /// Shows the next of nothing, romaji, and translation under each lyric line.
    CycleLyricsExtra,
    /// Shows the songs of an album, an artist, or a genre.
    OpenCollection(Collection),
    /// Changes the order of every song list except albums.
    SetSort(Sort),
    Expand,
    Minimize,
    ToggleLike,
    ToggleShuffle,
    ToggleRepeat,
    OpenFolder,
    Seek(f32),
    SaveSettings,
    Spotify(SpotifyAction),
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
    /// Set by the audio thread when the output stops, as it does after sleep or when the
    /// device goes away. Also set when Windows picks another default device.
    pub audio_lost: Arc<AtomicBool>,
    last_reopen: Option<Instant>,
    /// The default device when the output opened. After sleep, Windows can make another
    /// device the default for a moment, such as while a monitor's speaker wakes up, and the
    /// output can stay there after the default changes back. See [`App::check_output_device`].
    pub sink_device: Option<String>,
    last_device_check: Option<Instant>,
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
    pub new_playlist: Option<NewPlaylist>,
    /// A playlist just created, which opens once the rescan finds it.
    open_when_found: Option<String>,
    /// Selected rows of the shown playlist, by their position in it.
    pub selected: BTreeSet<usize>,
    /// Where the next Shift+click range starts.
    select_anchor: Option<usize>,
    /// A short confirmation, such as songs added to a playlist, and when it goes away.
    pub notice: Option<(String, f64)>,
    pub lyrics_editor: Option<LyricsEditor>,
    /// Until then, a paste from the clipboard becomes the lyrics of the song that plays.
    pub paste_lyrics_until: Option<Instant>,
    pub spotify: SpotifyPage,
    /// True while the full layout shows the Spotify page in place of a playlist.
    pub spotify_open: bool,
    /// Counts changes to the library's songs and tags, so work that depends on them knows
    /// when to start again.
    pub library_revision: u64,
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
        let spotify = SpotifyPage::new(&ctx, settings.spotify_client.as_deref(), settings.spotify_token.as_deref());
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
            audio_lost: Arc::new(AtomicBool::new(false)),
            last_reopen: None,
            sink_device: None,
            last_device_check: None,
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
            new_playlist: None,
            open_when_found: None,
            selected: BTreeSet::new(),
            select_anchor: None,
            notice: None,
            lyrics_editor: None,
            paste_lyrics_until: None,
            spotify,
            spotify_open: false,
            library_revision: 0,
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
                    self.library_revision += 1;
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
                        self.library_revision += 1;
                    }
                }
                ScanEvent::Finished { generation } if generation == self.generation => {
                    self.scanning = false;
                    // Sorting by the tags waits for all of them, so the rows move only once.
                    if self.library.sort_songs() {
                        self.clear_selection_if_sorted();
                    }
                    self.update_shown_collection();
                }
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
        library.set_sort(self.settings.sort);
        // The album, artist, or genre shown stays, with the songs it has now.
        if let Some(collection) = self.library.shown_collection() {
            let songs = library.collection(collection);
            library.show_collection(songs);
        }
        let positions: HashMap<&Path, usize> =
            library.tracks.iter().enumerate().map(|(index, track)| (track.path.as_path(), index)).collect();
        // An album can have the name of a playlist, so the kind has to match too.
        let playlist_named = |old: usize| {
            let old = self.library.playlists.get(old)?;
            library.playlists.iter().position(|playlist| playlist.name == old.name && playlist.collection == old.collection)
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
        if view != self.view || self.library.playlists.get(view).map(|p| &p.tracks) != library.playlists.get(view).map(|p| &p.tracks) {
            // The rows moved, so the selection would point at other songs.
            self.selected.clear();
            self.select_anchor = None;
        }
        self.view = view;
        self.library = library;
        self.scanning = true;
        if let Some(name) = self.open_when_found.take() {
            match self.library.playlists.iter().position(|playlist| playlist.name == name) {
                Some(index) => {
                    self.view = index;
                    self.minimized = true;
                }
                // The file is there, so a later rescan finds it.
                None => self.open_when_found = Some(name),
            }
        }
    }

    fn select(&mut self, row: usize, how: Selection) {
        match how {
            Selection::Only => {
                self.selected = BTreeSet::from([row]);
                self.select_anchor = Some(row);
            }
            Selection::Toggle => {
                if !self.selected.remove(&row) {
                    self.selected.insert(row);
                }
                self.select_anchor = Some(row);
            }
            Selection::Range => {
                // The range follows the shown order, which a search can shorten.
                let rows = self.visible_rows();
                let anchor = self.select_anchor.unwrap_or(row);
                let (Some(from), Some(to)) =
                    (rows.iter().position(|&r| r == anchor), rows.iter().position(|&r| r == row))
                else {
                    self.selected = BTreeSet::from([row]);
                    self.select_anchor = Some(row);
                    return;
                };
                self.selected = rows[from.min(to)..=from.max(to)].iter().copied().collect();
            }
        }
    }

    /// The selected songs in playlist order.
    fn selected_songs(&self) -> Vec<PathBuf> {
        let Some(playlist) = self.library.playlists.get(self.view) else { return Vec::new() };
        self.selected
            .iter()
            .filter_map(|&row| playlist.tracks.get(row))
            .map(|&track| self.library.tracks[track].path.clone())
            .collect()
    }

    /// Shows a short confirmation in the title bar for a few seconds.
    fn notify(&mut self, text: String) {
        let until = self.ctx.input(|input| input.time) + NOTICE_SECONDS;
        self.notice = Some((text, until));
        self.ctx.request_repaint_after_secs(NOTICE_SECONDS as f32);
    }

    fn add_to_playlist(&mut self, index: usize) {
        let songs = self.selected_songs();
        let Some(playlist) = self.library.playlists.get(index) else { return };
        let Some(path) = playlist.path.clone() else { return };
        let name = playlist.name.clone();
        match library::add_to_playlist(&path, &songs) {
            Ok(0) if songs.len() == 1 => self.notify(format!("Already in {name}.")),
            Ok(0) => self.notify(format!("These songs are already in {name}.")),
            Ok(added) => {
                let skipped = songs.len() - added;
                let text = if skipped == 0 {
                    format!("Added {} to {name}.", plural_songs(added))
                } else {
                    format!("Added {} to {name}. Skipped {} already in it.", plural_songs(added), plural_songs(skipped))
                };
                self.notify(text);
                self.load_library(self.roots.clone(), LoadKind::Refresh);
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// Writes the playlist file into the music folder, and rescans so it shows.
    fn create_playlist(&mut self) {
        let Some(form) = &mut self.new_playlist else { return };
        let Some(folder) = self.roots.iter().find(|root| root.is_dir()) else {
            form.error = Some("Open a music folder first.".to_owned());
            return;
        };
        match library::create_playlist(folder, &form.name, &form.songs) {
            Ok(name) => {
                self.new_playlist = None;
                self.open_when_found = Some(name);
                self.load_library(self.roots.clone(), LoadKind::Refresh);
            }
            Err(err) => {
                form.error = Some(err);
                form.wants_focus = true;
            }
        }
    }

    /// Saves songs found for a Spotify playlist as a playlist file in the music folder. A taken
    /// name gets a number. `missing` counts the songs that were not found.
    pub fn import_playlist(&mut self, name: &str, songs: &[PathBuf], missing: usize) {
        if songs.is_empty() {
            self.notify("None of these songs are in your music folder yet.".to_owned());
            return;
        }
        let Some(folder) = self.roots.iter().find(|root| root.is_dir()) else {
            self.error = Some("Open a music folder first.".to_owned());
            return;
        };
        let taken: Vec<String> = self.library.playlists.iter().map(|playlist| playlist.name.clone()).collect();
        match library::create_numbered_playlist(folder, name, songs, &taken) {
            Ok(created) => {
                let imported = format!("Imported {} as “{created}”.", plural_songs(songs.len()));
                let text = if missing == 0 {
                    imported
                } else {
                    format!("{imported} Left out {} not in your folder.", plural_songs(missing))
                };
                self.notify(text);
                self.load_library(self.roots.clone(), LoadKind::Refresh);
            }
            Err(err) => self.error = Some(err),
        }
    }

    fn install_library(&mut self, mut library: Library, autoplay: bool, roots: Vec<PathBuf>, folder: Option<PathBuf>) {
        if library.tracks.is_empty() {
            self.error = Some(NO_AUDIO.to_owned());
            return;
        }
        library.set_sort(self.settings.sort);
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

        let started = self.start(&path, false);
        self.tech = tech_lines(&path, &info, started.as_ref().ok().copied(), self.duration);
        self.error = started.err().map(|err| format!("{}: {err}", info.title));
    }

    fn restart_marquee(&mut self) {
        self.marquee_started = self.ctx.input(|input| input.time);
    }

    /// Opens the default output device. Its errors mark the output as lost, and the next
    /// update opens it again. See [`App::recover_audio`].
    ///
    /// The default device follows Windows when it switches devices. If it fails to open, the
    /// device that is the default right now is opened directly. Other devices are never
    /// tried, because they can be silent ones, such as Steam Streaming Speakers. A speaker in
    /// a monitor is missing for a moment after sleep, and the next try finds it again.
    fn open_sink(&self) -> Result<MixerDeviceSink, String> {
        let lost = Arc::clone(&self.audio_lost);
        let ctx = self.ctx.clone();
        let on_error = move |_: cpal::Error| {
            lost.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        };
        let open = |builder: Result<DeviceSinkBuilder, DeviceSinkError>| {
            builder.and_then(|builder| builder.with_error_callback(on_error.clone()).open_sink_or_fallback())
        };

        let mut sink = match open(DeviceSinkBuilder::from_default_device()) {
            Ok(sink) => sink,
            Err(err) => {
                let host = cpal::default_host();
                let default_id = host.default_output_device().and_then(|device| device.id().ok());
                let device = default_id.and_then(|id| {
                    host.output_devices().ok()?.find(|device| device.id().ok().as_ref() == Some(&id))
                });
                device
                    .and_then(|device| open(DeviceSinkBuilder::from_device(device)).ok())
                    .ok_or_else(|| error_chain(&err))?
            }
        };
        sink.log_on_drop(false);
        Ok(sink)
    }

    /// Returns the sample rate and channel count of the decoded audio.
    fn start(&mut self, path: &Path, paused: bool) -> Result<(u32, u16), String> {
        let source = open_source(path)?;
        // An output left on another device, as after sleep, opens again on the default one.
        if self.sink.is_some() && default_device_id().is_some_and(|current| Some(current) != self.sink_device) {
            self.sink = None;
        }
        if self.sink.is_none() {
            self.sink = Some(self.open_sink()?);
            self.sink_device = default_device_id();
        }
        let Some(sink) = &self.sink else {
            unreachable!("sink was opened above");
        };
        let format = (source.sample_rate().get(), source.channels().get());
        let player = Player::connect_new(sink.mixer());
        if paused {
            player.pause();
        }
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
            Some(player) if player.is_paused() => {
                player.play();
                // The PC may have slept while the track was paused.
                self.check_output_device(true);
            }
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

    /// Shows the songs of an album, an artist, or a genre, in album and track order. Playing
    /// from it queues all of them.
    fn open_collection(&mut self, collection: Collection) {
        let songs = self.library.collection(&collection);
        if songs.tracks.is_empty() {
            return;
        }
        let replaced = self.library.shown_collection().is_some_and(|shown| *shown != collection);
        let index = self.library.show_collection(songs);
        if replaced && let Some(queue) = &mut self.queue
            && queue.playlist == Some(index)
        {
            // The queue came from the collection shown before, which this one replaces.
            queue.playlist = None;
        }
        self.view = index;
        self.selected.clear();
        self.select_anchor = None;
        self.search.clear();
        self.minimized = true;
        self.spotify_open = false;
    }

    /// Changes the order of the song lists, and remembers it.
    fn set_sort(&mut self, sort: Sort) {
        self.settings.sort = sort;
        self.save_settings();
        if self.library.set_sort(sort) {
            self.clear_selection_if_sorted();
        }
    }

    /// The rows of a sorted list moved, so its selection would point at other songs.
    fn clear_selection_if_sorted(&mut self) {
        if self.library.playlists.get(self.view).is_some_and(library::Playlist::follows_sort) {
            self.selected.clear();
            self.select_anchor = None;
        }
    }

    /// Songs found by the scan join the collection shown. Their tags come in after the song list.
    fn update_shown_collection(&mut self) {
        let Some(collection) = self.library.shown_collection() else { return };
        let songs = self.library.collection(collection);
        let index = self.library.playlists.len() - 1;
        if self.library.playlists[index].tracks != songs.tracks {
            if self.view == index {
                self.selected.clear();
                self.select_anchor = None;
            }
            self.library.playlists[index] = songs;
        }
    }

    /// What can show under the lines of the current lyrics, starting with nothing.
    pub fn lyric_extra_choices(&self) -> Vec<Option<Extra>> {
        let mut choices = vec![None];
        if let Some(lyrics) = &self.lyrics {
            choices.extend([Extra::Romaji, Extra::Translation].into_iter().filter(|&extra| lyrics.has(extra)).map(Some));
        }
        choices
    }

    /// What shows under each lyric line: the chosen kind, when these lyrics have it.
    pub fn lyric_extra(&self) -> Option<Extra> {
        self.settings.lyrics_extra.filter(|&extra| self.lyrics.as_ref().is_some_and(|lyrics| lyrics.has(extra)))
    }

    /// Saves `text` as the `.lrc` file beside the song that plays, and shows it at once.
    fn write_lyrics(&mut self, text: &str) -> Result<String, String> {
        let track = self.current_track().ok_or("Play a song first, then add its lyrics.")?;
        let lyrics = Lyrics::parse(text).ok_or("These lyrics have no lines.")?;
        let path = self.library.tracks[track].path.clone();
        let mut text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\n', "\r\n");
        if !text.ends_with("\r\n") {
            text.push_str("\r\n");
        }
        fs::write(path.with_extension("lrc"), text).map_err(|err| format!("Could not save the lyrics: {err}"))?;
        self.lyrics = Some(lyrics);
        self.lyric_motion = LyricMotion::default();
        Ok(self.library.tracks[track].info.title.clone())
    }

    /// Lyrics pasted from the clipboard, or dropped on the window as a file.
    pub fn add_lyrics(&mut self, text: &str) {
        match self.write_lyrics(text) {
            Ok(title) => self.notify(format!("Saved the lyrics of {title}.")),
            Err(err) => self.error = Some(err),
        }
    }

    fn edit_lyrics(&mut self) {
        let Some(track) = self.current_track() else { return };
        let track = &self.library.tracks[track];
        let bytes = fs::read(track.path.with_extension("lrc")).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').replace("\r\n", "\n");
        self.lyrics_editor = Some(LyricsEditor {
            track: track.path.clone(),
            title: track.info.title.clone(),
            text,
            error: None,
            wants_focus: true,
        });
    }

    fn save_lyrics(&mut self) {
        let Some(editor) = self.lyrics_editor.take() else { return };
        // The song may have changed while the form was open.
        if !self.current_track().is_some_and(|track| self.library.tracks[track].path == editor.track) {
            self.lyrics_editor = Some(LyricsEditor { error: Some("This song stopped playing.".to_owned()), ..editor });
            return;
        }
        match self.write_lyrics(&editor.text) {
            Ok(title) => self.notify(format!("Saved the lyrics of {title}.")),
            Err(err) => self.lyrics_editor = Some(LyricsEditor { error: Some(err), wants_focus: true, ..editor }),
        }
    }

    /// Takes the selected songs out of the shown playlist. The song files stay.
    fn remove_from_playlist(&mut self) {
        let songs = self.selected_songs();
        let Some(playlist) = self.library.playlists.get(self.view) else { return };
        let name = playlist.name.clone();
        let count = plural_songs(songs.len());
        if self.view == LIKED_SONGS {
            for song in &songs {
                self.settings.liked.remove(song);
            }
            self.library.refresh_liked(&self.settings.liked);
            if self.library.playlists[LIKED_SONGS].tracks.is_empty() {
                self.view = ALL_SONGS;
            }
            self.save_settings();
        } else {
            let Some(path) = playlist.path.clone() else { return };
            if let Err(err) = library::remove_from_playlist(&path, &songs) {
                self.error = Some(err);
                return;
            }
            self.load_library(self.roots.clone(), LoadKind::Refresh);
        }
        self.selected.clear();
        self.select_anchor = None;
        self.notify(format!("Removed {count} from {name}."));
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
                self.selected.clear();
                self.select_anchor = Some(row);
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
                if index != self.view {
                    self.selected.clear();
                    self.select_anchor = None;
                }
                self.view = index;
                self.minimized = true;
                self.spotify_open = false;
            }
            Action::Expand => {
                self.minimized = false;
                self.restart_marquee();
            }
            Action::Minimize => self.minimized = true,
            Action::ToggleFull => self.ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!self.full)),
            Action::HideToTray => self.hide_to_tray(),
            Action::NewPlaylist => {
                self.new_playlist = Some(NewPlaylist { wants_focus: true, ..NewPlaylist::default() });
            }
            Action::NewPlaylistFromSelection => {
                let songs = self.selected_songs();
                self.new_playlist = Some(NewPlaylist { songs, wants_focus: true, ..NewPlaylist::default() });
            }
            Action::CreatePlaylist => self.create_playlist(),
            Action::Select(row, how) => self.select(row, how),
            Action::SelectAll => {
                self.selected = self.visible_rows().into_iter().collect();
                self.select_anchor = None;
            }
            Action::ClearSelection => {
                self.selected.clear();
                self.select_anchor = None;
            }
            Action::AddToPlaylist(index) => self.add_to_playlist(index),
            Action::RemoveFromPlaylist => self.remove_from_playlist(),
            Action::PasteLyrics => {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                self.paste_lyrics_until = Some(Instant::now() + PASTE_WAIT);
            }
            Action::EditLyrics => self.edit_lyrics(),
            Action::SaveLyrics => self.save_lyrics(),
            Action::CloseLyricsEditor => self.lyrics_editor = None,
            Action::OpenCollection(collection) => self.open_collection(collection),
            Action::SetSort(sort) => self.set_sort(sort),
            Action::CycleLyricsExtra => {
                let choices = self.lyric_extra_choices();
                let current = choices.iter().position(|&choice| choice == self.lyric_extra()).unwrap_or(0);
                self.settings.lyrics_extra = choices[(current + 1) % choices.len()];
                self.save_settings();
            }
            Action::ToggleLike => self.toggle_like(),
            Action::ToggleShuffle => self.toggle_shuffle(),
            Action::ToggleRepeat => self.repeat = !self.repeat,
            Action::OpenFolder => self.open_folder(),
            Action::Seek(seconds) => self.seek(seconds),
            Action::SaveSettings => self.save_settings(),
            Action::Spotify(action) => self.spotify_action(action),
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
        let ctx = self.ctx.clone();
        self.tray = Tray::new(window, move || ctx.request_repaint());
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

    /// Windows stops the audio stream when the PC sleeps or the device goes away, and the
    /// track then stands still. This opens the output again and continues the track where it
    /// was, playing or paused as before. Until a device answers, it tries again each second.
    fn recover_audio(&mut self, ctx: &egui::Context) {
        if !self.audio_lost.load(Ordering::Relaxed) {
            return;
        }
        if self.sink.is_none() {
            // Nothing is open, so the next track opens a new output anyway.
            self.audio_lost.store(false, Ordering::Relaxed);
            return;
        }
        if self.last_reopen.is_some_and(|last| last.elapsed() < REOPEN_INTERVAL) {
            ctx.request_repaint_after(REOPEN_INTERVAL);
            return;
        }
        self.last_reopen = Some(Instant::now());
        self.audio_lost.store(false, Ordering::Relaxed);
        let sink = match self.open_sink() {
            Ok(sink) => sink,
            Err(err) => {
                self.audio_lost.store(true, Ordering::Relaxed);
                self.error = Some(format!("{NO_OUTPUT}: {err}"));
                ctx.request_repaint_after(REOPEN_INTERVAL);
                return;
            }
        };
        if self.error.as_deref().is_some_and(|error| error.starts_with(NO_OUTPUT)) {
            self.error = None;
        }
        let resume = self.player.take().map(|player| (player.get_pos(), player.is_paused()));
        self.sink = Some(sink);
        self.sink_device = default_device_id();
        let (Some((position, paused)), Some(track)) = (resume, self.current_track()) else { return };
        let path = self.library.tracks[track].path.clone();
        match self.start(&path, paused) {
            Ok(_) => {
                if let Some(player) = &self.player
                    && let Err(err) = player.try_seek(position)
                {
                    self.error = Some(format!("Could not seek: {err}"));
                }
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// Marks the output as lost when it is not on the default device of Windows, so the next
    /// update opens it there. The stream's own error does not come when the default changes
    /// while the output opens, as it can after sleep. `now` checks at once, as before playing.
    pub fn check_output_device(&mut self, now: bool) {
        if self.sink.is_none() || (!now && self.last_device_check.is_some_and(|last| last.elapsed() < DEVICE_CHECK_INTERVAL)) {
            return;
        }
        self.last_device_check = Some(Instant::now());
        let current = default_device_id();
        // No default at all is a missing device, which the stream reports itself.
        if current.is_some() && current != self.sink_device {
            self.audio_lost.store(true, Ordering::Relaxed);
            self.ctx.request_repaint();
        }
    }

    /// Follows the playing audio. Runs while the window is minimized too.
    pub fn update_playback(&mut self, ctx: &egui::Context) {
        self.receive_scan_events();
        self.covers.poll(ctx);
        self.poll_spotify(ctx);
        if self.is_playing() {
            self.check_output_device(false);
        }
        self.recover_audio(ctx);
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
