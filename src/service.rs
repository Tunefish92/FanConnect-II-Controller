//! Windows service: `gpu-fanctl install` registers it (automatic start, LocalSystem,
//! restart on crash), `gpu-fanctl uninstall` removes it. The service runs the daemon loop,
//! logs to %ProgramData%\gpu-fanctl\gpu-fanctl.log, and a stop or system shutdown sets the
//! fans to the fail-safe duty.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::{config, daemon, log, logging};

pub const NAME: &str = "gpu-fanctl";
const DISPLAY_NAME: &str = "FanConnect II Controller";
const DESCRIPTION: &str =
    "Drives the FanConnect II external fan headers of the ASUS ROG Strix RTX 2080 Ti from the GPU temperature.";
/// Wait between attempts when the controller or driver isn't available yet (e.g. early at boot).
const RETRY: Duration = Duration::from_secs(5);

define_windows_service!(ffi_service_main, service_main);

/// Entry point when started by the service control manager (`gpu-fanctl service`).
pub fn dispatch() -> Result<()> {
    service_dispatcher::start(NAME, ffi_service_main).context("not started by the Windows service manager")
}

fn service_main(_arguments: Vec<OsString>) {
    let _ = logging::to_file(&config::data_dir().join("gpu-fanctl.log"));
    if let Err(e) = run_service() {
        log!("service error: {e:#}");
    }
}

fn status(state: ServiceState, exit_code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    }
}

fn run_service() -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
            flag.store(true, Ordering::Relaxed);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::Running, 0))?;
    log!("service started");

    // The daemon returns an error when the controller or driver isn't reachable. Keep retrying
    // until stopped rather than exiting, since the service manager only restarts a few times.
    while !stop.load(Ordering::Relaxed) {
        match daemon::run(&stop) {
            Ok(()) => break,
            Err(e) => {
                log!("error: {e:#}; retrying in {} s", RETRY.as_secs());
                let since = Instant::now();
                while since.elapsed() < RETRY && !stop.load(Ordering::Relaxed) {
                    sleep(Duration::from_millis(100));
                }
            }
        }
    }

    log!("service stopped");
    handle.set_service_status(status(ServiceState::Stopped, 0))?;
    Ok(())
}

/// The service's current state, `None` if it isn't installed. Works without administrator rights.
pub fn state() -> Option<ServiceState> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager.open_service(NAME, ServiceAccess::QUERY_STATUS).ok()?;
    service.query_status().ok().map(|s| s.current_state)
}

/// Lets members of Users change the settings (e.g. from the GUI without elevation). The values
/// are range-checked, so this can't make the fans unsafe.
fn allow_users_to_edit_settings() {
    let dir = config::data_dir();
    let granted = std::fs::create_dir_all(&dir).is_ok()
        && std::process::Command::new("icacls")
            .arg(&dir)
            // S-1-5-32-545 = BUILTINUsers; (OI)(CI)M = modify, inherited by files and folders.
            .args(["/grant", "*S-1-5-32-545:(OI)(CI)M", "/Q"])
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    if !granted {
        eprintln!("warning: could not let Users edit {}; changing settings will need an administrator", dir.display());
    }
}

/// The programs `install` copies into `install_dir()`.
const INSTALL_FILES: [&str; 2] = ["gpu-fanctl.exe", "gpu-fanctl-gui.exe"];

/// Where the service and the GUI are installed: %ProgramFiles%\gpu-fanctl.
pub fn install_dir() -> std::path::PathBuf {
    let program_files = std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into());
    std::path::Path::new(&program_files).join("gpu-fanctl")
}

/// Copies this executable and the GUI next to it into `install_dir()` (unless already running
/// from there) and returns the path of the installed gpu-fanctl.exe.
fn copy_to_install_dir() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let source_dir = exe.parent().context("locating this executable's folder")?;
    let target_dir = install_dir();
    std::fs::create_dir_all(&target_dir).with_context(|| format!("creating {}", target_dir.display()))?;
    let same_dir = std::fs::canonicalize(source_dir).ok() == std::fs::canonicalize(&target_dir).ok();
    if !same_dir {
        for name in INSTALL_FILES {
            let source = source_dir.join(name);
            if source.exists() {
                let target = target_dir.join(name);
                std::fs::copy(&source, &target).with_context(|| {
                    format!("copying {name} to {} (is the installed app still open?)", target_dir.display())
                })?;
            }
        }
    }
    Ok(target_dir.join("gpu-fanctl.exe"))
}

/// Copies the programs to %ProgramFiles%\gpu-fanctl, then registers and starts the service there.
pub fn install() -> Result<()> {
    allow_users_to_edit_settings();
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .context("opening the service manager (run as administrator)")?;
    let info = ServiceInfo {
        name: NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: copy_to_install_dir()?,
        launch_arguments: vec!["service".into()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .context("creating the service (already installed? run `gpu-fanctl uninstall` first)")?;
    service.set_description(DESCRIPTION)?;
    let restart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(3) };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })?;
    service.start::<&str>(&[]).context("starting the service")?;
    println!("Service `{NAME}` installed and started, running {}", info.executable_path.display());
    println!("Log: {}", config::data_dir().join("gpu-fanctl.log").display());

    let gui = install_dir().join("gpu-fanctl-gui.exe");
    if gui.exists() {
        create_shortcuts(&gui).context("the service is installed and running, but creating the shortcuts failed")?;
        println!("Shortcuts created in the Start menu and on the desktop");
    }
    Ok(())
}

/// The executable the installed service runs, `None` if it isn't installed.
pub fn installed_path() -> Option<std::path::PathBuf> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager.open_service(NAME, ServiceAccess::QUERY_CONFIG).ok()?;
    let command = service.query_config().ok()?.executable_path;
    // The service manager stores the whole command line, e.g. `"C:...gpu-fanctl.exe" service`.
    let command = command.to_string_lossy();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.strip_suffix(" service").unwrap_or(&command),
    };
    Some(std::path::PathBuf::from(program))
}

/// Uninstalls (if installed) and installs again, e.g. to run a new build or another copy.
pub fn reinstall() -> Result<()> {
    if state().is_some() {
        uninstall()?;
    }
    // Windows finishes deleting a service only once every handle to it is closed (e.g. an open
    // Services window), so retry "marked for deletion" / "exists" for a short while.
    let since = Instant::now();
    loop {
        match install() {
            Err(e) if since.elapsed() < Duration::from_secs(15) && is_pending_delete(&e) => sleep(Duration::from_millis(500)),
            result => return result,
        }
    }
}

fn is_pending_delete(e: &anyhow::Error) -> bool {
    const ERROR_SERVICE_MARKED_FOR_DELETE: i32 = 1072;
    const ERROR_SERVICE_EXISTS: i32 = 1073;
    e.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .is_some_and(|code| code == ERROR_SERVICE_MARKED_FOR_DELETE || code == ERROR_SERVICE_EXISTS)
    })
}

/// Stops (fans go to the fail-safe duty) and removes the service.
pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the service manager (run as administrator)")?;
    let service = manager
        .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
        .context("opening the service (is it installed?)")?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        let since = Instant::now();
        while service.query_status()?.current_state != ServiceState::Stopped && since.elapsed() < Duration::from_secs(15) {
            sleep(Duration::from_millis(250));
        }
    }
    service.delete()?;
    remove_shortcuts();
    println!("Service `{NAME}` stopped and removed. The fans stay at the fail-safe duty until something else sets them.");
    Ok(())
}

const SHORTCUT_FILE: &str = "FanConnect II Controller.lnk";
const SHORTCUT_DESCRIPTION: &str = "FanConnect II fan control for the ASUS ROG Strix RTX 2080 Ti";

/// The all-users Start menu and desktop shortcuts. All-users locations, because the elevated
/// install may run under a different administrator account than the person using the PC.
fn shortcut_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(program_data) = std::env::var_os("ProgramData") {
        paths.push(std::path::Path::new(&program_data).join(r"Microsoft\Windows\Start Menu\Programs").join(SHORTCUT_FILE));
    }
    if let Some(public) = std::env::var_os("PUBLIC") {
        paths.push(std::path::Path::new(&public).join("Desktop").join(SHORTCUT_FILE));
    }
    paths
}

/// Creates the shortcuts to the GUI with Windows' own shortcut object (WScript.Shell).
fn create_shortcuts(gui: &std::path::Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // PowerShell single-quoted strings: a quote is escaped by doubling it.
    let quote = |p: &std::path::Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let folder = gui.parent().unwrap_or(gui);
    let mut script = String::from("$ErrorActionPreference = 'Stop'; $shell = New-Object -ComObject WScript.Shell;");
    for link in shortcut_paths() {
        script.push_str(&format!(
            " $s = $shell.CreateShortcut({}); $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.Description = '{}'; $s.Save();",
            quote(&link),
            quote(gui),
            quote(folder),
            SHORTCUT_DESCRIPTION
        ));
    }
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("running PowerShell")?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

fn remove_shortcuts() {
    for path in shortcut_paths() {
        let _ = std::fs::remove_file(path);
    }
}
