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
    pub album: String,
    pub genre: String,
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
    /// The `.m3u` file. All Songs and Liked Songs have none, so songs cannot be added to them.
    pub path: Option<PathBuf>,
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
        set(&mut info.album, tag.album());
        set(&mut info.genre, tag.genre());
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

/// Characters Windows does not allow in a file name.
const NOT_IN_FILE_NAMES: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// How a song is written in a playlist: relative to the playlist's folder when it is inside
/// it, so the folder can move, and as a full path otherwise.
fn playlist_entry(base: &Path, track: &Path) -> String {
    track.strip_prefix(base).unwrap_or(track).to_string_lossy().into_owned()
}

/// Appends songs to an `.m3u` playlist, and returns how many it gained. Songs it already has
/// are left out, so adding twice changes nothing.
pub fn add_to_playlist(playlist: &Path, tracks: &[PathBuf]) -> Result<usize, String> {
    let failed = |err: std::io::Error| format!("Could not change the playlist: {err}");
    let bytes = fs::read(playlist).map_err(failed)?;
    let text = String::from_utf8_lossy(&bytes);
    let base = playlist.parent().unwrap_or(Path::new(""));
    let mut known: std::collections::HashSet<String> =
        parse_m3u(&text, base).iter().map(|entry| path_key(entry)).collect();
    let newline = if text.contains("\r\n") || text.is_empty() { "\r\n" } else { "\n" };

    let mut added = String::new();
    let mut count = 0;
    for track in tracks {
        if known.insert(path_key(track)) {
            added.push_str(&playlist_entry(base, track));
            added.push_str(newline);
            count += 1;
        }
    }
    if count == 0 {
        return Ok(0);
    }
    let mut start = String::new();
    if text.is_empty() {
        start.push_str("#EXTM3U");
        start.push_str(newline);
    } else if !text.ends_with('\n') {
        start.push_str(newline);
    }
    let mut file = fs::OpenOptions::new().append(true).open(playlist).map_err(failed)?;
    std::io::Write::write_all(&mut file, (start + &added).as_bytes()).map_err(failed)?;
    Ok(count)
}

/// Takes songs out of an `.m3u` playlist, each `#EXTINF` line about them too, and returns
/// how many entries went. Everything else in the file stays as it was.
pub fn remove_from_playlist(playlist: &Path, tracks: &[PathBuf]) -> Result<usize, String> {
    let failed = |err: std::io::Error| format!("Could not change the playlist: {err}");
    let bytes = fs::read(playlist).map_err(failed)?;
    let text = String::from_utf8_lossy(&bytes);
    let base = playlist.parent().unwrap_or(Path::new(""));
    let unwanted: std::collections::HashSet<String> = tracks.iter().map(|track| path_key(track)).collect();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

    let mut kept: Vec<&str> = Vec::new();
    let mut removed = 0;
    for line in text.lines() {
        let entry = line.trim().trim_start_matches('\u{feff}');
        if !entry.is_empty() && !entry.starts_with('#') && unwanted.contains(&path_key(&base.join(entry))) {
            removed += 1;
            // An `#EXTINF` line describes the entry after it.
            if kept.last().is_some_and(|last| last.trim().to_ascii_uppercase().starts_with("#EXTINF")) {
                kept.pop();
            }
            continue;
        }
        kept.push(line);
    }
    if removed == 0 {
        return Ok(0);
    }
    let mut new_text = kept.join(newline);
    if text.ends_with('\n') {
        new_text.push_str(newline);
    }
    fs::write(playlist, new_text).map_err(failed)?;
    Ok(removed)
}

/// Writes a new `.m3u` playlist into `folder` with `tracks` in it, and returns the name the
/// library will show for it. Fails when the name is empty or a playlist file of that name
/// exists.
pub fn create_playlist(folder: &Path, name: &str, tracks: &[PathBuf]) -> Result<String, String> {
    let file_name = file_name_for(name)?;
    let path = folder.join(format!("{file_name}.m3u"));
    match write_new_playlist(folder, &path, tracks) {
        Ok(()) => Ok(playlist_name(&path)),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(format!("A playlist named “{file_name}” already exists."))
        }
        Err(err) => Err(format!("Could not create the playlist: {err}")),
    }
}

/// Like [`create_playlist`], but a name that is taken gets a number: `Mix`, then `Mix 2`,
/// `Mix 3`, and so on. `taken` holds the names of playlists the library already shows, which
/// can live in other folders.
pub fn create_numbered_playlist(folder: &Path, name: &str, tracks: &[PathBuf], taken: &[String]) -> Result<String, String> {
    let file_name = file_name_for(name)?;
    let taken: std::collections::HashSet<String> = taken.iter().map(|name| name.to_lowercase()).collect();
    for number in 1..1000 {
        let candidate = if number == 1 { file_name.clone() } else { format!("{file_name} {number}") };
        let path = folder.join(format!("{candidate}.m3u"));
        let shown = playlist_name(&path);
        if taken.contains(&shown.to_lowercase()) || path.with_extension("m3u8").exists() {
            continue;
        }
        match write_new_playlist(folder, &path, tracks) {
            Ok(()) => return Ok(shown),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("Could not create the playlist: {err}")),
        }
    }
    Err(format!("Too many playlists are named “{file_name}”."))
}

/// The name as Windows allows it in a file name.
fn file_name_for(name: &str) -> Result<String, String> {
    let allowed: String = name.chars().filter(|c| !c.is_control() && !NOT_IN_FILE_NAMES.contains(c)).collect();
    let file_name = allowed.split_whitespace().collect::<Vec<_>>().join(" ");
    // Windows drops dots and spaces at the end of a file name.
    let file_name = file_name.trim_end_matches('.').trim_end();
    if file_name.is_empty() {
        return Err("Type a name for the playlist.".to_owned());
    }
    Ok(file_name.to_owned())
}

/// Fails with `AlreadyExists` when a file is at `path`, and then leaves it alone.
fn write_new_playlist(folder: &Path, path: &Path, tracks: &[PathBuf]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut text = "#EXTM3U\r\n".to_owned();
    for track in tracks {
        text.push_str(&playlist_entry(folder, track));
        text.push_str("\r\n");
    }
    std::io::Write::write_all(&mut file, text.as_bytes())
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
            Playlist { name: "All Songs".to_owned(), tracks: (0..tracks.len()).collect(), path: None },
            Playlist { name: "Liked Songs".to_owned(), tracks: Vec::new(), path: None },
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
        // A new playlist has no songs yet. One whose songs are all missing stays hidden.
        if entries.is_empty() || !found.is_empty() {
            library.playlists.push(Playlist { name: playlist_name(&file), tracks: found, path: Some(file) });
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
        assert_eq!(names, ["All Songs", "Liked Songs", "Empty", "Mix"], "an empty playlist shows, a missing one does not");
        assert_eq!(library.playlists[ALL_SONGS].tracks.len(), 2);
        assert_eq!(library.tracks[library.playlists[LIKED_SONGS].tracks[0]].info.title, "b");
        assert!(library.playlists[2].tracks.is_empty());
        assert_eq!(library.tracks[library.playlists[3].tracks[0]].info.artist, "X");
        assert_eq!(library.missing_entries, 1);
    }

    #[test]
    fn creates_an_empty_playlist_file_once() {
        let root = std::env::temp_dir().join(format!("amplo-new-playlist-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let created = create_playlist(&root, "  Road / Trip?  ", &[]);
        let again = create_playlist(&root, "road trip", &[]);
        let blank = create_playlist(&root, " ?* ", &[]);
        let text = fs::read_to_string(root.join("Road Trip.m3u"));
        let with_songs = create_playlist(&root, "Mix", &[root.join("sub").join("a.flac"), PathBuf::from("Z:\\b.mp3")]);
        let mix = fs::read_to_string(root.join("Mix.m3u"));
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(created.unwrap(), "Road Trip", "characters Windows refuses are left out");
        assert_eq!(text.unwrap(), "#EXTM3U\r\n");
        assert!(again.unwrap_err().contains("already exists"), "file names ignore case on Windows");
        assert!(blank.is_err());
        assert_eq!(with_songs.unwrap(), "Mix");
        assert_eq!(mix.unwrap(), "#EXTM3U\r\nsub\\a.flac\r\nZ:\\b.mp3\r\n", "songs outside the folder keep a full path");
    }

    #[test]
    fn removes_songs_and_their_details_from_a_playlist() {
        let root = std::env::temp_dir().join(format!("amplo-remove-from-playlist-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Mix.m3u");
        let text = "#EXTM3U\r\n#EXTINF:120,A\r\nsub\\a.flac\r\n#EXTINF:90,B\r\nb.mp3\r\nSUB/A.FLAC\r\nZ:\\c.mp3\r\n";
        fs::write(&playlist, text).unwrap();
        let removed = remove_from_playlist(&playlist, &[root.join("sub").join("a.flac"), PathBuf::from("Z:\\c.mp3")]);
        let after = fs::read_to_string(&playlist);
        let none = remove_from_playlist(&playlist, &[root.join("other.mp3")]);
        let unchanged = fs::read_to_string(&playlist);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(removed.unwrap(), 3, "every entry of the song goes, in any spelling");
        assert_eq!(after.unwrap(), "#EXTM3U\r\n#EXTINF:90,B\r\nb.mp3\r\n");
        assert_eq!(none.unwrap(), 0);
        assert_eq!(unchanged.unwrap(), "#EXTM3U\r\n#EXTINF:90,B\r\nb.mp3\r\n");
    }

    #[test]
    fn numbers_a_playlist_name_that_is_taken() {
        let root = std::env::temp_dir().join(format!("amplo-numbered-playlist-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Daily Drive.m3u"), "#EXTM3U\r\n").unwrap();
        let taken = ["Daily Drive".to_owned(), "daily drive 2".to_owned()];
        let song = root.join("a.flac");
        let first = create_numbered_playlist(&root, "Daily Drive", std::slice::from_ref(&song), &taken);
        let second = create_numbered_playlist(&root, "Daily Drive", &[], &[]);
        let fresh = create_numbered_playlist(&root, "Chill?", &[], &taken);
        let text = fs::read_to_string(root.join("Daily Drive 3.m3u"));
        let old = fs::read_to_string(root.join("Daily Drive.m3u"));
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(first.unwrap(), "Daily Drive 3", "a playlist the library shows counts as taken");
        assert_eq!(second.unwrap(), "Daily Drive 2", "a file in the folder counts as taken");
        assert_eq!(fresh.unwrap(), "Chill");
        assert_eq!(text.unwrap(), "#EXTM3U\r\na.flac\r\n");
        assert_eq!(old.unwrap(), "#EXTM3U\r\n", "the existing playlist stays as it was");
    }

    #[test]
    fn adds_new_songs_to_a_playlist_once() {
        let root = std::env::temp_dir().join(format!("amplo-add-to-playlist-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Mix.m3u");
        // Written by another program: LF line endings, a different case, no newline at the end.
        fs::write(&playlist, "#EXTM3U\nSUB/A.flac").unwrap();
        let songs = [root.join("sub").join("a.flac"), root.join("b.flac"), root.join("b.flac")];
        let added = add_to_playlist(&playlist, &songs);
        let again = add_to_playlist(&playlist, &songs);
        let text = fs::read_to_string(&playlist);
        fs::write(&playlist, "").unwrap();
        let into_empty = add_to_playlist(&playlist, &songs[1..2]);
        let empty_text = fs::read_to_string(&playlist);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(added.unwrap(), 1, "a.flac was there, and b.flac counts once");
        assert_eq!(again.unwrap(), 0);
        assert_eq!(text.unwrap(), "#EXTM3U\nSUB/A.flac\nb.flac\n", "keeps the file's line endings");
        assert_eq!(into_empty.unwrap(), 1);
        assert_eq!(empty_text.unwrap(), "#EXTM3U\r\nb.flac\r\n");
    }
}
