//! The music library: tracks, their tags, and playlists.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::Accessor,
};
use walkdir::WalkDir;

use crate::audio::collect_tracks;

pub const ALL_SONGS: usize = 0;
pub const LIKED_SONGS: usize = 1;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub duration: Option<Duration>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub info: TrackInfo,
}

impl Track {
    /// Starts with the title and artist from the file name, until the tags have been read.
    pub fn from_path(path: PathBuf) -> Self {
        let (title, artist) = name_from_file(&path);
        Self { path, info: TrackInfo { title, artist, ..TrackInfo::default() } }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    /// Always starts with All Songs and Liked Songs, followed by the `.m3u` playlists.
    pub playlists: Vec<Playlist>,
    /// Playlist entries that point to a file outside the library, or to a missing file.
    pub missing_entries: usize,
}

/// Splits the `Title - Artist` file name pattern. Anything else counts as a title only.
fn name_from_file(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    match stem.rsplit_once(" - ") {
        Some((title, artist)) => (title.to_owned(), artist.to_owned()),
        None => (stem, String::new()),
    }
}

/// Reads the tags and audio properties. Values a file does not have come from its name.
pub fn read_info(path: &Path) -> TrackInfo {
    let (file_title, file_artist) = name_from_file(path);
    let mut info = TrackInfo { title: file_title, artist: file_artist, ..TrackInfo::default() };

    let tagged = Probe::open(path).ok().and_then(|probe| {
        let probe = probe.options(ParseOptions::new().read_cover_art(false));
        probe.guess_file_type().ok()?.read().ok()
    });
    let Some(tagged) = tagged else {
        return info;
    };

    let properties = tagged.properties();
    info.duration = Some(properties.duration()).filter(|duration| !duration.is_zero());
    info.bitrate_kbps = properties.audio_bitrate().filter(|&rate| rate > 0);
    info.sample_rate = properties.sample_rate().filter(|&rate| rate > 0);
    info.channels = properties.channels().filter(|&channels| channels > 0);

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        let set = |target: &mut String, value: Option<std::borrow::Cow<'_, str>>| {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                value.trim().clone_into(target);
            }
        };
        set(&mut info.title, tag.title());
        set(&mut info.artist, tag.artist());
    }
    info
}

/// Windows paths are not case sensitive, and playlists can use either slash.
fn path_key(path: &Path) -> String {
    let mut parts = Vec::new();
    for part in path.to_string_lossy().replace('\\', "/").split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part.to_lowercase()),
        }
    }
    parts.join("/")
}

/// Returns the entries of an `.m3u` playlist as paths relative to `base`.
pub fn parse_m3u(text: &str, base: &Path) -> Vec<PathBuf> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| base.join(line))
        .collect()
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u") || ext.eq_ignore_ascii_case("m3u8"))
}

/// Removes the symbols around names such as `!#SamsungMusic_favorites#!`.
fn playlist_name(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy();
    let name = stem.trim_matches(|c: char| !c.is_alphanumeric());
    if name.is_empty() { stem.into_owned() } else { name.to_owned() }
}

/// Loads a folder, or any mix of folders and files. Tags are read later, because reading
/// them takes too long for the UI thread. See [`read_info`].
pub fn load(paths: impl IntoIterator<Item = PathBuf>, liked: &BTreeSet<PathBuf>) -> Library {
    let paths: Vec<PathBuf> = paths.into_iter().collect();
    let tracks: Vec<Track> = collect_tracks(paths.iter().cloned()).into_iter().map(Track::from_path).collect();
    let by_path: HashMap<String, usize> =
        tracks.iter().enumerate().map(|(index, track)| (path_key(&track.path), index)).collect();

    let mut library = Library {
        playlists: vec![
            Playlist { name: "All Songs".to_owned(), tracks: (0..tracks.len()).collect() },
            Playlist { name: "Liked Songs".to_owned(), tracks: Vec::new() },
        ],
        tracks,
        missing_entries: 0,
    };
    library.refresh_liked(liked);

    let mut files: Vec<PathBuf> = paths
        .iter()
        .flat_map(|path| WalkDir::new(path).into_iter().filter_map(Result::ok))
        .filter(|entry| entry.file_type().is_file() && is_playlist(entry.path()))
        .map(walkdir::DirEntry::into_path)
        .collect();
    files.sort_by_key(|path| playlist_name(path).to_lowercase());

    for file in files {
        let Ok(bytes) = fs::read(&file) else { continue };
        let base = file.parent().unwrap_or(Path::new(""));
        let entries = parse_m3u(&String::from_utf8_lossy(&bytes), base);
        let found: Vec<usize> = entries.iter().filter_map(|entry| by_path.get(&path_key(entry)).copied()).collect();
        library.missing_entries += entries.len() - found.len();
        if !found.is_empty() {
            library.playlists.push(Playlist { name: playlist_name(&file), tracks: found });
        }
    }
    library
}

impl Library {
    pub fn refresh_liked(&mut self, liked: &BTreeSet<PathBuf>) {
        self.playlists[LIKED_SONGS].tracks = (0..self.tracks.len())
            .filter(|&index| liked.contains(&self.tracks[index].path))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_title_and_artist_at_the_last_separator() {
        let path = Path::new("Medusa - 2026 Repented - Bring Me The Horizon.flac");
        let (title, artist) = name_from_file(path);
        assert_eq!(title, "Medusa - 2026 Repented");
        assert_eq!(artist, "Bring Me The Horizon");
        assert_eq!(name_from_file(Path::new("plain.mp3")), ("plain".to_owned(), String::new()));
    }

    #[test]
    fn parses_m3u_entries_and_ignores_comments() {
        let text = "\u{feff}#EXTM3U\r\nsub/a.flac\r\n\r\n#EXTINF:1,x\r\nb.flac\r\n";
        let entries = parse_m3u(text, Path::new("root"));
        assert_eq!(entries, [Path::new("root").join("sub/a.flac"), Path::new("root").join("b.flac")]);
    }

    #[test]
    fn path_keys_ignore_case_slashes_and_parent_folders() {
        assert_eq!(path_key(Path::new("G:\\Music\\X\\..\\A.flac")), path_key(Path::new("g:/music/a.FLAC")));
    }

    #[test]
    fn cleans_playlist_names() {
        assert_eq!(playlist_name(Path::new("!#SamsungMusic_favorites#!.m3u")), "SamsungMusic_favorites");
        assert_eq!(playlist_name(Path::new("Arcane.m3u")), "Arcane");
    }

    #[test]
    fn loads_tracks_and_playlists_from_a_folder() {
        let root = std::env::temp_dir().join(format!("amplo-library-{}", std::process::id()));
        fs::create_dir_all(root.join("sub")).unwrap();
        for file in ["b - Y.mp3", "sub/a - X.flac"] {
            fs::File::create(root.join(file)).unwrap();
        }
        fs::write(root.join("Mix.m3u"), "#EXTM3U\nsub\\a - X.flac\nmissing.flac\n").unwrap();
        fs::write(root.join("Empty.m3u"), "").unwrap();

        let liked = BTreeSet::from([root.join("b - Y.mp3")]);
        let library = load([root.clone()], &liked);
        fs::remove_dir_all(&root).unwrap();

        let names: Vec<&str> = library.playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["All Songs", "Liked Songs", "Mix"]);
        assert_eq!(library.playlists[ALL_SONGS].tracks.len(), 2);
        assert_eq!(library.tracks[library.playlists[LIKED_SONGS].tracks[0]].info.title, "b");
        assert_eq!(library.tracks[library.playlists[2].tracks[0]].info.artist, "X");
        assert_eq!(library.missing_entries, 1);
    }
}
