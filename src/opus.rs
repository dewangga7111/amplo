//! Ogg Opus playback. Rodio's decoders (Symphonia 0.5) have no Opus codec.

use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    num::NonZero,
    ops::Range,
    path::Path,
    sync::Arc,
    time::Duration,
};

use opus_pure::{MAX_PACKET_SAMPLES, OggOpusReader, OggPacket, OpusDecoder, OpusHead, Trim, packet};
use rodio::{ChannelCount, SampleRate, Source, source::SeekError};

/// Opus always decodes at 48 kHz here, which is also the unit of Ogg Opus granule positions.
const RATE: u32 = 48_000;
const SAMPLE_RATE: SampleRate = NonZero::new(RATE).unwrap();
/// RFC 7845 section 4.6: decode at least 80 ms before a seek target so the decoder converges.
const SEEK_PRE_ROLL: u64 = 3_840;
/// An Ogg page is at most 65 307 bytes, so this always holds several complete pages.
const TAIL_BYTES: u64 = 256 * 1024;

pub struct OpusSource {
    reader: OggOpusReader<BufReader<File>>,
    head: OpusHead,
    decoder: OpusDecoder,
    trim: Trim,
    channels: ChannelCount,
    duration: Option<Duration>,
    packet: OggPacket,
    block: Vec<f32>,
    /// The part of `block` that is audio and has not been returned yet.
    pending: Range<usize>,
}

impl OpusSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|err| err.to_string())?;
        let reader = OggOpusReader::new(BufReader::new(file)).map_err(|err| err.to_string())?;
        let head = reader.head().clone();
        let channel_count = NonZero::new(u16::from(head.channel_count))
            .ok_or("Opus stream has no channels")?;
        let decoder = head.decoder(RATE as i32).map_err(|err| err.to_string())?;
        let trim = new_trim(&head, channel_count)?;

        // A missing length only hides the seek slider, so it is not an error.
        let last_granule = File::open(path)
            .and_then(|mut file| last_granule(&mut file, reader.serial()))
            .ok()
            .flatten();
        let duration = last_granule.map(|granule| {
            let samples = granule.saturating_sub(u64::from(head.pre_skip));
            Duration::from_secs_f64(samples as f64 / f64::from(RATE))
        });

        Ok(Self {
            reader,
            head,
            decoder,
            trim,
            channels: channel_count,
            duration,
            packet: OggPacket::default(),
            block: vec![0.0; MAX_PACKET_SAMPLES * usize::from(channel_count.get())],
            pending: 0..0,
        })
    }

    fn channel_count(&self) -> usize {
        usize::from(self.channels.get())
    }

    /// Reads the next packet into `packet`. Returns `false` at the end of the stream.
    ///
    /// FFmpeg can write a page with no segments after large tags. The Ogg format allows this,
    /// but `OggOpusReader` reports it as an error. The page has no content and the reader has
    /// already moved past it, so reading again continues with the next page.
    fn read_packet(&mut self) -> opus_pure::Result<bool> {
        loop {
            match self.reader.read_packet_into(&mut self.packet) {
                Err(opus_pure::Error::InvalidStream(message)) if message.contains("empty segment table") => {}
                result => return result,
            }
        }
    }

    /// Decodes the next packet into `block`. Returns `None` at the end of the stream.
    fn decode_next(&mut self) -> Option<()> {
        self.read_packet().ok()?.then_some(())?;
        // A corrupt packet loses its own audio only. Playback continues with the next packet.
        let samples = self
            .decoder
            .decode(&self.packet.data, MAX_PACKET_SAMPLES, &mut self.block)
            .unwrap_or(0);
        self.pending = self.trim.keep_range(&self.packet, samples * self.channel_count());
        Some(())
    }

    /// Ogg has no index, so this walks the packets from the start. Packets before the
    /// pre-roll are only counted, which makes the walk far cheaper than decoding them.
    fn seek_to(&mut self, position: Duration) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = (position.as_secs_f64() * f64::from(RATE)) as u64;
        let decode_from = target.saturating_sub(SEEK_PRE_ROLL);

        self.reader.rewind()?;
        self.decoder.reset_state()?;
        self.trim = new_trim(&self.head, self.channels)?;
        self.pending = 0..0;

        let channels = self.channel_count();
        while self.read_packet()? {
            let samples = packet::samples_48k(&self.packet.data).unwrap_or(0);
            if self.trim.samples_emitted() + samples as u64 <= decode_from {
                self.trim.keep_range(&self.packet, samples * channels);
                continue;
            }

            let decoded = self
                .decoder
                .decode(&self.packet.data, MAX_PACKET_SAMPLES, &mut self.block)
                .unwrap_or(0);
            let before = self.trim.samples_emitted();
            let audio = self.trim.keep_range(&self.packet, decoded * channels);
            if self.trim.samples_emitted() > target {
                let into_packet = target.saturating_sub(before) as usize * channels;
                self.pending = audio.start + into_packet..audio.end;
                return Ok(());
            }
        }
        Ok(())
    }
}

fn new_trim(head: &OpusHead, channels: ChannelCount) -> Result<Trim, String> {
    Trim::new(head, RATE as i32, usize::from(channels.get())).map_err(|err| err.to_string())
}

/// Finds the granule position of the stream's last page, which gives the track length.
fn last_granule(file: &mut File, serial: u32) -> io::Result<Option<u64>> {
    const HEADER: usize = 27;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;

    let granule = (0..tail.len().saturating_sub(HEADER - 1)).rev().find_map(|start| {
        let page = &tail[start..start + HEADER];
        let is_page = page.starts_with(b"OggS") && page[4] == 0;
        if !is_page || page[14..18] != serial.to_le_bytes() {
            return None;
        }
        let granule = i64::from_le_bytes(page[6..14].try_into().ok()?);
        u64::try_from(granule).ok()
    });
    Ok(granule)
}

impl Iterator for OpusSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        while self.pending.is_empty() {
            self.decode_next()?;
        }
        let sample = self.block[self.pending.start];
        self.pending.start += 1;
        Some(sample)
    }
}

impl Source for OpusSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.seek_to(position).map_err(|err| SeekError::Other(Arc::from(err)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opus_pure::{Application, MAX_PACKET_BYTES, OggOpusWriter, OpusEncoder};
    use std::fs;

    const SILENT_SECONDS: usize = 2;
    const TONE_SECONDS: usize = 2;

    /// Writes stereo Opus: `SILENT_SECONDS` of silence, then `TONE_SECONDS` of a 440 Hz tone.
    fn write_silence_then_tone(path: &Path) {
        let frame = 960;
        let rate = RATE as usize;
        let total = (SILENT_SECONDS + TONE_SECONDS) * rate;
        let mut encoder = OpusEncoder::new(RATE as i32, 2, Application::Audio).unwrap();
        let head = OpusHead::for_encoder(&encoder, RATE);
        let pre_skip = head.pre_skip as usize;
        let final_granule = pre_skip + total;
        let frames = (total + pre_skip).div_ceil(frame);
        let mut writer =
            OggOpusWriter::new(io::BufWriter::new(File::create(path).unwrap()), head).unwrap();

        let mut bytes = vec![0u8; MAX_PACKET_BYTES];
        let mut pcm = vec![0f32; frame * 2];
        for index in 0..frames {
            for offset in 0..frame {
                let sample_index = index * frame + offset;
                let value = if sample_index >= SILENT_SECONDS * rate && sample_index < total {
                    let t = sample_index as f32 / RATE as f32;
                    0.5 * (t * 440.0 * std::f32::consts::TAU).sin()
                } else {
                    0.0
                };
                pcm[offset * 2] = value;
                pcm[offset * 2 + 1] = value;
            }
            let written = encoder.encode(&pcm, frame, &mut bytes).unwrap();
            if index + 1 == frames {
                let duration = final_granule as u64 - writer.granule() as u64;
                writer.write_packet_with_duration(&bytes[..written], duration as u32).unwrap();
            } else {
                writer.write_packet(&bytes[..written]).unwrap();
            }
        }
        writer.finish().unwrap();
    }

    fn test_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("amplo-{name}-{}.opus", std::process::id()));
        write_silence_then_tone(&path);
        path
    }

    fn peak(samples: impl Iterator<Item = f32>) -> f32 {
        samples.fold(0.0, |peak, sample| peak.max(sample.abs()))
    }

    #[test]
    fn decodes_exact_length_and_reports_duration() {
        let path = test_file("length");
        let source = OpusSource::open(&path).unwrap();
        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.sample_rate().get(), RATE);
        let expected = SILENT_SECONDS + TONE_SECONDS;
        assert_eq!(source.total_duration(), Some(Duration::from_secs(expected as u64)));
        assert_eq!(source.count(), expected * RATE as usize * 2);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn seeks_forward_and_backward_to_the_right_audio() {
        let path = test_file("seek");
        let mut source = OpusSource::open(&path).unwrap();
        let second = RATE as usize * 2;
        let total = (SILENT_SECONDS + TONE_SECONDS) * second;

        // Into the tone: the remaining length is exact and the audio is the tone.
        source.try_seek(Duration::from_millis(3_000)).unwrap();
        let tone: Vec<f32> = source.by_ref().take(second / 2).collect();
        assert!(peak(tone.into_iter()) > 0.3, "expected the tone after seeking into it");
        assert_eq!(source.by_ref().count(), total - 3 * second - second / 2);

        // Back into the silence, after the stream ended: the pre-skip state is rebuilt.
        source.try_seek(Duration::from_millis(500)).unwrap();
        assert!(peak(source.by_ref().take(second)) < 0.01, "expected silence after seeking back");

        // Past the end simply ends the track.
        source.try_seek(Duration::from_secs(60)).unwrap();
        assert_eq!(source.next(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_past_an_empty_page_after_the_tags() {
        let path = test_file("empty-page");
        let mut bytes = fs::read(&path).unwrap();
        let pages: Vec<usize> = (0..bytes.len() - 4).filter(|&i| bytes[i..].starts_with(b"OggS")).collect();

        // A page header with no segments, placed before the first audio page.
        let mut empty_page = b"OggS\0\0".to_vec();
        empty_page.extend_from_slice(&[0xff; 8]); // Granule position -1.
        empty_page.extend_from_slice(&bytes[14..18]); // Serial number of the stream.
        empty_page.extend_from_slice(&[0; 9]); // Sequence, CRC, and segment count.
        bytes.splice(pages[2]..pages[2], empty_page);
        fs::write(&path, bytes).unwrap();

        let mut source = OpusSource::open(&path).unwrap();
        let second = RATE as usize * 2;
        let total = (SILENT_SECONDS + TONE_SECONDS) * second;
        assert_eq!(source.by_ref().count(), total);

        source.try_seek(Duration::from_secs(3)).unwrap();
        assert_eq!(source.count(), total - 3 * second);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_files_that_are_not_ogg_opus() {
        let path = std::env::temp_dir().join(format!("amplo-not-opus-{}.opus", std::process::id()));
        fs::write(&path, b"OggS but not a real stream").unwrap();
        assert!(OpusSource::open(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
