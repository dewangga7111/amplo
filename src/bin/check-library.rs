//! Decodes every audio file in a folder from start to end, without playing anything.
//! It only reads the files.
//!
//! Usage: cargo run --release --bin check-library -- <folder> [threads]

use std::{
    collections::{BTreeMap, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use amplo::audio::{BoxedSource, collect_tracks, open_source};
use rodio::Source;
use walkdir::WalkDir;

/// Allowed difference between the reported length and the audio that actually decodes.
const LENGTH_TOLERANCE: f64 = 1.0;
/// Below this peak level a whole track counts as silent.
const SILENCE_PEAK: f32 = 0.0001;

#[derive(Default)]
struct Report {
    failures: Vec<String>,
    warnings: Vec<String>,
    seconds: f64,
}

struct Decoded {
    seconds: f64,
    peak: f32,
}

fn decode_all(mut source: BoxedSource) -> Decoded {
    let samples_per_second = f64::from(source.sample_rate().get()) * f64::from(source.channels().get());
    let (mut count, mut peak) = (0u64, 0f32);
    for sample in source.by_ref() {
        count += 1;
        peak = peak.max(sample.abs());
    }
    Decoded { seconds: count as f64 / samples_per_second, peak }
}

/// Seeks to the middle of the track and expects a full second of audio from there.
fn check_seek(path: &Path, duration: Duration) -> Result<(), String> {
    let mut source = open_source(path)?;
    let middle = duration / 2;
    source.try_seek(middle).map_err(|err| format!("seek gagal: {err}"))?;
    let wanted = source.sample_rate().get() as usize * usize::from(source.channels().get());
    let got = source.by_ref().take(wanted).count();
    if got < wanted && duration.as_secs_f64() > 3.0 {
        return Err(format!("seek ke tengah hanya menghasilkan {got} dari {wanted} sample"));
    }
    Ok(())
}

fn check_file(path: &Path, report: &mut Report) {
    let name = path.display();
    let source = match open_source(path) {
        Ok(source) => source,
        Err(err) => {
            report.failures.push(format!("{name}: {err}"));
            return;
        }
    };
    let duration = source.total_duration();
    let decoded = decode_all(source);
    report.seconds += decoded.seconds;

    if decoded.seconds == 0.0 {
        report.failures.push(format!("{name}: tidak ada audio yang ter-decode"));
        return;
    }
    if decoded.peak < SILENCE_PEAK {
        report.warnings.push(format!("{name}: seluruh track sunyi"));
    }
    let Some(duration) = duration else {
        report.warnings.push(format!("{name}: durasi tidak diketahui, slider seek tidak tampil"));
        return;
    };
    let difference = decoded.seconds - duration.as_secs_f64();
    if difference < -LENGTH_TOLERANCE {
        report.failures.push(format!(
            "{name}: berhenti lebih awal, {:.1}s dari {:.1}s",
            decoded.seconds,
            duration.as_secs_f64()
        ));
    } else if difference > LENGTH_TOLERANCE {
        report.warnings.push(format!(
            "{name}: audio {:.1}s lebih panjang dari durasi file {:.1}s",
            difference,
            duration.as_secs_f64()
        ));
    }
    if let Err(err) = check_seek(path, duration) {
        report.failures.push(format!("{name}: {err}"));
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(ToString::to_string)
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic tanpa pesan".to_owned())
}

fn extension(path: &Path) -> String {
    path.extension()
        .map_or_else(|| "(tanpa ekstensi)".to_owned(), |ext| ext.to_string_lossy().to_lowercase())
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(folder) = args.next().map(PathBuf::from) else {
        eprintln!("Usage: check-library <folder> [threads]");
        return ExitCode::from(2);
    };
    let threads: usize = args
        .next()
        .and_then(|value| value.to_str()?.parse().ok())
        .unwrap_or(4);

    let tracks = collect_tracks([folder.clone()]);
    let mut per_extension: BTreeMap<String, usize> = BTreeMap::new();
    for path in &tracks {
        *per_extension.entry(extension(path)).or_default() += 1;
    }
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for entry in WalkDir::new(&folder).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() && !amplo::audio::is_audio(entry.path()) {
            *skipped.entry(extension(entry.path())).or_default() += 1;
        }
    }

    let total = tracks.len();
    println!("Memeriksa {total} file audio: {per_extension:?}");
    println!("Bukan audio, dilewati: {skipped:?}");

    let started = Instant::now();
    let queue = Arc::new(Mutex::new(tracks.into_iter().collect::<VecDeque<_>>()));
    let report = Arc::new(Mutex::new(Report::default()));
    let done = Arc::new(Mutex::new(0usize));

    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (queue, report, done) = (queue.clone(), report.clone(), done.clone());
            thread::spawn(move || {
                loop {
                    let Some(path) = queue.lock().unwrap().pop_front() else { break };
                    let mut local = Report::default();
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| check_file(&path, &mut local))) {
                        local.failures.push(format!("{}: panic: {}", path.display(), panic_message(&*payload)));
                    }
                    let mut shared = report.lock().unwrap();
                    shared.failures.append(&mut local.failures);
                    shared.warnings.append(&mut local.warnings);
                    shared.seconds += local.seconds;
                    drop(shared);

                    let mut done = done.lock().unwrap();
                    *done += 1;
                    if *done % 50 == 0 || *done == total {
                        println!("{}/{total} selesai, {:.0}s", *done, started.elapsed().as_secs_f64());
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let report = report.lock().unwrap();
    println!(
        "\nSelesai dalam {:.0}s. Audio ter-decode: {:.1} jam.",
        started.elapsed().as_secs_f64(),
        report.seconds / 3600.0
    );
    println!("Gagal: {}. Peringatan: {}.", report.failures.len(), report.warnings.len());
    for failure in &report.failures {
        println!("GAGAL {failure}");
    }
    for warning in &report.warnings {
        println!("PERINGATAN {warning}");
    }
    if report.failures.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
