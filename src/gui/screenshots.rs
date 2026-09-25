//! Screenshot mode for the README: `gpu-fanctl-gui --screenshots <folder>` shows every page in
//! turn, saves each as a PNG, then closes. Run it again after UI changes to refresh the images.
//! It only changes what is shown, never the settings or the saved theme.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, Theme};

use crate::app::Page;

/// Live data collected before the first screenshot, so the history chart has something to show.
const WARMUP: Duration = Duration::from_secs(60);
/// Time and frames to let a page settle (layout, fonts, plots) before capturing it.
const SETTLE: Duration = Duration::from_millis(800);
const SETTLE_FRAMES: u32 = 10;

pub struct Shot {
    pub file: &'static str,
    pub page: Page,
    pub theme: Theme,
    /// Scroll the page to its end, for pages taller than the window.
    pub scroll_to_end: bool,
}

const fn shot(file: &'static str, page: Page, theme: Theme) -> Shot {
    Shot { file, page, theme, scroll_to_end: false }
}

pub const SHOTS: &[Shot] = &[
    shot("overview-dark.png", Page::Overview, Theme::Dark),
    shot("fan-curve-dark.png", Page::Curve, Theme::Dark),
    shot("gpu-dark.png", Page::Gpu, Theme::Dark),
    shot("service-dark.png", Page::Service, Theme::Dark),
    shot("settings-dark.png", Page::Settings, Theme::Dark),
    shot("about-dark.png", Page::About, Theme::Dark),
    Shot { file: "about-changelog-dark.png", page: Page::About, theme: Theme::Dark, scroll_to_end: true },
    shot("overview-light.png", Page::Overview, Theme::Light),
    shot("fan-curve-light.png", Page::Curve, Theme::Light),
];

enum Phase {
    Warmup,
    Settle { since: Instant, frames: u32 },
    Capture,
}

pub struct Shooter {
    dir: PathBuf,
    started: Instant,
    index: usize,
    phase: Phase,
}

impl Shooter {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, started: Instant::now(), index: 0, phase: Phase::Warmup }
    }

    /// Called every frame. Returns the shot to show, or `None` once all are saved (the window
    /// is then closed).
    pub fn step(&mut self, ctx: &egui::Context, live_ready: bool) -> Option<&'static Shot> {
        if self.index >= SHOTS.len() {
            return None;
        }
        ctx.request_repaint();
        match &mut self.phase {
            Phase::Warmup => {
                if live_ready && self.started.elapsed() >= WARMUP {
                    self.phase = Phase::Settle { since: Instant::now(), frames: 0 };
                }
            }
            Phase::Settle { since, frames } => {
                *frames += 1;
                if *frames >= SETTLE_FRAMES && since.elapsed() >= SETTLE {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                    self.phase = Phase::Capture;
                }
            }
            Phase::Capture => {
                let image = ctx.input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Screenshot { image, .. } => Some(image.clone()),
                        _ => None,
                    })
                });
                if let Some(image) = image {
                    let path = self.dir.join(SHOTS[self.index].file);
                    if let Err(e) = save(&path, &image) {
                        eprintln!("could not save {}: {e}", path.display());
                    }
                    self.index += 1;
                    if self.index == SHOTS.len() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return None;
                    }
                    self.phase = Phase::Settle { since: Instant::now(), frames: 0 };
                }
            }
        }
        Some(&SHOTS[self.index])
    }
}

fn save(path: &std::path::Path, image: &ColorImage) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let [width, height] = image.size;
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    ::image::save_buffer(path, &rgba, width as u32, height as u32, ::image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}
