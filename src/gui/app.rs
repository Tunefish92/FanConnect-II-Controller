//! The main window: status, live values, history, fan curve editor and service management,
//! laid out as cards.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use eframe::egui::{self, Margin, RichText};
use egui_plot::{AxisHints, Legend, Line, LineStyle, MarkerShape, Plot, PlotPoints, PlotTransform, Points, VLine};
use gpu_fanctl::config::{self, Config, MAX_TEMP_RANGE};
use gpu_fanctl::curve::{self, CUSTOM_POINTS, CUSTOM_TEMPS, Curve, FLOOR_DUTY};
use gpu_fanctl::fanconnect::{MODE_AUTO, MODE_HOST};

use crate::live::{self, Live, ServiceState, Source};
use crate::theme::{self, Palette, ThemeChoice};
use crate::widgets::{self, card, card_title, danger_button, primary_button, secondary_button};

/// How close (°C / %) the pointer must be to grab a curve point.
const GRAB_DISTANCE: f64 = 3.5;
/// Below this content width the stat cards go two per row.
const WIDE_LAYOUT: f32 = 720.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Overview,
    Curve,
    Gpu,
    Service,
    Settings,
    About,
}

const PAGES: &[(Page, &str, &str)] = &[
    (Page::Overview, "🏠", "Overview"),
    (Page::Curve, "📈", "Fan curve"),
    (Page::Gpu, "🖥", "GPU"),
    (Page::Service, "🔧", "Service"),
    (Page::Settings, "⚙", "Settings"),
    (Page::About, "ℹ", "About"),
];

/// Resolution of the app icon texture shown in the sidebar and on the About page.
const ICON_TEXTURE: u32 = 128;

pub struct App {
    live: Arc<Mutex<Live>>,
    /// Screenshot mode for the README (`--screenshots <folder>`).
    shooter: Option<crate::screenshots::Shooter>,
    icon: egui::TextureHandle,
    page: Page,
    theme: ThemeChoice,
    /// The settings as last loaded from or saved to disk.
    saved: Config,
    saved_stamp: Option<SystemTime>,
    last_disk_check: Instant,
    /// Curve editor state.
    custom: bool,
    max_temp: u32,
    points: Vec<(f32, f32)>,
    dragging: Option<usize>,
    message: Option<(String, bool)>,
    service: ServicePanel,
}

#[derive(Default)]
struct ServicePanel {
    job: Option<crate::elevate::Job>,
    confirm_uninstall: bool,
    installed_path: Option<std::path::PathBuf>,
    message: Option<(String, bool)>,
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(config::path()).and_then(|m| m.modified()).ok()
}

/// Auto points rounded to whole numbers, as a starting point for a custom curve.
fn rounded_auto_points(max_temp: u32) -> Vec<(f32, f32)> {
    let mut points: Vec<(f32, f32)> =
        curve::auto_points(max_temp).into_iter().map(|(t, d)| (t.round(), d.round())).collect();
    points.dedup_by(|b, a| b.0 <= a.0);
    points
}

/// Gap between the chart area and the tick values.
const TICK_GAP: f32 = 8.0;

/// An axis with a title but without egui_plot's own tick values, which it draws flush against
/// the chart area. `paint_ticks` draws them with a gap instead; the thickness leaves room for both.
fn x_axis<'a>(title: &str) -> AxisHints<'a> {
    AxisHints::new_x().label(title).formatter(|_, _| String::new()).min_thickness(50.0)
}

fn y_axis<'a>(title: &str) -> AxisHints<'a> {
    AxisHints::new_y().label(title).formatter(|_, _| String::new()).min_thickness(60.0)
}

/// Draws tick values below and left of the chart area, `TICK_GAP` away from it.
fn paint_ticks(ui: &egui::Ui, transform: &PlotTransform, xs: &[(f64, String)], ys: &[(f64, String)]) {
    let painter = ui.painter();
    let frame = *transform.frame();
    let font = egui::FontId::proportional(13.5);
    let color = ui.visuals().text_color();
    for (value, text) in xs {
        let x = transform.position_from_point_x(*value);
        painter.text(egui::pos2(x, frame.bottom() + TICK_GAP), egui::Align2::CENTER_TOP, text, font.clone(), color);
    }
    for (value, text) in ys {
        let y = transform.position_from_point_y(*value);
        painter.text(egui::pos2(frame.left() - TICK_GAP, y), egui::Align2::RIGHT_CENTER, text, font.clone(), color);
    }
}

/// Evenly spaced tick values from `from` to `to` (inclusive), labelled as whole numbers.
fn ticks(from: i32, to: i32, step: usize) -> Vec<(f64, String)> {
    (from..=to).step_by(step).map(|v| (f64::from(v), v.to_string())).collect()
}

fn message_label(ui: &mut egui::Ui, message: &Option<(String, bool)>) {
    if let Some((text, is_error)) = message {
        let p = Palette::current(ui.ctx());
        ui.label(RichText::new(text).color(if *is_error { p.danger } else { p.ok }));
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshots: Option<std::path::PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        let theme = theme::load_choice();
        cc.egui_ctx.set_theme(theme.preference());

        let live = live::spawn(cc.egui_ctx.clone());
        let (saved, warning) = config::load(&config::path());
        let icon = cc.egui_ctx.load_texture(
            "app-icon",
            egui::ColorImage::from_rgba_unmultiplied([ICON_TEXTURE as usize; 2], &gpu_fanctl::icon_art::render(ICON_TEXTURE)),
            egui::TextureOptions::LINEAR,
        );
        let mut app = Self {
            live,
            shooter: screenshots.map(crate::screenshots::Shooter::new),
            icon,
            page: Page::Overview,
            theme,
            saved: saved.clone(),
            saved_stamp: modified(),
            last_disk_check: Instant::now(),
            custom: false,
            max_temp: saved.max_temp,
            points: Vec::new(),
            dragging: None,
            message: warning.map(|w| (w, true)),
            service: ServicePanel { installed_path: gpu_fanctl::service::installed_path(), ..Default::default() },
        };
        app.load_into_editor(&saved);
        app
    }

    /// Loads settings into the editor. `points` always holds the user's custom curve (active or
    /// remembered); the Auto curve is only computed for display and never stored there.
    fn load_into_editor(&mut self, config: &Config) {
        self.max_temp = config.max_temp;
        self.custom = matches!(config.curve, Curve::Custom(_));
        self.points = config.custom_points().map_or_else(Vec::new, <[_]>::to_vec);
        self.dragging = None;
    }

    /// The settings as currently edited, or why they are invalid. With Auto selected, the custom
    /// points are kept as the remembered custom curve (if they are valid).
    fn edited(&self) -> Result<Config, String> {
        if self.custom {
            return Ok(Config { max_temp: self.max_temp, curve: Curve::custom(self.points.clone())?, remembered_custom: None });
        }
        let remembered_custom = Curve::custom(self.points.clone()).ok().map(|_| self.points.clone());
        Ok(Config { max_temp: self.max_temp, curve: Curve::Auto, remembered_custom })
    }

    /// The points shown in the chart: the custom points, or the Auto curve's points.
    fn shown_points(&self) -> Vec<(f32, f32)> {
        if self.custom { self.points.clone() } else { rounded_auto_points(self.max_temp) }
    }

    fn is_dirty(&self) -> bool {
        self.edited().map_or(true, |c| c != self.saved)
    }

    /// Picks up changes made outside the GUI (CLI, text editor), unless there are unsaved edits.
    fn check_disk(&mut self) {
        if self.last_disk_check.elapsed().as_secs_f32() < 1.0 {
            return;
        }
        self.last_disk_check = Instant::now();
        {
            self.service.installed_path = gpu_fanctl::service::installed_path();
        }
        let stamp = modified();
        if stamp == self.saved_stamp {
            return;
        }
        self.saved_stamp = stamp;
        let (config, warning) = config::load(&config::path());
        let dirty = self.is_dirty();
        self.saved = config.clone();
        if !dirty {
            self.load_into_editor(&config);
        }
        if let Some(w) = warning {
            self.message = Some((w, true));
        }
    }

    fn apply(&mut self, service_running: bool) {
        let Ok(config) = self.edited() else { return };
        match config::save(&config::path(), &config) {
            Ok(()) => {
                self.saved = config;
                self.saved_stamp = modified();
                let note = if service_running {
                    "Saved. The service applies it within a second."
                } else {
                    "Saved. Nothing is controlling the fans until the service runs."
                };
                self.message = Some((note.into(), false));
            }
            Err(e) => {
                self.message = Some((
                    format!(
                        "Could not save {}: {e}. Install the service (it lets users edit the settings) or run this app as administrator/root.",
                        config::path().display()
                    ),
                    true,
                ));
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.check_disk();
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut scroll_to_end = false;
        if let Some(shooter) = &mut self.shooter
            && let Some(shot) = shooter.step(ui.ctx(), live.latest.is_some())
        {
            // Only what is shown changes; the saved theme and settings stay as they are.
            self.page = shot.page;
            ui.ctx().set_theme(shot.theme);
            scroll_to_end = shot.scroll_to_end;
        }
        let service_running = live.source == Some(Source::Daemon);
        let p = Palette::current(ui.ctx());

        egui::Panel::left("navigation")
            .exact_size(230.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.card).inner_margin(Margin::symmetric(14, 20)))
            .show(ui, |ui| self.navigation(ui, &live));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                // One scroll area per page, so each page keeps its own scroll position.
                egui::ScrollArea::vertical().id_salt(("page", self.page)).auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 14.0;
                    match self.page {
                        Page::Overview => {
                            widgets::page_header(ui, "Overview", "Live readings from the card and the fan service");
                            status_banners(ui, &live);
                            stat_cards(ui, &live);
                            card(ui, |ui| history_card(ui, &live));
                        }
                        Page::Curve => {
                            widgets::page_header(ui, "Fan curve", "How the external fans follow the GPU temperature");
                            card(ui, |ui| self.curve_card(ui, &live, service_running));
                        }
                        Page::Gpu => {
                            widgets::page_header(ui, "GPU", "The graphics card and its FanConnect II controller");
                            crate::gpu::page(ui, &live);
                        }
                        Page::Service => {
                            widgets::page_header(
                                ui,
                                "Service",
                                "The background service controls the fans, also while this window is closed",
                            );
                            card(ui, |ui| self.service_card(ui, &live));
                        }
                        Page::Settings => {
                            widgets::page_header(ui, "Settings", "Appearance and file locations");
                            self.settings_page(ui);
                        }
                        Page::About => {
                            widgets::page_header(ui, "About", "Version, technology and credits");
                            crate::about::page(ui, &self.icon);
                        }
                    }
                    if scroll_to_end {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
            });
    }
}

fn status_banners(ui: &mut egui::Ui, live: &Live) {
    let p = Palette::current(ui.ctx());
    let (color, title, detail) = match (live.source, live.service) {
        (Some(Source::Daemon), _) => (p.ok, "Service running", "The fans follow the fan curve."),
        (_, ServiceState::Stopped) => {
            (p.warn, "Service stopped", "Nothing is controlling the fans. Reinstall it on the Service page to start it.")
        }
        (_, ServiceState::NotInstalled) => {
            (p.warn, "Service not installed", "Nothing is controlling the fans. Install it on the Service page.")
        }
        _ => (p.warn, "No service running", "Nothing is controlling the fans. Showing readings taken directly from the card."),
    };
    widgets::banner(ui, color, |ui| {
        widgets::dot(ui, color);
        ui.label(RichText::new(title).strong());
        ui.label(RichText::new(detail).weak());
    });
    if let Some(name) = live.other_controller {
        widgets::banner(ui, p.warn, |ui| {
            ui.label(RichText::new("⚠").color(p.warn).strong());
            ui.label(RichText::new(format!("{name} is running.")).strong());
            ui.label(RichText::new("Turn off GPU Tweak's external fan control, or it will fight over the fan speed.").weak());
        });
    }
    if let Some(warning) = live.latest.as_ref().and_then(|s| s.warning.clone()) {
        widgets::banner(ui, p.warn, |ui| {
            ui.label(RichText::new("⚠").color(p.warn).strong());
            ui.label(warning);
        });
    }
    if live.latest.is_none() {
        widgets::banner(ui, p.danger, |ui| {
            ui.label(RichText::new("No live data").strong());
            if let Some(e) = &live.error {
                ui.label(RichText::new(e).weak());
            }
        });
    }
}

fn stat_cards(ui: &mut egui::Ui, live: &Live) {
    let p = Palette::current(ui.ctx());
    let s = live.latest.as_ref();
    let temp = s.and_then(|s| s.gpu_temp).map_or("–".into(), |t| format!("{t:.0}"));
    let duty = s.map_or("–".into(), |s| format!("{:.0}", s.duty));
    let target = s.and_then(|s| s.target).map_or(String::new(), |t| format!("Target {t:.0} %"));
    let mode = s.map_or("", |s| match s.mode {
        MODE_HOST => "PC control",
        MODE_AUTO => "Card auto mode",
        _ => "Unknown mode",
    });
    let fan = |rpm: u32| if rpm == 0 && s.is_some_and(|s| s.duty >= FLOOR_DUTY) { "⚠ Reports 0 RPM" } else { "" };
    let rpm1 = s.map_or(0, |s| s.fan1_rpm);
    let rpm2 = s.map_or(0, |s| s.fan2_rpm);
    let rpm_text = |rpm: u32| if s.is_some() { rpm.to_string() } else { "–".into() };

    let tile = |ui: &mut egui::Ui, index: usize| match index {
        0 => widgets::stat_card(ui, "GPU TEMPERATURE", &temp, "°C", p.temp, mode, None),
        1 => widgets::stat_card(ui, "FAN DUTY", &duty, "%", p.accent, &target, s.map(|s| s.duty / 100.0)),
        2 => widgets::stat_card(ui, "FAN 1", &rpm_text(rpm1), "RPM", p.text, fan(rpm1), None),
        _ => widgets::stat_card(ui, "FAN 2", &rpm_text(rpm2), "RPM", p.text, fan(rpm2), None),
    };
    let per_row = if ui.available_width() >= WIDE_LAYOUT { 4 } else { 2 };
    for first in (0..4).step_by(per_row) {
        ui.columns(per_row, |columns| {
            for (offset, column) in columns.iter_mut().enumerate() {
                tile(column, first + offset);
            }
        });
    }
}

fn history_card(ui: &mut egui::Ui, live: &Live) {
    let p = Palette::current(ui.ctx());
    card_title(ui, "History", "GPU temperature and fan duty over the last 10 minutes");
    let now = live.started.elapsed().as_secs_f64();
    let series = |f: &dyn Fn(&live::Sample) -> Option<f32>| -> PlotPoints<'static> {
        live.history.iter().filter_map(|s| f(s).map(|v| [s.t - now, f64::from(v)])).collect::<Vec<_>>().into()
    };
    let response = Plot::new("history")
        .height(210.0)
        .legend(Legend::default())
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .default_x_bounds(-600.0, 0.0)
        .default_y_bounds(0.0, 105.0)
        .custom_x_axes(vec![x_axis("Time")])
        .custom_y_axes(vec![y_axis("°C  /  %")])
        .show(ui, |plot| {
            plot.line(Line::new("GPU °C", series(&|s| s.gpu_temp)).color(p.temp).width(2.0));
            plot.line(Line::new("Fan duty %", series(&|s| Some(s.duty))).color(p.accent).width(2.0));
        });
    // Every 2 minutes, labelled "-10:00" … "now".
    let times: Vec<(f64, String)> = (0..=600)
        .step_by(120)
        .map(|s| (-f64::from(s), if s == 0 { "now".into() } else { format!("-{}:{:02}", s / 60, s % 60) }))
        .collect();
    paint_ticks(ui, &response.transform, &times, &ticks(0, 100, 20));
}

impl App {
    fn curve_card(&mut self, ui: &mut egui::Ui, live: &Live, service_running: bool) {
        let p = Palette::current(ui.ctx());

        ui.horizontal_wrapped(|ui| {
            widgets::segmented(ui, &mut self.custom, &[(false, "Auto"), (true, "Custom")]);
            // First switch to Custom without any custom curve yet: start from the Auto shape.
            if self.custom && self.points.is_empty() {
                self.points = rounded_auto_points(self.max_temp);
            }
            ui.add_space(20.0);
            ui.label("Max GPU temp");
            ui.add(egui::Slider::new(&mut self.max_temp, MAX_TEMP_RANGE).suffix(" °C"));
        });
        ui.label(
            RichText::new(if self.custom {
                "Drag the points in the chart or edit them in the table. From max temp the fans always run at 100 %."
            } else {
                "Built-in curve: 30 % up to 45 °C, rising to 100 % at max temp. Choose Custom to shape your own."
            })
            .weak(),
        );
        self.curve_plot(ui, live);
        if self.custom {
            self.points_editor(ui);
        }

        let edited = self.edited();
        if let Err(e) = &edited {
            widgets::banner(ui, p.danger, |ui| {
                ui.label(RichText::new(format!("✖ {e}")).color(p.danger));
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let dirty = self.is_dirty();
            if primary_button(ui, "Apply", edited.is_ok() && dirty).clicked() {
                self.apply(service_running);
            }
            if secondary_button(ui, "Revert", dirty).clicked() {
                let saved = self.saved.clone();
                self.load_into_editor(&saved);
                self.message = None;
            }
            if dirty {
                ui.label(RichText::new("Unsaved changes").color(p.warn));
            }
        });
        message_label(ui, &self.message);
    }

    fn curve_plot(&mut self, ui: &mut egui::Ui, live: &Live) {
        let p = Palette::current(ui.ctx());
        let max_temp = self.max_temp;
        // Drawn even while invalid, so dragging stays responsive.
        let preview = if self.custom { Curve::Custom(self.points.clone()) } else { Curve::Auto };
        let line: Vec<[f64; 2]> = (40..=200)
            .map(|half| {
                let t = f64::from(half) / 2.0;
                let duty = if t >= f64::from(max_temp) { 100.0 } else { preview.duty(t as f32, max_temp) };
                [t, f64::from(duty)]
            })
            .collect();
        let handles: Vec<[f64; 2]> = self.shown_points().iter().map(|&(t, d)| [f64::from(t), f64::from(d)]).collect();
        let now = live.latest.as_ref().and_then(|s| s.gpu_temp.map(|t| [f64::from(t), f64::from(s.duty)]));
        let custom = self.custom;

        let response = Plot::new("curve")
            .height(300.0)
            .legend(Legend::default())
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .default_x_bounds(f64::from(*CUSTOM_TEMPS.start()), f64::from(*CUSTOM_TEMPS.end()))
            .default_y_bounds(20.0, 105.0)
            .custom_x_axes(vec![x_axis("GPU temperature °C")])
            .custom_y_axes(vec![y_axis("Fan duty %")])
            .show(ui, |plot| {
                plot.line(Line::new("Curve", line).color(p.accent).width(3.0));
                plot.vline(
                    VLine::new("Max temp", f64::from(max_temp)).color(p.danger).width(2.0).style(LineStyle::dashed_loose()),
                );
                plot.points(
                    Points::new("Points", handles)
                        .radius(if custom { 7.0 } else { 4.0 })
                        .filled(custom)
                        .shape(MarkerShape::Circle)
                        .color(p.accent),
                );
                if let Some(now) = now {
                    plot.points(Points::new("Now", vec![now]).radius(8.0).shape(MarkerShape::Diamond).color(p.temp));
                }
                plot.pointer_coordinate()
            });
        paint_ticks(ui, &response.transform, &ticks(20, 100, 10), &ticks(20, 100, 10));

        if !custom {
            return;
        }
        let pointer = response.inner;
        if response.response.drag_started()
            && let Some(p) = pointer
        {
            self.dragging = self
                .points
                .iter()
                .enumerate()
                .map(|(i, &(t, d))| (i, (f64::from(t) - p.x).hypot(f64::from(d) - p.y)))
                .filter(|&(_, dist)| dist <= GRAB_DISTANCE)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
        }
        if response.response.dragged()
            && let (Some(i), Some(p)) = (self.dragging, pointer)
        {
            // Keep the point between its neighbours so the curve stays valid while dragging.
            let prev = i.checked_sub(1).map(|j| self.points[j]);
            let next = self.points.get(i + 1).copied();
            let t_min = prev.map_or(*CUSTOM_TEMPS.start(), |p| p.0 + 1.0);
            let t_max = next.map_or(*CUSTOM_TEMPS.end(), |n| n.0 - 1.0);
            let d_min = prev.map_or(FLOOR_DUTY, |p| p.1);
            let d_max = next.map_or(100.0, |n| n.1);
            if t_min <= t_max && d_min <= d_max {
                self.points[i] = ((p.x as f32).round().clamp(t_min, t_max), (p.y as f32).round().clamp(d_min, d_max));
            }
        }
        if response.response.drag_stopped() {
            self.dragging = None;
        }
    }

    fn points_editor(&mut self, ui: &mut egui::Ui) {
        const BADGE: [f32; 2] = [44.0, 32.0];
        const FIELD: [f32; 2] = [120.0, 32.0];
        let p = Palette::current(ui.ctx());
        let mut remove = None;
        // A framed table without stripes: the input fields carry the row structure.
        egui::Frame::new()
            .fill(p.plot_bg)
            .stroke(egui::Stroke::new(1.0, p.card_stroke))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                egui::Grid::new("points").num_columns(4).spacing([16.0, 8.0]).show(ui, |ui| {
                    let header = |ui: &mut egui::Ui, size: [f32; 2], text: &str| {
                        ui.add_sized(size, egui::Label::new(RichText::new(text).small().weak()));
                    };
                    header(ui, BADGE, "POINT");
                    header(ui, FIELD, "TEMPERATURE");
                    header(ui, FIELD, "DUTY");
                    ui.label("");
                    ui.end_row();

                    let can_remove = self.points.len() > *CUSTOM_POINTS.start();
                    for (i, (t, d)) in self.points.iter_mut().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(BADGE.into(), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 13.0, p.accent.gamma_multiply(0.18));
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            (i + 1).to_string(),
                            egui::FontId::proportional(14.0),
                            p.accent,
                        );
                        ui.add_sized(
                            FIELD,
                            egui::DragValue::new(t).range(CUSTOM_TEMPS).speed(0.5).fixed_decimals(0).suffix(" °C"),
                        );
                        ui.add_sized(
                            FIELD,
                            egui::DragValue::new(d).range(FLOOR_DUTY..=100.0).speed(0.5).fixed_decimals(0).suffix(" %"),
                        );
                        let remove_button = egui::Button::new("🗑").min_size(egui::vec2(38.0, 32.0));
                        if ui.add_enabled(can_remove, remove_button).on_hover_text("Remove this point").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
            });
        if let Some(i) = remove {
            self.points.remove(i);
        }
        ui.horizontal(|ui| {
            let can_add = self.points.len() < *CUSTOM_POINTS.end();
            if secondary_button(ui, "➕ Add point", can_add).clicked() {
                let &(t, d) = self.points.last().unwrap_or(&(60.0, 50.0));
                self.points.push(((t + 5.0).min(*CUSTOM_TEMPS.end()), d));
            }
            if secondary_button(ui, "Start from Auto shape", true).clicked() {
                self.points = rounded_auto_points(self.max_temp);
            }
        });
    }
}

impl App {
    fn service_card(&mut self, ui: &mut egui::Ui, live: &Live) {
        use crate::elevate;

        let p = Palette::current(ui.ctx());
        let panel = &mut self.service;
        if let Some(result) = panel.job.as_ref().and_then(elevate::Job::finished) {
            let action = panel.job.take().map_or("", |j| j.action);
            panel.message = Some(match result {
                Ok(()) => (format!("Service {action} finished."), false),
                Err(e) => (e, true),
            });
            panel.installed_path = gpu_fanctl::service::installed_path();
        }

        let title = if cfg!(windows) { "Windows service" } else { "systemd service" };
        card_title(ui, title, "Runs in the background and controls the fans, also without this window");
        let (color, state) = match live.service {
            ServiceState::Running => (p.ok, "Installed and running"),
            ServiceState::Stopped => (p.warn, "Installed, but stopped"),
            ServiceState::NotInstalled => (p.weak, "Not installed"),
            ServiceState::Unknown => (p.weak, "Checking…"),
        };
        ui.horizontal(|ui| {
            widgets::dot(ui, color);
            ui.label(RichText::new(state).strong());
        });
        if let Some(path) = &panel.installed_path {
            ui.label(RichText::new(format!("Runs {}", path.display())).small().weak());
        }

        let cli = elevate::cli_path();
        match &cli {
            Some(path) => {
                let install_dir = gpu_fanctl::service::install_dir();
                let running_installed_copy = path
                    .parent()
                    .is_some_and(|dir| std::fs::canonicalize(dir).ok() == std::fs::canonicalize(&install_dir).ok());
                let hint = match (&panel.installed_path, running_installed_copy) {
                    (None, _) => {
                        let shortcuts = if cfg!(windows) {
                            "Start menu and desktop shortcuts"
                        } else {
                            "an app menu entry and a desktop shortcut"
                        };
                        format!(
                            "Install copies this app to {}, runs the service from there and adds {shortcuts}.",
                            install_dir.display()
                        )
                    }
                    (Some(_), false) => format!("Reinstall updates {} with this version of the app.", install_dir.display()),
                    (Some(_), true) => String::new(),
                };
                if !hint.is_empty() {
                    ui.label(RichText::new(hint).weak());
                }
            }
            None => {
                ui.label(
                    RichText::new("The gpu-fanctl program was not found next to this app, so the service can't be managed here.")
                        .color(p.danger),
                );
            }
        }

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let Some(job) = &panel.job {
                ui.spinner();
                ui.label(format!("Running {}: confirm the administrator prompt…", job.action));
                return;
            }
            let enabled = cli.is_some();
            let mut start = None;
            match live.service {
                ServiceState::NotInstalled => {
                    if primary_button(ui, "Install service", enabled).clicked() {
                        start = Some("install");
                    }
                }
                ServiceState::Running | ServiceState::Stopped => {
                    if primary_button(ui, "Reinstall", enabled).clicked() {
                        start = Some("reinstall");
                    }
                    if panel.confirm_uninstall {
                        ui.label("Uninstall? The fans stay at 100 % until something else controls them.");
                        if danger_button(ui, "Yes, uninstall", true).clicked() {
                            start = Some("uninstall");
                        }
                        if secondary_button(ui, "Cancel", true).clicked() {
                            panel.confirm_uninstall = false;
                        }
                    } else if danger_button(ui, "Uninstall", enabled).clicked() {
                        panel.confirm_uninstall = true;
                    }
                }
                ServiceState::Unknown => {}
            }
            if let Some(action) = start {
                panel.confirm_uninstall = false;
                panel.message = None;
                panel.job = Some(elevate::start(action, ui.ctx().clone()));
            }
        });
        message_label(ui, &panel.message);
        ui.label(RichText::new("Each action asks for administrator approval.").small().weak());
    }
}

/// The status shown at the bottom of the navigation pane, on every page.
fn service_summary(live: &Live, p: &Palette) -> (egui::Color32, &'static str) {
    match (live.source, live.service) {
        (Some(Source::Daemon), _) => (p.ok, "Service running"),
        (_, ServiceState::Stopped) => (p.warn, "Service stopped"),
        (_, ServiceState::NotInstalled) => (p.warn, "Service not installed"),
        _ => (p.warn, "No service running"),
    }
}

/// Opens a folder in the file manager.
fn open_folder(path: &std::path::Path) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

impl App {
    fn navigation(&mut self, ui: &mut egui::Ui, live: &Live) {
        let p = Palette::current(ui.ctx());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.add(egui::Image::new((self.icon.id(), egui::vec2(40.0, 40.0))));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("FanConnect II Controller").size(16.0).strong());
                ui.label(RichText::new("ROG Strix RTX 2080 Ti").small().weak());
            });
        });
        ui.add_space(22.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        for &(page, icon, label) in PAGES {
            if widgets::nav_item(ui, icon, label, self.page == page).clicked() {
                self.page = page;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            egui::Frame::new()
                .fill(p.widget)
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    // Bottom-up like the pane, so the card only grows as tall as its content; the
                    // lines are therefore added last-to-first. Centred horizontally.
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        if let Some(s) = &live.latest {
                            let temp = s.gpu_temp.map_or("–".into(), |t| format!("{t:.0} °C"));
                            ui.label(RichText::new(format!("GPU {temp}  ·  Fans {:.0} %", s.duty)).small().weak());
                        }
                        let (color, text) = service_summary(live, &p);
                        // Dot and text as one label, so they are centred together. "●" is only in
                        // the monospace font.
                        let mut status = egui::text::LayoutJob::default();
                        let format = |font_id: egui::FontId, color: egui::Color32| egui::TextFormat {
                            font_id,
                            color,
                            valign: egui::Align::Center,
                            ..Default::default()
                        };
                        status.append("●", 0.0, format(egui::FontId::monospace(13.0), color));
                        status.append(text, 8.0, format(egui::FontId::proportional(13.5), p.text));
                        ui.label(status);
                    });
                });
        });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            card_title(ui, "Appearance", "Follow the system setting or pick a theme");
            let mut choice = self.theme;
            if widgets::segmented(
                ui,
                &mut choice,
                &[(ThemeChoice::System, "System"), (ThemeChoice::Light, "Light"), (ThemeChoice::Dark, "Dark")],
            ) {
                self.theme = choice;
                ui.ctx().set_theme(choice.preference());
                theme::save_choice(choice);
            }
        });

        card(ui, |ui| {
            card_title(ui, "Files", "Where the settings and logs are kept");
            let settings = config::path();
            egui::Grid::new("files").num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Settings").weak());
                ui.label(settings.display().to_string());
                ui.end_row();
                ui.label(RichText::new("Service log").weak());
                if cfg!(windows) {
                    ui.label(config::data_dir().join("gpu-fanctl.log").display().to_string());
                } else {
                    ui.label("journalctl -u gpu-fanctl");
                }
                ui.end_row();
                ui.label(RichText::new("Live status").weak());
                ui.label(gpu_fanctl::status::path().display().to_string());
                ui.end_row();
            });
            if let Some(dir) = settings.parent()
                && secondary_button(ui, "Open settings folder", dir.exists()).clicked()
            {
                open_folder(dir);
            }
        });
    }
}
