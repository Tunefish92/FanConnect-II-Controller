//! External fan control for the FanConnect II headers of the ASUS ROG Strix RTX 2080 Ti,
//! on Linux (i2c-dev) and Windows (NVAPI).
//! See HARDWARE.md for the protocol and DESIGN.md for the control behaviour.

pub mod config;
pub mod curve;
pub mod daemon;
pub mod fanconnect;
pub mod gpu;
pub mod icon_art;
pub mod logging;
pub mod status;

#[cfg(target_os = "linux")]
mod i2c;
#[cfg(windows)]
mod nvapi;
/// Installing and managing the background service: a Windows service, or a systemd unit on Linux.
#[cfg(windows)]
pub mod service;
#[cfg(target_os = "linux")]
#[path = "service_linux.rs"]
pub mod service;
#[cfg(windows)]
mod winproc;
