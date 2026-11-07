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
use amplo::{lyrics::Lyrics, settings::Settings};

use crate::{
    app::{Action, App, CardTab},
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
    Settings { folder: None, volume: 0.0, liked: BTreeSet::new() }
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
    assert!(harness.state().minimized, "a row click stays in the list");
    assert!(harness.state().player.is_some());
    assert_eq!(harness.state().error, None);
    harness.get_by_label("Expand");

    click(&mut harness, "Expand");
    assert!(!harness.state().minimized);
    assert_eq!(harness.state().slide, 1.0);
    assert!(harness.query_by_label("Expand").is_none(), "the mini player leaves");
    assert!(harness.query_by_label("3. c long").is_none(), "the list is covered");
    assert_eq!(harness.state().tech, ["706 KBPS", "44.1 KHZ", "MONO"]);

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
    click(&mut harness, "Expand");

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
    click(&mut harness, "Expand");

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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    assert!(harness.state().minimized, "like a row click, it stays in the list");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    }
    assert!(orders.len() > 1, "the order never changed");
}

#[test]
fn turning_shuffle_off_restores_the_playlist_order() {
    let folder = TestFolder::new("shuffle-off");
    let mut harness = harness_for(&folder);
    click(&mut harness, "3. c long");
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");
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
    click(&mut harness, "Expand");

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

    let with_lyrics = |app: &App, wanted: bool| {
        app.library.tracks.iter().position(|track| track.path.with_extension("lrc").exists() == wanted)
    };
    if let Some(row) = with_lyrics(harness.state(), true) {
        harness.state_mut().apply(Action::PlayRow(row));
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
    if let Some(row) = with_lyrics(harness.state(), false) {
        harness.state_mut().apply(Action::PlayRow(row));
        settle(&mut harness, "now-playing-no-lyrics");
    }
    let playlists = harness.state().library.playlists.len();
    harness.state_mut().apply(Action::OpenPlaylist(playlists - 1));
    settle(&mut harness, "playlist");
}

/// The window has no frame, so these buttons are the only way to close or minimize it.
#[test]
fn title_bar_buttons_command_the_window() {
    let folder = TestFolder::new("window");
    let mut harness = harness_for(&folder);
    let mut pressed = |label: &str, wanted: egui::ViewportCommand| {
        harness.get_by_label(label).click();
        let mut sent = false;
        for _ in 0..4 {
            harness.step();
            let output = &harness.output().viewport_output[&egui::ViewportId::ROOT];
            sent |= output.commands.contains(&wanted);
        }
        assert!(sent, "{label} did not send {wanted:?}");
    };
    pressed("Minimize", egui::ViewportCommand::Minimized(true));
    pressed("Close", egui::ViewportCommand::Close);
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
