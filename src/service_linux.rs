//! Linux counterpart of the Windows service module: `gpu-fanctl install` (as root) copies the
//! programs to /usr/local/bin, sets up and starts a systemd service, loads i2c-dev at boot,
//! adds the app-menu entry with icon plus a desktop shortcut, and lets users edit the settings.
//! `gpu-fanctl uninstall` stops (fans go to the fail-safe duty) and removes all of that except
//! the programs and the settings.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::{config, icon_art};

pub const NAME: &str = "gpu-fanctl";
const UNIT_PATH: &str = "/etc/systemd/system/gpu-fanctl.service";
const MODULES_LOAD_PATH: &str = "/etc/modules-load.d/gpu-fanctl.conf";
const DESKTOP_ENTRY_PATH: &str = "/usr/share/applications/gpu-fanctl.desktop";
const ICON_PATH: &str = "/usr/share/icons/hicolor/256x256/apps/gpu-fanctl.png";
const DESKTOP_FILE: &str = "gpu-fanctl.desktop";
/// The programs `install` copies into `install_dir()`.
const INSTALL_FILES: [&str; 2] = ["gpu-fanctl", "gpu-fanctl-gui"];

/// Where the programs are installed.
pub fn install_dir() -> PathBuf {
    PathBuf::from("/usr/local/bin")
}

fn systemctl(args: &[&str]) -> Result<String> {
    let output = Command::new("systemctl").args(args).output().context("running systemctl")?;
    if !output.status.success() {
        bail!("systemctl {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether the service is running; `None` if it isn't installed. Works without root.
pub fn state() -> Option<bool> {
    let output = Command::new("systemctl")
        .args(["show", NAME, "--property=LoadState,ActiveState"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)).unwrap_or("").trim().to_string();
    if value("LoadState=") != "loaded" {
        return None;
    }
    Some(value("ActiveState=") == "active")
}

/// The program the installed service runs, `None` if it isn't installed.
pub fn installed_path() -> Option<PathBuf> {
    let unit = fs::read_to_string(UNIT_PATH).ok()?;
    let exec = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
    exec.split_whitespace().next().map(PathBuf::from)
}

fn require_root() -> Result<()> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        bail!("needs root: run with sudo (the app asks for your password through pkexec)");
    }
    Ok(())
}

/// Copies this program and the GUI next to it into `install_dir()` (unless already running from
/// there) and returns the path of the installed gpu-fanctl. Uses a temporary file and a rename,
/// so a running copy of the old program doesn't block the update.
fn copy_to_install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let source_dir = exe.parent().context("locating this program's folder")?;
    let target_dir = install_dir();
    fs::create_dir_all(&target_dir).with_context(|| format!("creating {}", target_dir.display()))?;
    if fs::canonicalize(source_dir).ok() != fs::canonicalize(&target_dir).ok() {
        for name in INSTALL_FILES {
            let source = source_dir.join(name);
            if source.exists() {
                let target = target_dir.join(name);
                let temporary = target_dir.join(format!(".{name}.new"));
                fs::copy(&source, &temporary).with_context(|| format!("copying {name} to {}", target_dir.display()))?;
                fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
                fs::rename(&temporary, &target)?;
            }
        }
    }
    Ok(target_dir.join("gpu-fanctl"))
}

fn unit_file(program: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=FanConnect II Controller (ASUS ROG Strix RTX 2080 Ti external fans)\n\
         After=systemd-modules-load.service\n\
         \n\
         [Service]\n\
         ExecStartPre=-/usr/bin/modprobe i2c-dev\n\
         ExecStart={} run\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        program.display()
    )
}

fn desktop_entry(gui: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=FanConnect II Controller\n\
         Comment=FanConnect II fan control for the ASUS ROG Strix RTX 2080 Ti\n\
         Exec={}\n\
         Icon=gpu-fanctl\n\
         Terminal=false\n\
         Categories=System;Settings;HardwareSettings;\n\
         StartupWMClass=gpu-fanctl-gui\n",
        gui.display()
    )
}

/// The user who asked for the install (through pkexec or sudo): uid, gid and home folder.
fn invoking_user() -> Option<(u32, u32, PathBuf)> {
    let uid: u32 = std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")).ok()?.parse().ok()?;
    // SAFETY: getpwuid returns a pointer into static storage or null; it is read immediately.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        let home = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        Some((uid, (*pw).pw_gid, PathBuf::from(home)))
    }
}

/// The user's desktop folder, from ~/.config/user-dirs.dirs (it may be localised, e.g.
/// "Schreibtisch"), falling back to ~/Desktop.
fn desktop_dir(home: &Path) -> PathBuf {
    let from_config = fs::read_to_string(home.join(".config/user-dirs.dirs")).ok().and_then(|text| {
        text.lines().find_map(|line| {
            let value = line.strip_prefix("XDG_DESKTOP_DIR=")?.trim_matches('"');
            Some(PathBuf::from(value.replace("$HOME", &home.to_string_lossy())))
        })
    });
    from_config.unwrap_or_else(|| home.join("Desktop"))
}

fn user_desktop_shortcut() -> Option<(PathBuf, u32, u32)> {
    let (uid, gid, home) = invoking_user()?;
    let dir = desktop_dir(&home);
    dir.is_dir().then(|| (dir.join(DESKTOP_FILE), uid, gid))
}

fn create_shortcuts(gui: &Path) -> Result<()> {
    fs::create_dir_all(Path::new(ICON_PATH).parent().unwrap_or(Path::new("/")))?;
    fs::write(ICON_PATH, icon_art::png(256)).with_context(|| format!("writing {ICON_PATH}"))?;
    let entry = desktop_entry(gui);
    fs::write(DESKTOP_ENTRY_PATH, &entry).with_context(|| format!("writing {DESKTOP_ENTRY_PATH}"))?;
    let _ = Command::new("gtk-update-icon-cache").args(["-f", "-t", "/usr/share/icons/hicolor"]).output();
    let _ = Command::new("update-desktop-database").arg("/usr/share/applications").output();

    if let Some((path, uid, gid)) = user_desktop_shortcut() {
        fs::write(&path, &entry).with_context(|| format!("writing {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes())?;
        // SAFETY: c_path is a valid NUL-terminated path.
        unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    }
    Ok(())
}

fn remove_shortcuts() {
    for path in [DESKTOP_ENTRY_PATH, ICON_PATH] {
        let _ = fs::remove_file(path);
    }
    if let Some((path, _, _)) = user_desktop_shortcut() {
        let _ = fs::remove_file(path);
    }
    let _ = Command::new("update-desktop-database").arg("/usr/share/applications").output();
}

/// Creates the settings file if needed and lets users change it (e.g. from the GUI without
/// root). The values are range-checked, so this can't make the fans unsafe.
fn allow_users_to_edit_settings() -> Result<()> {
    let path = config::path();
    if !path.exists() {
        config::save(&path, &config::Config::default()).with_context(|| format!("writing {}", path.display()))?;
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).with_context(|| format!("changing {}", path.display()))
}

/// Copies the programs to /usr/local/bin, then sets up, enables and starts the service.
pub fn install() -> Result<()> {
    require_root()?;
    let program = copy_to_install_dir()?;
    fs::write(UNIT_PATH, unit_file(&program)).with_context(|| format!("writing {UNIT_PATH}"))?;
    fs::write(MODULES_LOAD_PATH, "i2c-dev\n").with_context(|| format!("writing {MODULES_LOAD_PATH}"))?;
    allow_users_to_edit_settings()?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", NAME])?;
    println!("Service `{NAME}` installed and started, running {}", program.display());
    println!("Log: journalctl -u {NAME}");

    let gui = install_dir().join("gpu-fanctl-gui");
    if gui.exists() {
        create_shortcuts(&gui).context("the service is installed and running, but creating the shortcuts failed")?;
        println!("App menu entry and desktop shortcut created");
    }
    Ok(())
}

/// Stops (fans go to the fail-safe duty) and removes the service and the shortcuts.
pub fn uninstall() -> Result<()> {
    require_root()?;
    if state().is_none() {
        bail!("the service is not installed");
    }
    // `disable --now` stops the service; its SIGTERM handler sets the fail-safe duty.
    systemctl(&["disable", "--now", NAME])?;
    for path in [UNIT_PATH, MODULES_LOAD_PATH] {
        let _ = fs::remove_file(path);
    }
    systemctl(&["daemon-reload"])?;
    remove_shortcuts();
    println!("Service `{NAME}` stopped and removed. The fans stay at the fail-safe duty until something else sets them.");
    Ok(())
}

/// Uninstalls (if installed) and installs again, e.g. to run a new build.
pub fn reinstall() -> Result<()> {
    require_root()?;
    if state().is_some() {
        uninstall()?;
    }
    install()
}
