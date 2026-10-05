//! The animated illustration on the GPU page: the graphics card with its three fans, the two
//! FanConnect II headers on its end, the case fans plugged into them, and live values as small
//! overlay tiles. Everything is painted in a fixed design space and scaled to the card's width.

use std::f32::consts::TAU;

use eframe::egui::epaint::CubicBezierShape;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use gpu_fanctl::calibration::Calibration;

use crate::live::Live;
use crate::theme::Palette;

/// Size of the design space.
const DESIGN: egui::Vec2 = vec2(860.0, 340.0);
/// The illustration isn't enlarged beyond this.
const MAX_SCALE: f32 = 1.3;

/// Hardware colours. The card is dark in both themes, as the real one is.
const BODY: Color32 = Color32::from_rgb(0x1A, 0x1E, 0x25);
const BODY_EDGE: Color32 = Color32::from_rgb(0x3A, 0x42, 0x50);
const PANEL: Color32 = Color32::from_rgb(0x26, 0x2C, 0x36);
const WELL: Color32 = Color32::from_rgb(0x0C, 0x0E, 0x12);
const BLADE: Color32 = Color32::from_rgb(0x39, 0x41, 0x4E);
const BLADE_EDGE: Color32 = Color32::from_rgb(0x55, 0x5F, 0x70);
const METAL: Color32 = Color32::from_rgb(0xA9, 0xB1, 0xBD);
const GOLD: Color32 = Color32::from_rgb(0xC9, 0xA2, 0x4A);
const CABLE: Color32 = Color32::from_rgb(0x4A, 0x52, 0x60);

/// Turns per second shown for a fan at `rpm`. Much slower than real (1000 RPM shows as half a
/// turn per second), so the motion is calm and the blades stay visible.
fn turns_per_second(rpm: f32) -> f32 {
    (rpm / 2000.0).min(1.5)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

/// Accent of the card's light strip: cool at idle, through the temperature colour, to red.
fn heat_color(temp: Option<f32>, p: &Palette) -> Color32 {
    let Some(temp) = temp else { return p.weak };
    let heat = ((temp - 40.0) / 45.0).clamp(0.0, 1.0);
    if heat < 0.5 { mix(p.accent, p.temp, heat * 2.0) } else { mix(p.temp, p.danger, heat * 2.0 - 1.0) }
}

/// Maps the design space onto the allocated rectangle.
struct Canvas {
    painter: egui::Painter,
    origin: Pos2,
    scale: f32,
}

impl Canvas {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y) * self.scale
    }

    fn rect(&self, x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(self.at(x, y), vec2(width, height) * self.scale)
    }

    fn len(&self, length: f32) -> f32 {
        length * self.scale
    }

    fn radius(&self, radius: f32) -> CornerRadius {
        CornerRadius::same(self.len(radius).round().clamp(0.0, 255.0) as u8)
    }

    fn font(&self, size: f32) -> FontId {
        FontId::proportional(self.len(size).max(9.0))
    }
}

/// A fan seen from the front: well, swept blades at `angle`, ring and hub.
fn fan(c: &Canvas, center: Pos2, radius: f32, angle: f32, blades: usize, hub: Color32) {
    let painter = &c.painter;
    let r = c.len(radius);
    painter.circle_filled(center, r, WELL);
    let polar = |share: f32, a: f32| center + vec2(a.cos(), a.sin()) * r * share;
    let edge = Stroke::new(c.len(1.0), BLADE_EDGE);
    for blade in 0..blades {
        let a = angle + blade as f32 * TAU / blades as f32;
        // Two quads per blade, so each stays convex while the blade sweeps back.
        let (inner_lead, inner_trail) = (polar(0.30, a), polar(0.30, a + 0.55));
        let (mid_lead, mid_trail) = (polar(0.63, a + 0.30), polar(0.63, a + 0.80));
        let (outer_lead, outer_trail) = (polar(0.93, a + 0.66), polar(0.93, a + 1.02));
        painter.add(Shape::convex_polygon(vec![inner_lead, mid_lead, mid_trail, inner_trail], BLADE, Stroke::NONE));
        painter.add(Shape::convex_polygon(vec![mid_lead, outer_lead, outer_trail, mid_trail], BLADE, Stroke::NONE));
        painter.line_segment([inner_lead, mid_lead], edge);
        painter.line_segment([mid_lead, outer_lead], edge);
    }
    painter.circle_stroke(center, r, Stroke::new(c.len(2.0), BODY_EDGE));
    painter.circle_filled(center, r * 0.30, PANEL);
    painter.circle_stroke(center, r * 0.30, Stroke::new(c.len(1.5), BODY_EDGE));
    painter.circle_stroke(center, r * 0.17, Stroke::new(c.len(2.0), hub));
}

/// A case fan: square frame with screw holes around a fan.
fn case_fan(c: &Canvas, x: f32, y: f32, angle: f32, hub: Color32) {
    let painter = &c.painter;
    const HALF: f32 = 54.0;
    let frame = c.rect(x - HALF, y - HALF, HALF * 2.0, HALF * 2.0);
    painter.rect_filled(frame, c.radius(10.0), BODY);
    painter.rect_stroke(frame, c.radius(10.0), Stroke::new(c.len(1.5), BODY_EDGE), StrokeKind::Inside);
    for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        painter.circle_filled(c.at(x + dx * (HALF - 8.0), y + dy * (HALF - 8.0)), c.len(3.0), WELL);
    }
    fan(c, c.at(x, y), 48.0, angle, 7, hub);
}

/// A 4-pin fan header sticking out of the card's end, with the plug of the cable on it.
fn header(c: &Canvas, y: f32, connected: bool) {
    let painter = &c.painter;
    let socket = c.rect(500.0, y - 13.0, 12.0, 26.0);
    painter.rect_filled(socket, c.radius(2.0), Color32::from_rgb(0xD8, 0xDC, 0xE2));
    for pin in 0..4 {
        let pin_y = y - 9.0 + pin as f32 * 6.0;
        painter.line_segment([c.at(512.0, pin_y), c.at(520.0, pin_y)], Stroke::new(c.len(1.5), GOLD));
    }
    if connected {
        painter.rect_filled(c.rect(514.0, y - 12.0, 13.0, 24.0), c.radius(2.0), WELL);
        painter.rect_stroke(c.rect(514.0, y - 12.0, 13.0, 24.0), c.radius(2.0), Stroke::new(c.len(1.0), CABLE), StrokeKind::Inside);
    }
}

/// The fan cable from a header to its case fan.
fn cable(c: &Canvas, from_y: f32, to_y: f32) {
    let painter = &c.painter;
    let points = [c.at(527.0, from_y), c.at(575.0, from_y), c.at(560.0, to_y), c.at(606.0, to_y)];
    painter.add(CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::new(c.len(4.0), CABLE)));
}

/// A small overlay tile: caption above a value with its unit.
fn tile(c: &Canvas, p: &Palette, (x, y): (f32, f32), caption: &str, value: &str, unit: &str, color: Color32) {
    let painter = &c.painter;
    let rect = c.rect(x, y, 122.0, 50.0);
    painter.rect_filled(rect.translate(vec2(0.0, c.len(2.0))), c.radius(9.0), p.shadow);
    painter.rect_filled(rect, c.radius(9.0), p.card);
    painter.rect_stroke(rect, c.radius(9.0), Stroke::new(1.0, p.card_stroke), StrokeKind::Inside);
    painter.text(c.at(x + 11.0, y + 8.0), Align2::LEFT_TOP, caption, c.font(10.5), p.weak);
    let value_rect = painter.text(c.at(x + 11.0, y + 43.0), Align2::LEFT_BOTTOM, value, c.font(21.0), color);
    painter.text(value_rect.right_bottom() + vec2(c.len(4.0), -c.len(2.5)), Align2::LEFT_BOTTOM, unit, c.font(11.5), color);
}

pub fn show(ui: &mut egui::Ui, live: &Live, calibration: Option<&Calibration>) {
    let p = Palette::current(ui.ctx());
    let scale = (ui.available_width() / DESIGN.x).min(MAX_SCALE);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), DESIGN.y * scale), egui::Sense::hover());
    let c = Canvas { painter: ui.painter_at(rect), origin: pos2(rect.center().x - DESIGN.x * scale / 2.0, rect.top()), scale };
    let painter = &c.painter;
    let s = live.latest.as_ref();

    // Fan angles live in egui's memory and advance with the measured speeds.
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let speeds = [
        // The card's own fans: NVML only gives percent; about 3000 RPM at 100 %.
        s.and_then(|s| s.gpu_fan).map_or(0.0, |percent| percent as f32 * 30.0),
        s.map_or(0.0, |s| s.fan1_rpm as f32),
        s.map_or(0.0, |s| s.fan2_rpm as f32),
    ];
    let id = ui.id().with("card-art-angles");
    let angles = ui.ctx().data_mut(|data| {
        let angles: &mut [f32; 3] = data.get_temp_mut_or_default(id);
        for (angle, rpm) in angles.iter_mut().zip(speeds) {
            *angle = (*angle + turns_per_second(rpm) * TAU * dt) % TAU;
        }
        *angles
    });
    if speeds.iter().any(|&rpm| rpm > 0.0) {
        ui.ctx().request_repaint();
    }

    let heat = heat_color(s.and_then(|s| s.gpu_temp), &p);
    let thin = Stroke::new(c.len(1.5), BODY_EDGE);

    // Slot bracket with its ports, and the PCIe edge connector.
    painter.rect_filled(c.rect(20.0, 78.0, 12.0, 196.0), c.radius(2.0), METAL);
    for port in 0..4 {
        painter.rect_filled(c.rect(23.0, 104.0 + port as f32 * 38.0, 6.0, 22.0), c.radius(1.0), WELL);
    }
    painter.rect_filled(c.rect(74.0, 246.0, 28.0, 12.0), CornerRadius::ZERO, GOLD);
    painter.rect_filled(c.rect(108.0, 246.0, 150.0, 12.0), CornerRadius::ZERO, GOLD);

    // Power plugs on the top edge.
    for plug in 0..2 {
        painter.rect_filled(c.rect(392.0 + plug as f32 * 44.0, 90.0, 38.0, 12.0), c.radius(2.0), WELL);
    }

    // Shroud, with the light strip along its top and the angular panels of the Strix design.
    let body = c.rect(32.0, 98.0, 468.0, 150.0);
    painter.rect_filled(body, c.radius(12.0), BODY);
    painter.rect_stroke(body, c.radius(12.0), thin, StrokeKind::Inside);
    for (x, lean) in [(190.0, 14.0), (345.0, -14.0)] {
        let panel = vec![c.at(x - 10.0 + lean, 104.0), c.at(x + 10.0 + lean, 104.0), c.at(x + 10.0 - lean, 242.0), c.at(x - 10.0 - lean, 242.0)];
        painter.add(Shape::convex_polygon(panel, PANEL, Stroke::NONE));
    }
    painter.line_segment([c.at(52.0, 105.0), c.at(372.0, 105.0)], Stroke::new(c.len(3.0), heat));
    painter.line_segment([c.at(52.0, 241.0), c.at(480.0, 241.0)], Stroke::new(c.len(1.5), heat.gamma_multiply(0.5)));
    for x in [112.0, 267.0, 422.0] {
        fan(&c, c.at(x, 173.0), 60.0, angles[0], 9, heat);
    }

    // FanConnect II: two headers on the card's end, each with a cable to a case fan.
    let connected = [speeds[1] > 0.0, speeds[2] > 0.0];
    for (index, (header_y, fan_y)) in [(140.0, 96.0), (206.0, 250.0)].into_iter().enumerate() {
        let hub = if connected[index] { p.accent } else { p.weak };
        if connected[index] {
            cable(&c, header_y, fan_y);
        }
        header(&c, header_y, connected[index]);
        case_fan(&c, 660.0, fan_y, angles[index + 1], hub);
    }
    painter.text(c.at(514.0, 173.0), Align2::LEFT_CENTER, "FanConnect II", c.font(11.0), p.weak);

    // Live values.
    let temp = s.and_then(|s| s.gpu_temp).map_or("–".into(), |t| format!("{t:.0}"));
    let usage = s.and_then(|s| s.gpu_usage).map_or("–".into(), |u| u.to_string());
    let duty = s.map_or("–".into(), |s| format!("{:.0}", calibration.map_or(s.duty, |c| c.speed_at(s.duty))));
    let duty_caption = if calibration.is_some() { "FAN SPEED" } else { "FAN DUTY" };
    tile(&c, &p, (44.0, 18.0), "GPU TEMPERATURE", &temp, "°C", p.temp);
    tile(&c, &p, (176.0, 18.0), "GPU USAGE", &usage, "%", p.text);
    tile(&c, &p, (308.0, 18.0), duty_caption, &duty, "%", p.accent);
    for (index, y) in [(1, 71.0), (2, 225.0)] {
        let rpm = s.map_or("–".into(), |_| format!("{:.0}", speeds[index]));
        tile(&c, &p, (726.0, y), &format!("FAN {index}"), &rpm, "RPM", p.text);
    }
}
