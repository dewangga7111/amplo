//! Notices when songs or playlists change in the library folders.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};

use crate::{audio::is_audio, library::is_playlist};

/// Copying a folder or downloading a file gives many events. The rescan waits for this
/// much quiet, so it runs once and sees finished files.
const QUIET_TIME: Duration = Duration::from_millis(1500);

fn is_library_file(path: &Path) -> bool {
    is_audio(path) || is_playlist(path)
}

/// Decides if an event can change the library. Windows also reports folders as modified
/// when their contents change, so plain modifications only count for library files.
fn affects_library(event: &Event) -> bool {
    match event.kind {
        // The path is gone by now, so it is unknown if this was a song or a whole folder.
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => true,
        EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => {
            event.paths.iter().any(|path| is_library_file(path) || path.is_dir())
        }
        EventKind::Modify(_) => event.paths.iter().any(|path| is_library_file(path)),
        _ => false,
    }
}

/// Watches folders until dropped. Files in `roots` are skipped, because only a folder can
/// gain new songs.
pub struct FolderWatcher {
    _watcher: RecommendedWatcher,
}

impl FolderWatcher {
    pub fn new(roots: &[PathBuf], on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let (changes, changed) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            if event.is_ok_and(|event| affects_library(&event)) {
                let _ = changes.send(());
            }
        })?;
        for root in roots.iter().filter(|root| root.is_dir()) {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }

        // This thread ends when the watcher is dropped and takes the sender with it.
        thread::spawn(move || {
            while changed.recv().is_ok() {
                loop {
                    match changed.recv_timeout(QUIET_TIME) {
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                on_change();
            }
        });
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, RemoveKind};
    use std::{fs, sync::mpsc::channel};

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn only_library_changes_count() {
        let create = EventKind::Create(CreateKind::File);
        let write = EventKind::Modify(ModifyKind::Data(DataChange::Any));
        assert!(affects_library(&event(create, "x/new song.flac")));
        assert!(affects_library(&event(create, "x/Mix.m3u")));
        assert!(affects_library(&event(write, "x/song.mp3")));
        assert!(affects_library(&event(EventKind::Remove(RemoveKind::Any), "x/gone")));

        assert!(!affects_library(&event(create, "x/cover.jpg")));
        assert!(!affects_library(&event(write, "x/song.lrc")));
        assert!(!affects_library(&event(write, "x")), "a folder's own modify event");
        assert!(!affects_library(&event(EventKind::Access(notify::event::AccessKind::Any), "x/song.mp3")));
    }

    #[test]
    fn a_burst_of_new_files_reports_one_change() {
        let root = std::env::temp_dir().join(format!("amplo-watch-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let (sender, receiver) = channel();
        let watcher = FolderWatcher::new(std::slice::from_ref(&root), move || sender.send(()).unwrap()).unwrap();

        for number in 0..5 {
            fs::File::create(root.join(format!("{number}.mp3"))).unwrap();
        }
        let first = receiver.recv_timeout(Duration::from_secs(10));
        let second = receiver.recv_timeout(QUIET_TIME * 2);
        drop(watcher);
        fs::remove_dir_all(&root).unwrap();

        assert!(first.is_ok(), "the change was not reported");
        assert!(second.is_err(), "one burst must report one change");
    }
}
