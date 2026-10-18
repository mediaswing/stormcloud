//! Debug logging to `output.log` in the configuration folder (next to `settings.ini`).
//!
//! Never log access/refresh tokens, passwords or client secrets.

use crate::config::config_dir;
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log is rotated to `output.log.1` at startup once it exceeds this size.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_path() -> PathBuf {
    config_dir().join("output.log")
}

struct FileLogger {
    file: Mutex<File>,
    /// Level for StormCloud's own messages.
    level: LevelFilter,
}

impl Log for FileLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        if m.target().starts_with("stormcloud") {
            m.level() <= self.level
        } else {
            // Dependencies (egui, wgpu, reqwest, …) are very chatty below Warn.
            m.level() <= Level::Warn
        }
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let target = r.target().strip_prefix("stormcloud::").unwrap_or(r.target());
        let thread = std::thread::current();
        let line = format!(
            "{} {:<5} [{}] ({}) {}\n",
            timestamp(),
            r.level(),
            target,
            thread.name().unwrap_or("worker"),
            r.args()
        );
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// Opens (and if necessary rotates) the log file and installs the logger and a panic hook.
/// Logging failures never stop the app from starting.
pub fn init(level: LevelFilter) {
    let path = log_path();
    let _ = fs::create_dir_all(config_dir());
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        eprintln!("StormCloud: could not open {}", path.display());
        return;
    };
    let logger = FileLogger { file: Mutex::new(file), level };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level.max(LevelFilter::Warn));
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        log::logger().flush();
        default_hook(info);
    }));
}

pub fn parse_level(s: &str) -> LevelFilter {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" | "warning" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Debug,
    }
}

/// UTC timestamp with milliseconds, e.g. `2026-10-14T13:45:01.123Z`.
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let (y, m, d) = crate::ops::civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(parse_level("WARNING"), LevelFilter::Warn);
        assert_eq!(parse_level(""), LevelFilter::Debug);
        assert_eq!(parse_level("off"), LevelFilter::Off);
    }

    #[test]
    fn timestamp_shape() {
        let t = timestamp();
        assert_eq!(t.len(), 24);
        assert!(t.ends_with('Z'));
    }
}
