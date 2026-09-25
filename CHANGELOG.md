# Changelog

All notable changes to FanConnect II Controller are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- MIT license.

## [0.1.0] - 2026-09-25

First release.

### Added

- Control of the two FanConnect II external fan headers on the ASUS ROG Strix GeForce RTX 2080 Ti,
  on Windows 10+ (through NVIDIA NVAPI) and Linux (through i2c-dev).
- Built-in Auto fan curve (30 % up to 45 °C, rising to 100 % at max temp) with smoothing: fast
  rise, 3 °C hysteresis and a slow ramp down.
- Custom fan curves with 2–16 points; the custom curve is remembered while Auto is active.
- Max GPU temp setting (60–90 °C, default 80 °C): at or above it the fans always run at 100 %.
- Fail-safe: the fans go to 100 % when the GPU temperature can't be read and when the service stops.
- Background service: a Windows service or a systemd service, with automatic restart.
- Install, Reinstall and Uninstall from the app with an administrator prompt (UAC on Windows,
  pkexec on Linux). Installing copies the programs to a fixed location and adds Start menu /
  app menu and desktop shortcuts.
- Desktop app with navigation: Overview (live values and 10-minute history), Fan curve editor with
  draggable points, GPU, Service, Settings and About pages; light, dark and system themes.
- Detection of all NVIDIA cards at startup (name, PCI IDs, VBIOS, memory, driver) with a
  supported / not supported indication, and matching of the temperature sensor to the card that
  carries the fan controller.
- Warnings when GPU Tweak III's external fan service is running or another program changes the
  fan duty, and when a fan reports 0 RPM.
- Command-line tool `gpu-fanctl` with `status`, `detect`, `curve`, `max-temp`, `set`, `run`,
  `install`, `uninstall` and `reinstall`.
- App icon, embedded in the Windows programs and installed for the Linux app menu.
- Crash report file when the app closes unexpectedly (`gpu-fanctl-gui-crash.log` in the temp folder).
- Research tools: a read-only Linux probe, a Linux write test, and a Windows hook that records
  GPU Tweak III's I2C traffic.
