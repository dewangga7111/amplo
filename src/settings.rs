//! What the app remembers between runs, in a small text file.

use std::{collections::BTreeSet, fmt::Write as _, fs, io, path::PathBuf};

use crate::{library::Sort, lyrics::Extra};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub folder: Option<PathBuf>,
    pub volume: f32,
    pub liked: BTreeSet<PathBuf>,
    /// The Client ID of the user's own app on the Spotify dashboard.
    pub spotify_client: Option<String>,
    /// Keeps the user logged in to Spotify between runs.
    pub spotify_token: Option<String>,
    /// What shows under each lyric line when the lyrics file has it. `None` shows nothing.
    pub lyrics_extra: Option<Extra>,
    /// The order of every song list except albums.
    pub sort: Sort,
    /// Shuffle and repeat stay as the user left them.
    pub shuffle: bool,
    pub repeat: bool,
    /// Lets phones on the same network sync with this computer.
    pub sync: bool,
    /// The keys of the phones that paired for sync.
    pub sync_keys: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: None,
            volume: 0.7,
            liked: BTreeSet::new(),
            spotify_client: None,
            spotify_token: None,
            lyrics_extra: Some(Extra::Romaji),
            sort: Sort::default(),
            shuffle: false,
            repeat: false,
            sync: false,
            sync_keys: BTreeSet::new(),
        }
    }
}

impl Settings {
    /// `%APPDATA%\amplo\state.txt`. `None` when the environment has no such folder.
    pub fn default_path() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("amplo").join("state.txt"))
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        // Windows editors often save UTF-8 with a byte order mark before the first key.
        for line in text.trim_start_matches('\u{feff}').lines() {
            match line.split_once('=') {
                Some(("folder", value)) if !value.is_empty() => settings.folder = Some(PathBuf::from(value)),
                Some(("volume", value)) => {
                    if let Ok(volume) = value.parse::<f32>() {
                        settings.volume = volume.clamp(0.0, 1.0);
                    }
                }
                Some(("liked", value)) if !value.is_empty() => {
                    settings.liked.insert(PathBuf::from(value));
                }
                Some(("spotify_client", value)) if !value.is_empty() => settings.spotify_client = Some(value.to_owned()),
                Some(("spotify_token", value)) if !value.is_empty() => settings.spotify_token = Some(value.to_owned()),
                Some(("lyrics_extra", "romaji")) => settings.lyrics_extra = Some(Extra::Romaji),
                Some(("lyrics_extra", "translation")) => settings.lyrics_extra = Some(Extra::Translation),
                Some(("lyrics_extra", "none")) => settings.lyrics_extra = None,
                Some(("shuffle", value)) => settings.shuffle = value == "on",
                Some(("repeat", value)) => settings.repeat = value == "on",
                Some(("sync", value)) => settings.sync = value == "on",
                Some(("sync_key", value)) if !value.is_empty() => {
                    settings.sync_keys.insert(value.to_owned());
                }
                Some(("sort", value)) => {
                    if let Some(sort) = Sort::from_key(value) {
                        settings.sort = sort;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    pub fn serialize(&self) -> String {
        let mut text = String::new();
        if let Some(folder) = &self.folder {
            let _ = writeln!(text, "folder={}", folder.display());
        }
        let _ = writeln!(text, "volume={}", self.volume);
        for path in &self.liked {
            let _ = writeln!(text, "liked={}", path.display());
        }
        if let Some(client) = &self.spotify_client {
            let _ = writeln!(text, "spotify_client={client}");
        }
        if let Some(token) = &self.spotify_token {
            let _ = writeln!(text, "spotify_token={token}");
        }
        let extra = match self.lyrics_extra {
            Some(Extra::Romaji) => "romaji",
            Some(Extra::Translation) => "translation",
            None => "none",
        };
        let _ = writeln!(text, "lyrics_extra={extra}");
        let _ = writeln!(text, "sort={}", self.sort.key());
        let on = |value: bool| if value { "on" } else { "off" };
        let _ = writeln!(text, "shuffle={}", on(self.shuffle));
        let _ = writeln!(text, "repeat={}", on(self.repeat));
        let _ = writeln!(text, "sync={}", on(self.sync));
        for key in &self.sync_keys {
            let _ = writeln!(text, "sync_key={key}");
        }
        text
    }

    /// `queue.txt` beside the settings file.
    pub fn queue_path(settings_path: &std::path::Path) -> PathBuf {
        settings_path.with_file_name("queue.txt")
    }

    pub fn load() -> Self {
        Self::default_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .map_or_else(Self::default, |text| Self::parse(&text))
    }

    pub fn save(&self, path: &std::path::Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.serialize())
    }
}

/// The queue when amplo last ran, so it comes back after a restart. It lives in a file of its
/// own, because it can hold thousands of songs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SavedQueue {
    /// The play order.
    pub tracks: Vec<PathBuf>,
    /// The order of the list the songs came from. Empty when it is the play order.
    pub original: Vec<PathBuf>,
    pub position: usize,
    /// How far the song had played.
    pub seconds: f32,
    /// The name of the playlist the songs came from, if any.
    pub playlist: Option<String>,
}

impl SavedQueue {
    pub fn parse(text: &str) -> Self {
        let mut queue = Self::default();
        for line in text.trim_start_matches('\u{feff}').lines() {
            match line.split_once('=') {
                Some(("position", value)) => queue.position = value.parse().unwrap_or(0),
                Some(("seconds", value)) => queue.seconds = value.parse::<f32>().unwrap_or(0.0).max(0.0),
                Some(("playlist", value)) if !value.is_empty() => queue.playlist = Some(value.to_owned()),
                Some(("track", value)) if !value.is_empty() => queue.tracks.push(PathBuf::from(value)),
                Some(("original", value)) if !value.is_empty() => queue.original.push(PathBuf::from(value)),
                _ => {}
            }
        }
        queue
    }

    pub fn serialize(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "position={}", self.position);
        let _ = writeln!(text, "seconds={}", self.seconds);
        if let Some(playlist) = &self.playlist {
            let _ = writeln!(text, "playlist={playlist}");
        }
        for path in &self.tracks {
            let _ = writeln!(text, "track={}", path.display());
        }
        if self.original != self.tracks {
            for path in &self.original {
                let _ = writeln!(text, "original={}", path.display());
            }
        }
        text
    }

    /// The order of the list the songs came from.
    pub fn original(&self) -> &[PathBuf] {
        if self.original.is_empty() { &self.tracks } else { &self.original }
    }

    pub fn load(path: &std::path::Path) -> Option<Self> {
        Some(Self::parse(&fs::read_to_string(path).ok()?)).filter(|queue| !queue.tracks.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_queue() {
        let queue = SavedQueue {
            tracks: vec![PathBuf::from("G:\\Music\\b = 1.flac"), PathBuf::from("G:\\Music\\a.mp3")],
            original: vec![PathBuf::from("G:\\Music\\a.mp3"), PathBuf::from("G:\\Music\\b = 1.flac")],
            position: 1,
            seconds: 81.5,
            playlist: Some("Road Trip".to_owned()),
        };
        assert_eq!(SavedQueue::parse(&queue.serialize()), queue);
        // A queue in the order of its list keeps that order once.
        let in_order = SavedQueue { original: Vec::new(), ..queue };
        assert!(!in_order.serialize().contains("original="));
        assert_eq!(SavedQueue::parse(&in_order.serialize()).original(), in_order.tracks.as_slice());
    }

    #[test]
    fn round_trips_paths_with_spaces_and_equals_signs() {
        let settings = Settings {
            folder: Some(PathBuf::from("G:\\Music")),
            volume: 0.25,
            liked: BTreeSet::from([PathBuf::from("G:\\Music\\a = b - 光.flac")]),
            spotify_client: Some("0123abcd".to_owned()),
            spotify_token: Some("AQ-x_y".to_owned()),
            lyrics_extra: Some(Extra::Translation),
            sort: Sort::Artist,
            shuffle: true,
            repeat: true,
            sync: true,
            sync_keys: BTreeSet::from(["0a1b".to_owned(), "ff00".to_owned()]),
        };
        assert_eq!(Settings::parse(&settings.serialize()), settings);
        let settings = Settings { lyrics_extra: None, ..settings };
        assert_eq!(Settings::parse(&settings.serialize()), settings);
    }

    #[test]
    fn reads_a_first_line_after_a_byte_order_mark() {
        assert_eq!(Settings::parse("\u{feff}volume=0\r\n").volume, 0.0);
    }

    #[test]
    fn ignores_damaged_values() {
        let settings = Settings::parse("volume=loud\nunknown=1\nno separator\nliked=\n");
        assert_eq!(settings, Settings::default());
        assert_eq!(Settings::parse("volume=9").volume, 1.0);
    }
}
