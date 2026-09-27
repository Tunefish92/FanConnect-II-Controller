//! Fan calibration: measures the RPM the connected fans reach at each duty, so fan curve
//! percentages can mean fan speed (percent of the highest RPM) instead of PWM duty. See DESIGN.md.

use crate::curve::{FLOOR_DUTY, interpolate};

/// Duty steps of a calibration run, from full speed down, so the highest RPM is measured first
/// and the fans only slow down from there.
pub const STEPS: [f32; 8] = [100.0, 90.0, 80.0, 70.0, 60.0, 50.0, 40.0, 30.0];

/// Readings (one a second) at a step before its RPM can count, while the fans change speed.
const MIN_SETTLE: u32 = 5;
/// Readings in a row within STABLE_RPM of the one before that count as settled.
const STABLE_READINGS: u32 = 2;
/// A step's RPM counts after this many readings even if it is still moving.
const MAX_SETTLE: u32 = 15;
/// Readings this close to the one before count as stable: one tach unit.
const STABLE_RPM: u32 = 30;

/// Measured (duty %, RPM) points, duty rising from FLOOR_DUTY to 100 %, RPM never falling.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    points: Vec<(f32, f32)>,
}

impl Calibration {
    /// Validates measured points: at least 2, duty strictly rising from FLOOR_DUTY to 100 %,
    /// RPM never falling and above 0 at 100 %.
    pub fn new(points: Vec<(f32, f32)>) -> Result<Self, String> {
        if points.len() < 2 {
            return Err(format!("calibration needs at least 2 duty:rpm points, got {}", points.len()));
        }
        let (first, last) = (points[0], points[points.len() - 1]);
        if first.0 != FLOOR_DUTY || last.0 != 100.0 {
            return Err(format!("calibration must run from {FLOOR_DUTY} % to 100 % duty"));
        }
        for pair in points.windows(2) {
            let ((d0, r0), (d1, r1)) = (pair[0], pair[1]);
            if d1 <= d0 {
                return Err(format!("calibration duty must rise: {d1} % follows {d0} %"));
            }
            if r1 < r0 {
                return Err(format!("calibration RPM must not fall: {r1} RPM at {d1} % follows {r0} RPM at {d0} %"));
            }
        }
        if first.1 < 0.0 || last.1 <= 0.0 {
            return Err("calibration RPM must be above 0 at 100 % duty".into());
        }
        Ok(Self { points })
    }

    /// The measured (duty %, RPM) points.
    pub fn points(&self) -> &[(f32, f32)] {
        &self.points
    }

    /// The highest RPM, measured at 100 % duty.
    pub fn max_rpm(&self) -> f32 {
        self.points[self.points.len() - 1].1
    }

    /// Expected RPM at `duty` percent.
    pub fn rpm_at(&self, duty: f32) -> f32 {
        interpolate(&self.points, duty)
    }

    /// Fan speed in percent of the highest RPM at `duty` percent.
    pub fn speed_at(&self, duty: f32) -> f32 {
        self.rpm_at(duty) * 100.0 / self.max_rpm()
    }

    /// The lowest speed the fans run at: the speed at the floor duty.
    pub fn floor_speed(&self) -> f32 {
        self.speed_at(FLOOR_DUTY)
    }

    /// The duty that runs the fans at `speed` percent of the highest RPM. Speeds below the
    /// floor speed give the floor duty.
    pub fn duty_for(&self, speed: f32) -> f32 {
        let rpm = speed.clamp(0.0, 100.0) * self.max_rpm() / 100.0;
        if rpm <= self.points[0].1 {
            return self.points[0].0;
        }
        for pair in self.points.windows(2) {
            let ((d0, r0), (d1, r1)) = (pair[0], pair[1]);
            // r1 > r0 here: rpm is above r0, and a flat segment would have returned already.
            if rpm <= r1 {
                return d0 + (d1 - d0) * (rpm - r0) / (r1 - r0);
            }
        }
        100.0
    }
}

/// A calibration run in progress. The caller writes `duty()`, reads the fans about a second
/// later and passes the RPMs to `observe`, until it returns the result.
#[derive(Clone, Debug, Default)]
pub struct Sweep {
    step: usize,
    readings: u32,
    last: Option<u32>,
    stable: u32,
    /// Which fans report an RPM at 100 %; only those are measured.
    fans: [bool; 2],
    /// (duty, RPM) in sweep order, so falling duty.
    points: Vec<(f32, f32)>,
}

impl Sweep {
    pub fn new() -> Self {
        Self::default()
    }

    /// The duty to write now, in percent.
    pub fn duty(&self) -> f32 {
        STEPS[self.step.min(STEPS.len() - 1)]
    }

    /// Share of the steps done, 0 to 1.
    pub fn progress(&self) -> f32 {
        self.step as f32 / STEPS.len() as f32
    }

    /// The last measured (duty %, RPM) point.
    pub fn last_measured(&self) -> Option<(f32, f32)> {
        self.points.last().copied()
    }

    /// Average RPM of the fans found at 100 %.
    fn average(&self, fan1: u32, fan2: u32) -> u32 {
        let measured: Vec<u32> = [fan1, fan2].into_iter().zip(self.fans).filter(|&(_, on)| on).map(|(rpm, _)| rpm).collect();
        measured.iter().sum::<u32>() / measured.len().max(1) as u32
    }

    /// Takes one reading of both fans' RPM. Returns the result once the last step is measured.
    pub fn observe(&mut self, fan1: u32, fan2: u32) -> Option<Result<Calibration, String>> {
        self.readings += 1;
        // At the first step the connected fans aren't known yet: the faster one shows settling.
        let rpm = if self.step == 0 { fan1.max(fan2) } else { self.average(fan1, fan2) };
        self.stable = if self.last.is_some_and(|last| last.abs_diff(rpm) <= STABLE_RPM) { self.stable + 1 } else { 0 };
        self.last = Some(rpm);
        let settled = self.readings >= MAX_SETTLE || (self.readings >= MIN_SETTLE && self.stable >= STABLE_READINGS);
        if !settled {
            return None;
        }
        if self.step == 0 {
            self.fans = [fan1 > 0, fan2 > 0];
            if rpm == 0 {
                return Some(Err("no fan reports an RPM at 100 % duty; are fans connected?".into()));
            }
        }
        // Slower duty never gives more RPM; a reading above the last one is noise.
        let rpm = self.average(fan1, fan2) as f32;
        let rpm = self.points.last().map_or(rpm, |&(_, previous)| rpm.min(previous));
        self.points.push((STEPS[self.step], rpm));
        self.step += 1;
        self.readings = 0;
        self.last = None;
        self.stable = 0;
        if self.step < STEPS.len() {
            return None;
        }
        let mut points = self.points.clone();
        points.reverse();
        Some(Calibration::new(points))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    /// The fans measured in HARDWARE.md: 750 RPM at 30 %, 1410 at 60 %, 2160 at 100 %.
    fn measured() -> Calibration {
        Calibration::new(vec![(30.0, 750.0), (60.0, 1410.0), (100.0, 2160.0)]).unwrap()
    }

    #[test]
    fn speed_is_percent_of_highest_rpm() {
        let c = measured();
        assert_eq!(c.max_rpm(), 2160.0);
        assert!(close(c.speed_at(100.0), 100.0));
        assert!(close(c.speed_at(60.0), 1410.0 * 100.0 / 2160.0));
        assert!(close(c.floor_speed(), 750.0 * 100.0 / 2160.0));
        assert!(close(c.speed_at(10.0), c.floor_speed()), "clamped below the floor");
    }

    #[test]
    fn duty_for_inverts_speed_at() {
        let c = measured();
        for duty in [30.0, 42.5, 60.0, 77.0, 100.0] {
            assert!(close(c.duty_for(c.speed_at(duty)), duty), "{duty}");
        }
        assert_eq!(c.duty_for(20.0), FLOOR_DUTY, "below the floor speed");
        assert_eq!(c.duty_for(100.0), 100.0);
        assert_eq!(c.duty_for(150.0), 100.0);
    }

    #[test]
    fn flat_segments_give_the_lowest_duty() {
        let c = Calibration::new(vec![(30.0, 900.0), (40.0, 900.0), (100.0, 1800.0)]).unwrap();
        assert_eq!(c.duty_for(50.0), 30.0);
        assert!(close(c.duty_for(75.0), 40.0 + 60.0 * 450.0 / 900.0));
    }

    #[test]
    fn validation() {
        assert!(Calibration::new(vec![(100.0, 2000.0)]).is_err(), "one point");
        assert!(Calibration::new(vec![(40.0, 900.0), (100.0, 2000.0)]).is_err(), "not from the floor");
        assert!(Calibration::new(vec![(30.0, 900.0), (90.0, 2000.0)]).is_err(), "not to 100 %");
        assert!(Calibration::new(vec![(30.0, 900.0), (30.0, 950.0), (100.0, 2000.0)]).is_err(), "duty not rising");
        assert!(Calibration::new(vec![(30.0, 900.0), (60.0, 800.0), (100.0, 2000.0)]).is_err(), "RPM falling");
        assert!(Calibration::new(vec![(30.0, 0.0), (100.0, 0.0)]).is_err(), "no RPM");
    }

    /// Feeds readings until the step changes, with the fans at `rpm` from the second reading.
    fn run_step(sweep: &mut Sweep, fan1: u32, fan2: u32) -> Option<Result<Calibration, String>> {
        let step = sweep.step;
        let mut readings = 0;
        // First reading still on the way to the new speed.
        let mut result = sweep.observe(fan1 + 300, fan2 + 300);
        while result.is_none() && sweep.step == step {
            readings += 1;
            assert!(readings <= MAX_SETTLE, "step never settled");
            result = sweep.observe(fan1, fan2);
        }
        result
    }

    #[test]
    fn sweep_measures_every_step_from_full_speed_down() {
        let mut sweep = Sweep::new();
        let mut result = None;
        for (i, duty) in STEPS.into_iter().enumerate() {
            assert_eq!(sweep.duty(), duty);
            assert!(close(sweep.progress(), i as f32 / STEPS.len() as f32));
            let rpm = duty as u32 * 216 / 10;
            // Fan 2 is not connected.
            result = run_step(&mut sweep, rpm, 0);
        }
        let c = result.unwrap().unwrap();
        assert_eq!(c.points().len(), STEPS.len());
        assert_eq!(c.points()[0], (30.0, 648.0));
        assert_eq!(c.max_rpm(), 2160.0);
    }

    #[test]
    fn sweep_averages_connected_fans_and_ignores_rises() {
        let mut sweep = Sweep::new();
        assert!(run_step(&mut sweep, 2160, 2100).is_none());
        assert_eq!(sweep.points[0], (100.0, 2130.0));
        // A noisy reading above the previous step is capped.
        assert!(run_step(&mut sweep, 2200, 2200).is_none());
        assert_eq!(sweep.points[1], (90.0, 2130.0));
    }

    #[test]
    fn sweep_fails_without_fans() {
        let mut sweep = Sweep::new();
        let mut result = None;
        for _ in 0..MAX_SETTLE {
            result = sweep.observe(0, 0);
        }
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn unsettled_step_counts_after_the_limit() {
        let mut sweep = Sweep::new();
        for i in 0..MAX_SETTLE - 1 {
            assert!(sweep.observe(1000 + i * 100, 0).is_none());
        }
        assert_eq!(sweep.step, 0);
        sweep.observe(3000, 0);
        assert_eq!(sweep.step, 1);
    }
}
