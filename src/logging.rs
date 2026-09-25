//! Log lines go to stderr (the journal under systemd) and, once `to_file` is called
//! (the Windows service), to a log file with timestamps.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// A log file bigger than this is started over when the service starts.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Also write log lines to `path`, creating its directory.
pub fn to_file(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let too_big = fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    let file = OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(path)?;
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    Ok(())
}

pub fn write(message: &str) {
    eprintln!("{message}");
    if let Some(file) = FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(file, "{} {message}", humantime::format_rfc3339_seconds(SystemTime::now()));
    }
}

/// `log!("...", args)`: formats like `format!` and writes a log line.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::logging::write(&format!($($arg)*))
    };
}
