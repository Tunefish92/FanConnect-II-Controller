//! Live state the daemon publishes once a second, for the GUI (or anything else) to read
//! without hardware access: %ProgramData%\gpu-fanctl\status.json on Windows,
//! /run/gpu-fanctl/status.json on Linux. Removed when the daemon stops.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A status older than this means the daemon isn't running (or hung).
pub const FRESH_FOR: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// When this was written, in milliseconds since the Unix epoch.
    pub updated_ms: u64,
    /// GPU core temperature, `None` if it could not be read (fail-safe duty in use).
    pub gpu_temp: Option<f32>,
    /// Duty the controller asked for, in percent.
    pub target_duty: f32,
    /// Duty register as read back from the chip.
    pub duty_reg: u8,
    pub fan1_rpm: u32,
    pub fan2_rpm: u32,
    pub mode: u8,
    pub max_temp: u32,
    /// `auto` or the custom points, as in the settings file.
    pub curve: String,
    /// A current problem worth showing (temperature unreadable, another program writing).
    pub warning: Option<String>,
    /// Where the fan controller was found, e.g. "NVAPI I2C port 1, GPU bus 0a".
    #[serde(default)]
    pub controller: String,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn path() -> PathBuf {
    if cfg!(windows) {
        crate::config::data_dir().join("status.json")
    } else {
        PathBuf::from("/run/gpu-fanctl/status.json")
    }
}

impl DaemonStatus {
    pub fn is_fresh(&self) -> bool {
        now_ms().saturating_sub(self.updated_ms) <= FRESH_FOR.as_millis() as u64
    }
}

/// Writes the status atomically (temporary file, then rename) so readers never see half a file.
pub fn write(status: &DaemonStatus) -> io::Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(status).map_err(io::Error::other)?)?;
    fs::rename(&tmp, &path)
}

/// The last published status, if there is one and it can be read.
pub fn read() -> Option<DaemonStatus> {
    serde_json::from_slice(&fs::read(path()).ok()?).ok()
}

pub fn remove() {
    let _ = fs::remove_file(path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_follows_age() {
        let mut s = DaemonStatus {
            updated_ms: now_ms(),
            gpu_temp: Some(50.0),
            target_duty: 40.0,
            duty_reg: 0x66,
            fan1_rpm: 990,
            fan2_rpm: 990,
            mode: 2,
            max_temp: 80,
            curve: "auto".into(),
            warning: None,
            controller: "i2c-4, GPU 0000:0a:00.0".into(),
        };
        assert!(s.is_fresh());
        s.updated_ms -= 10_000;
        assert!(!s.is_fresh());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<DaemonStatus>(&json).unwrap(), s);
    }
}
