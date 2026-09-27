//! The control loop: read the GPU temperature once a second, apply the curve, write the
//! duty. On exit, for any reason, the fans are set to the fail-safe duty. See DESIGN.md.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};

use crate::calibration::{self, Calibration, Sweep};
use crate::config::{self, Config};
use crate::curve::{self, Controller, Curve, FAILSAFE_DUTY, FLOOR_DUTY};
use crate::fanconnect::{FanConnect, MODE_HOST};
use crate::gpu::Gpu;
use crate::log;
use crate::status::{self, DaemonStatus};

const POLL: Duration = Duration::from_secs(1);
const GPU_RETRY: Duration = Duration::from_secs(5);
const MAX_I2C_FAILURES: u32 = 10;
const LOG_DUTY_STEP: f32 = 5.0;
/// How often "another program is writing the duty" is repeated while it keeps happening.
const OTHER_WRITER_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Runs until `stop` is set. Always tries to leave the fans at the fail-safe duty.
pub fn run(stop: &AtomicBool) -> Result<()> {
    let fc = FanConnect::open()?;
    log!("FanConnect II found ({})", fc.describe());
    if let Some(name) = other_controller_running() {
        log!("warning: {name} is running. If its external fan control is active, it overrides the duty every second");
    }

    let result = control_loop(&fc, stop);
    status::remove();

    log!("stopping: setting fail-safe duty {FAILSAFE_DUTY} %");
    if let Err(e) = fc.take_control().and_then(|()| fc.set_duty(FAILSAFE_DUTY)) {
        log!("could not set fail-safe duty: {e:#}");
    }
    result
}

/// A known program that writes the FanConnect duty itself, if one is running.
pub fn other_controller_running() -> Option<&'static str> {
    #[cfg(windows)]
    if crate::winproc::is_running("ASUSGPUFanServiceEx.exe") {
        return Some("GPU Tweak III's external fan service (ASUSGPUFanServiceEx.exe)");
    }
    None
}

fn load_config() -> Config {
    let (config, warning) = config::load(&config::path());
    if let Some(w) = warning {
        log!("warning: {w}");
    }
    config
}

fn describe(config: &Config) -> String {
    let curve = match config.curve {
        Curve::Auto => format!("Auto curve, max temp {} °C", config.max_temp),
        Curve::Custom(_) => format!("custom curve {}, max temp {} °C", config::format_curve(&config.curve), config.max_temp),
    };
    match &config.calibration {
        Some(c) => format!("{curve}, fan speed calibrated to {:.0} RPM", c.max_rpm()),
        None => format!("{curve}, uncalibrated (curve percentages are duty)"),
    }
}

/// One line about a calibration, e.g. "highest 2160 RPM, lowest 35 % (750 RPM at 30 % duty)".
pub fn summarize(calibration: &Calibration) -> String {
    format!(
        "highest {:.0} RPM, lowest {:.0} % ({:.0} RPM at {FLOOR_DUTY} % duty)",
        calibration.max_rpm(),
        calibration.floor_speed(),
        calibration.rpm_at(FLOOR_DUTY)
    )
}

/// Ends a calibration run: saves a successful one to the settings file and logs the outcome.
/// Returns the outcome for the status, and the settings as saved.
fn finish_calibration(result: Result<Calibration, String>) -> (Result<String, String>, Option<Config>) {
    let saved = result.and_then(|calibration| {
        let summary = summarize(&calibration);
        config::modify(&config::path(), |c| {
            c.calibration = Some(calibration);
            Ok(())
        })
        .map(|config| (summary, config))
    });
    match saved {
        Ok((summary, config)) => {
            log!("calibration done: {summary}");
            (Ok(summary), Some(config))
        }
        Err(e) => {
            log!("calibration failed: {e}");
            (Err(e), None)
        }
    }
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(config::path()).and_then(|m| m.modified()).ok()
}

/// Opens NVML, logging the result. `None` means the fail-safe duty is used until a retry works.
fn open_gpu(pci_bus: u32) -> Option<Gpu> {
    match Gpu::open(Some(pci_bus)) {
        Ok(g) => {
            log!("NVML ready");
            Some(g)
        }
        Err(e) => {
            log!("NVML unavailable, using fail-safe duty: {e:#}");
            None
        }
    }
}

struct Writer {
    last_written: Option<u8>,
    other_writer_logged: Option<Instant>,
    other_writer_seen: Option<Instant>,
}

impl Writer {
    /// One control write. Takes host control again if the mode changed, and notices when
    /// something else changed the duty since the last write.
    fn apply(&mut self, fc: &FanConnect, duty: f32) -> Result<()> {
        let mode = fc.mode()?;
        if mode != MODE_HOST {
            log!("mode was 0x{mode:02X}, taking control again");
            fc.take_control()?;
        }
        let current = fc.duty_reg()?;
        if let Some(last) = self.last_written
            && current != last
        {
            self.other_writer_seen = Some(Instant::now());
            if self.other_writer_logged.is_none_or(|t| t.elapsed() >= OTHER_WRITER_LOG_INTERVAL) {
                let who = other_controller_running().unwrap_or("another program");
                log!("warning: duty changed to 0x{current:02X} since our write of 0x{last:02X}; {who} is also controlling the fans");
                self.other_writer_logged = Some(Instant::now());
            }
        }
        let value = curve::duty_to_reg(duty);
        fc.set_duty(duty)?;
        self.last_written = Some(value);
        Ok(())
    }

    /// A warning for the status file while another writer was seen in the last minute.
    fn warning(&self) -> Option<String> {
        self.other_writer_seen.filter(|t| t.elapsed() < OTHER_WRITER_LOG_INTERVAL).map(|_| {
            let who = other_controller_running().unwrap_or("Another program");
            format!("{who} is also setting the fan duty")
        })
    }
}

fn control_loop(fc: &FanConnect, stop: &AtomicBool) -> Result<()> {
    let mut config = load_config();
    let mut config_stamp = modified();
    let mut controller = Controller::new(config.max_temp, config.curve.clone());
    log!("{}", describe(&config));

    let mut gpu = open_gpu(fc.pci_bus());
    let mut gpu_attempt = Instant::now();
    let mut temp_ok = true;
    let mut failures = 0;
    let mut logged_duty: Option<f32> = None;
    let mut writer = Writer { last_written: None, other_writer_logged: None, other_writer_seen: None };
    let mut last = Instant::now();
    let mut status_ok = true;
    let mut sweep: Option<Sweep> = None;
    let mut calibration_result: Option<Result<String, String>> = None;

    fc.take_control()?;

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32();
        last = now;

        let stamp = modified();
        if stamp != config_stamp {
            config_stamp = stamp;
            let new_config = load_config();
            if new_config != config {
                config = new_config;
                controller.configure(config.max_temp, config.curve.clone());
                log!("settings changed: {}", describe(&config));
            }
        }
        if config.calibrate && sweep.is_none() {
            // Take the request out of the file first, so a restart doesn't calibrate again.
            match config::modify(&config::path(), |c| {
                c.calibrate = false;
                Ok(())
            }) {
                Ok(saved) => {
                    config = saved;
                    config_stamp = modified();
                }
                Err(e) => log!("could not remove the calibration request: {e}"),
            }
            config.calibrate = false;
            sweep = Some(Sweep::new());
            calibration_result = None;
            log!(
                "calibrating the fans: {} % down to {} % duty, measuring the RPM at each step",
                calibration::STEPS[0],
                calibration::STEPS[calibration::STEPS.len() - 1]
            );
        }

        if gpu.is_none() && gpu_attempt.elapsed() >= GPU_RETRY {
            gpu_attempt = Instant::now();
            gpu = open_gpu(fc.pci_bus());
        }
        let temp = match gpu.as_ref().map(Gpu::temperature) {
            Some(Ok(t)) => Some(t as f32),
            Some(Err(e)) => {
                if temp_ok {
                    log!("GPU temperature unreadable, using fail-safe duty: {e:#}");
                }
                gpu = None;
                gpu_attempt = Instant::now();
                None
            }
            None => None,
        };
        if temp.is_some() && !temp_ok {
            log!("GPU temperature readable again");
        }
        temp_ok = temp.is_some();

        // Curve percent: fan speed when calibrated, duty otherwise.
        let percent = controller.update(temp, dt);
        let mut duty = config.duty_for(percent);
        if let Some(s) = &sweep {
            if temp.is_some_and(|t| t < config.max_temp as f32) {
                duty = s.duty();
            } else {
                sweep = None;
                let reason = if temp.is_some() { "GPU reached max temp" } else { "GPU temperature unreadable" };
                calibration_result = Some(finish_calibration(Err(format!("stopped: {reason}"))).0);
            }
        }
        match writer.apply(fc, duty) {
            Ok(()) => failures = 0,
            Err(e) => {
                failures += 1;
                log!("I2C error ({failures}/{MAX_I2C_FAILURES}): {e:#}");
                if failures >= MAX_I2C_FAILURES {
                    bail!("giving up after {MAX_I2C_FAILURES} consecutive I2C failures");
                }
            }
        }

        let hw = fc.status().ok();
        if let (Some(s), Some(hw)) = (&mut sweep, hw)
            && let Some(result) = s.observe(hw.fan1_rpm, hw.fan2_rpm)
        {
            sweep = None;
            let (outcome, saved) = finish_calibration(result);
            if let Some(saved) = saved {
                config = saved;
                config_stamp = modified();
                controller.configure(config.max_temp, config.curve.clone());
            }
            calibration_result = Some(outcome);
        }
        if let Some(hw) = hw {
            let warning = if temp.is_none() {
                Some(format!("GPU temperature unreadable, fans at the fail-safe {FAILSAFE_DUTY} %"))
            } else {
                writer.warning()
            };
            let published = status::write(&DaemonStatus {
                updated_ms: status::now_ms(),
                gpu_temp: temp,
                target_duty: duty,
                duty_reg: hw.duty_reg,
                fan1_rpm: hw.fan1_rpm,
                fan2_rpm: hw.fan2_rpm,
                mode: hw.mode,
                max_temp: config.max_temp,
                curve: config::format_curve(&config.curve),
                warning,
                controller: fc.describe(),
                calibrating: sweep.as_ref().map(Sweep::progress),
                calibration_result: calibration_result.clone(),
            });
            if let Err(e) = &published
                && status_ok
            {
                log!("could not write {}: {e}", status::path().display());
            }
            status_ok = published.is_ok();
        }

        if logged_duty.is_none_or(|d| (d - duty).abs() >= LOG_DUTY_STEP) {
            logged_duty = Some(duty);
            let temp_text = temp.map_or("?".to_string(), |t| format!("{t:.0}"));
            let rpm = hw.map(|s| format!("{}/{} RPM", s.fan1_rpm, s.fan2_rpm)).unwrap_or_default();
            log!("GPU {temp_text} °C -> duty {duty:.0} % (0x{:02X}) {rpm}", curve::duty_to_reg(duty));
        }

        while now.elapsed() < POLL && !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
