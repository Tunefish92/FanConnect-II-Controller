//! The About page: author, version, technology with versions, changelog and credits.

use std::time::SystemTime;

use eframe::egui::{self, Color32, CornerRadius, Margin, RichText};

use crate::theme::Palette;
use crate::widgets::{badge, card, card_title, info_grid};

const AUTHOR: &str = "Tunefish";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const RUSTC: &str = env!("GPU_FANCTL_RUSTC");
const TARGET: &str = env!("GPU_FANCTL_TARGET");
const BUILD_DATE: &str = env!("GPU_FANCTL_BUILD_DATE");
/// `name=version` pairs separated by `;`, written by build.rs from Cargo.lock.
const DEPS: &str = env!("GPU_FANCTL_DEPS");

/// CHANGELOG.md, embedded at build time so the app always shows its own history.
const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// One block of a changelog release: a section heading ("Added"), a bullet or a paragraph.
enum Block {
    Heading(String),
    Bullet(String),
    Text(String),
}

/// Splits CHANGELOG.md into releases (`## [version] - date`) with their blocks. Bullet
/// continuation lines (indented) are joined; inline code marks are dropped.
fn releases() -> Vec<(String, Vec<Block>)> {
    let mut releases: Vec<(String, Vec<Block>)> = Vec::new();
    for line in CHANGELOG.lines() {
        let clean = line.replace('`', "");
        if let Some(title) = clean.strip_prefix("## ") {
            releases.push((title.replace(['[', ']'], ""), Vec::new()));
            continue;
        }
        let Some((_, blocks)) = releases.last_mut() else { continue };
        if let Some(heading) = clean.strip_prefix("### ") {
            blocks.push(Block::Heading(heading.to_string()));
        } else if let Some(item) = clean.strip_prefix("- ") {
            blocks.push(Block::Bullet(item.to_string()));
        } else if line.starts_with("  ")
            && let Some(Block::Bullet(item)) = blocks.last_mut()
        {
            item.push(' ');
            item.push_str(clean.trim());
        } else if !clean.trim().is_empty() {
            blocks.push(Block::Text(clean.trim().to_string()));
        }
    }
    releases
}

/// Colour of a changelog section ("Added", "Fixed", …), as in Keep a Changelog.
fn section_color(section: &str, p: &Palette) -> Color32 {
    match section {
        "Added" => p.ok,
        "Changed" => p.accent,
        "Fixed" => p.warn,
        "Removed" | "Deprecated" | "Security" => p.danger,
        _ => p.text,
    }
}

/// A chevron that points right when closed and down when open (`openness` 0..1).
fn chevron(ui: &mut egui::Ui, openness: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let rotation = egui::emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
    let c = rect.center();
    let points = [egui::vec2(-2.5, -5.0), egui::vec2(2.5, 0.0), egui::vec2(-2.5, 5.0)].map(|v| c + rotation * v);
    let stroke = egui::Stroke::new(2.0, color);
    ui.painter().line_segment([points[0], points[1]], stroke);
    ui.painter().line_segment([points[1], points[2]], stroke);
}

/// One release as a framed row that opens on click. All releases start closed.
/// `latest`: the newest published release (not "Unreleased"), marked with a badge.
fn release(ui: &mut egui::Ui, index: usize, title: &str, blocks: &[Block], latest: bool) {
    let p = Palette::current(ui.ctx());
    let (version, date) = title.split_once(" - ").unwrap_or((title, ""));
    let id = ui.make_persistent_id(("release", index));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);

    egui::Frame::new()
        .fill(p.plot_bg)
        .stroke(egui::Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let header = ui
                .horizontal(|ui| {
                    chevron(ui, state.openness(ui.ctx()), p.weak);
                    let label =
                        if version == "Unreleased" { "Unreleased changes".to_string() } else { format!("Version {version}") };
                    ui.label(RichText::new(label).size(16.0).strong());
                    if !date.is_empty() {
                        ui.label(RichText::new(date).weak());
                    }
                    if version == "Unreleased" {
                        badge(ui, "Upcoming", p.warn);
                    } else if latest {
                        badge(ui, "Latest", p.accent);
                    }
                    // Stretch the clickable header across the whole row.
                    ui.allocate_space(egui::vec2(ui.available_width(), 0.0));
                })
                .response
                .interact(egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if header.clicked() {
                state.toggle(ui);
            }

            state.show_body_unindented(ui, |ui| {
                ui.add_space(4.0);
                ui.separator();
                let mut color = p.accent;
                for block in blocks {
                    match block {
                        Block::Heading(section) => {
                            color = section_color(section, &p);
                            ui.add_space(6.0);
                            ui.label(RichText::new(section.to_uppercase()).small().strong().color(color));
                        }
                        Block::Bullet(text) => {
                            ui.horizontal_top(|ui| {
                                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::hover());
                                ui.painter().circle_filled(dot.center(), 3.0, color);
                                ui.add(egui::Label::new(text).wrap());
                            });
                        }
                        Block::Text(text) => {
                            ui.label(RichText::new(text).weak());
                        }
                    }
                }
            });
        });
}

fn changelog(ui: &mut egui::Ui) {
    card_title(ui, "Changelog", "What changed in each version, newest first. Click a version to open it.");
    let releases = releases();
    let latest = releases.iter().position(|(title, _)| title != "Unreleased");
    for (index, (title, blocks)) in releases.iter().enumerate() {
        release(ui, index, title, blocks, Some(index) == latest);
    }
}

fn dep_version(name: &str) -> &'static str {
    DEPS.split(';').find_map(|entry| entry.strip_prefix(name)?.strip_prefix('=')).unwrap_or("?")
}

fn current_year() -> String {
    humantime::format_rfc3339_seconds(SystemTime::now()).to_string().chars().take(4).collect()
}

pub fn page(ui: &mut egui::Ui, icon: &egui::TextureHandle) {
    let p = Palette::current(ui.ctx());

    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((icon.id(), egui::vec2(72.0, 72.0))));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(RichText::new("FanConnect II Controller").size(26.0).strong());
                ui.label(RichText::new(format!("Version {VERSION}")).color(p.accent).strong());
                ui.label(
                    RichText::new(
                        "Temperature-controlled external fans for the ASUS ROG Strix RTX 2080 Ti, on Windows and Linux.",
                    )
                    .weak(),
                );
            });
        });
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("© {} {AUTHOR}", current_year())).strong());
            ui.label(RichText::new("·").weak());
            ui.label(format!("{} License", env!("CARGO_PKG_LICENSE")));
            ui.label(RichText::new("·").weak());
            ui.hyperlink_to(REPOSITORY.trim_start_matches("https://"), REPOSITORY);
        });
    });

    card(ui, |ui| {
        card_title(ui, "Build", "");
        let platform = if cfg!(windows) { "Windows 10 or later" } else { "Linux" };
        info_grid(
            ui,
            "build",
            &[
                ("Version", VERSION.to_string()),
                ("Build date", BUILD_DATE.to_string()),
                ("Platform", format!("{platform} ({TARGET})")),
                ("Compiler", RUSTC.to_string()),
            ],
        );
    });

    card(ui, |ui| {
        card_title(ui, "Technology", "What this app is built with");
        let rust = RUSTC.split_whitespace().nth(1).unwrap_or("?").to_string();
        let mut rows = vec![
            ("Rust", rust, "Programming language"),
            ("eframe", dep_version("eframe").into(), "Native window and rendering"),
            ("egui", dep_version("egui").into(), "User interface"),
            ("egui_plot", dep_version("egui_plot").into(), "History and fan curve charts"),
            ("nvml-wrapper", dep_version("nvml-wrapper").into(), "GPU temperature through NVIDIA NVML"),
            ("serde / serde_json", dep_version("serde_json").into(), "Settings and live status files"),
        ];
        if cfg!(windows) {
            rows.push(("NVIDIA NVAPI", "driver".into(), "I2C access to the FanConnect II controller"));
            rows.push(("libloading", dep_version("libloading").into(), "Loading nvapi64.dll at runtime"));
            rows.push(("windows-service", dep_version("windows-service").into(), "Background Windows service"));
        } else {
            rows.push(("Linux i2c-dev", "kernel".into(), "I2C access to the FanConnect II controller"));
        }
        egui::Grid::new("technology").num_columns(3).spacing([24.0, 10.0]).striped(true).show(ui, |ui| {
            ui.label(RichText::new("COMPONENT").small().weak());
            ui.label(RichText::new("VERSION").small().weak());
            ui.label(RichText::new("USED FOR").small().weak());
            ui.end_row();
            for (name, version, purpose) in &rows {
                ui.label(RichText::new(*name).strong());
                ui.label(RichText::new(version).monospace());
                ui.label(RichText::new(*purpose).weak());
                ui.end_row();
            }
        });
    });

    card(ui, changelog);

    card(ui, |ui| {
        card_title(ui, "Credits & disclaimer", "");
        ui.label(
            "The FanConnect II protocol was worked out by recording the I2C traffic of ASUS GPU Tweak III \
             and verified on the card under Windows and Linux.",
        );
        ui.label(
            RichText::new(
                "This project is not affiliated with, endorsed by or supported by ASUS or NVIDIA. ROG, Strix, \
                 FanConnect and GPU Tweak are trademarks of ASUSTeK Computer Inc.; NVIDIA and GeForce are trademarks \
                 of NVIDIA Corporation. Use at your own risk.",
            )
            .weak(),
        );
    });
}
