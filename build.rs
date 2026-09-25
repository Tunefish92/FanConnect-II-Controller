//! Records build information for the GUI's About page (compiler version, target, build date and
//! the versions of the main libraries, read from Cargo.lock), and on Windows embeds the app icon
//! and version information into the executables.

#[path = "src/icon_art.rs"]
mod icon_art;

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

/// Libraries shown on the About page.
const SHOWN_CRATES: [&str; 8] =
    ["eframe", "egui", "egui_plot", "nvml-wrapper", "serde", "serde_json", "windows-service", "libloading"];

/// The newest version of `name` in Cargo.lock (a crate can be locked in several versions).
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let marker = format!("name = \"{name}\"\nversion = \"");
    let numeric = |v: &str| v.split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    lock.match_indices(&marker)
        .filter_map(|(i, _)| lock[i + marker.len()..].split('"').next())
        .max_by_key(|v| numeric(v))
        .map(str::to_string)
}

/// UTC date (YYYY-MM-DD) for a Unix timestamp; days-to-civil algorithm by Howard Hinnant.
fn utc_date(unix_secs: u64) -> String {
    let z = (unix_secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let lock = fs::read_to_string(Path::new(&manifest_dir).join("Cargo.lock")).unwrap_or_default().replace("\r\n", "\n");
    let versions: Vec<String> = SHOWN_CRATES
        .iter()
        .filter_map(|name| locked_version(&lock, name).map(|v| format!("{name}={v}")))
        .collect();
    println!("cargo:rustc-env=GPU_FANCTL_DEPS={}", versions.join(";"));

    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rustc_version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    println!("cargo:rustc-env=GPU_FANCTL_RUSTC={rustc_version}");
    println!("cargo:rustc-env=GPU_FANCTL_TARGET={}", env::var("TARGET").unwrap_or_default());

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    println!("cargo:rustc-env=GPU_FANCTL_BUILD_DATE={}", utc_date(now));

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_resources(&utc_date(now)[..4]);
    }
}

/// Embeds the app icon and version information into the Windows executables.
fn embed_windows_resources(year: &str) {
    let ico = Path::new(&env::var("OUT_DIR").unwrap_or_default()).join("app.ico");
    if let Err(e) = fs::write(&ico, icon_art::ico(&[16, 24, 32, 48, 64, 128, 256])) {
        println!("cargo:warning=could not write the app icon: {e}");
        return;
    }
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon(&ico.to_string_lossy());
    resources.set("ProductName", "FanConnect II Controller");
    resources.set("FileDescription", "FanConnect II Controller");
    resources.set("CompanyName", "Tunefish");
    resources.set("LegalCopyright", &format!("© {year} Tunefish"));
    if let Err(e) = resources.compile() {
        println!("cargo:warning=could not embed the app icon: {e}");
    }
}
