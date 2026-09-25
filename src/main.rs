//! gpu-fanctl: control the FanConnect II external fan headers of the ASUS ROG Strix RTX 2080 Ti.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use gpu_fanctl::config::{self, Config};
use gpu_fanctl::curve::{self, Curve, FLOOR_DUTY};
use gpu_fanctl::daemon;
use gpu_fanctl::fanconnect::{FanConnect, MODE_AUTO, MODE_HOST};
use gpu_fanctl::gpu::Gpu;

#[cfg(target_os = "linux")]
const ADMIN: &str = "Hardware commands, install/uninstall/reinstall and (before installing) changing settings need root.";
#[cfg(windows)]
const ADMIN: &str = "install, uninstall and changing settings need an administrator prompt.";

#[cfg(windows)]
const PLATFORM_COMMANDS: &str = "
  install              copy to C:\\Program Files\\gpu-fanctl, install and start the Windows service,
                       add Start menu and desktop shortcuts
  uninstall            stop (fans go to 100 %) and remove the service and shortcuts
  reinstall            uninstall (if installed) and install again, e.g. after updating the .exe";
#[cfg(target_os = "linux")]
const PLATFORM_COMMANDS: &str = "
  install              copy to /usr/local/bin, install and start the systemd service, load i2c-dev at
                       boot, add the app menu entry and a desktop shortcut
  uninstall            stop (fans go to 100 %) and remove the service and shortcuts
  reinstall            uninstall (if installed) and install again, e.g. after rebuilding";

fn usage() -> String {
    format!(
        "usage: gpu-fanctl <command>

  status               show fan duty and RPM, GPU temperature and the curve's target duty
  detect               find and identify the FanConnect II controller (read-only)
  curve                show the active fan curve
  curve set <points>   use a custom curve of temp:duty points, e.g. `curve set 40:30 60:50 75:80 80:100`
  curve auto           go back to the built-in Auto curve (your custom curve is remembered)
  curve custom         switch back to your remembered custom curve
  max-temp [<°C>]      show or set the max GPU temp (60-90, default 80): fans always run 100 % from there
  set <percent>        set a fixed duty, 30-100 (a running daemon overrides it within a second)
  run                  run the control loop in this console; on exit the fans go to 100 %{PLATFORM_COMMANDS}

Settings: {}
{ADMIN}",
        config::path().display()
    )
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--result <file>`: also write "ok" or the error to <file>. The GUI uses this to learn the
    // outcome of an elevated install/uninstall whose console window is hidden.
    let result_file = args.iter().position(|a| a == "--result").and_then(|i| {
        let path = args.get(i + 1).cloned();
        args.drain(i..(i + 2).min(args.len()));
        path
    });
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["status"] => status(),
        ["detect"] => detect(),
        ["curve"] => print_curve(),
        ["curve", "auto"] => update_config(|c| {
            c.use_auto();
            Ok(())
        }),
        ["curve", "custom"] => update_config(Config::use_remembered_custom),
        ["curve", "set", points @ ..] if !points.is_empty() => set_curve(&points.join(" ")),
        ["max-temp"] => show_max_temp(),
        ["max-temp", value] => set_max_temp(value),
        ["set", percent] => set(percent),
        ["run"] => run(),
        ["install"] => gpu_fanctl::service::install(),
        ["uninstall"] => gpu_fanctl::service::uninstall(),
        ["reinstall"] => gpu_fanctl::service::reinstall(),
        #[cfg(windows)]
        ["service"] => gpu_fanctl::service::dispatch(),
        _ => {
            eprintln!("{}", usage());
            std::process::exit(2);
        }
    };
    if let Some(path) = result_file {
        let outcome = result.as_ref().map_or_else(|e| format!("{e:#}"), |()| "ok".to_string());
        let _ = std::fs::write(path, outcome);
    }
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// The settings in effect, as the daemon would load them (defaults if the file is invalid).
fn load_config() -> Config {
    let (config, warning) = config::load(&config::path());
    if let Some(w) = warning {
        eprintln!("warning: {w}");
    }
    config
}

/// Changes the settings file. Refuses to touch a file that doesn't parse, so hand edits aren't lost.
fn update_config(change: impl FnOnce(&mut Config) -> Result<(), String>) -> Result<()> {
    let path = config::path();
    let mut config = match std::fs::read_to_string(&path) {
        Ok(text) => config::parse(&text)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("{} is invalid; fix or delete it first", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    change(&mut config).map_err(anyhow::Error::msg)?;
    config::save(&path, &config).with_context(|| format!("writing {} (needs administrator/root)", path.display()))?;
    println!("Saved. A running daemon picks it up within a second.\n");
    print_config(&config);
    Ok(())
}

fn mode_name(mode: u8) -> &'static str {
    match mode {
        MODE_HOST => "host",
        MODE_AUTO => "card auto",
        _ => "unknown",
    }
}

fn curve_name(curve: &Curve) -> &'static str {
    match curve {
        Curve::Auto => "Auto (built-in)",
        Curve::Custom(_) => "custom",
    }
}

fn warn_other_controller() {
    if let Some(name) = daemon::other_controller_running() {
        eprintln!("warning: {name} is running. If its external fan control is active, it overrides the duty.");
    }
}

/// Lists every NVIDIA card with its identity, then looks for the fan controller. The output is
/// meant to be shareable, e.g. to request support for another card.
fn detect() -> Result<()> {
    match gpu_fanctl::gpu::detect() {
        Ok(system) => {
            println!("Driver: {}", system.driver_version.as_deref().unwrap_or("unknown"));
            if system.gpus.is_empty() {
                println!("No NVIDIA graphics card found.");
            }
            for gpu in &system.gpus {
                let support = gpu.supported.map_or("not supported".to_string(), |c| format!("supported: {}", c.name));
                println!("GPU {}: {} [{support}]", gpu.pci_address, gpu.name);
                println!("  PCI IDs: {}", gpu_fanctl::fanconnect::format_pci_ids(gpu.device_id, gpu.subsystem_id));
                if let Some(vbios) = &gpu.vbios {
                    println!("  VBIOS:   {vbios}");
                }
            }
        }
        Err(e) => println!("Could not read the graphics cards through NVML: {e:#}"),
    }
    let fc = FanConnect::open()?;
    println!("FanConnect II controller identified on {} ({})", fc.card().name, fc.describe());
    Ok(())
}

fn status() -> Result<()> {
    let fc = FanConnect::open()?;
    let s = fc.status()?;
    let config = load_config();
    println!("Controller:   {}", fc.describe());
    println!("Mode:         {} (0x{:02X})", mode_name(s.mode), s.mode);
    println!("Duty:         {:.0} % (0x{:02X})", s.duty_percent(), s.duty_reg);
    println!("Fan 1:        {} RPM", s.fan1_rpm);
    println!("Fan 2:        {} RPM", s.fan2_rpm);
    println!("Curve:        {}", curve_name(&config.curve));
    println!("Max temp:     {} °C", config.max_temp);
    match Gpu::open(Some(fc.pci_bus())).and_then(|g| g.temperature()) {
        Ok(t) => {
            let target =
                if t >= config.max_temp { 100.0 } else { config.curve.duty(t as f32, config.max_temp) };
            println!("GPU temp:     {t} °C");
            println!("Target duty:  {target:.0} %");
        }
        Err(e) => println!("GPU temp:     unavailable ({e:#})"),
    }
    warn_other_controller();
    Ok(())
}

fn print_config(config: &Config) {
    println!("Curve: {}, max temp {} °C", curve_name(&config.curve), config.max_temp);
    let points = config.curve.points(config.max_temp);
    for (i, (temp, duty)) in points.iter().enumerate() {
        let prefix = match i {
            0 => "<=",
            _ if i == points.len() - 1 => ">=",
            _ => "  ",
        };
        println!("  {prefix} {temp:5.1} °C  {duty:5.1} %  (0x{:02X})", curve::duty_to_reg(*duty));
    }
    let last_temp = points.last().map_or(0.0, |p| p.0);
    if last_temp > config.max_temp as f32 {
        println!("  Note: from {} °C (max temp) the fans run at 100 %, before the curve ends.", config.max_temp);
    } else if !matches!(config.curve, Curve::Auto) {
        println!("  From {} °C (max temp) the fans always run at 100 %.", config.max_temp);
    }
    if let Some(points) = &config.remembered_custom {
        println!("Remembered custom curve: {} (`curve custom` switches back)", config::format_points(points));
    }
}

fn print_curve() -> Result<()> {
    print_config(&load_config());
    Ok(())
}

fn set_curve(points: &str) -> Result<()> {
    let curve = config::parse_curve(points).map_err(anyhow::Error::msg)?;
    update_config(|c| {
        if curve == Curve::Auto {
            c.use_auto();
        } else {
            c.set_custom(curve);
        }
        Ok(())
    })
}

fn show_max_temp() -> Result<()> {
    println!("{} °C", load_config().max_temp);
    Ok(())
}

fn set_max_temp(value: &str) -> Result<()> {
    let max_temp = config::validate_max_temp(value).map_err(anyhow::Error::msg)?;
    update_config(|c| {
        c.max_temp = max_temp;
        Ok(())
    })
}

fn set(percent: &str) -> Result<()> {
    let percent: f32 = percent.parse().with_context(|| format!("`{percent}` is not a number"))?;
    if !(FLOOR_DUTY..=100.0).contains(&percent) {
        bail!("duty must be between {FLOOR_DUTY} and 100 %");
    }
    let fc = FanConnect::open()?;
    fc.take_control()?;
    fc.set_duty(percent)?;
    println!("Duty set to {percent} % (0x{:02X}).", curve::duty_to_reg(percent));
    warn_other_controller();
    Ok(())
}

fn run() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed)).context("installing Ctrl+C handler")?;
    daemon::run(&stop)
}
