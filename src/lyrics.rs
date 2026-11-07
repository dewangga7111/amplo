//! Lyrics from `.lrc` files beside the audio files.

use std::{fs, path::Path, time::Duration};

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub time: Duration,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    /// False when the file has no timestamps. All line times are then zero.
    pub synced: bool,
    pub lines: Vec<Line>,
}

impl Lyrics {
    pub fn for_track(track: &Path) -> Option<Self> {
        let bytes = fs::read(track.with_extension("lrc")).ok()?;
        Self::parse(&String::from_utf8_lossy(&bytes))
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut timed = Vec::new();
        let mut plain = Vec::new();
        for raw in text.trim_start_matches('\u{feff}').lines() {
            let mut rest = raw.trim();
            let mut times = Vec::new();
            while let Some((time, after)) = leading_timestamp(rest) {
                times.push(time);
                rest = after.trim_start();
            }
            if times.is_empty() && rest.starts_with('[') {
                continue; // A header such as [ti:Title].
            }
            let line = clean_text(rest);
            if line.is_empty() {
                continue;
            }
            if times.is_empty() {
                plain.push(line);
            } else {
                timed.extend(times.into_iter().map(|time| Line { time, text: line.clone() }));
            }
        }

        if timed.is_empty() {
            let lines: Vec<Line> = plain.into_iter().map(|text| Line { time: Duration::ZERO, text }).collect();
            return (!lines.is_empty()).then_some(Self { synced: false, lines });
        }
        // Some downloads stamp every line [00:00.00]. That file order is all they really know.
        let first_time = timed[0].time;
        if timed.len() > 1 && timed.iter().all(|line| line.time == first_time) {
            for line in &mut timed {
                line.time = Duration::ZERO;
            }
            return Some(Self { synced: false, lines: timed });
        }
        timed.sort_by_key(|line| line.time);
        Some(Self { synced: true, lines: timed })
    }

    /// The index of the line being sung at `position`. Before the first line there is none.
    pub fn active_line(&self, position: Duration) -> Option<usize> {
        if !self.synced {
            return None;
        }
        self.lines.partition_point(|line| line.time <= position).checked_sub(1)
    }
}

/// Parses `[mm:ss.xx]` at the start of the text.
fn leading_timestamp(text: &str) -> Option<(Duration, &str)> {
    let (stamp, rest) = text.strip_prefix('[')?.split_once(']')?;
    Some((parse_time(stamp)?, rest))
}

fn parse_time(stamp: &str) -> Option<Duration> {
    let (minutes, seconds) = stamp.split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: f64 = seconds.replace(':', ".").parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs(minutes * 60) + Duration::from_secs_f64(seconds))
}

/// Removes the per-word `<mm:ss.xx>` stamps and the `v1:` or `bg:` voice prefixes.
fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        match rest[start + 1..].split_once('>') {
            Some((stamp, after)) if parse_time(stamp).is_some() => rest = after,
            _ => {
                out.push('<');
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);

    let trimmed = out.trim();
    let without_voice = trimmed.split_once(':').and_then(|(prefix, after)| {
        let is_voice = prefix == "bg"
            || prefix.strip_prefix('v').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        is_voice.then_some(after)
    });
    // Word-stamped files pad every stamp with spaces, which would leave wide gaps.
    without_voice.unwrap_or(trimmed).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_library_format_with_headers_and_voice_prefixes() {
        let text = "[ti:Medusa]\n[ar:BMTH]\n[by:x (source: y)]\n\n[00:20.91]v1:Dead hands clutch my arm\n[00:23.44]v1:The smell\n";
        let lyrics = Lyrics::parse(text).unwrap();
        assert!(lyrics.synced);
        assert_eq!(lyrics.lines[0].text, "Dead hands clutch my arm");
        assert_eq!(lyrics.lines[0].time, Duration::from_millis(20_910));
        assert_eq!(lyrics.lines.len(), 2);
    }

    #[test]
    fn handles_repeated_stamps_word_stamps_and_order() {
        let lyrics = Lyrics::parse("[00:10.00][00:02.00]Chorus\n[00:05.00]<00:05.00>One <00:05.50>two\n").unwrap();
        let texts: Vec<&str> = lyrics.lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["Chorus", "One two", "Chorus"]);
    }

    #[test]
    fn word_stamps_with_padding_leave_single_spaces() {
        let text = "<00:26.55> Which <00:26.57>   <00:26.58> are <00:26.60>   <00:26.61> institutions";
        assert_eq!(clean_text(text), "Which are institutions");
    }

    #[test]
    fn keeps_normal_text_with_colons_and_angle_brackets() {
        assert_eq!(clean_text("Note: 1 < 2 > 0"), "Note: 1 < 2 > 0");
        assert_eq!(clean_text("v2:Hello"), "Hello");
        assert_eq!(clean_text("bg:Ooh"), "Ooh");
    }

    #[test]
    fn finds_the_active_line() {
        let lyrics = Lyrics::parse("[00:10.00]A\n[00:20.00]B\n").unwrap();
        assert_eq!(lyrics.active_line(Duration::from_secs(5)), None);
        assert_eq!(lyrics.active_line(Duration::from_secs(10)), Some(0));
        assert_eq!(lyrics.active_line(Duration::from_secs(99)), Some(1));
    }

    #[test]
    fn lines_that_all_share_one_stamp_are_not_synced() {
        let lyrics = Lyrics::parse("[00:00.00]First\n[00:00.00]Second\n").unwrap();
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines[1].text, "Second");
        assert_eq!(lyrics.active_line(Duration::from_secs(5)), None);
        assert!(Lyrics::parse("[00:07.00]Only line\n").unwrap().synced);
    }

    #[test]
    fn plain_text_is_not_synced_and_empty_files_have_no_lyrics() {
        let lyrics = Lyrics::parse("[ti:X]\nFirst\nSecond\n").unwrap();
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.active_line(Duration::from_secs(5)), None);
        assert_eq!(Lyrics::parse("[ti:X]\n\n"), None);
    }
}
