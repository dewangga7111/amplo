//! The audio visualizer: copies playing samples and turns them into frequency bars.

use std::{
    f32::consts::TAU,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use rodio::{ChannelCount, Sample, SampleRate, Source, source::SeekError};
use rustfft::{Fft, FftPlanner, num_complex::Complex};

use crate::audio::BoxedSource;

const FFT_SIZE: usize = 2048;
/// Samples collected on the audio thread before it hands them over.
const BATCH: usize = 256;
const LOWEST_HZ: f32 = 50.0;
const HIGHEST_HZ: f32 = 14_000.0;
/// The level range that the bars show, from empty to full.
const FLOOR_DB: f32 = -60.0;
const RANGE_DB: f32 = 48.0;

/// The latest mono samples, shared between the audio thread and the UI thread.
#[derive(Default)]
pub struct SampleBuffer {
    samples: Mutex<Vec<f32>>,
    sample_rate: AtomicU32,
}

impl SampleBuffer {
    pub fn clear(&self) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }
    }

    fn push(&self, batch: &mut Vec<f32>) {
        // The audio thread must not wait for the UI. A busy lock only delays this batch.
        if let Ok(mut samples) = self.samples.try_lock() {
            samples.extend_from_slice(batch);
            let extra = samples.len().saturating_sub(FFT_SIZE);
            samples.drain(..extra);
            batch.clear();
        } else if batch.len() > FFT_SIZE {
            batch.clear();
        }
    }
}

/// Passes audio through unchanged, and copies it to a [`SampleBuffer`].
pub struct Tap {
    inner: BoxedSource,
    buffer: Arc<SampleBuffer>,
    batch: Vec<f32>,
    frame_sum: f32,
    frame_position: u16,
}

impl Tap {
    pub fn new(inner: BoxedSource, buffer: Arc<SampleBuffer>) -> Self {
        buffer.sample_rate.store(inner.sample_rate().get(), Ordering::Relaxed);
        buffer.clear();
        Self { inner, buffer, batch: Vec::with_capacity(FFT_SIZE), frame_sum: 0.0, frame_position: 0 }
    }
}

impl Iterator for Tap {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        self.frame_sum += sample;
        self.frame_position += 1;
        let channels = self.inner.channels().get();
        if self.frame_position >= channels {
            self.batch.push(self.frame_sum / f32::from(channels));
            self.frame_sum = 0.0;
            self.frame_position = 0;
            if self.batch.len() >= BATCH {
                self.buffer.push(&mut self.batch);
            }
        }
        Some(sample)
    }
}

impl Source for Tap {
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
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.batch.clear();
        self.buffer.clear();
        self.frame_sum = 0.0;
        self.frame_position = 0;
        self.inner.try_seek(position)
    }
}

/// Turns samples into bars from 0 to 1, spaced evenly on a logarithmic frequency scale.
pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    bars: Vec<f32>,
}

impl Analyzer {
    pub fn new(bars: usize) -> Self {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window,
            scratch: vec![Complex::default(); FFT_SIZE],
            bars: vec![0.0; bars],
        }
    }

    pub fn bars(&self) -> &[f32] {
        &self.bars
    }

    /// Lets every bar fall, for pauses and for silence.
    pub fn decay(&mut self, dt: f32) {
        let keep = (-dt * 9.0).exp();
        for bar in &mut self.bars {
            *bar *= keep;
        }
    }

    /// Measures the newest samples. Bars rise fast and fall slowly, so they look steady.
    pub fn update(&mut self, buffer: &SampleBuffer, dt: f32) {
        let rate = buffer.sample_rate.load(Ordering::Relaxed) as f32;
        let ready = buffer.samples.lock().is_ok_and(|samples| {
            if samples.len() < FFT_SIZE {
                return false;
            }
            for ((slot, sample), weight) in self.scratch.iter_mut().zip(samples.iter()).zip(&self.window) {
                *slot = Complex::new(sample * weight, 0.0);
            }
            true
        });
        if !ready || rate <= 0.0 {
            self.decay(dt);
            return;
        }
        self.fft.process(&mut self.scratch);

        let count = self.bars.len();
        let top = HIGHEST_HZ.min(rate / 2.0);
        let hz_per_bin = rate / FFT_SIZE as f32;
        let edge = |index: usize| LOWEST_HZ * (top / LOWEST_HZ).powf(index as f32 / count as f32);
        let rise = 1.0 - (-dt * 30.0).exp();
        let fall = 1.0 - (-dt * 6.0).exp();
        for (index, bar) in self.bars.iter_mut().enumerate() {
            let first = (edge(index) / hz_per_bin) as usize;
            let last = ((edge(index + 1) / hz_per_bin) as usize).max(first + 1).min(FFT_SIZE / 2);
            let peak = self.scratch[first.min(last - 1)..last]
                .iter()
                .fold(0.0f32, |peak, bin| peak.max(bin.norm()));
            // A full-scale sine under a Hann window peaks at a quarter of the FFT size.
            let db = 20.0 * (peak / (FFT_SIZE as f32 / 4.0)).max(1e-6).log10();
            // Squaring keeps most music from filling every bar, so the shape stays readable.
            let level = ((db - FLOOR_DB) / RANGE_DB).clamp(0.0, 1.0).powi(2);
            *bar +=(level - *bar) * if level > *bar { rise } else { fall };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    const RATE: u32 = 48_000;

    fn tone(hz: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(RATE as f32 * seconds) as usize)
            .map(|i| amplitude * (TAU * hz * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn analyze(samples: Vec<f32>) -> Analyzer {
        let source: BoxedSource = Box::new(SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(RATE).unwrap(), samples));
        let buffer = Arc::new(SampleBuffer::default());
        Tap::new(source, buffer.clone()).for_each(drop);
        let mut analyzer = Analyzer::new(28);
        for _ in 0..30 {
            analyzer.update(&buffer, 0.1);
        }
        analyzer
    }

    fn loudest(analyzer: &Analyzer) -> usize {
        let bars = analyzer.bars();
        (0..bars.len()).max_by(|&a, &b| bars[a].total_cmp(&bars[b])).unwrap()
    }

    #[test]
    fn passes_audio_through_unchanged() {
        let samples = tone(440.0, 0.5, 0.1);
        let source: BoxedSource = Box::new(SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(RATE).unwrap(), samples.clone()));
        let output: Vec<f32> = Tap::new(source, Arc::new(SampleBuffer::default())).collect();
        assert_eq!(output, samples);
    }

    #[test]
    fn a_higher_tone_lights_a_higher_bar() {
        let low = loudest(&analyze(tone(200.0, 0.8, 0.2)));
        let high = loudest(&analyze(tone(5_000.0, 0.8, 0.2)));
        assert!(low < high, "low bar {low}, high bar {high}");
        assert!(analyze(tone(200.0, 0.8, 0.2)).bars()[low] > 0.8);
    }

    #[test]
    fn silence_gives_empty_bars() {
        let analyzer = analyze(vec![0.0; RATE as usize / 5]);
        assert!(analyzer.bars().iter().all(|&bar| bar < 0.01));
    }

    #[test]
    fn bars_fall_without_new_samples() {
        let mut analyzer = analyze(tone(1_000.0, 0.8, 0.2));
        let before: f32 = analyzer.bars().iter().sum();
        analyzer.decay(0.5);
        assert!(analyzer.bars().iter().sum::<f32>() < before * 0.1);
    }
}
