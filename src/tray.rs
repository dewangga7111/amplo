//! The notification area icon, which stands in for the window while it is hidden.

use std::sync::atomic::{AtomicU8, Ordering};

use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                DefSubclassProc, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, DestroyMenu, EnumWindows, GetCursorPos,
                GetPropW, GetSystemMetrics, GetWindowThreadProcessId, HICON, IDI_APPLICATION, IMAGE_ICON,
                LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MF_SEPARATOR, MF_STRING, PostMessageW, RemovePropW,
                SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, SetPropW, TPM_BOTTOMALIGN, TPM_RETURNCMD,
                TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
            },
        },
    },
    core::{BOOL, PCWSTR, w},
};

/// The message Windows sends to the window when the icon is clicked.
const WM_TRAY: u32 = WM_APP + 1;
/// Sent by a second start of the app, which then exits, to bring this window back instead.
const WM_SHOW_INSTANCE: u32 = WM_APP + 2;
/// Marks the window of a running app, so a second start can find it while it is hidden.
const INSTANCE_PROPERTY: PCWSTR = w!("amplo.instance");
/// The resource number of the icon in `build.rs`.
const ICON_RESOURCE: usize = 1;
const SUBCLASS_ID: usize = 2;
const ICON_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

const NO_COMMAND: u8 = 0;
const SHOW: u8 = 1;
const QUIT: u8 = 2;
/// Set on the window's thread by the icon, and read by the app on its next update.
static COMMAND: AtomicU8 = AtomicU8::new(NO_COMMAND);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Brings the window back.
    Show,
    Quit,
}

pub struct Tray {
    window: HWND,
    icon: HICON,
    shown: bool,
}

unsafe extern "system" fn on_message(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match message {
        WM_TRAY => {}
        WM_SHOW_INSTANCE => {
            COMMAND.store(SHOW, Ordering::Relaxed);
            return LRESULT(0);
        }
        _ => {
            if message == WM_DESTROY {
                // Removes the icon on every way out, so none stays behind in the notification area.
                let data = NOTIFYICONDATAW {
                    cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                    hWnd: window,
                    uID: ICON_ID,
                    ..Default::default()
                };
                // SAFETY: The data names this window's icon, and outlives the call.
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
            }
            // SAFETY: Forwards the message as it came, to the next handler of this window.
            return unsafe { DefSubclassProc(window, message, wparam, lparam) };
        }
    }
    // Without a version set, the low bits of `lparam` hold the mouse message.
    match lparam.0 as u32 & 0xFFFF {
        WM_LBUTTONUP => COMMAND.store(SHOW, Ordering::Relaxed),
        WM_RBUTTONUP => match show_menu(window) {
            MENU_OPEN => COMMAND.store(SHOW, Ordering::Relaxed),
            MENU_QUIT => COMMAND.store(QUIT, Ordering::Relaxed),
            _ => {}
        },
        _ => {}
    }
    LRESULT(0)
}

/// Returns the chosen item, or 0 when the menu was dismissed.
fn show_menu(window: HWND) -> usize {
    // SAFETY: The menu is created, used and destroyed here, on the window's own thread.
    // Windows closes a tray menu on an outside click only when its window is in the
    // foreground, and the message after it is the documented workaround for the same problem.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let mut cursor = POINT::default();
        let chosen = if AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open amplo")).is_ok()
            && AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()).is_ok()
            && AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit")).is_ok()
            && GetCursorPos(&mut cursor).is_ok()
        {
            let _ = SetForegroundWindow(window);
            let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
            let chosen = TrackPopupMenu(menu, flags, cursor.x, cursor.y, None, window, None);
            let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
            chosen.0 as usize
        } else {
            0
        };
        let _ = DestroyMenu(menu);
        chosen
    }
}

/// Brings back the window of an app that already runs, hidden or not. Returns false when
/// there is none, and this start should open its own window.
pub fn show_running_instance() -> bool {
    unsafe extern "system" fn find(window: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` points to the `Option<HWND>` in the caller, which outlives the
        // enumeration.
        unsafe {
            if GetPropW(window, INSTANCE_PROPERTY).is_invalid() {
                return true.into();
            }
            *(found.0 as *mut Option<HWND>) = Some(window);
        }
        false.into()
    }

    let mut found: Option<HWND> = None;
    // SAFETY: The callback only writes to `found`, through the pointer passed here. Stopping
    // early makes `EnumWindows` return an error, so `found` is what tells the result.
    let _ = unsafe { EnumWindows(Some(find), LPARAM((&raw mut found) as isize)) };
    let Some(window) = found else { return false };
    let mut process = 0;
    // SAFETY: The window was just found, and a window that closed since fails these calls
    // harmlessly. This start was just opened by the user, so it may give the other process
    // the right to come to the front.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut process));
        let _ = AllowSetForegroundWindow(process);
        PostMessageW(Some(window), WM_SHOW_INSTANCE, WPARAM(0), LPARAM(0)).is_ok()
    }
}

impl Tray {
    /// Listens for clicks on the icon, and marks the window so a second start of the app finds
    /// it. See [`show_running_instance`]. The icon shows with [`Tray::show_icon`].
    pub fn new(window: HWND) -> Option<Self> {
        // SAFETY: The resource number stands in for a name, as `LoadImageW` documents. The
        // window belongs to this thread, as subclassing requires, and the procedure stays valid
        // for the life of the process.
        unsafe {
            let module = GetModuleHandleW(None).ok()?;
            let (width, height) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            let name = PCWSTR(ICON_RESOURCE as *const u16);
            // A build without the icon resource falls back to the generic program icon.
            let icon = match LoadImageW(Some(module.into()), name, IMAGE_ICON, width, height, LR_DEFAULTCOLOR) {
                Ok(icon) => HICON(icon.0),
                Err(_) => LoadIconW(None, IDI_APPLICATION).ok()?,
            };
            SetWindowSubclass(window, Some(on_message), SUBCLASS_ID, 0).ok().ok()?;
            let _ = SetPropW(window, INSTANCE_PROPERTY, Some(HANDLE(1 as *mut _)));
            Some(Self { window, icon, shown: false })
        }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: ICON_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        for (target, unit) in data.szTip.iter_mut().zip("amplo".encode_utf16()) {
            *target = unit;
        }
        data
    }

    /// Returns false when Windows refused, so the caller can keep the window reachable.
    /// Showing it again also restores an icon that was lost.
    pub fn show_icon(&mut self) -> bool {
        if !self.shown {
            // SAFETY: The data is fully set up and outlives the call.
            self.shown = unsafe { Shell_NotifyIconW(NIM_ADD, &self.data()) }.as_bool();
        }
        self.shown
    }

    pub fn hide_icon(&mut self) {
        if self.shown {
            // SAFETY: The data names the icon added in `show_icon`.
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data()) };
            self.shown = false;
        }
    }

    /// The last click on the icon since this was called.
    pub fn take_command(&self) -> Option<TrayCommand> {
        match COMMAND.swap(NO_COMMAND, Ordering::Relaxed) {
            SHOW => Some(TrayCommand::Show),
            QUIT => Some(TrayCommand::Quit),
            _ => None,
        }
    }
}

impl Drop for Tray {
    /// An icon left behind stays in the notification area until the mouse moves over it.
    fn drop(&mut self) {
        self.hide_icon();
        // SAFETY: Removes the procedure and the mark set in `new`, from the same window.
        unsafe {
            let _ = RemoveWindowSubclass(self.window, Some(on_message), SUBCLASS_ID);
            let _ = RemovePropW(self.window, INSTANCE_PROPERTY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    /// Uses a message-only window, which other starts of the app cannot find, and never shows
    /// the icon, so nothing appears on the screen or in the notification area.
    #[test]
    fn clicks_and_second_starts_ask_to_show_the_window() {
        // SAFETY: The window is created, used and destroyed on this thread.
        unsafe {
            let window = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("tray test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap();
            let tray = Tray::new(window).unwrap();
            assert_eq!(tray.take_command(), None);
            SendMessageW(window, WM_TRAY, Some(WPARAM(1)), Some(LPARAM(WM_LBUTTONUP as isize)));
            assert_eq!(tray.take_command(), Some(TrayCommand::Show));
            assert_eq!(tray.take_command(), None, "a click is taken once");
            SendMessageW(window, WM_SHOW_INSTANCE, None, None);
            assert_eq!(tray.take_command(), Some(TrayCommand::Show), "a second start shows the window");
            drop(tray);
            DestroyWindow(window).unwrap();
        }
    }
}
