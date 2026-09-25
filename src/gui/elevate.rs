//! Runs `gpu-fanctl install|uninstall|reinstall` with administrator rights and reports its
//! outcome; the GUI itself stays unelevated. Windows asks through the UAC prompt (hidden console),
//! Linux through pkexec (the desktop's polkit password prompt).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{env, fs, thread};

use eframe::egui;

#[cfg(windows)]
const CLI_NAME: &str = "gpu-fanctl.exe";
#[cfg(not(windows))]
const CLI_NAME: &str = "gpu-fanctl";

/// The command-line tool next to this GUI program, which the service will run.
pub fn cli_path() -> Option<PathBuf> {
    let path = env::current_exe().ok()?.with_file_name(CLI_NAME);
    path.exists().then_some(path)
}

/// A running or finished elevated action.
pub struct Job {
    pub action: &'static str,
    pub outcome: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Job {
    pub fn finished(&self) -> Option<Result<(), String>> {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Starts `gpu-fanctl <action>` elevated in the background.
pub fn start(action: &'static str, ctx: egui::Context) -> Job {
    let outcome = Arc::new(Mutex::new(None));
    let shared = Arc::clone(&outcome);
    thread::spawn(move || {
        let result = match cli_path() {
            Some(exe) => run_elevated(&exe, action),
            None => Err(format!("{CLI_NAME} was not found next to this app")),
        };
        *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        ctx.request_repaint();
    });
    Job { action, outcome }
}

/// Runs the action and returns its outcome. The elevated program writes "ok" or its error into
/// a result file in a private folder (a folder, because Linux may not let root write into a
/// user's file directly in /tmp).
fn run_elevated(exe: &Path, action: &str) -> Result<(), String> {
    let dir = env::temp_dir().join(format!("gpu-fanctl-{action}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let result_file = dir.join("result.txt");

    let exit_code = launch(exe, action, &result_file);
    let reported = fs::read_to_string(&result_file).ok();
    let _ = fs::remove_dir_all(&dir);
    let exit_code = exit_code?;
    match reported.as_deref().map(str::trim) {
        Some("ok") => Ok(()),
        Some(error) if !error.is_empty() => Err(error.to_string()),
        _ if exit_code == 0 => Ok(()),
        _ => Err(format!("gpu-fanctl {action} failed (exit code {exit_code})")),
    }
}

#[cfg(windows)]
fn launch(exe: &Path, action: &str, result_file: &Path) -> Result<i64, String> {
    use std::ffi::OsStr;
    use std::io;
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    const ERROR_CANCELLED: u32 = 1223;
    let wide = |s: &OsStr| s.encode_wide().chain(once(0)).collect::<Vec<u16>>();

    let parameters = format!("{action} --result \"{}\"", result_file.display());
    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(OsStr::new(&parameters));
    // SAFETY: the struct is zero-initialised as the API allows, and every string it points to
    // outlives the call. The returned process handle is closed below.
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(match GetLastError() {
                ERROR_CANCELLED => "Cancelled at the administrator prompt.".into(),
                code => format!("Could not start {CLI_NAME}: {}", io::Error::from_raw_os_error(code as i32)),
            });
        }
        if info.hProcess.is_null() {
            return Err(format!("{CLI_NAME} started without a process handle"));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        Ok(i64::from(code))
    }
}

#[cfg(not(windows))]
fn launch(exe: &Path, action: &str, result_file: &Path) -> Result<i64, String> {
    // pkexec exit codes: 126 = the prompt was dismissed, 127 = not authorised.
    let status = std::process::Command::new("pkexec")
        .arg(exe)
        .args([action, "--result"])
        .arg(result_file)
        .status()
        .map_err(|e| format!("Could not start pkexec ({e}). Is polkit installed?"))?;
    match status.code() {
        Some(126) => Err("Cancelled at the administrator prompt.".into()),
        Some(127) => Err("Not authorised at the administrator prompt.".into()),
        Some(code) => Ok(i64::from(code)),
        None => Err(format!("gpu-fanctl {action} was terminated")),
    }
}
