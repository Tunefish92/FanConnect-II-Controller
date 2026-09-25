//! Fan curves (the built-in Auto curve or a custom one) and the smoothing controller.
//! See DESIGN.md.

use std::ops::RangeInclusive;

/// The Auto curve starts here; at or below it the fans run at the floor duty.
pub const IDLE_TEMP: f32 = 45.0;

/// Lowest duty ever used, in percent. Fans are never stopped.
pub const FLOOR_DUTY: f32 = 30.0;

/// Duty used when the GPU temperature cannot be read or the daemon stops.
pub const FAILSAFE_DUTY: f32 = 100.0;

/// The duty only falls once the temperature is this far below the one that set it.
pub const HYSTERESIS: f32 = 3.0;

/// Maximum duty decrease per second while falling, in percent.
pub const RAMP_DOWN_PER_SEC: f32 = 2.0;

/// Limits for custom curves.
pub const CUSTOM_POINTS: RangeInclusive<usize> = 2..=16;
pub const CUSTOM_TEMPS: RangeInclusive<f32> = 20.0..=100.0;

/// Auto curve points as (fraction of the span IDLE_TEMP..max_temp, duty %).
const AUTO_POINTS: [(f32, f32); 7] = [
    (0.00, 30.0),
    (0.25, 35.0),
    (0.40, 45.0),
    (0.55, 55.0),
    (0.70, 70.0),
    (0.85, 85.0),
    (1.00, 100.0),
];

/// Linear interpolation over (x, duty) points, clamped to the first and last duty.
fn interpolate(points: &[(f32, f32)], x: f32) -> f32 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for pair in points.windows(2) {
        let ((x0, d0), (x1, d1)) = (pair[0], pair[1]);
        if x <= x1 {
            return d0 + (d1 - d0) * (x - x0) / (x1 - x0);
        }
    }
    points[points.len() - 1].1
}

/// Auto curve duty in percent for `temp`, with 100 % reached at `max_temp`.
pub fn auto_duty(temp: f32, max_temp: u32) -> f32 {
    interpolate(&AUTO_POINTS, (temp - IDLE_TEMP) / (max_temp as f32 - IDLE_TEMP))
}

/// The Auto curve's points as (temperature °C, duty %) for `max_temp`.
pub fn auto_points(max_temp: u32) -> Vec<(f32, f32)> {
    let span = max_temp as f32 - IDLE_TEMP;
    AUTO_POINTS.iter().map(|&(x, duty)| (IDLE_TEMP + x * span, duty)).collect()
}

/// The active fan curve.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// The built-in curve, scaled so 100 % is reached at max temp.
    Auto,
    /// User-defined (temperature °C, duty %) points; see `Curve::custom`.
    Custom(Vec<(f32, f32)>),
}

impl Curve {
    /// Validates user-defined points: 2-16 points, temperatures 20-100 °C and strictly rising,
    /// duty 30-100 % and never falling.
    pub fn custom(points: Vec<(f32, f32)>) -> Result<Self, String> {
        if !CUSTOM_POINTS.contains(&points.len()) {
            return Err(format!(
                "a curve needs {} to {} points, got {}",
                CUSTOM_POINTS.start(),
                CUSTOM_POINTS.end(),
                points.len()
            ));
        }
        for &(temp, duty) in &points {
            if !CUSTOM_TEMPS.contains(&temp) {
                return Err(format!(
                    "temperature {temp} °C is outside {}..={} °C",
                    CUSTOM_TEMPS.start(),
                    CUSTOM_TEMPS.end()
                ));
            }
            if !(FLOOR_DUTY..=100.0).contains(&duty) {
                return Err(format!("duty {duty} % is outside {FLOOR_DUTY}..=100 %"));
            }
        }
        for pair in points.windows(2) {
            let ((t0, d0), (t1, d1)) = (pair[0], pair[1]);
            if t1 <= t0 {
                return Err(format!("temperatures must rise: {t1} °C follows {t0} °C"));
            }
            if d1 < d0 {
                return Err(format!("duty must not fall: {d1} % at {t1} °C follows {d0} % at {t0} °C"));
            }
        }
        Ok(Curve::Custom(points))
    }

    /// Curve duty in percent for `temp`. The max temp override is applied by `Controller`.
    pub fn duty(&self, temp: f32, max_temp: u32) -> f32 {
        match self {
            Curve::Auto => auto_duty(temp, max_temp),
            Curve::Custom(points) => interpolate(points, temp),
        }
    }

    /// The curve's points as (temperature °C, duty %).
    pub fn points(&self, max_temp: u32) -> Vec<(f32, f32)> {
        match self {
            Curve::Auto => auto_points(max_temp),
            Curve::Custom(points) => points.clone(),
        }
    }
}

/// Duty percent to the FanConnect duty register value (0-255), as GPU Tweak III does.
pub fn duty_to_reg(percent: f32) -> u8 {
    (percent.clamp(0.0, 100.0) * 255.0 / 100.0).round() as u8
}

/// FanConnect duty register value (0-255) to percent.
pub fn reg_to_duty(value: u8) -> f32 {
    value as f32 * 100.0 / 255.0
}

/// Applies the curve with hysteresis and a slow ramp down, and forces 100 % at max temp.
///
/// The effective temperature follows rising temperatures immediately and falling ones only once
/// they are more than HYSTERESIS below it, trailing them by HYSTERESIS. The duty rises
/// immediately and falls by at most RAMP_DOWN_PER_SEC.
pub struct Controller {
    max_temp: u32,
    curve: Curve,
    effective_temp: Option<f32>,
    duty: Option<f32>,
}

impl Controller {
    pub fn new(max_temp: u32, curve: Curve) -> Self {
        Self { max_temp, curve, effective_temp: None, duty: None }
    }

    pub fn configure(&mut self, max_temp: u32, curve: Curve) {
        self.max_temp = max_temp;
        self.curve = curve;
    }

    /// Advances by `dt` seconds with the current GPU temperature (`None` if it could not be
    /// read) and returns the duty to write, in percent.
    pub fn update(&mut self, temp: Option<f32>, dt: f32) -> f32 {
        let Some(temp) = temp else {
            self.effective_temp = None;
            return *self.duty.insert(FAILSAFE_DUTY);
        };
        if temp >= self.max_temp as f32 {
            self.effective_temp = Some(temp);
            return *self.duty.insert(100.0);
        }

        let effective = match self.effective_temp {
            Some(e) if temp < e - HYSTERESIS => temp + HYSTERESIS,
            Some(e) if temp <= e => e,
            _ => temp,
        };
        self.effective_temp = Some(effective);

        let target = self.curve.duty(effective, self.max_temp);
        let duty = match self.duty {
            Some(current) if target < current => (current - RAMP_DOWN_PER_SEC * dt).max(target),
            _ => target,
        };
        *self.duty.insert(duty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    fn auto(max_temp: u32) -> Controller {
        Controller::new(max_temp, Curve::Auto)
    }

    fn custom() -> Curve {
        Curve::custom(vec![(40.0, 30.0), (60.0, 50.0), (70.0, 80.0), (85.0, 100.0)]).unwrap()
    }

    #[test]
    fn auto_curve_matches_design_table_at_default_max() {
        let expected = [(45.0, 30.0), (53.75, 35.0), (59.0, 45.0), (64.25, 55.0), (69.5, 70.0), (74.75, 85.0), (80.0, 100.0)];
        for ((t, d), (et, ed)) in auto_points(80).into_iter().zip(expected) {
            assert!(close(t, et) && close(d, ed), "({t}, {d}) != ({et}, {ed})");
            assert!(close(auto_duty(t, 80), d));
        }
    }

    #[test]
    fn auto_curve_scales_with_max_temp() {
        assert!(close(auto_duty(55.0, 85), 35.0));
        assert!(close(auto_duty(85.0, 85), 100.0));
        assert!(auto_duty(80.0, 85) < 100.0);
    }

    #[test]
    fn auto_curve_clamps_to_floor_and_full() {
        assert_eq!(auto_duty(20.0, 80), FLOOR_DUTY);
        assert_eq!(auto_duty(45.0, 80), FLOOR_DUTY);
        assert_eq!(auto_duty(95.0, 80), 100.0);
    }

    #[test]
    fn auto_curve_interpolates_linearly() {
        // Halfway between 69.5 °C (70 %) and 74.75 °C (85 %).
        assert!(close(auto_duty(72.125, 80), 77.5));
    }

    #[test]
    fn custom_curve_interpolates_and_clamps() {
        let c = custom();
        assert_eq!(c.duty(20.0, 90), 30.0);
        assert!(close(c.duty(50.0, 90), 40.0));
        assert!(close(c.duty(65.0, 90), 65.0));
        assert_eq!(c.duty(95.0, 90), 100.0);
    }

    #[test]
    fn custom_curve_validation() {
        assert!(Curve::custom(vec![(50.0, 40.0)]).is_err(), "one point");
        assert!(Curve::custom((0..17).map(|i| (20.0 + i as f32, 50.0)).collect()).is_err(), "17 points");
        assert!(Curve::custom(vec![(50.0, 40.0), (50.0, 60.0)]).is_err(), "temps not rising");
        assert!(Curve::custom(vec![(50.0, 60.0), (60.0, 40.0)]).is_err(), "duty falling");
        assert!(Curve::custom(vec![(50.0, 20.0), (60.0, 40.0)]).is_err(), "below floor");
        assert!(Curve::custom(vec![(50.0, 40.0), (60.0, 110.0)]).is_err(), "above 100 %");
        assert!(Curve::custom(vec![(10.0, 40.0), (60.0, 50.0)]).is_err(), "temp too low");
        assert!(Curve::custom(vec![(50.0, 40.0), (60.0, 40.0)]).is_ok(), "flat is fine");
    }

    #[test]
    fn register_values_match_gpu_tweak_capture() {
        assert_eq!(duty_to_reg(30.0), 0x4D);
        assert_eq!(duty_to_reg(60.0), 0x99);
        assert_eq!(duty_to_reg(100.0), 0xFF);
        assert_eq!(duty_to_reg(150.0), 0xFF);
        assert!(close(reg_to_duty(0x99), 60.0));
    }

    #[test]
    fn first_reading_applies_curve_directly() {
        let mut c = auto(80);
        assert!(close(c.update(Some(70.0), 1.0), auto_duty(70.0, 80)));
    }

    #[test]
    fn rising_temperature_raises_duty_immediately() {
        let mut c = auto(80);
        c.update(Some(50.0), 1.0);
        assert!(close(c.update(Some(75.0), 1.0), auto_duty(75.0, 80)));
    }

    #[test]
    fn small_dips_inside_hysteresis_keep_duty() {
        let mut c = auto(80);
        let d = c.update(Some(70.0), 1.0);
        assert_eq!(c.update(Some(68.0), 1.0), d);
        assert_eq!(c.update(Some(67.0), 1.0), d);
        assert_eq!(c.update(Some(69.0), 1.0), d);
    }

    #[test]
    fn falling_temperature_ramps_down_slowly() {
        let mut c = auto(80);
        c.update(Some(79.0), 1.0);
        let start = c.update(Some(79.0), 1.0);
        let d1 = c.update(Some(40.0), 1.0);
        assert!(close(d1, start - RAMP_DOWN_PER_SEC));
        let mut d = d1;
        for _ in 0..100 {
            d = c.update(Some(40.0), 1.0);
        }
        // Settles on the curve value for 40 + HYSTERESIS, which is the floor.
        assert!(close(d, FLOOR_DUTY));
    }

    #[test]
    fn ramp_scales_with_elapsed_time() {
        let mut c = auto(80);
        let start = c.update(Some(79.0), 1.0);
        assert!(close(c.update(Some(40.0), 2.5), start - 2.5 * RAMP_DOWN_PER_SEC));
    }

    #[test]
    fn at_or_above_max_temp_goes_full_immediately() {
        let mut c = auto(80);
        c.update(Some(50.0), 1.0);
        assert_eq!(c.update(Some(80.0), 1.0), 100.0);
    }

    #[test]
    fn max_temp_overrides_custom_curve() {
        let mut c = Controller::new(75, custom());
        assert!(close(c.update(Some(70.0), 1.0), 80.0));
        assert_eq!(c.update(Some(75.0), 1.0), 100.0);
    }

    #[test]
    fn configure_switches_curve() {
        let mut c = auto(80);
        assert!(close(c.update(Some(40.0), 1.0), FLOOR_DUTY));
        c.configure(90, Curve::custom(vec![(30.0, 60.0), (90.0, 100.0)]).unwrap());
        assert!(close(c.update(Some(40.0), 1.0), 60.0 + 40.0 * 10.0 / 60.0));
    }

    #[test]
    fn unreadable_temperature_uses_failsafe_then_ramps_down() {
        let mut c = auto(80);
        c.update(Some(50.0), 1.0);
        assert_eq!(c.update(None, 1.0), FAILSAFE_DUTY);
        assert!(close(c.update(Some(50.0), 1.0), FAILSAFE_DUTY - RAMP_DOWN_PER_SEC));
    }

    #[test]
    fn duty_never_drops_below_floor() {
        let mut c = auto(80);
        for t in [60.0, 30.0, 10.0, 0.0] {
            assert!(c.update(Some(t), 60.0) >= FLOOR_DUTY);
        }
    }
}
