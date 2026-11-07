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

/// The logo from the title bar: dark bars on a rounded accent square.
fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    const RADIUS: f32 = 18.0;
    // Center and half height of each bar, in the design's 24 unit icon grid.
    const BARS: [(f32, f32); 5] = [(5.0, 2.0), (9.0, 6.0), (13.0, 3.0), (17.0, 8.0), (21.0, 2.0)];

    let scale = SIZE as f32 / 24.0;
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let corner_x = (RADIUS - px).max(px - (SIZE as f32 - RADIUS)).max(0.0);
            let corner_y = (RADIUS - py).max(py - (SIZE as f32 - RADIUS)).max(0.0);
            let alpha = ((RADIUS - corner_x.hypot(corner_y) + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            let on_bar = BARS.iter().any(|&(center, half)| {
                (px / scale - center).abs() <= 1.4 && (py / scale - 12.0).abs() <= half + 1.4
            });
            let color = if on_bar { theme::ON_ACCENT } else { theme::ACCENT };
            rgba.extend_from_slice(&[color.r(), color.g(), color.b(), alpha]);
        }
    }
    egui::IconData { rgba, width: SIZE as u32, height: SIZE as u32 }
}

/// A window without a frame has square corners unless it asks Windows for round ones.
#[cfg(windows)]
fn round_corners(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
    };

    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::Win32(window) = handle.as_raw() else { return };
    let preference = DWMWCP_ROUND;
    // SAFETY: The handle belongs to the live window of `cc`, and the pointer and size describe
    // `preference`, which outlives the call. Older Windows versions return an error, and the
    // window then stays square.
    let _ = unsafe {
        DwmSetWindowAttribute(
            HWND(window.hwnd.get() as *mut _),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const preference).cast(),
            size_of_val(&preference) as u32,
        )
    };
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
            round_corners(cc);
            let app = App::new(cc.egui_ctx.clone(), Settings::load(), Settings::default_path(), startup_paths);
            Ok(Box::new(app))
        }),
    )
}
