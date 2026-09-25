//! Reusable building blocks: cards, stat tiles, segmented control and buttons.

use eframe::egui::{self, Color32, CornerRadius, Margin, Response, RichText, Stroke, Ui, vec2};
use eframe::epaint::Shadow;

use crate::theme::Palette;

pub const BUTTON_HEIGHT: f32 = 38.0;

/// A rounded card that fills the available width.
pub fn card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let p = Palette::current(ui.ctx());
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(18))
        .shadow(Shadow { offset: [0, 2], blur: 10, spread: 0, color: p.shadow })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A tinted banner for status and warnings.
pub fn banner(ui: &mut Ui, color: Color32, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(add_contents);
        });
}

/// Card heading with an optional one-line description.
pub fn card_title(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).heading().strong());
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).weak());
    }
    ui.add_space(4.0);
}

/// Width of the label column in key/value tables, so values line up across cards.
const INFO_KEY_WIDTH: f32 = 110.0;

/// A two-column key/value table.
pub fn info_grid(ui: &mut Ui, id: &str, rows: &[(&str, String)]) {
    info_grid_rich(ui, id, rows.iter().map(|(key, value)| (*key, RichText::new(value))).collect());
}

/// A two-column key/value table whose values can be styled (e.g. coloured).
pub fn info_grid_rich(ui: &mut Ui, id: &str, rows: Vec<(&str, RichText)>) {
    egui::Grid::new(id).num_columns(2).spacing([24.0, 10.0]).min_col_width(INFO_KEY_WIDTH).show(ui, |ui| {
        for (key, value) in rows {
            ui.label(RichText::new(key).weak());
            ui.label(value);
            ui.end_row();
        }
    });
}

/// A small coloured label, e.g. "Supported" or "Latest".
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.15))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(color));
        });
}

/// A coloured status dot.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.5, color);
}

/// A statistic tile: caption, big value with unit, optional bar (0..1) and detail line.
///
/// Every tile has the same rows (the bar row is reserved even without a bar, and text is
/// truncated instead of wrapped), so tiles side by side are exactly the same height.
pub fn stat_card(ui: &mut Ui, caption: &str, value: &str, unit: &str, color: Color32, detail: &str, bar: Option<f32>) {
    card(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add(egui::Label::new(RichText::new(caption).small().weak()).truncate());

            // Value and unit as one text so they are centred together on a shared baseline.
            let mut value_text = egui::text::LayoutJob::default();
            let format = |size: f32| egui::TextFormat { font_id: egui::FontId::proportional(size), color, ..Default::default() };
            value_text.append(value, 0.0, format(36.0));
            value_text.append(unit, 5.0, format(18.0));
            ui.label(value_text);

            // Every tile reserves exactly this bar row; the bar is painted into it by hand, so a
            // tile with a bar is exactly as tall as one without.
            let (bar_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
            if let Some(fraction) = bar {
                let p = Palette::current(ui.ctx());
                let painter = ui.painter();
                painter.rect_filled(bar_rect, CornerRadius::same(3), p.widget);
                let mut filled = bar_rect;
                filled.set_width(bar_rect.width() * fraction.clamp(0.0, 1.0));
                painter.rect_filled(filled, CornerRadius::same(3), color);
            }

            let detail = if detail.is_empty() { " " } else { detail };
            ui.add(egui::Label::new(RichText::new(detail).small().weak()).truncate());
        });
    });
}

/// An entry in the navigation pane.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> Response {
    let p = Palette::current(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), egui::Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        p.accent.gamma_multiply(0.16)
    } else if response.hovered() {
        p.widget_hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    if selected {
        let marker = egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 10.0), vec2(3.0, rect.height() - 20.0));
        painter.rect_filled(marker, CornerRadius::same(2), p.accent);
    }
    let color = if selected { p.accent } else { p.text };
    painter.text(rect.left_center() + vec2(16.0, 0.0), egui::Align2::LEFT_CENTER, icon, egui::FontId::proportional(17.0), color);
    painter.text(rect.left_center() + vec2(46.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(15.0), color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Page title and description at the top of the content area.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(26.0).strong());
    ui.label(RichText::new(subtitle).weak());
    ui.add_space(4.0);
}

/// A row of mutually exclusive options. Returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    let p = Palette::current(ui.ctx());
    let mut changed = false;
    egui::Frame::new()
        .fill(p.widget)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for &(value, label) in options {
                    let selected = *current == value;
                    let text = RichText::new(label).color(if selected { p.on_accent } else { p.text });
                    let button = egui::Button::new(text)
                        .fill(if selected { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(84.0, BUTTON_HEIGHT - 6.0));
                    if ui.add(button).clicked() && !selected {
                        *current = value;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Filled accent button for the main action.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = Palette::current(ui.ctx());
    let fill = if enabled { p.accent } else { p.widget };
    let color = if enabled { p.on_accent } else { p.weak };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).strong().color(color))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(130.0, BUTTON_HEIGHT)),
    )
}

/// Outlined button for secondary actions.
pub fn secondary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).corner_radius(CornerRadius::same(10)).min_size(vec2(110.0, BUTTON_HEIGHT)),
    )
}

/// Outlined button in the danger colour.
pub fn danger_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = Palette::current(ui.ctx());
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(p.danger))
            .stroke(Stroke::new(1.0, p.danger.gamma_multiply(0.6)))
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(110.0, BUTTON_HEIGHT)),
    )
}
