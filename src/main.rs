// Release builds run without a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod covers;
#[cfg(windows)]
mod media_controls;
#[cfg(test)]
mod tests;
mod theme;
#[cfg(windows)]
mod tray;
mod ui;
mod ui_full;
mod widgets;

use std::path::PathBuf;

use app::{APP_NAME, App};
use eframe::egui;
use amplo::settings::Settings;

impl eframe::App for App {
    /// Runs while the window is minimized too, so playback continues to the next track.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_playback(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

/// The icon of the window itself. `build.rs` embeds the same logo in the executable.
fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/icons/amplo.png"))
        .expect("the embedded icon is a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData { rgba: image.into_raw(), width, height }
}

#[cfg(windows)]
fn native_window(cc: &eframe::CreationContext<'_>) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(window) => Some(windows::Win32::Foundation::HWND(window.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// A window without a frame has square corners unless it asks Windows for round ones.
#[cfg(windows)]
fn round_corners(window: windows::Win32::Foundation::HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};

    let preference = DWMWCP_ROUND;
    // SAFETY: The handle belongs to a live window, and the pointer and size describe
    // `preference`, which outlives the call. Older Windows versions return an error, and the
    // window then stays square.
    let _ = unsafe {
        DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const preference).cast(),
            size_of_val(&preference) as u32,
        )
    };
}

/// The taskbar shows a window's big icon, and the window builder only sets the small one.
#[cfg(windows)]
fn set_taskbar_icon(window: windows::Win32::Foundation::HWND) {
    use windows::{
        Win32::{
            Foundation::{LPARAM, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                GetSystemMetrics, ICON_BIG, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CYICON,
                SendMessageW, WM_SETICON,
            },
        },
        core::PCWSTR,
    };

    /// The resource number of the icon in `build.rs`.
    const ICON_RESOURCE: usize = 1;

    // SAFETY: The handles come from the calls before them, and the resource number stands in
    // for a name, as `LoadImageW` documents. A build without the icon returns an error, and the
    // window then keeps its default. The icon stays loaded for the life of the process.
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let (width, height) = (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
        let name = PCWSTR(ICON_RESOURCE as *const u16);
        let Ok(icon) = LoadImageW(Some(module.into()), name, IMAGE_ICON, width, height, LR_DEFAULTCOLOR) else {
            return;
        };
        SendMessageW(
            window,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(icon.0 as isize)),
        );
    }
}

/// Windows maximizes a window without a title bar over the whole monitor, as if it were a
/// fullscreen game, which hides the taskbar. This limits it to the work area instead.
#[cfg(windows)]
fn keep_taskbar_visible(window: windows::Win32::Foundation::HWND) {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
        UI::{
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{MINMAXINFO, WM_GETMINMAXINFO},
        },
    };

    unsafe extern "system" fn limit_maximized(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // SAFETY: Forwards the message as it came, to the next handler of this window.
        let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
        if message == WM_GETMINMAXINFO && lparam.0 != 0 {
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            // SAFETY: `info` is a valid MONITORINFO whose size field is set, as the call needs.
            let found = unsafe { GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut info) };
            if found.as_bool() {
                // SAFETY: For WM_GETMINMAXINFO, Windows passes a valid MINMAXINFO in `lparam`.
                let limits = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                let (work, monitor) = (info.rcWork, info.rcMonitor);
                limits.ptMaxPosition = POINT { x: work.left - monitor.left, y: work.top - monitor.top };
                limits.ptMaxSize = POINT { x: work.right - work.left, y: work.bottom - work.top };
            }
        }
        result
    }

    // SAFETY: The window belongs to this thread, as subclassing requires, and the procedure
    // stays valid for the life of the process.
    let _ = unsafe { SetWindowSubclass(window, Some(limit_maximized), 1, 0) };
}

fn main() -> eframe::Result {
    let startup_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // Starting the app again brings back the one that runs, often hidden in the notification
    // area. Opening files with it still starts a new one, which plays them.
    #[cfg(windows)]
    if startup_paths.is_empty() && tray::show_running_instance() {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size(ui::WINDOW_SIZE)
            .with_resizable(false)
            .with_maximize_button(false)
            .with_decorations(false)
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            #[cfg_attr(not(windows), expect(unused_mut))]
            let mut app = App::new(cc.egui_ctx.clone(), Settings::load(), Settings::default_path(), startup_paths);
            #[cfg(windows)]
            if let Some(window) = native_window(cc) {
                round_corners(window);
                set_taskbar_icon(window);
                keep_taskbar_visible(window);
                app.attach_media_controls(window);
                app.attach_tray(window);
            }
            Ok(Box::new(app))
        }),
    )
}
