//! These tests drive the real UI without a window. They open the default audio device, but
//! stay inaudible: the WAV data is silence and the volume is 0.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eframe::egui::{self, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use amplo::{
    library::ALL_SONGS,
    lyrics::{Extra, Lyrics},
    settings::Settings,
};

use crate::{
    app::{Action, App, CardTab},
    spotify_page::SpotifyAction,
    ui::WINDOW_SIZE,
};

fn write_silent_wav(path: &Path, seconds: f32) {
    let data_len = (44_100.0 * seconds) as u32 * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + data_len as usize, 0);
    fs::write(path, bytes).unwrap();
}

/// Removes the test folder also when an assertion fails.
struct TestFolder(PathBuf);

impl TestFolder {
    /// All Songs: 1. a short, 2. b short2, 3. c long (20 s), 4. d corrupt.
    /// Playlist `Mix`: c long, a short.
    fn new(test_name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("amplo-{test_name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_silent_wav(&root.join("a short.wav"), 0.2);
        write_silent_wav(&root.join("b short2.wav"), 0.2);
        write_silent_wav(&root.join("c long.wav"), 20.0);
        fs::write(root.join("d corrupt.mp3"), [1u8; 200]).unwrap();
        fs::write(root.join("Mix.m3u"), "#EXTM3U\nc long.wav\na short.wav\n").unwrap();
        fs::write(root.join("c long.lrc"), "[ti:x]\n[00:00.00]v1:First line\n[00:10.00]v1:Second line\n").unwrap();
        Self(root)
    }
}

impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn silent_settings() -> Settings {
    Settings { volume: 0.0, ..Settings::default() }
}

fn wait_until(harness: &mut Harness<'static, App>, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(harness.state()) {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
}

fn harness_with(folder: &TestFolder, settings: Settings, settings_path: Option<PathBuf>) -> Harness<'static, App> {
    let root = folder.0.clone();
    let mut harness = Harness::builder()
        .with_size(WINDOW_SIZE)
        .allow_missing_glyphs()
        .build_eframe(move |cc| App::new(cc.egui_ctx.clone(), settings, settings_path, Vec::new()).with_test_library(root));
    wait_until(&mut harness, "library scan", |app| !app.library.tracks.is_empty() && !app.scanning);
    harness.run_steps(4);
    harness
}

fn harness_for(folder: &TestFolder) -> Harness<'static, App> {
    harness_with(folder, silent_settings(), None)
}

fn click(harness: &mut Harness<'static, App>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(8);
}

fn position(harness: &Harness<'static, App>) -> f32 {
    harness.state().player.as_ref().unwrap().get_pos().as_secs_f32()
}

fn is_paused(harness: &Harness<'static, App>) -> bool {
    harness.state().player.as_ref().unwrap().is_paused()
}

fn current_title(harness: &Harness<'static, App>) -> String {
    let app = harness.state();
    app.library.tracks[app.current_track().unwrap()].info.title.clone()
}

fn pointer_button(harness: &mut Harness<'static, App>, pos: egui::Pos2, pressed: bool) {
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(3);
}

#[test]
fn the_embedded_window_icon_is_valid() {
    let icon = crate::app_icon();
    assert_eq!((icon.width, icon.height), (256, 256));
    assert_eq!(icon.rgba.len(), 256 * 256 * 4);
}

#[test]
fn starts_on_the_list_without_autoplay() {
    let folder = TestFolder::new("start");
    let harness = harness_for(&folder);
    let app = harness.state();
    assert!(app.minimized);
    assert!(app.current_track().is_none());
    assert!(app.player.is_none());
    assert_eq!(app.library.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["All Songs", "Liked Songs", "Mix"]);
    harness.get_by_label("All Songs");
    harness.get_by_label("Mix");
    assert!(harness.query_by_label("Liked Songs").is_none(), "an empty Liked Songs must stay hidden");
    assert!(harness.query_by_label("Pause").is_none(), "no mini player without a track");
}

#[test]
fn rows_play_in_the_list_and_the_mini_player_opens_now_playing() {
    let folder = TestFolder::new("views");
    let mut harness = harness_for(&folder);

    click(&mut harness, "3. c long");
    assert_eq!(current_title(&harness), "c long");
    assert!(!harness.state().minimized, "a row click opens Now Playing");
    assert_eq!(harness.state().slide, 1.0);
    assert!(harness.state().player.is_some());
    assert_eq!(harness.state().error, None);
    assert!(harness.query_by_label("Expand").is_none(), "no mini player over Now Playing");
    assert!(harness.query_by_label("3. c long").is_none(), "the list is covered");
    assert_eq!(harness.state().tech, ["706 KBPS", "44.1 KHZ", "MONO"]);

    click(&mut harness, "Back to song list");
    assert!(harness.state().minimized);
    click(&mut harness, "Expand");
    assert!(!harness.state().minimized, "the mini player opens Now Playing again");

    click(&mut harness, "Mix");
    assert!(harness.state().minimized);
    assert_eq!(harness.state().view, 2);
    assert_eq!(harness.state().queue.as_ref().unwrap().playlist, Some(0), "opening a playlist keeps the queue");
    harness.get_by_label("2. a short");
}

#[test]
fn transport_pauses_seeks_and_resumes() {
    let folder = TestFolder::new("transport");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");

    click(&mut harness, "Pause");
    assert!(is_paused(&harness));
    // The audio thread picks up the pause and reports its position a few milliseconds later.
    std::thread::sleep(Duration::from_millis(100));
    let paused_at = position(&harness);
    std::thread::sleep(Duration::from_millis(300));
    harness.run_steps(2);
    assert_eq!(position(&harness), paused_at, "position moved while paused");

    // Hold the seek slider: nothing seeks until release.
    let sliders: Vec<_> = harness.get_all_by_role(Role::Slider).collect();
    assert_eq!(sliders.len(), 2, "expected seek and volume sliders");
    let center = sliders[0].rect().center();
    drop(sliders);
    harness.input_mut().events.push(egui::Event::PointerMoved(center));
    harness.run_steps(3);
    pointer_button(&mut harness, center, true);
    let preview = harness.state().seek_preview.expect("slider held");
    assert!((preview - 10.0).abs() < 1.0, "preview was {preview}");
    assert_eq!(position(&harness), paused_at, "seeked before release");

    pointer_button(&mut harness, center, false);
    assert_eq!(harness.state().seek_preview, None);
    assert_eq!(harness.state().error, None);
    let target = position(&harness);
    assert!((target - 10.0).abs() < 1.0, "seeked to {target}");
    assert!(is_paused(&harness), "seek must not resume playback");

    click(&mut harness, "Play");
    assert!(!is_paused(&harness));
    std::thread::sleep(Duration::from_millis(300));
    harness.run_steps(2);
    assert!(position(&harness) > target, "playback did not resume");
}

#[test]
fn volume_slider_changes_the_player() {
    let folder = TestFolder::new("volume");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");

    let center = harness.get_all_by_role(Role::Slider).nth(1).unwrap().rect().left_center();
    harness.input_mut().events.push(egui::Event::PointerMoved(center));
    harness.run_steps(2);
    pointer_button(&mut harness, center, true);
    pointer_button(&mut harness, center, false);
    assert_eq!(harness.state().settings.volume, 0.0, "the left end must stay silent");
    assert_eq!(harness.state().player.as_ref().unwrap().volume(), 0.0);
}

#[test]
fn next_and_previous_wrap_around() {
    let folder = TestFolder::new("wrap");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Next");
    assert_eq!(current_title(&harness), "d corrupt");
    assert!(harness.state().player.is_none(), "a broken file does not play");
    assert!(harness.state().error.as_deref().unwrap().contains("d corrupt"));

    click(&mut harness, "Next");
    assert_eq!(current_title(&harness), "a short", "next wraps to the first track");
    assert_eq!(harness.state().error, None);
    click(&mut harness, "Previous");
    assert_eq!(current_title(&harness), "d corrupt", "previous wraps to the last track");
}

#[test]
fn previous_restarts_a_track_after_three_seconds() {
    let folder = TestFolder::new("restart");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(8)).unwrap();
    click(&mut harness, "Previous");
    assert_eq!(current_title(&harness), "c long");
    assert!(position(&harness) < 3.0);
}

/// After sleep, Windows stops the audio stream. Its error callback sets the same flag.
#[test]
fn a_lost_audio_output_opens_again_and_continues_the_track() {
    let folder = TestFolder::new("audio-lost");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(8)).unwrap();
    click(&mut harness, "Pause");
    let lost = std::sync::Arc::clone(&harness.state().audio_lost);

    lost.store(true, std::sync::atomic::Ordering::Relaxed);
    harness.run_steps(2);
    assert!(!lost.load(std::sync::atomic::Ordering::Relaxed), "the output was not opened again");
    assert_eq!(current_title(&harness), "c long");
    assert!(is_paused(&harness), "a paused track stays paused");
    assert!((position(&harness) - 8.0).abs() < 0.5, "continued at {}", position(&harness));

    click(&mut harness, "Play");
    wait_until(&mut harness, "playback", |app| app.player.as_ref().unwrap().get_pos() > Duration::from_millis(8300));
}

#[test]
fn track_end_skips_broken_files_and_keeps_the_message() {
    let folder = TestFolder::new("skip");
    let mut harness = harness_for(&folder);
    // The queue becomes: a short, d corrupt, c long.
    click(&mut harness, "1. a short");
    harness.state_mut().queue.as_mut().unwrap().tracks = vec![0, 3, 2];

    wait_until(&mut harness, "track end", |app| app.current_track() != Some(0));
    assert_eq!(current_title(&harness), "c long", "did not skip the broken file");
    assert!(harness.state().player.is_some());
    let error = harness.state().error.clone().expect("the skip must stay visible");
    assert!(error.contains("d corrupt"), "error was {error}");
}

#[test]
fn repeat_plays_the_same_track_again() {
    let folder = TestFolder::new("repeat");
    let mut harness = harness_for(&folder);
    click(&mut harness, "1. a short");
    click(&mut harness, "Repeat");
    assert!(harness.state().repeat);
    // The next finish restarts the track from the start.
    let deadline = Instant::now() + Duration::from_millis(800);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        harness.run_steps(1);
    }
    assert_eq!(current_title(&harness), "a short");
    assert!(harness.state().player.is_some());
}

#[test]
fn like_saves_and_fills_the_liked_playlist() {
    let folder = TestFolder::new("like");
    let settings_path = folder.0.join("state.txt");
    let mut harness = harness_with(&folder, silent_settings(), Some(settings_path.clone()));

    click(&mut harness, "3. c long");
    click(&mut harness, "Like");
    let saved = Settings::parse(&fs::read_to_string(&settings_path).unwrap());
    assert_eq!(saved.liked, BTreeSet::from([folder.0.join("c long.wav")]));
    assert_eq!(harness.state().library.playlists[1].tracks.len(), 1);

    click(&mut harness, "Liked Songs");
    harness.get_by_label("1. c long");

    click(&mut harness, "Expand");
    click(&mut harness, "Unlike");
    let saved = Settings::parse(&fs::read_to_string(&settings_path).unwrap());
    assert!(saved.liked.is_empty());
    assert_eq!(harness.state().view, 0, "an emptied Liked Songs must close");
    assert!(harness.query_by_label("Liked Songs").is_none());
}

#[test]
fn lyrics_follow_playback() {
    let folder = TestFolder::new("lyrics");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    assert!(harness.state().lyrics.as_ref().unwrap().synced);
    assert_eq!(harness.state().lyric_motion.active, Some(0));

    harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(12)).unwrap();
    harness.run_steps(4);
    assert_eq!(harness.state().lyric_motion.active, Some(1));
    assert_eq!(harness.state().lyric_motion.previous, Some(0));

    click(&mut harness, "Next");
    assert_eq!(harness.state().lyrics, None, "the next track has no lyrics file");
}

#[test]
fn long_list_scrolls_to_the_last_row() {
    let root = std::env::temp_dir().join(format!("amplo-scroll-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    for number in 1..=400 {
        fs::File::create(root.join(format!("t{number:03}.wav"))).unwrap();
    }
    let folder = TestFolder(root);
    let mut harness = harness_for(&folder);

    let over_list = harness.get_by_label("1. t001").rect().center();
    harness.input_mut().events.push(egui::Event::PointerMoved(over_list));
    harness.run_steps(2);
    harness.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -1_000_000.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Trackpad,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(30);

    let last = harness.get_by_label("400. t400").rect();
    let previous = harness.get_by_label("399. t399").rect();
    assert_eq!(previous.bottom(), last.top(), "rows must touch");
    assert!(last.bottom() <= WINDOW_SIZE.y, "the last row must fit in the window");
    assert!(harness.query_by_label("1. t001").is_none(), "rows out of view are not drawn");
}

#[test]
fn now_playing_minimizes_back_to_the_song_list() {
    let folder = TestFolder::new("minimize");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    assert!(harness.query_by_label("3. c long").is_none());

    click(&mut harness, "Back to song list");
    assert!(harness.state().minimized);
    assert_eq!(harness.state().slide, 0.0);
    harness.get_by_label("3. c long");
    harness.get_by_label("Expand");
    assert!(!is_paused(&harness), "minimizing must not pause");
}

#[test]
fn shuffle_play_builds_a_new_order_each_time() {
    let folder = TestFolder::new("random");
    let mut harness = harness_for(&folder);
    click(&mut harness, "Mix");

    let mut starts = BTreeSet::new();
    for _ in 0..30 {
        click(&mut harness, "Shuffle play");
        starts.insert(current_title(&harness));
        let queue = harness.state().queue.as_ref().unwrap();
        assert_eq!(queue.playlist, Some(2));
        assert_eq!(queue.position, 0, "the order is decided before the first song");
    }
    assert_eq!(starts, BTreeSet::from(["a short".to_owned(), "c long".to_owned()]));
    assert!(harness.state().shuffle, "shuffle stays on");
    assert!(harness.state().minimized, "the playlist's buttons stay in the list");
}

fn play_order(harness: &Harness<'static, App>) -> Vec<usize> {
    harness.state().queue.as_ref().unwrap().tracks.clone()
}

fn queue_position(harness: &Harness<'static, App>) -> usize {
    harness.state().queue.as_ref().unwrap().position
}

#[test]
fn next_and_previous_walk_the_shuffled_order() {
    let folder = TestFolder::new("shuffle-walk");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Shuffle");

    let order = play_order(&harness);
    assert_eq!(harness.state().library.tracks[order[0]].info.title, "c long", "the playing song goes first");
    assert_eq!(queue_position(&harness), 0);
    assert!(!is_paused(&harness), "turning shuffle on must not restart the song");

    click(&mut harness, "Next");
    assert_eq!(harness.state().current_track(), Some(order[1]));
    click(&mut harness, "Next");
    assert_eq!(harness.state().current_track(), Some(order[2]));
    click(&mut harness, "Previous");
    assert_eq!(harness.state().current_track(), Some(order[1]), "back finds the song played before");
    assert_eq!(play_order(&harness), order, "walking must not reshuffle");
}

#[test]
fn back_after_a_track_ends_finds_the_song_before_it() {
    let folder = TestFolder::new("shuffle-end");
    let mut harness = harness_for(&folder);
    click(&mut harness, "1. a short");
    click(&mut harness, "Shuffle");
    // a short, b short2, c long, in that order.
    harness.state_mut().queue.as_mut().unwrap().tracks = vec![0, 1, 2];
    harness.state_mut().queue.as_mut().unwrap().position = 0;

    wait_until(&mut harness, "track end", |app| app.current_track() != Some(0));
    assert_eq!(harness.state().current_track(), Some(1), "the end of a track follows the order");
    click(&mut harness, "Previous");
    assert_eq!(harness.state().current_track(), Some(0));
}

#[test]
fn clicking_in_the_queue_keeps_the_order() {
    let folder = TestFolder::new("queue-click");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Shuffle");
    click(&mut harness, "Queue");
    let order = play_order(&harness);

    // The queue shows every other song, and not the one that plays now.
    for &track in &order[1..] {
        let title = harness.state().library.tracks[track].info.title.clone();
        harness.get_by_label(&format!("Queue: {title}"));
    }
    assert!(harness.query_by_label("Queue: c long").is_none());

    let target = harness.state().library.tracks[order[2]].info.title.clone();
    click(&mut harness, &format!("Queue: {target}"));
    assert_eq!(harness.state().current_track(), Some(order[2]));
    assert_eq!(queue_position(&harness), 2);
    assert_eq!(play_order(&harness), order, "a click in the queue must not reshuffle");
}

#[test]
fn clicking_a_song_outside_the_queue_reshuffles() {
    let folder = TestFolder::new("queue-outside");
    let mut harness = harness_for(&folder);
    click(&mut harness, "1. a short");
    click(&mut harness, "Shuffle");
    click(&mut harness, "Back to song list");

    let mut orders = BTreeSet::new();
    for _ in 0..20 {
        click(&mut harness, "3. c long");
        assert_eq!(current_title(&harness), "c long");
        assert_eq!(queue_position(&harness), 0, "the clicked song goes first");
        let order = play_order(&harness);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3]);
        orders.insert(order);
        click(&mut harness, "Back to song list");
    }
    assert!(orders.len() > 1, "the order never changed");
}

#[test]
fn turning_shuffle_off_restores_the_playlist_order() {
    let folder = TestFolder::new("shuffle-off");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Shuffle");
    click(&mut harness, "Shuffle");
    assert!(!harness.state().shuffle);
    assert_eq!(play_order(&harness), [0, 1, 2, 3]);
    assert_eq!(queue_position(&harness), 2, "the same song keeps playing");
    assert_eq!(current_title(&harness), "c long");
}

#[test]
fn a_rescan_keeps_the_shuffled_order_and_adds_new_songs_after_it() {
    let folder = TestFolder::new("shuffle-rescan");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Shuffle");
    let titles = |harness: &Harness<'static, App>| -> Vec<String> {
        let app = harness.state();
        let queue = app.queue.as_ref().unwrap();
        queue.tracks.iter().map(|&track| app.library.tracks[track].info.title.clone()).collect()
    };
    let before = titles(&harness);

    write_silent_wav(&folder.0.join("e new.wav"), 0.2);
    wait_until(&mut harness, "new song", |app| app.library.tracks.len() == 5);
    harness.run_steps(4);
    let after = titles(&harness);
    assert_eq!(after[..4], before[..], "the order so far stays");
    assert_eq!(after[4], "e new");
    assert_eq!(current_title(&harness), "c long");
    assert!(!is_paused(&harness));
}

#[test]
fn new_songs_and_playlists_appear_without_stopping_playback() {
    let folder = TestFolder::new("watch-add");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Back to song list");
    harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(5)).unwrap();

    // Sorts before the playing track, so the track numbers shift.
    write_silent_wav(&folder.0.join("a first new.wav"), 0.2);
    fs::write(folder.0.join("Brand New.m3u"), "a first new.wav\n").unwrap();
    wait_until(&mut harness, "new playlist", |app| {
        app.library.tracks.len() == 5 && app.library.playlists.iter().any(|p| p.name == "Brand New")
    });
    harness.run_steps(4);

    harness.get_by_label("1. a first new");
    harness.get_by_label("4. c long");
    harness.get_by_label("Brand New");
    assert_eq!(current_title(&harness), "c long");
    assert_eq!(harness.state().queue.as_ref().unwrap().position, 3);
    assert_eq!(harness.state().queue.as_ref().unwrap().tracks.len(), 5, "the queue gains the song");
    assert!(!is_paused(&harness));
    assert!(position(&harness) >= 5.0, "playback restarted at {}", position(&harness));
}

#[test]
fn removing_files_updates_the_list_and_keeps_the_shown_playlist() {
    let folder = TestFolder::new("watch-remove");
    let mut harness = harness_for(&folder);
    click(&mut harness, "Mix");
    click(&mut harness, "1. c long");

    fs::remove_file(folder.0.join("a short.wav")).unwrap();
    wait_until(&mut harness, "removal", |app| app.library.tracks.len() == 3);
    harness.run_steps(4);
    assert_eq!(harness.state().view, 2, "still on Mix");
    assert_eq!(current_title(&harness), "c long");
    assert_eq!(harness.state().queue.as_ref().unwrap().tracks.len(), 1);
    assert!(harness.state().player.is_some());
    assert!(harness.query_by_label("2. a short").is_none());

    // The playlist file goes too: the song keeps playing, and the view falls back.
    fs::remove_file(folder.0.join("Mix.m3u")).unwrap();
    wait_until(&mut harness, "playlist removal", |app| app.library.playlists.len() == 2);
    harness.run_steps(4);
    assert_eq!(harness.state().view, 0);
    assert_eq!(harness.state().queue.as_ref().unwrap().playlist, None);
    assert_eq!(current_title(&harness), "c long");
    assert!(harness.state().player.is_some());
}

#[test]
fn removing_the_playing_file_stops_it_with_a_message() {
    let folder = TestFolder::new("watch-playing");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");

    fs::remove_file(folder.0.join("c long.wav")).unwrap();
    wait_until(&mut harness, "removal", |app| app.library.tracks.len() == 3);
    harness.run_steps(8);
    assert!(harness.state().player.is_none());
    assert!(harness.state().current_track().is_none());
    assert!(harness.state().minimized, "Now Playing has nothing to show");
    assert!(harness.state().error.as_deref().unwrap().contains("c long"));
}

/// Saves pictures of each screen, to compare with the design by eye. Set `AMPLO_RENDER_DIR`
/// for the output and `AMPLO_RENDER_LIBRARY` for a real music folder. Stays silent at volume 0.
#[test]
#[ignore = "needs a graphics adapter and writes image files"]
fn render_screens() {
    let output = PathBuf::from(std::env::var_os("AMPLO_RENDER_DIR").expect("AMPLO_RENDER_DIR"));
    fs::create_dir_all(&output).unwrap();
    let folder = TestFolder::new("render");
    let library = std::env::var_os("AMPLO_RENDER_LIBRARY").map_or_else(|| folder.0.clone(), PathBuf::from);

    let mut harness = Harness::builder()
        .with_size(WINDOW_SIZE)
        .allow_missing_glyphs()
        .build_eframe(move |cc| {
            cc.egui_ctx.add_font_provider(std::sync::Arc::new(egui_system_font_provider::SystemFontProvider::new()));
            App::new(cc.egui_ctx.clone(), silent_settings(), None, Vec::new()).with_test_library(library)
        });
    let settle = |harness: &mut Harness<'static, App>, name: &str| {
        // Covers load on another thread.
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            harness.run_steps(1);
        }
        harness.render().unwrap().save(output.join(format!("{name}.png"))).unwrap();
    };

    let deadline = Instant::now() + Duration::from_secs(300);
    while (harness.state().library.tracks.is_empty() || harness.state().scanning) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        harness.run_steps(1);
    }
    settle(&mut harness, "list");

    harness.state_mut().apply(Action::Select(0, crate::app::Selection::Only));
    harness.state_mut().apply(Action::Select(2, crate::app::Selection::Range));
    let third = {
        let app = harness.state();
        app.library.tracks[app.library.playlists[app.view].tracks[2]].info.title.clone()
    };
    harness.get_by_label(&format!("3. {third}")).click_secondary();
    harness.run_steps(4);
    harness.get_by_label("Add 3 songs to playlist").hover();
    harness.run_steps(4);
    harness.get_by_label("New playlist…").hover();
    settle(&mut harness, "list-menu");
    harness.key_press(egui::Key::Escape);
    harness.state_mut().apply(Action::ClearSelection);
    harness.run_steps(2);

    let with_lyrics = |app: &App, wanted: bool| {
        app.library.tracks.iter().position(|track| track.path.with_extension("lrc").exists() == wanted)
    };
    if let Some(row) = with_lyrics(harness.state(), true) {
        harness.state_mut().apply(Action::PlayRow(row));
        harness.state_mut().apply(Action::Minimize);
        settle(&mut harness, "list-playing");
        harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(40)).unwrap();
        harness.state_mut().apply(Action::Expand);
        settle(&mut harness, "now-playing-lyrics");
        harness.state_mut().apply(Action::ToggleShuffle);
        harness.state_mut().apply(Action::ShowCardTab(CardTab::Queue));
        settle(&mut harness, "now-playing-queue");
        harness.state_mut().apply(Action::ToggleShuffle);
        harness.state_mut().apply(Action::ShowCardTab(CardTab::Lyrics));
    }
    // The longest lyric line in the library shows the worst case for wrapping.
    let longest = harness
        .state()
        .library
        .tracks
        .iter()
        .enumerate()
        .filter_map(|(row, track)| {
            let lyrics = Lyrics::for_track(&track.path).filter(|lyrics| lyrics.synced)?;
            let line = lyrics.lines.into_iter().max_by_key(|line| line.text.chars().count())?;
            Some((row, line))
        })
        .max_by_key(|(_, line)| line.text.chars().count());
    if let Some((row, line)) = longest {
        harness.state_mut().apply(Action::PlayRow(row));
        harness.state().player.as_ref().unwrap().try_seek(line.time).unwrap();
        settle(&mut harness, "now-playing-long-lyric");
    }
    let with_romaji = |app: &App| {
        app.library.tracks.iter().position(|track| Lyrics::for_track(&track.path).is_some_and(|lyrics| lyrics.has(Extra::Romaji)))
    };
    if let Some(row) = with_romaji(harness.state()) {
        harness.state_mut().apply(Action::PlayRow(row));
        harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(30)).unwrap();
        settle(&mut harness, "now-playing-romaji");
    }
    if let Some(row) = with_lyrics(harness.state(), false) {
        harness.state_mut().apply(Action::PlayRow(row));
        settle(&mut harness, "now-playing-no-lyrics");
        // Opening the form reads the folder only. Closing it writes nothing.
        harness.state_mut().apply(Action::EditLyrics);
        settle(&mut harness, "lyrics-editor");
        harness.state_mut().apply(Action::CloseLyricsEditor);
    }
    // Only in memory, so a real music folder gets no new files.
    harness.state_mut().apply(Action::Minimize);
    let empty = amplo::library::Playlist { name: "Road Trip".to_owned(), tracks: Vec::new(), path: None };
    harness.state_mut().library.playlists.push(empty);
    let empty = harness.state().library.playlists.len() - 1;
    harness.state_mut().apply(Action::OpenPlaylist(empty));
    settle(&mut harness, "empty-playlist");
    harness.state_mut().apply(Action::NewPlaylist);
    harness.state_mut().new_playlist.as_mut().unwrap().name = "Late Night".to_owned();
    settle(&mut harness, "new-playlist");
    harness.state_mut().new_playlist = None;
    harness.state_mut().library.playlists.pop();
    let playlists = harness.state().library.playlists.len();
    harness.state_mut().apply(Action::OpenPlaylist(playlists - 1));
    settle(&mut harness, "playlist");

    // The full layout at the design's size, and at a maximized 1920 by 1080 screen at 125 %.
    for (size, suffix) in [(egui::vec2(1280.0, 780.0), ""), (egui::vec2(1536.0, 824.0), "-wide")] {
        harness.set_size(size);
        harness.state_mut().full = true;
        harness.state_mut().apply(Action::OpenPlaylist(ALL_SONGS));
        settle(&mut harness, &format!("full-list{suffix}"));
        if let Some(row) = with_lyrics(harness.state(), true) {
            harness.state_mut().apply(Action::PlayRow(row));
            harness.state_mut().apply(Action::Minimize);
            harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(40)).unwrap();
            settle(&mut harness, &format!("full-list-playing{suffix}"));
            harness.state_mut().apply(Action::Expand);
            settle(&mut harness, &format!("full-now-playing{suffix}"));
            harness.state_mut().apply(Action::ShowCardTab(CardTab::Queue));
            settle(&mut harness, &format!("full-queue{suffix}"));
            harness.state_mut().apply(Action::ShowCardTab(CardTab::Lyrics));
            harness.state_mut().apply(Action::Minimize);
        }
        if let Some(row) = with_romaji(harness.state()) {
            harness.state_mut().apply(Action::PlayRow(row));
            harness.state().player.as_ref().unwrap().try_seek(Duration::from_secs(30)).unwrap();
            settle(&mut harness, &format!("full-romaji{suffix}"));
        }
        if let Some(row) = with_lyrics(harness.state(), false) {
            harness.state_mut().apply(Action::PlayRow(row));
            settle(&mut harness, &format!("full-no-lyrics{suffix}"));
            harness.state_mut().apply(Action::Minimize);
        }
    }
    // Spotify, with songs from the folder and some it lacks. Nothing goes over the network.
    harness.state_mut().apply(Action::Spotify(SpotifyAction::Open));
    settle(&mut harness, "full-spotify-login");
    let sample = |name: &str, artist: &str, album: &str| amplo::spotify::Track {
        name: name.to_owned(),
        artists: vec![artist.to_owned()],
        album: album.to_owned(),
        image: None,
        duration_ms: 0,
    };
    let mut tracks: Vec<_> = harness
        .state()
        .library
        .tracks
        .iter()
        .take(7)
        .map(|track| sample(&track.info.title, &track.info.artist, &track.info.album))
        .collect();
    tracks.insert(2, sample("Midnight City", "M83", "Hurry Up, We're Dreaming"));
    tracks.push(sample("Dreams", "Fleetwood Mac", "Rumours"));
    tracks.push(sample("Holocene", "Bon Iver", "Bon Iver"));
    let playlist = |id: &str, name: &str, tracks: usize, owner: &str| amplo::spotify::Playlist {
        id: id.to_owned(),
        name: name.to_owned(),
        owner: owner.to_owned(),
        tracks,
        image: None,
    };
    let count = tracks.len();
    harness.state_mut().spotify.show_sample(vec![
        (playlist("p1", "Daily Drive", count, "Tester"), tracks),
        (playlist("p2", "Late Night Lofi", 48, "Tester"), Vec::new()),
        (playlist("p3", "Road Trip", 112, "A friend"), Vec::new()),
    ]);
    settle(&mut harness, "full-spotify");
    harness.state_mut().apply(Action::Spotify(SpotifyAction::ShowMissingOnly(true)));
    settle(&mut harness, "full-spotify-missing");
    harness.state_mut().apply(Action::OpenPlaylist(ALL_SONGS));

    harness.state_mut().search = "love".to_owned();
    settle(&mut harness, "full-search");
}

#[test]
fn spotify_page_marks_the_songs_the_folder_has() {
    let folder = TestFolder::new("spotify");
    let mut harness = harness_for(&folder);
    harness.set_size(egui::vec2(1280.0, 780.0));
    harness.state_mut().full = true;
    harness.run_steps(4);

    click(&mut harness, "Spotify");
    assert!(harness.state().spotify_open);
    harness.get_by_label("Log in with Spotify");
    harness.get_by_label("Copy");

    let playlist = |id: &str, name: &str| amplo::spotify::Playlist {
        id: id.to_owned(),
        name: name.to_owned(),
        owner: "Tester".to_owned(),
        tracks: 2,
        image: None,
    };
    let track = |name: &str, artist: &str| amplo::spotify::Track {
        name: name.to_owned(),
        artists: vec![artist.to_owned()],
        album: String::new(),
        image: None,
        duration_ms: 0,
    };
    harness.state_mut().spotify.show_sample(vec![
        // The folder has a playlist named Mix already.
        (playlist("p1", "Mix"), vec![track("c long", "Anyone"), track("Not Here", "Nobody")]),
        (playlist("p2", "Other"), vec![track("a short (Live)", "Someone")]),
    ]);
    harness.run_steps(4);
    let found = |harness: &mut Harness<'static, App>| {
        let app = harness.state_mut();
        let tracks = app.library.tracks.clone();
        let revision = app.library_revision;
        let matches = app.spotify.matches(&tracks, revision).unwrap();
        matches.into_iter().map(|found| found.map(|track| tracks[track].info.title.clone())).collect::<Vec<_>>()
    };
    assert_eq!(found(&mut harness), [Some("c long".to_owned()), None]);
    click(&mut harness, "Import as playlist");
    let imported = fs::read_to_string(folder.0.join("Mix 2.m3u")).unwrap();
    assert_eq!(imported, "#EXTM3U\r\nc long.wav\r\n", "a taken name gets a number, and missing songs stay out");
    assert!(harness.state().notice.as_ref().unwrap().0.contains("“Mix 2”"));
    wait_until(&mut harness, "the rescan", |app| app.library.playlists.iter().any(|playlist| playlist.name == "Mix 2"));
    assert!(harness.state().spotify_open, "the Spotify page stays open");
    click(&mut harness, "Other");
    assert_eq!(harness.state().spotify.open.as_deref(), Some("p2"));
    assert_eq!(found(&mut harness), [Some("a short".to_owned())], "a version of a song still counts");

    harness.state_mut().settings.spotify_token = Some("saved".to_owned());
    click(&mut harness, "Log out");
    assert_eq!(harness.state().settings.spotify_token, None);
    harness.get_by_label("Log in with Spotify");

    click(&mut harness, "Mix");
    assert!(!harness.state().spotify_open, "a playlist replaces the Spotify page");
    harness.get_by_label("1. c long");
}

#[test]
fn full_layout_lists_searches_and_plays() {
    let folder = TestFolder::new("full");
    let mut harness = harness_for(&folder);
    harness.set_size(egui::vec2(1280.0, 780.0));
    harness.state_mut().full = true;
    harness.run_steps(4);

    harness.get_by_label("Play playlist");
    harness.get_by_label("Shuffle play");
    harness.get_by_label("Restore");
    assert!(harness.query_by_label("Maximize").is_none(), "the full layout restores instead");
    click(&mut harness, "Liked Songs");
    assert_eq!(harness.state().view, ALL_SONGS, "an empty Liked Songs does not open");

    harness.state_mut().search = "LONG".to_owned();
    harness.run_steps(2);
    assert_eq!(harness.state().visible_rows(), [2], "search ignores case");
    assert!(harness.query_by_label("1. a short").is_none());
    click(&mut harness, "3. c long");
    assert_eq!(current_title(&harness), "c long", "the row keeps its place in the playlist");

    click(&mut harness, "Now Playing");
    assert!(!harness.state().minimized);
    harness.get_by_label("Like");
    click(&mut harness, "Queue");
    assert_eq!(harness.state().card_tab, CardTab::Queue);
    click(&mut harness, "Back to song list");
    assert!(harness.state().minimized);
    harness.get_by_label("Open Now Playing: c long");

    harness.state_mut().search = "nothing like this".to_owned();
    harness.run_steps(2);
    assert!(harness.state().visible_rows().is_empty());
    harness.state_mut().full = false;
    assert_eq!(harness.state().visible_rows().len(), 4, "the compact layout has no search box");
}

#[test]
fn space_types_into_the_search_box() {
    let folder = TestFolder::new("search-space");
    let mut harness = harness_for(&folder);
    harness.set_size(egui::vec2(1280.0, 780.0));
    harness.state_mut().full = true;
    harness.run_steps(4);
    click(&mut harness, "3. c long");

    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(2);
    press_space(&mut harness, 0);
    assert!(!is_paused(&harness), "Space paused while typing a search");
}

/// Now Playing covers the song list, where the results show.
#[test]
fn typing_a_search_shows_the_song_list() {
    let folder = TestFolder::new("search-list");
    let mut harness = harness_for(&folder);
    harness.set_size(egui::vec2(1280.0, 780.0));
    harness.state_mut().full = true;
    harness.run_steps(4);
    click(&mut harness, "3. c long");
    assert!(!harness.state().minimized, "a clicked song opens Now Playing");

    harness.get_by_role(Role::TextInput).focus();
    harness.run_steps(2);
    harness.get_by_role(Role::TextInput).type_text("short");
    harness.run_steps(2);
    assert_eq!(harness.state().search, "short");
    assert!(harness.state().minimized, "Now Playing still covers the results");
    harness.get_by_label("1. a short");
    assert!(!is_paused(&harness), "the music keeps playing");
}

/// The window has no frame, so these buttons are the only way to minimize or maximize it.
/// Close keeps the music playing and never quits. Tests have no tray icon, so it minimizes.
#[test]
fn title_bar_buttons_command_the_window() {
    let folder = TestFolder::new("window");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    let commands = |harness: &mut Harness<'static, App>, label: &str| {
        harness.get_by_label(label).click();
        let mut sent = Vec::new();
        for _ in 0..4 {
            harness.step();
            sent.extend(harness.output().viewport_output[&egui::ViewportId::ROOT].commands.iter().cloned());
        }
        sent
    };
    assert!(commands(&mut harness, "Minimize").contains(&egui::ViewportCommand::Minimized(true)));
    assert!(commands(&mut harness, "Maximize").contains(&egui::ViewportCommand::Maximized(true)));
    let close = commands(&mut harness, "Close");
    assert!(close.contains(&egui::ViewportCommand::Minimized(true)), "Close sent {close:?}");
    assert!(!close.contains(&egui::ViewportCommand::Close), "Close must not quit");
    assert!(!is_paused(&harness), "the music keeps playing");

    harness.set_size(egui::vec2(1280.0, 780.0));
    harness.state_mut().full = true;
    harness.run_steps(4);
    assert!(commands(&mut harness, "Restore").contains(&egui::ViewportCommand::Maximized(false)));
    let close = commands(&mut harness, "Close");
    assert!(close.contains(&egui::ViewportCommand::Minimized(true)) && !close.contains(&egui::ViewportCommand::Close));
}

#[test]
fn new_playlist_writes_a_file_and_opens_it() {
    let folder = TestFolder::new("new-playlist");
    let mut harness = harness_for(&folder);
    click(&mut harness, "New playlist");
    assert!(harness.state().new_playlist.is_some());
    harness.get_by_role(Role::TextInput).type_text("Road Trip");
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
    assert!(harness.state().new_playlist.is_none(), "Enter saves and closes the form");
    assert_eq!(fs::read_to_string(folder.0.join("Road Trip.m3u")).unwrap(), "#EXTM3U\r\n");

    wait_until(&mut harness, "the new playlist to open", |app| {
        app.library.playlists.get(app.view).is_some_and(|playlist| playlist.name == "Road Trip")
    });
    assert!(harness.state().library.playlists[harness.state().view].tracks.is_empty());
    harness.get_by_label("Road Trip");
    harness.run_steps(4);

    click(&mut harness, "New playlist");
    harness.get_by_role(Role::TextInput).type_text("mix");
    harness.run_steps(2);
    click(&mut harness, "Save");
    let error = harness.state().new_playlist.as_ref().and_then(|form| form.error.clone());
    assert!(error.is_some_and(|error| error.contains("already exists")), "a taken name keeps the form open");
    click(&mut harness, "Cancel");
    assert!(harness.state().new_playlist.is_none());
    assert_eq!(fs::read_to_string(folder.0.join("Mix.m3u")).unwrap(), "#EXTM3U\nc long.wav\na short.wav\n");
}

#[test]
fn selected_songs_go_into_playlists() {
    let folder = TestFolder::new("add-to-playlist");
    let mut harness = harness_for(&folder);
    harness.get_by_label("1. a short").click_modifiers(egui::Modifiers::COMMAND);
    harness.run_steps(4);
    harness.get_by_label("2. b short2").click_modifiers(egui::Modifiers::SHIFT);
    harness.run_steps(4);
    assert_eq!(harness.state().selected, BTreeSet::from([0, 1]));
    assert!(harness.state().player.is_none(), "selecting does not play");

    harness.get_by_label("2. b short2").click_secondary();
    harness.run_steps(4);
    harness.get_by_label("Add 2 songs to playlist");
    let mix = harness.state().library.playlists.iter().position(|playlist| playlist.name == "Mix").unwrap();
    harness.state_mut().apply(Action::AddToPlaylist(mix));
    let text = fs::read_to_string(folder.0.join("Mix.m3u")).unwrap();
    assert_eq!(text, "#EXTM3U\nc long.wav\na short.wav\nb short2.wav\n", "a short was there already");
    let notice = harness.state().notice.clone().unwrap().0;
    assert_eq!(notice, "Added 1 song to Mix. Skipped 1 song already in it.");
    wait_until(&mut harness, "the playlist to grow", |app| app.library.playlists[mix].tracks.len() == 3);

    // The first Escape closes the menu, which the test left open, and the second deselects.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert_eq!(harness.state().selected.len(), 2);
    harness.key_press(egui::Key::Escape);
    harness.run_steps(2);
    assert!(harness.state().selected.is_empty(), "Escape clears the selection");
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
    assert_eq!(harness.state().selected.len(), 4, "Ctrl+A selects every song");

    harness.get_by_label("3. c long").click_secondary();
    harness.run_steps(4);
    harness.get_by_label("Add 4 songs to playlist");
    harness.state_mut().apply(Action::NewPlaylistFromSelection);
    harness.state_mut().new_playlist.as_mut().unwrap().name = "Everything".to_owned();
    harness.state_mut().apply(Action::CreatePlaylist);
    let text = fs::read_to_string(folder.0.join("Everything.m3u")).unwrap();
    assert_eq!(text, "#EXTM3U\r\na short.wav\r\nb short2.wav\r\nc long.wav\r\nd corrupt.mp3\r\n");

    // The test chose from the menu without clicking it, so the menu is still open.
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert_eq!(harness.state().selected.len(), 4, "Escape closed the menu and kept the selection");
    click(&mut harness, "3. c long");
    assert!(harness.state().player.is_some(), "a plain click still plays");
    assert!(harness.state().selected.is_empty());
}

/// A file dragged onto the window from Explorer.
#[derive(Debug)]
struct DroppedPath(PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        fs::read(&self.0).map_err(|err| err.to_string())
    }
}

#[test]
fn lyrics_are_pasted_written_and_dropped() {
    let folder = TestFolder::new("add-lyrics");
    write_silent_wav(&folder.0.join("e long.wav"), 20.0);
    let mut harness = harness_for(&folder);
    click(&mut harness, "5. e long");
    assert!(harness.state().lyrics.is_none());
    harness.get_by_label("Write lyrics");

    click(&mut harness, "Paste lyrics");
    harness.event(egui::Event::Paste("[00:01.00]Hello\n[00:02.00]World".to_owned()));
    harness.run_steps(2);
    let lrc = folder.0.join("e long.lrc");
    assert_eq!(harness.state().error, None);
    assert_eq!(fs::read_to_string(&lrc).unwrap(), "[00:01.00]Hello\r\n[00:02.00]World\r\n");
    assert!(harness.state().lyrics.as_ref().unwrap().synced, "the pasted lyrics show at once");
    assert_eq!(harness.state().notice.clone().unwrap().0, "Saved the lyrics of e long.");

    click(&mut harness, "Edit lyrics");
    assert_eq!(harness.state().lyrics_editor.as_ref().unwrap().text, "[00:01.00]Hello\n[00:02.00]World\n");
    harness.state_mut().lyrics_editor.as_mut().unwrap().text = "Just words\nNo times".to_owned();
    click(&mut harness, "Save");
    assert!(harness.state().lyrics_editor.is_none());
    assert_eq!(fs::read_to_string(&lrc).unwrap(), "Just words\r\nNo times\r\n");
    assert!(!harness.state().lyrics.as_ref().unwrap().synced);

    click(&mut harness, "Edit lyrics");
    harness.state_mut().lyrics_editor.as_mut().unwrap().text = "[ti:Only a header]".to_owned();
    click(&mut harness, "Save");
    let error = harness.state().lyrics_editor.as_ref().unwrap().error.clone();
    assert_eq!(error.as_deref(), Some("These lyrics have no lines."), "the form stays open to fix them");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);
    assert!(harness.state().lyrics_editor.is_none(), "Escape closes the form");
    assert_eq!(fs::read_to_string(&lrc).unwrap(), "Just words\r\nNo times\r\n", "a closed form saves nothing");

    let dropped = folder.0.join("downloaded.txt");
    fs::write(&dropped, "\u{feff}[00:03.00]From a file\n").unwrap();
    harness.input_mut().dropped_files.push(std::sync::Arc::new(DroppedPath(dropped)));
    harness.run_steps(2);
    assert_eq!(fs::read_to_string(&lrc).unwrap(), "[00:03.00]From a file\r\n");
    assert_eq!(harness.state().lyrics.as_ref().unwrap().lines[0].text, "From a file");
}

#[test]
fn romaji_and_translation_switch_under_the_lyrics() {
    let folder = TestFolder::new("lyric-extras");
    // Base64 of "hajimari" and "The beginning".
    let lrc = "[00:00.50]始まり\n[00:10.00]Second line\n[x-romaji:500:aGFqaW1hcmk=]\n[x-translation:500:VGhlIGJlZ2lubmluZw==]\n";
    fs::write(folder.0.join("c long.lrc"), lrc).unwrap();
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    assert_eq!(harness.state().lyric_extra(), Some(Extra::Romaji), "romaji shows first");
    assert_eq!(harness.state().lyrics.as_ref().unwrap().lines[0].romaji.as_deref(), Some("hajimari"));

    let switch = "Change what shows under each line";
    click(&mut harness, switch);
    assert_eq!(harness.state().lyric_extra(), Some(Extra::Translation));
    click(&mut harness, switch);
    assert_eq!(harness.state().lyric_extra(), None);
    click(&mut harness, switch);
    assert_eq!(harness.state().lyric_extra(), Some(Extra::Romaji));
    assert!(harness.state().settings.serialize().contains("lyrics_extra=romaji"));

    fs::write(folder.0.join("c long.lrc"), "[00:00.50]Plain\n").unwrap();
    harness.state_mut().lyrics = amplo::lyrics::Lyrics::for_track(&folder.0.join("c long.wav"));
    harness.run_steps(2);
    assert!(harness.query_by_label(switch).is_none(), "lyrics without extras have no switch");
}

#[test]
fn songs_come_out_of_playlists() {
    let folder = TestFolder::new("remove-from-playlist");
    let mut harness = harness_for(&folder);
    harness.get_by_label("1. a short").click_secondary();
    harness.run_steps(4);
    assert!(harness.query_by_label("Remove from All Songs").is_none(), "All Songs shows the folder");
    harness.key_press(egui::Key::Escape);
    harness.run_steps(4);

    let mix = harness.state().library.playlists.iter().position(|playlist| playlist.name == "Mix").unwrap();
    harness.state_mut().apply(Action::OpenPlaylist(mix));
    harness.run_steps(4);
    harness.get_by_label("1. c long").click_secondary();
    harness.run_steps(4);
    click(&mut harness, "Remove from Mix");
    let text = fs::read_to_string(folder.0.join("Mix.m3u")).unwrap();
    assert_eq!(text, "#EXTM3U\na short.wav\n");
    assert_eq!(harness.state().notice.clone().unwrap().0, "Removed 1 song from Mix.");
    wait_until(&mut harness, "the playlist to shrink", |app| app.library.playlists[mix].tracks.len() == 1);
    assert!(folder.0.join("c long.wav").exists(), "the song file stays");

    let liked: BTreeSet<PathBuf> = harness.state().library.tracks.iter().take(2).map(|track| track.path.clone()).collect();
    harness.state_mut().settings.liked = liked.clone();
    harness.state_mut().library.refresh_liked(&liked);
    harness.state_mut().apply(Action::OpenPlaylist(amplo::library::LIKED_SONGS));
    harness.run_steps(4);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
    harness.get_by_label("1. a short").click_secondary();
    harness.run_steps(4);
    click(&mut harness, "Remove 2 songs from Liked Songs");
    assert!(harness.state().settings.liked.is_empty(), "removing from Liked Songs unlikes");
    assert_eq!(harness.state().view, ALL_SONGS, "an empty Liked Songs closes");
}

#[test]
fn an_empty_start_offers_to_open_a_folder() {
    let mut harness = Harness::builder()
        .with_size(WINDOW_SIZE)
        .allow_missing_glyphs()
        .build_eframe(|cc| App::new(cc.egui_ctx.clone(), silent_settings(), None, Vec::new()));
    harness.run_steps(3);
    harness.get_by_label("Open folder");
    harness.get_by_label("Open music folder");
}

#[cfg(windows)]
#[test]
fn media_keys_play_pause_and_skip() {
    use crate::media_controls::MediaButton;

    let folder = TestFolder::new("media-keys");
    let mut harness = harness_for(&folder);

    harness.state_mut().press(MediaButton::Play);
    harness.run_steps(2);
    assert_eq!(current_title(&harness), "a short", "Play with nothing loaded starts the shown playlist");
    harness.state_mut().apply(Action::PlayRow(2));
    harness.run_steps(2);
    assert_eq!(current_title(&harness), "c long");

    harness.state_mut().press(MediaButton::Pause);
    assert!(is_paused(&harness));
    harness.state_mut().press(MediaButton::Pause);
    assert!(is_paused(&harness), "a second Pause keeps it paused");
    harness.state_mut().press(MediaButton::Play);
    assert!(!is_paused(&harness));
    harness.state_mut().press(MediaButton::Play);
    assert!(!is_paused(&harness), "a second Play keeps it playing");

    harness.state_mut().press(MediaButton::Next);
    assert_eq!(current_title(&harness), "d corrupt");
    harness.state_mut().press(MediaButton::Previous);
    assert_eq!(current_title(&harness), "c long");
}

fn space_event(harness: &mut Harness<'static, App>, pressed: bool, repeat: bool) {
    harness.input_mut().events.push(egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed,
        repeat,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(2);
}

/// Presses Space, with `repeats` extra presses from holding it down, then lets go.
fn press_space(harness: &mut Harness<'static, App>, repeats: usize) {
    space_event(harness, true, false);
    for _ in 0..repeats {
        space_event(harness, true, true);
    }
    space_event(harness, false, false);
}

#[test]
fn space_pauses_and_resumes() {
    let folder = TestFolder::new("space");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");

    press_space(&mut harness, 0);
    assert!(is_paused(&harness));
    press_space(&mut harness, 3);
    assert!(!is_paused(&harness), "holding the key toggles once");

    // Space would click a focused button too. Only one toggle may happen.
    harness.get_by_label("Pause").focus();
    harness.run_steps(2);
    press_space(&mut harness, 0);
    assert!(is_paused(&harness), "Space toggled twice");
}

/// A maximized window must leave the taskbar visible. Checks the size limit that Windows asks
/// for when it maximizes, on a hidden window that is never shown.
#[cfg(windows)]
#[test]
fn maximizing_stops_at_the_taskbar() {
    use windows::{
        Win32::{
            Foundation::{LPARAM, RECT, WPARAM},
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, MINMAXINFO, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                SendMessageW, SystemParametersInfoW, WINDOW_EX_STYLE, WM_GETMINMAXINFO, WS_POPUP,
            },
        },
        core::w,
    };

    // SAFETY: The window is hidden, belongs to this thread, and is destroyed below. The
    // pointers passed to Windows point at live values of the types each call expects.
    unsafe {
        let window =
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("amplo test"), WS_POPUP, 0, 0, 100, 100, None, None, None, None)
                .unwrap();
        crate::keep_taskbar_visible(window);
        let mut limits = MINMAXINFO::default();
        SendMessageW(window, WM_GETMINMAXINFO, Some(WPARAM(0)), Some(LPARAM(&raw mut limits as isize)));
        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, Some((&raw mut work).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
            .unwrap();
        DestroyWindow(window).unwrap();

        assert_eq!(limits.ptMaxSize.x, work.right - work.left);
        assert_eq!(limits.ptMaxSize.y, work.bottom - work.top);
    }
}
