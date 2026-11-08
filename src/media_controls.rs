//! The Windows media controls: play, pause, next, and previous keys on keyboards and macro
//! pads, and the media panel that Windows shows beside the volume.

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

use windows::{
    Foundation::TypedEventHandler,
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls, SystemMediaTransportControlsButton,
        SystemMediaTransportControlsButtonPressedEventArgs,
    },
    Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference},
    Win32::{Foundation::HWND, System::WinRT::ISystemMediaTransportControlsInterop},
    core::{HSTRING, Result, factory},
};

use crate::covers::cover_bytes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaButton {
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Playing,
    Paused,
}

/// The song that Windows shows in its media panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
}

pub struct MediaControls {
    controls: SystemMediaTransportControls,
    token: i64,
    presses: Receiver<MediaButton>,
    status: Option<Status>,
    /// `Some(None)` once Windows has been told that no song is loaded.
    shown: Option<Option<NowPlaying>>,
}

impl MediaControls {
    /// Connects to the controls of `window`. Windows sends a key press to one app only, and
    /// picks the app that played last. `on_press` runs on a Windows thread after each press.
    pub fn new(window: HWND, on_press: impl Fn() + Send + 'static) -> Result<Self> {
        let interop = factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        // SAFETY: `window` is a live top-level window of this process, and the type of the
        // result matches the interface that it asks for.
        let controls: SystemMediaTransportControls = unsafe { interop.GetForWindow(window)? };
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        controls.SetIsEnabled(true)?;

        let (sender, presses) = mpsc::channel();
        let handler = TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            let button = match args.ok()?.Button()? {
                SystemMediaTransportControlsButton::Play => MediaButton::Play,
                SystemMediaTransportControlsButton::Pause => MediaButton::Pause,
                SystemMediaTransportControlsButton::Next => MediaButton::Next,
                SystemMediaTransportControlsButton::Previous => MediaButton::Previous,
                _ => return Ok(()),
            };
            if sender.send(button).is_ok() {
                on_press();
            }
            Ok(())
        });
        let token = controls.ButtonPressed(&handler)?;
        Ok(Self { controls, token, presses, status: None, shown: None })
    }

    /// The buttons pressed since the last call, oldest first.
    pub fn presses(&self) -> Vec<MediaButton> {
        self.presses.try_iter().collect()
    }

    /// Tells Windows what plays. Only changes reach Windows, so this can run every frame.
    pub fn update(&mut self, now: Option<NowPlaying>, status: Status) {
        if self.status != Some(status) {
            self.status = Some(status);
            let status = match status {
                Status::Stopped => MediaPlaybackStatus::Stopped,
                Status::Playing => MediaPlaybackStatus::Playing,
                Status::Paused => MediaPlaybackStatus::Paused,
            };
            // The panel is a convenience, so a failure leaves the player as it is.
            let _ = self.controls.SetPlaybackStatus(status);
        }
        if self.shown.as_ref() != Some(&now) {
            // Stored first, so a song that fails to show is not tried again every frame.
            self.shown = Some(now.clone());
            let _ = self.show(now.as_ref());
        }
    }

    fn show(&self, now: Option<&NowPlaying>) -> Result<()> {
        let updater = self.controls.DisplayUpdater()?;
        updater.ClearAll()?;
        if let Some(now) = now {
            updater.SetType(MediaPlaybackType::Music)?;
            let music = updater.MusicProperties()?;
            music.SetTitle(&HSTRING::from(now.title.as_str()))?;
            music.SetArtist(&HSTRING::from(now.artist.as_str()))?;
            if let Some(thumbnail) = cover_bytes(&now.path).and_then(|bytes| thumbnail(&bytes).ok()) {
                updater.SetThumbnail(&thumbnail)?;
            }
        }
        updater.Update()
    }
}

impl Drop for MediaControls {
    fn drop(&mut self) {
        let _ = self.controls.RemoveButtonPressed(self.token);
    }
}

/// Windows reads the cover from a stream, and decodes the image itself.
fn thumbnail(bytes: &[u8]) -> Result<RandomAccessStreamReference> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    // Detached, so dropping the writer leaves the stream open.
    writer.DetachStream()?;
    stream.Seek(0)?;
    RandomAccessStreamReference::CreateFromStream(&stream)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use windows::{
        Media::Control::GlobalSystemMediaTransportControlsSessionManager,
        Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE},
        core::w,
    };

    use super::*;

    fn wait_for_press(controls: &MediaControls) -> Option<MediaButton> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(&button) = controls.presses().first() {
                return Some(button);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// The app calls this on its window thread, which is single-threaded COM. Waiting there
    /// for an async call could deadlock, so this checks that it finishes.
    #[test]
    fn thumbnails_are_made_on_a_single_threaded_com_thread() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

        let png = include_bytes!("../assets/icons/amplo.png");
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            // SAFETY: Each successful initialization is paired with the uninitialize below.
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let made = thumbnail(png).is_ok();
            if initialized {
                // SAFETY: Paired with the successful initialization above, on the same thread.
                unsafe { CoUninitialize() };
            }
            let _ = sender.send(made);
        });
        let made = result.recv_timeout(Duration::from_secs(5)).expect("making the thumbnail hung");
        assert!(made, "making the thumbnail failed");
    }

    /// Talks to the real Windows media service, the way the volume panel does. It uses a
    /// hidden window and plays no sound, but it shows in the media panel while it runs.
    #[test]
    #[ignore = "registers with the Windows media panel"]
    fn windows_delivers_media_buttons() -> Result<()> {
        // SAFETY: The window is hidden, belongs to this thread, and is destroyed below.
        let window = unsafe {
            CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("amplo test"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, None, None)?
        };
        let mut controls = MediaControls::new(window, || {})?;
        let title = format!("amplo media test {}", std::process::id());
        let now = NowPlaying { path: PathBuf::from("none"), title: title.clone(), artist: "amplo".to_owned() };
        controls.update(Some(now), Status::Playing);

        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let session = loop {
            let found = manager.GetSessions()?.into_iter().find(|session| {
                session
                    .TryGetMediaPropertiesAsync()
                    .and_then(|properties| properties.join())
                    .and_then(|properties| properties.Title())
                    .is_ok_and(|shown| shown == title.as_str())
            });
            if let Some(session) = found {
                break session;
            }
            assert!(Instant::now() < deadline, "Windows never listed the test session");
            std::thread::sleep(Duration::from_millis(100));
        };

        session.TryTogglePlayPauseAsync()?.join()?;
        assert_eq!(wait_for_press(&controls), Some(MediaButton::Pause), "playing, so the key pauses");
        session.TrySkipNextAsync()?.join()?;
        assert_eq!(wait_for_press(&controls), Some(MediaButton::Next));

        drop(controls);
        // SAFETY: The window was created above on this thread.
        unsafe { DestroyWindow(window)? };
        Ok(())
    }
}
