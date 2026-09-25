//! Background thread that collects live data once a second.
//!
//! It prefers the daemon's status file, which needs no privileges. When no daemon is running it
//! reads the card directly (works without admin on Windows; needs root on Linux).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use gpu_fanctl::curve;
use gpu_fanctl::daemon;
use gpu_fanctl::fanconnect::FanConnect;
use gpu_fanctl::gpu::{self, Gpu};
use gpu_fanctl::status;

/// Ten minutes of history at one sample per second.
const HISTORY_LEN: usize = 600;
const PERIOD: Duration = Duration::from_secs(1);
const DIRECT_RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// From the running daemon/service.
    Daemon,
    /// Read from the card by the GUI; nothing is controlling the fans.
    Direct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    Stopped,
    NotInstalled,
    /// Not known on this platform (Linux: judged by the status file only).
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Sample {
    /// Seconds since the GUI started.
    pub t: f64,
    pub gpu_temp: Option<f32>,
    /// Duty as read back from the chip, in percent.
    pub duty: f32,
    /// Duty the daemon asked for (only when the daemon is the source).
    pub target: Option<f32>,
    pub fan1_rpm: u32,
    pub fan2_rpm: u32,
    pub mode: u8,
    pub warning: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Live {
    pub started: Instant,
    pub latest: Option<Sample>,
    pub source: Option<Source>,
    pub error: Option<String>,
    pub service: ServiceState,
    pub other_controller: Option<&'static str>,
    pub history: VecDeque<Sample>,
    /// All NVIDIA cards with their identity, read once at startup (`None` until then).
    pub system: Option<Result<gpu::System, String>>,
    /// Where the fan controller was found, e.g. "NVAPI I2C port 1, GPU bus 0a".
    pub controller: Option<String>,
}

pub fn spawn(ctx: egui::Context) -> Arc<Mutex<Live>> {
    let live = Arc::new(Mutex::new(Live {
        started: Instant::now(),
        latest: None,
        source: None,
        error: None,
        service: ServiceState::Unknown,
        other_controller: None,
        history: VecDeque::with_capacity(HISTORY_LEN),
        system: None,
        controller: None,
    }));
    let shared = Arc::clone(&live);
    thread::spawn(move || collector(&shared, &ctx));
    live
}

fn service_state() -> ServiceState {
    #[cfg(windows)]
    {
        use windows_service::service::ServiceState as S;
        match gpu_fanctl::service::state() {
            None => ServiceState::NotInstalled,
            Some(S::Running) => ServiceState::Running,
            Some(_) => ServiceState::Stopped,
        }
    }
    #[cfg(target_os = "linux")]
    match gpu_fanctl::service::state() {
        None => ServiceState::NotInstalled,
        Some(true) => ServiceState::Running,
        Some(false) => ServiceState::Stopped,
    }
}

/// Direct hardware access, opened lazily and only while no daemon is publishing.
struct Direct {
    fc: Option<FanConnect>,
    gpu: Option<Gpu>,
    attempt: Option<Instant>,
}

impl Direct {
    fn sample(&mut self, t: f64) -> Result<Sample, String> {
        if self.fc.is_none() && self.attempt.is_none_or(|a| a.elapsed() >= DIRECT_RETRY) {
            self.attempt = Some(Instant::now());
            self.fc = Some(FanConnect::open().map_err(|e| format!("{e:#}"))?);
            self.gpu = self.fc.as_ref().and_then(|fc| Gpu::open(Some(fc.pci_bus())).ok());
        }
        let Some(fc) = &self.fc else {
            return Err("FanConnect II controller not available".into());
        };
        let s = match fc.status() {
            Ok(s) => s,
            Err(e) => {
                self.fc = None;
                return Err(format!("{e:#}"));
            }
        };
        let gpu_temp = self.gpu.as_ref().and_then(|g| g.temperature().ok()).map(|t| t as f32);
        Ok(Sample {
            t,
            gpu_temp,
            duty: s.duty_percent(),
            target: None,
            fan1_rpm: s.fan1_rpm,
            fan2_rpm: s.fan2_rpm,
            mode: s.mode,
            warning: None,
        })
    }
}

fn collector(live: &Mutex<Live>, ctx: &egui::Context) {
    let started = live.lock().unwrap_or_else(|e| e.into_inner()).started;
    // The hardware identity doesn't change while the app runs, so it is read once.
    let system = gpu::detect().map_err(|e| format!("{e:#}"));
    live.lock().unwrap_or_else(|e| e.into_inner()).system = Some(system);
    let mut direct = Direct { fc: None, gpu: None, attempt: None };
    loop {
        let tick = Instant::now();
        let t = started.elapsed().as_secs_f64();
        let service = service_state();
        let other_controller = daemon::other_controller_running();

        let controller;
        let (sample, source, error) = match status::read().filter(status::DaemonStatus::is_fresh) {
            Some(s) => {
                direct = Direct { fc: None, gpu: None, attempt: None };
                controller = Some(s.controller.clone()).filter(|c| !c.is_empty());
                let sample = Sample {
                    t,
                    gpu_temp: s.gpu_temp,
                    duty: curve::reg_to_duty(s.duty_reg),
                    target: Some(s.target_duty),
                    fan1_rpm: s.fan1_rpm,
                    fan2_rpm: s.fan2_rpm,
                    mode: s.mode,
                    warning: s.warning,
                };
                (Some(sample), Some(Source::Daemon), None)
            }
            None => {
                let result = direct.sample(t);
                controller = direct.fc.as_ref().map(FanConnect::describe);
                match result {
                    Ok(sample) => (Some(sample), Some(Source::Direct), None),
                    Err(e) => (None, None, Some(e)),
                }
            }
        };

        {
            let mut l = live.lock().unwrap_or_else(|e| e.into_inner());
            l.service = service;
            l.other_controller = other_controller;
            l.source = source;
            l.error = error;
            l.controller = controller;
            if let Some(sample) = sample {
                if l.history.len() == HISTORY_LEN {
                    l.history.pop_front();
                }
                l.history.push_back(sample.clone());
                l.latest = Some(sample);
            } else {
                l.latest = None;
            }
        }
        ctx.request_repaint();
        thread::sleep(PERIOD.saturating_sub(tick.elapsed()));
    }
}
