//! Colours, fonts and spacing for light and dark mode, and the user's saved theme choice
//! (%APPDATA%\gpu-fanctl\gui.json on Windows, ~/.config/gpu-fanctl/gui.json on Linux).

use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub shadow: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    /// Alternate row colour of striped tables; distinct from `widget` so input fields stand out.
    pub stripe: Color32,
    pub plot_bg: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub temp: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0F, 0x12, 0x17),
    card: Color32::from_rgb(0x19, 0x1E, 0x26),
    card_stroke: Color32::from_rgb(0x28, 0x2F, 0x3A),
    shadow: Color32::from_black_alpha(90),
    text: Color32::from_rgb(0xE6, 0xEA, 0xF0),
    weak: Color32::from_rgb(0x8C, 0x96, 0xA5),
    widget: Color32::from_rgb(0x23, 0x2A, 0x34),
    widget_hover: Color32::from_rgb(0x2D, 0x36, 0x43),
    stripe: Color32::from_rgb(0x1E, 0x24, 0x2D),
    plot_bg: Color32::from_rgb(0x13, 0x17, 0x1D),
    accent: Color32::from_rgb(0x4C, 0x8D, 0xFF),
    on_accent: Color32::WHITE,
    temp: Color32::from_rgb(0xFF, 0x8A, 0x3D),
    ok: Color32::from_rgb(0x3F, 0xB9, 0x50),
    warn: Color32::from_rgb(0xE3, 0xA0, 0x08),
    danger: Color32::from_rgb(0xF0, 0x52, 0x4F),
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF2, 0xF4, 0xF7),
    card: Color32::WHITE,
    card_stroke: Color32::from_rgb(0xE0, 0xE4, 0xEA),
    shadow: Color32::from_black_alpha(18),
    text: Color32::from_rgb(0x1B, 0x22, 0x30),
    weak: Color32::from_rgb(0x69, 0x73, 0x82),
    widget: Color32::from_rgb(0xEC, 0xEF, 0xF3),
    widget_hover: Color32::from_rgb(0xE0, 0xE6, 0xEE),
    stripe: Color32::from_rgb(0xF6, 0xF8, 0xFA),
    plot_bg: Color32::from_rgb(0xF8, 0xFA, 0xFC),
    accent: Color32::from_rgb(0x2F, 0x6F, 0xEB),
    on_accent: Color32::WHITE,
    temp: Color32::from_rgb(0xE0, 0x66, 0x1A),
    ok: Color32::from_rgb(0x1F, 0x8F, 0x3A),
    warn: Color32::from_rgb(0xB7, 0x79, 0x1F),
    danger: Color32::from_rgb(0xD9, 0x3B, 0x38),
};

impl Palette {
    pub fn of(theme: Theme) -> Self {
        match theme {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }

    /// The palette for the theme currently shown.
    pub fn current(ctx: &egui::Context) -> Self {
        Self::of(ctx.theme())
    }
}

/// Installs fonts, spacing and colours for both themes.
pub fn install(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        let p = Palette::of(theme);
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
                (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(10.0, 10.0);
            style.spacing.button_padding = egui::vec2(16.0, 8.0);
            style.spacing.interact_size.y = 34.0;
            style.spacing.slider_width = 240.0;

            let v = &mut style.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.card;
            v.window_stroke = Stroke::new(1.0, p.card_stroke);
            v.window_corner_radius = CornerRadius::same(12);
            v.extreme_bg_color = p.plot_bg;
            v.faint_bg_color = p.stripe;
            v.weak_text_color = Some(p.weak);
            v.hyperlink_color = p.accent;
            v.warn_fg_color = p.warn;
            v.error_fg_color = p.danger;
            v.selection.bg_fill = p.accent;
            v.selection.stroke = Stroke::new(1.5, p.on_accent);
            v.slider_trailing_fill = true;

            let w = &mut v.widgets;
            w.noninteractive.bg_fill = p.card;
            w.noninteractive.weak_bg_fill = p.card;
            w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
            w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
            w.noninteractive.corner_radius = CornerRadius::same(8);
            for (state, fill, stroke) in [
                (&mut w.inactive, p.widget, p.card_stroke),
                (&mut w.hovered, p.widget_hover, p.accent),
                (&mut w.active, p.widget_hover, p.accent),
                (&mut w.open, p.widget_hover, p.card_stroke),
            ] {
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke = Stroke::new(1.0, stroke);
                state.fg_stroke = Stroke::new(1.5, p.text);
                state.corner_radius = CornerRadius::same(8);
            }
        });
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn preference(self) -> ThemePreference {
        match self {
            ThemeChoice::System => ThemePreference::System,
            ThemeChoice::Light => ThemePreference::Light,
            ThemeChoice::Dark => ThemePreference::Dark,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Prefs {
    #[serde(default)]
    theme: ThemeChoice,
}

fn prefs_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?
    };
    Some(base.join("gpu-fanctl").join("gui.json"))
}

pub fn load_choice() -> ThemeChoice {
    prefs_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Prefs>(&bytes).ok())
        .map(|p| p.theme)
        .unwrap_or_default()
}

pub fn save_choice(theme: ThemeChoice) {
    let Some(path) = prefs_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(&Prefs { theme }) {
        let _ = std::fs::write(path, json);
    }
}
