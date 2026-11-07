//! Finding audio files and opening them for playback.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use rodio::{ChannelCount, Decoder, Sample, SampleRate, Source, source::SeekError};
use walkdir::WalkDir;

use crate::opus::OpusSource;

pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "opus", "wav", "m4a", "mp4"];
/// How much of a file start to inspect. The MP4 boxes needed here sit in the first kilobytes.
const HEAD_BYTES: u64 = 64 * 1024;

pub type BoxedSource = Box<dyn Source + Send>;

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| AUDIO_EXTENSIONS.iter().any(|a| ext.eq_ignore_ascii_case(a)))
}

/// Expands files and folders into a list of audio files, sorted by name within each folder.
pub fn collect_tracks(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut tracks = Vec::new();
    for path in paths {
        tracks.extend(
            WalkDir::new(path)
                .sort_by_file_name()
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file() && is_audio(entry.path()))
                .map(walkdir::DirEntry::into_path),
        );
    }
    tracks
}

/// Picks the decoder from the file content, because the extension can be wrong:
/// an `.opus` file can hold Ogg FLAC, an `.ogg` file can hold Opus, and a `.flac` file
/// can hold FLAC inside MP4.
pub fn open_source(path: &Path) -> Result<BoxedSource, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let mut head = Vec::new();
    file.take(HEAD_BYTES).read_to_end(&mut head).map_err(|err| err.to_string())?;
    if head.is_empty() {
        return Err("file kosong (0 byte)".to_owned());
    }

    if head.starts_with(b"OggS") && let Ok(source) = OpusSource::open(path) {
        return Ok(Box::new(source));
    }

    let file = File::open(path).map_err(|err| err.to_string())?;
    let decoder_error = match Decoder::try_from(file) {
        Ok(decoder) => return Ok(with_known_duration(Box::new(decoder), &head)),
        Err(err) => err.to_string(),
    };

    if is_encrypted_mp4(&head) {
        return Err("file terenkripsi DRM, tidak bisa diputar".to_owned());
    }
    #[cfg(windows)]
    if let Ok(source) = crate::media_foundation::decode_to_memory(path) {
        return Ok(Box::new(source));
    }
    Err(decoder_error)
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|window| window == needle)
}

/// ISO common encryption marks protected audio with an `enca` sample entry and a `sinf` box.
fn is_encrypted_mp4(head: &[u8]) -> bool {
    head.get(4..8) == Some(b"ftyp") && contains(head, b"enca") && contains(head, b"sinf")
}

/// Symphonia reports no length for FLAC in fragmented MP4, but the `dfLa` box holds the
/// FLAC STREAMINFO block, which has the sample rate and the total sample count.
fn flac_in_mp4_duration(head: &[u8]) -> Option<Duration> {
    // After the box type: 4 bytes of version and flags, then a 4-byte metadata block header.
    const STREAMINFO_OFFSET: usize = 12;
    let dfla = head.windows(4).position(|window| window == b"dfLa")?;
    let info = head.get(dfla + STREAMINFO_OFFSET..dfla + STREAMINFO_OFFSET + 18)?;
    let rate = (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
    let low_bits = u32::from_be_bytes(info[14..18].try_into().ok()?);
    let total = (u64::from(info[13] & 0x0f) << 32) | u64::from(low_bits);
    (rate > 0 && total > 0).then(|| Duration::from_secs_f64(total as f64 / f64::from(rate)))
}

fn with_known_duration(source: BoxedSource, head: &[u8]) -> BoxedSource {
    if source.total_duration().is_some() {
        return source;
    }
    match flac_in_mp4_duration(head) {
        Some(duration) => Box::new(KnownDuration { inner: source, duration }),
        None => source,
    }
}

/// Adds a length to a source that cannot report one itself.
struct KnownDuration {
    inner: BoxedSource,
    duration: Duration,
}

impl Iterator for KnownDuration {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        self.inner.next()
    }
}

impl Source for KnownDuration {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration)
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("amplo-{name}-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn recognizes_audio_extensions_case_insensitively() {
        assert!(is_audio(Path::new("a.MP3")));
        assert!(is_audio(Path::new("dir/b.flac")));
        assert!(is_audio(Path::new("c.opus")));
        assert!(is_audio(Path::new("d.mp4")));
        assert!(!is_audio(Path::new("cover.jpg")));
        assert!(!is_audio(Path::new("lyrics.lrc")));
        assert!(!is_audio(Path::new("mp3")));
    }

    #[test]
    fn collects_audio_recursively_in_name_order() {
        let root = temp_dir("collect");
        fs::create_dir_all(root.join("sub")).unwrap();
        for file in ["b.mp3", "a.wav", "notes.txt", "sub/c.flac"] {
            File::create(root.join(file)).unwrap();
        }

        let tracks = collect_tracks([root.clone()]);
        let names: Vec<_> = tracks.iter().map(|p| p.file_stem().unwrap().to_owned()).collect();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn reads_flac_duration_from_dfla_box() {
        // A real dfLa box: 96 kHz, 31 766 400 samples.
        let mut head = b"....dfLa\0\0\0\0\x80\0\0\x22".to_vec();
        head.extend_from_slice(&[
            0x10, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x49, 0x6c, 0x17, 0x70, 0x03, 0x70,
            0x01, 0xe4, 0xb7, 0x80,
        ]);
        let duration = flac_in_mp4_duration(&head).unwrap();
        assert!((duration.as_secs_f64() - 330.9).abs() < 0.01, "was {duration:?}");
        assert_eq!(flac_in_mp4_duration(b"no such box"), None);
    }

    #[test]
    fn detects_encrypted_mp4_only_with_both_markers() {
        assert!(is_encrypted_mp4(b"\0\0\0\x20ftypisom enca sinf"));
        assert!(!is_encrypted_mp4(b"\0\0\0\x20ftypisom enca"));
        assert!(!is_encrypted_mp4(b"not an mp4 file enca sinf"));
    }

    #[test]
    fn empty_file_reports_a_clear_error() {
        let root = temp_dir("empty");
        let path = root.join("empty.opus");
        File::create(&path).unwrap();
        let error = open_source(&path).err().unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(error, "file kosong (0 byte)");
    }
}
