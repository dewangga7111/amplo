//! The music library: tracks, their tags, and playlists.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey},
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
    /// The artist of the whole album. Compilations have one, while each song has its own artist.
    pub album_artist: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
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
    /// When the file came into the folder. Windows sets a file's creation time when it is
    /// copied or downloaded there, while the modified time stays from before.
    pub added: Option<SystemTime>,
}

impl Track {
    /// Starts with the title and artist from the file name, until the tags have been read.
    pub fn from_path(path: PathBuf) -> Self {
        let (title, artist) = name_from_file(&path);
        let added = fs::metadata(&path).ok().and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok());
        Self { path, info: TrackInfo { title, artist, ..TrackInfo::default() }, added }
    }
}

/// The order of every song list except albums, which keep their discs and track numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// The newest files first. In a playlist file, the songs added to it last come first.
    #[default]
    RecentlyAdded,
    Title,
    /// By artist, then album by album in track order.
    Artist,
    /// Album by album in track order.
    Album,
}

impl Sort {
    pub const ALL: [Self; 4] = [Self::RecentlyAdded, Self::Title, Self::Artist, Self::Album];

    pub fn label(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "Recently added",
            Self::Title => "Title",
            Self::Artist => "Artist",
            Self::Album => "Album",
        }
    }

    /// The word saved in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "added",
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Playlist {
    pub name: String,
    /// The songs in the order they show. See [`Library::sort`].
    pub tracks: Vec<usize>,
    /// The `.m3u` file. All Songs and Liked Songs have none, so songs cannot be added to them.
    pub path: Option<PathBuf>,
    /// Set for an album, artist, or genre opened from a song. It has no file either.
    pub collection: Option<Collection>,
    /// The songs in the order of the `.m3u` file. Empty for lists without a file.
    pub in_file: Vec<usize>,
}

impl Playlist {
    /// An album keeps the order of its discs and track numbers. Every other list follows
    /// the sort menu.
    pub fn follows_sort(&self) -> bool {
        !matches!(self.collection, Some(Collection::Album { .. }))
    }
}

#[derive(Debug, Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    /// Always starts with All Songs and Liked Songs, followed by the `.m3u` playlists.
    pub playlists: Vec<Playlist>,
    /// Playlist entries that point to a file outside the library, or to a missing file.
    pub missing_entries: usize,
    /// The order of every song list except albums. See [`Library::set_sort`].
    pub sort: Sort,
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
        set(&mut info.album_artist, tag.get_string(ItemKey::AlbumArtist).map(std::borrow::Cow::Borrowed));
        set(&mut info.genre, tag.genre());
        info.track_number = tag.track().filter(|&number| number > 0);
        info.disc_number = tag.disk().filter(|&number| number > 0);
    }
    info
}

/// Names with a comma of their own, which [`split_names`] keeps whole.
const NAMES_WITH_COMMAS: [&str; 3] = ["Tyler, The Creator", "Earth, Wind & Fire", "Crosby, Stills, Nash & Young"];

/// Where each name is in an artist or genre tag that lists several, such as
/// `Mili, Yamato Kasai` or `Alternative; Indie Rock`. Names are split at `, ` and `; ` only,
/// as `&` and `/` are part of names such as `MYTH & ROID` and `Indie Rock/Rock pop`.
pub fn split_names(text: &str) -> Vec<Range<usize>> {
    let mut names = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let rest = &text[start..];
        let whole = |name: &&&str| {
            rest.get(..name.len()).is_some_and(|head| head.eq_ignore_ascii_case(name))
                && rest.get(name.len()..).is_some_and(|after| after.is_empty() || after.starts_with(", ") || after.starts_with("; "))
        };
        let length = match NAMES_WITH_COMMAS.iter().find(whole) {
            Some(name) => name.len(),
            None => [", ", "; "].iter().filter_map(|separator| rest.find(separator)).min().unwrap_or(rest.len()),
        };
        let piece = &rest[..length];
        let lead = piece.len() - piece.trim_start().len();
        let trimmed = piece.trim();
        if !trimmed.is_empty() {
            names.push(start + lead..start + lead + trimmed.len());
        }
        // Both separators are two bytes long.
        start += length + 2;
    }
    names
}

/// Whether one of the names in an artist or genre tag is `name`.
fn names_include(text: &str, name: &str) -> bool {
    split_names(text).into_iter().any(|range| text[range].eq_ignore_ascii_case(name))
}

/// Songs grouped by what their tags share. Opening one shows its songs as a playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collection {
    /// An album name, and the album artist when the tags have one.
    Album { name: String, artist: String },
    /// One of the artists of a song.
    Artist(String),
    /// One of the genres of a song.
    Genre(String),
}

impl Collection {
    /// The album of a song. `None` for a song without an album name.
    pub fn album_of(info: &TrackInfo) -> Option<Self> {
        (!info.album.is_empty()).then(|| Self::Album { name: info.album.clone(), artist: info.album_artist.clone() })
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Album { name, .. } | Self::Artist(name) | Self::Genre(name) => name,
        }
    }

    /// The word above the name in the header, and in "Playing from".
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Album { .. } => "ALBUM",
            Self::Artist(_) => "ARTIST",
            Self::Genre(_) => "GENRE",
        }
    }

    /// Songs without an album artist match an album by its name alone, as many files lack one.
    fn contains(&self, info: &TrackInfo) -> bool {
        match self {
            Self::Album { name, artist } => {
                info.album.eq_ignore_ascii_case(name)
                    && (artist.is_empty() || info.album_artist.eq_ignore_ascii_case(artist))
            }
            Self::Artist(name) => names_include(&info.artist, name),
            Self::Genre(name) => names_include(&info.genre, name),
        }
    }
}

impl Library {
    /// The songs of a collection as a playlist. An album is in the order of its discs and
    /// track numbers, with songs without a number last, by title. An artist or a genre
    /// follows the sort menu.
    pub fn collection(&self, collection: &Collection) -> Playlist {
        let mut tracks: Vec<usize> =
            (0..self.tracks.len()).filter(|&track| collection.contains(&self.tracks[track].info)).collect();
        tracks.sort_by_cached_key(|&track| {
            let info = &self.tracks[track].info;
            (info.disc_number.unwrap_or(1), info.track_number.is_none(), info.track_number, info.title.to_lowercase())
        });
        let mut playlist =
            Playlist { name: collection.name().to_owned(), tracks, collection: Some(collection.clone()), ..Playlist::default() };
        if playlist.follows_sort() {
            playlist.tracks = self.sorted(&playlist);
        }
        playlist
    }

    /// Shows a collection as the last playlist, in place of the one shown before. Returns
    /// its position. Collections do not show in the sidebar.
    pub fn show_collection(&mut self, collection: Playlist) -> usize {
        self.playlists.retain(|playlist| playlist.collection.is_none());
        self.playlists.push(collection);
        self.playlists.len() - 1
    }

    /// The collection shown, if any.
    pub fn shown_collection(&self) -> Option<&Collection> {
        self.playlists.last().and_then(|playlist| playlist.collection.as_ref())
    }
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
            Playlist { name: "All Songs".to_owned(), tracks: (0..tracks.len()).collect(), path: None, collection: None, in_file: Vec::new() },
            Playlist { name: "Liked Songs".to_owned(), tracks: Vec::new(), path: None, collection: None, in_file: Vec::new() },
        ],
        tracks,
        missing_entries: 0,
        sort: Sort::default(),
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
            library.playlists.push(Playlist {
                name: playlist_name(&file),
                tracks: found.clone(),
                path: Some(file),
                collection: None,
                in_file: found,
            });
        }
    }
    library.sort_songs();
    library
}

impl Library {
    pub fn refresh_liked(&mut self, liked: &BTreeSet<PathBuf>) {
        self.playlists[LIKED_SONGS].tracks = (0..self.tracks.len())
            .filter(|&index| liked.contains(&self.tracks[index].path))
            .collect();
        self.sort_songs();
    }

    /// Puts every list that follows the sort in this order. Returns whether any changed.
    pub fn set_sort(&mut self, sort: Sort) -> bool {
        self.sort = sort;
        self.sort_songs()
    }

    /// Puts every list that follows the sort in the order of [`Library::sort`] again, as after
    /// the tags have been read. Returns whether any changed.
    pub fn sort_songs(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.playlists.len() {
            if !self.playlists[index].follows_sort() {
                continue;
            }
            let tracks = self.sorted(&self.playlists[index]);
            if tracks != self.playlists[index].tracks {
                self.playlists[index].tracks = tracks;
                changed = true;
            }
        }
        changed
    }

    /// The songs of a list in the order of [`Library::sort`]. In a playlist file, the songs
    /// added last are at its end, so they come first as the recently added ones. Elsewhere
    /// the newest files come first. Equal songs keep the order of the file, or of the folder.
    fn sorted(&self, playlist: &Playlist) -> Vec<usize> {
        let base = if playlist.path.is_some() {
            playlist.in_file.clone()
        } else {
            let mut tracks = playlist.tracks.clone();
            tracks.sort_unstable();
            tracks
        };
        let mut entries: Vec<(usize, usize)> = base.into_iter().enumerate().collect();
        let lower = |text: &str| text.to_lowercase();
        let in_album = |info: &TrackInfo| (info.disc_number.unwrap_or(1), info.track_number.is_none(), info.track_number);
        // Empty names go last.
        let name = |text: &str| (text.is_empty(), lower(text));
        let info = |track: usize| &self.tracks[track].info;
        match self.sort {
            Sort::RecentlyAdded if playlist.path.is_some() => entries.reverse(),
            Sort::RecentlyAdded => {
                entries.sort_by_key(|&(position, track)| (std::cmp::Reverse(self.tracks[track].added), position));
            }
            Sort::Title => entries.sort_by_cached_key(|&(position, track)| {
                (lower(&info(track).title), name(&info(track).artist), position)
            }),
            Sort::Artist => entries.sort_by_cached_key(|&(position, track)| {
                let info = info(track);
                (name(&info.artist), name(&info.album), in_album(info), lower(&info.title), position)
            }),
            Sort::Album => entries.sort_by_cached_key(|&(position, track)| {
                let info = info(track);
                (name(&info.album), in_album(info), lower(&info.title), position)
            }),
        }
        entries.into_iter().map(|(_, track)| track).collect()
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
    fn albums_follow_disc_and_track_numbers() {
        let song = |title: &str, album: &str, album_artist: &str, disc: Option<u32>, number: Option<u32>| Track {
            path: PathBuf::from(format!("{title}.flac")),
            info: TrackInfo {
                title: title.to_owned(),
                album: album.to_owned(),
                album_artist: album_artist.to_owned(),
                disc_number: disc,
                track_number: number,
                ..TrackInfo::default()
            },
            added: None,
        };
        let mut library = Library {
            tracks: vec![
                song("Outro", "Night", "", None, None),
                song("Two", "night", "", Some(1), Some(2)),
                song("Disc two", "Night", "", Some(2), Some(1)),
                song("One", "Night", "", None, Some(1)),
                song("Other band", "Night", "Someone else", Some(1), Some(1)),
                song("Elsewhere", "Day", "", Some(1), Some(1)),
                song("Intro", "Night", "", None, None),
            ],
            playlists: Vec::new(),
            missing_entries: 0,
            sort: Sort::default(),
        };
        let key = Collection::album_of(&library.tracks[0].info).unwrap();
        let album = library.collection(&key);
        let titles: Vec<&str> = album.tracks.iter().map(|&track| library.tracks[track].info.title.as_str()).collect();
        assert_eq!(titles, ["One", "Other band", "Two", "Intro", "Outro", "Disc two"], "no album artist matches the name only");
        let strict = library.collection(&Collection::album_of(&library.tracks[4].info).unwrap());
        assert_eq!(strict.tracks, [4], "an album artist keeps other albums of that name out");
        assert_eq!(Collection::album_of(&TrackInfo::default()), None);

        let first = library.show_collection(album);
        let second = library.show_collection(strict);
        assert_eq!((first, second, library.playlists.len()), (0, 0, 1), "one collection shows at a time");
        assert_eq!(
            library.shown_collection(),
            Some(&Collection::Album { name: "Night".to_owned(), artist: "Someone else".to_owned() })
        );
    }

    #[test]
    fn splits_artists_and_genres_into_names() {
        let names = |text: &'static str| -> Vec<&'static str> { split_names(text).into_iter().map(|range| &text[range]).collect() };
        assert_eq!(names("Mili, Yamato Kasai, Cassie Wei"), ["Mili", "Yamato Kasai", "Cassie Wei"]);
        assert_eq!(names("Alternative; Indie Rock"), ["Alternative", "Indie Rock"]);
        assert_eq!(names("MYTH & ROID"), ["MYTH & ROID"]);
        assert_eq!(names("Rock, Indie Rock/Rock pop"), ["Rock", "Indie Rock/Rock pop"]);
        assert_eq!(names("nothing,nowhere."), ["nothing,nowhere."]);
        assert_eq!(names("Tyler, The Creator, Kali Uchis"), ["Tyler, The Creator", "Kali Uchis"]);
        assert_eq!(names("Kali Uchis, Tyler, The Creator"), ["Kali Uchis", "Tyler, The Creator"]);
        assert_eq!(names("Tyler, The Creators"), ["Tyler", "The Creators"]);
        assert_eq!(names("宝鐘マリン, Kobo Kanaeru"), ["宝鐘マリン", "Kobo Kanaeru"]);
        assert_eq!(names(" A ,  ; B"), ["A", "B"]);
        assert!(names("").is_empty());
    }

    #[test]
    fn artists_and_genres_list_their_songs_in_the_sort_order() {
        let song = |title: &str, artist: &str, album: &str, genre: &str, number: u32| Track {
            path: PathBuf::from(format!("{title}.flac")),
            info: TrackInfo {
                title: title.to_owned(),
                artist: artist.to_owned(),
                album: album.to_owned(),
                genre: genre.to_owned(),
                track_number: Some(number),
                ..TrackInfo::default()
            },
            added: None,
        };
        let library = Library {
            tracks: vec![
                song("Zeta two", "Mili", "Zeta", "Pop", 2),
                song("Single", "Mili, Yamato Kasai", "", "Rock; Pop", 1),
                song("Alpha one", "mili", "Alpha", "Metal", 1),
                song("Zeta one", "Mili", "Zeta", "Pop", 1),
                song("Elsewhere", "Milia", "Alpha", "Pop", 3),
            ],
            playlists: Vec::new(),
            missing_entries: 0,
            // Artists and genres follow the sort menu.
            sort: Sort::Artist,
        };
        let titles = |collection: Collection| -> Vec<String> {
            let playlist = library.collection(&collection);
            assert_eq!(playlist.name, collection.name());
            playlist.tracks.iter().map(|&track| library.tracks[track].info.title.clone()).collect()
        };
        assert_eq!(titles(Collection::Artist("Mili".to_owned())), ["Alpha one", "Zeta one", "Zeta two", "Single"]);
        assert_eq!(titles(Collection::Artist("Yamato Kasai".to_owned())), ["Single"]);
        assert_eq!(titles(Collection::Genre("pop".to_owned())), ["Zeta one", "Zeta two", "Single", "Elsewhere"]);
    }

    #[test]
    fn lists_follow_the_sort_except_albums() {
        let song = |title: &str, artist: &str, album: &str, number: u32, added: u64| Track {
            path: PathBuf::from(format!("{title}.flac")),
            info: TrackInfo {
                title: title.to_owned(),
                artist: artist.to_owned(),
                album: album.to_owned(),
                track_number: Some(number),
                ..TrackInfo::default()
            },
            added: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(added)),
        };
        let tracks = vec![
            song("beta", "Zed", "Night", 2, 30),
            song("Alpha", "", "Day", 1, 10),
            song("gamma", "amy", "Night", 1, 20),
            song("Delta", "Zed", "", 1, 40),
        ];
        let mut library = Library {
            playlists: vec![
                Playlist { name: "All Songs".to_owned(), tracks: (0..tracks.len()).collect(), path: None, collection: None, in_file: Vec::new() },
                Playlist { name: "Liked Songs".to_owned(), tracks: Vec::new(), path: None, collection: None, in_file: Vec::new() },
                Playlist {
                    name: "Mix".to_owned(),
                    tracks: vec![2, 0, 3],
                    path: Some(PathBuf::from("Mix.m3u")),
                    in_file: vec![2, 0, 3],
                    ..Playlist::default()
                },
                Playlist {
                    name: "Night".to_owned(),
                    tracks: vec![0, 2],
                    collection: Some(Collection::Album { name: "Night".to_owned(), artist: String::new() }),
                    ..Playlist::default()
                },
            ],
            tracks,
            missing_entries: 0,
            sort: Sort::default(),
        };
        let titles = |library: &Library, index: usize| -> Vec<String> {
            library.playlists[index].tracks.iter().map(|&track| library.tracks[track].info.title.clone()).collect()
        };
        assert!(library.sort_songs());
        assert_eq!(titles(&library, ALL_SONGS), ["Delta", "beta", "gamma", "Alpha"], "newest first");
        assert_eq!(titles(&library, 2), ["Delta", "beta", "gamma"], "the last songs added to the playlist first");
        assert!(library.set_sort(Sort::Title));
        assert_eq!(titles(&library, ALL_SONGS), ["Alpha", "beta", "Delta", "gamma"]);
        assert_eq!(titles(&library, 2), ["beta", "Delta", "gamma"]);
        assert!(library.set_sort(Sort::Artist));
        assert_eq!(titles(&library, ALL_SONGS), ["gamma", "beta", "Delta", "Alpha"], "unknown artists last");
        assert!(library.set_sort(Sort::Album));
        assert_eq!(titles(&library, ALL_SONGS), ["Alpha", "gamma", "beta", "Delta"], "songs without an album last");
        assert_eq!(titles(&library, 2), ["gamma", "beta", "Delta"]);
        assert!(!library.set_sort(Sort::Album), "nothing changes the second time");
        assert_eq!(titles(&library, 3), ["beta", "gamma"], "an album keeps its own order");

        library.refresh_liked(&BTreeSet::from([PathBuf::from("beta.flac"), PathBuf::from("gamma.flac")]));
        assert_eq!(titles(&library, LIKED_SONGS), ["gamma", "beta"], "Liked Songs follows the sort too");
        library.set_sort(Sort::RecentlyAdded);
        assert_eq!(titles(&library, 2), ["Delta", "beta", "gamma"], "the playlist's own order comes back");
        assert_eq!(Sort::from_key(Sort::Artist.key()), Some(Sort::Artist));
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
