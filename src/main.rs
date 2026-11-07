// Release builds run without a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod covers;
#[cfg(test)]
mod tests;
mod theme;
mod ui;
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

fn main() -> eframe::Result {
    let startup_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
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
            #[cfg(windows)]
            if let Some(window) = native_window(cc) {
                round_corners(window);
                set_taskbar_icon(window);
            }
            let app = App::new(cc.egui_ctx.clone(), Settings::load(), Settings::default_path(), startup_paths);
            Ok(Box::new(app))
        }),
    )
}
