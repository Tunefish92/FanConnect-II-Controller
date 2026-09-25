//! gpu-fanctl-gui: live data and settings for the FanConnect II external fans.
//! The running service does the controlling; this window shows its data and edits its settings.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod about;
mod app;
mod elevate;
mod gpu;
mod live;
mod screenshots;
mod theme;
mod widgets;

use std::sync::Arc;

use eframe::egui;
use gpu_fanctl::icon_art;

/// Size of the window and taskbar icon.
const ICON_SIZE: u32 = 256;

/// Where a crash report is written. The app has no console on Windows, so without this a panic
/// would vanish without a trace.
fn crash_log() -> std::path::PathBuf {
    std::env::temp_dir().join("gpu-fanctl-gui-crash.log")
}

fn main() -> eframe::Result {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(crash_log(), format!("{info}\n\n{backtrace}"));
        default_hook(info);
    }));

    // `--screenshots <folder>`: capture every page for the README, then close.
    let args: Vec<String> = std::env::args().collect();
    let screenshots = args.iter().position(|a| a == "--screenshots").map(|i| {
        std::path::PathBuf::from(args.get(i + 1).map_or("docs/screenshots", String::as_str))
    });
    let size = if screenshots.is_some() { [1240.0, 1080.0] } else { [1060.0, 820.0] };
    let icon = egui::IconData { rgba: icon_art::render(ICON_SIZE), width: ICON_SIZE, height: ICON_SIZE };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FanConnect II Controller – ROG Strix RTX 2080 Ti")
            .with_icon(Arc::new(icon))
            // Matches StartupWMClass in the Linux .desktop entry, so the desktop shows the app icon.
            .with_app_id("gpu-fanctl-gui")
            .with_inner_size(size)
            .with_min_inner_size([820.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("gpu-fanctl-gui", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, screenshots)))))
}
