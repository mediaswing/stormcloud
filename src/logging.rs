//! Logging to `output.log` in the configuration folder (next to `settings.ini`).
//!
//! Never log access/refresh tokens, passwords or client secrets. At `debug` and `trace` the log
//! also has the path of every request, which can name users, so only the owner can read it.

use crate::config::config_dir;
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log is moved to `output.log.1`, replacing the one there, once it reaches this size.
const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn log_path() -> PathBuf {
    config_dir().join("output.log")
}

/// The log file, which starts afresh when it gets too big.
struct LogFile {
    path: PathBuf,
    /// None when it couldn't be opened again after starting afresh.
    file: Option<File>,
    written: u64,
    max: u64,
}

impl LogFile {
    /// Opens the log at `path` to add to it, first starting afresh if it is too big already.
    fn open(path: &Path, max: u64) -> std::io::Result<LogFile> {
        let written = fs::metadata(path).map_or(0, |m| m.len());
        let mut log = LogFile { path: path.to_path_buf(), file: None, written, max };
        if written >= max {
            log.rotate()?;
        } else {
            log.file = Some(open_private(path)?);
        }
        Ok(log)
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.written > 0 && self.written + bytes.len() as u64 > self.max {
            // A log that can't be rotated carries on in the file it has, if any.
            let _ = self.rotate();
        }
        if let Some(f) = &mut self.file
            && f.write_all(bytes).is_ok() {
                self.written += bytes.len() as u64;
            }
    }

    /// Moves the log to `.1` and starts a new one. The file is closed first, as Windows can't
    /// move a file that is open.
    fn rotate(&mut self) -> std::io::Result<()> {
        if let Some(mut f) = self.file.take() {
            let _ = f.flush();
        }
        let _ = fs::rename(&self.path, self.path.with_extension("log.1"));
        self.written = 0;
        self.file = Some(open_private(&self.path)?);
        Ok(())
    }
}

/// Opens `path` to add to it, making it readable by its owner only on Unix.
fn open_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        // A log made by an earlier version may be readable by everyone. Some file systems (FAT,
        // network shares) have no such permissions and refuse, which mustn't stop the log.
        if path.exists() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
    options.open(path)
}

struct FileLogger {
    file: Mutex<LogFile>,
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
        if let Ok(mut log) = self.file.lock() {
            log.write(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut log) = self.file.lock()
            && let Some(f) = &mut log.file {
                let _ = f.flush();
            }
    }
}

/// Opens (and if necessary rotates) the log file and installs the logger and a panic hook.
/// Logging failures never stop the app from starting.
pub fn init(level: LevelFilter) {
    let path = log_path();
    let _ = fs::create_dir_all(config_dir());
    let file = match LogFile::open(&path, MAX_BYTES) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("StormCloud: could not open {}: {e}", path.display());
            return;
        }
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
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
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
        assert_eq!(parse_level(""), LevelFilter::Info);
        assert_eq!(parse_level("Debug"), LevelFilter::Debug);
        assert_eq!(parse_level("off"), LevelFilter::Off);
    }

    #[test]
    fn starts_afresh_when_too_big() {
        let dir = std::env::temp_dir().join(format!("stormcloud-log-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output.log");
        let old = path.with_extension("log.1");
        // Too big already, from an earlier run.
        fs::write(&path, "x".repeat(30)).unwrap();
        let mut log = LogFile::open(&path, 25).unwrap();
        assert_eq!(fs::read_to_string(&old).unwrap().len(), 30);
        // And again while running: the line that doesn't fit starts the new file.
        log.write(b"first line, 20 bytes");
        log.write(b"second line\n");
        assert_eq!(fs::read_to_string(&old).unwrap(), "first line, 20 bytes");
        assert_eq!(fs::read_to_string(&path).unwrap(), "second line\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(log);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn owner_only_even_if_made_readable_before() {
        let dir = std::env::temp_dir().join(format!("stormcloud-log-perm-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output.log");
        fs::write(&path, "old\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            drop(LogFile::open(&path, MAX_BYTES).unwrap());
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timestamp_shape() {
        let t = timestamp();
        assert_eq!(t.len(), 24);
        assert!(t.ends_with('Z'));
    }
}
