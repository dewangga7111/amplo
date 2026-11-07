//! What the app remembers between runs, in a small text file.

use std::{collections::BTreeSet, fmt::Write as _, fs, io, path::PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub folder: Option<PathBuf>,
    pub volume: f32,
    pub liked: BTreeSet<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { folder: None, volume: 0.7, liked: BTreeSet::new() }
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
        text
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths_with_spaces_and_equals_signs() {
        let settings = Settings {
            folder: Some(PathBuf::from("G:\\Music")),
            volume: 0.25,
            liked: BTreeSet::from([PathBuf::from("G:\\Music\\a = b - 光.flac")]),
        };
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
