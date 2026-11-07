//! Fallback decoding through Windows Media Foundation, for codecs that Symphonia lacks.
//! The file with Dolby Digital Plus (E-AC-3) audio needs it, for example.

use std::{path::Path, thread};

use rodio::{ChannelCount, SampleRate, buffer::SamplesBuffer};
use windows::{
    Win32::{
        Media::MediaFoundation::{
            IMFSourceReader, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
            MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS,
            MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION,
            MFAudioFormat_Float, MFCreateMediaType, MFCreateSourceReaderFromURL,
            MFMediaType_Audio, MFSTARTUP_FULL, MFShutdown, MFStartup,
        },
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    },
    core::{HSTRING, Result},
};

/// Decodes the whole file into memory. The work runs on its own thread so that COM can use
/// the multithreaded apartment, because the UI thread is already in a single-threaded one.
pub fn decode_to_memory(path: &Path) -> std::result::Result<SamplesBuffer, String> {
    let path = HSTRING::from(path.as_os_str());
    thread::spawn(move || decode_on_this_thread(&path))
        .join()
        .map_err(|_| "Media Foundation thread panicked".to_owned())?
        .map_err(|err| err.to_string())
}

fn decode_on_this_thread(path: &HSTRING) -> Result<SamplesBuffer> {
    // SAFETY: Each startup call is paired with its shutdown call on this same thread, and
    // `read_all` drops every Media Foundation object before the shutdown runs.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let result = MFStartup(MF_VERSION, MFSTARTUP_FULL).and_then(|()| {
            let decoded = read_all(path);
            MFShutdown()?;
            decoded
        });
        CoUninitialize();
        result
    }
}

/// Asks for stereo, so the decoder itself downmixes surround and keeps the centre channel.
/// A decoder that refuses stereo keeps its own channel layout.
unsafe fn request_float_output(reader: &IMFSourceReader, stream: u32) -> Result<()> {
    unsafe {
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
        output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        if reader.SetCurrentMediaType(stream, None, &output).is_ok() {
            return Ok(());
        }
        output.DeleteItem(&MF_MT_AUDIO_NUM_CHANNELS)?;
        reader.SetCurrentMediaType(stream, None, &output)
    }
}

unsafe fn read_all(path: &HSTRING) -> Result<SamplesBuffer> {
    unsafe {
        let reader = MFCreateSourceReaderFromURL(path, None)?;
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(stream, true)?;
        request_float_output(&reader, stream)?;

        let actual = reader.GetCurrentMediaType(stream)?;
        let channels = actual.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?;
        let rate = actual.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
        let invalid = || windows::core::Error::from(windows::Win32::Foundation::E_FAIL);
        let channels: ChannelCount = u16::try_from(channels)
            .ok()
            .and_then(std::num::NonZero::new)
            .ok_or_else(invalid)?;
        let rate: SampleRate = std::num::NonZero::new(rate).ok_or_else(invalid)?;

        let mut samples: Vec<f32> = Vec::new();
        loop {
            let mut flags = 0u32;
            let mut sample = None;
            reader.ReadSample(stream, 0, None, Some(&raw mut flags), None, Some(&raw mut sample))?;
            if let Some(sample) = sample {
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut data = std::ptr::null_mut();
                let mut length = 0u32;
                buffer.Lock(&raw mut data, None, Some(&raw mut length))?;
                // SAFETY: The buffer stays locked here, and the requested output format is
                // 32-bit float. Media Foundation aligns its audio buffers for that format.
                samples.extend_from_slice(std::slice::from_raw_parts(
                    data.cast::<f32>(),
                    length as usize / size_of::<f32>(),
                ));
                buffer.Unlock()?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        Ok(SamplesBuffer::new(channels, rate, samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;
    use std::{fs, time::Duration};

    #[test]
    fn decodes_a_wav_file_to_memory() {
        let path = std::env::temp_dir().join(format!("amplo-mf-{}.wav", std::process::id()));
        let data_len: u32 = 44_100 * 2 * 2; // 1 second of 16-bit stereo silence.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&176_400u32.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);
        fs::write(&path, bytes).unwrap();

        let source = decode_to_memory(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.sample_rate().get(), 44_100);
        let duration = source.total_duration().unwrap();
        assert!(duration.abs_diff(Duration::from_secs(1)) < Duration::from_millis(50));
    }

    #[test]
    fn reports_an_error_for_a_missing_file() {
        assert!(decode_to_memory(Path::new("no such file.mp4")).is_err());
    }
}
