//! Platform-specific code. For now this is packaging: a release build can wrap itself into the
//! native package for the platform it runs on, which `.github/workflows/release.yml` uses.
//!
//! - macOS (Apple Silicon): `stormcloud --package-macos DIR` builds `Graphical Cloud Management.app`,
//!   signs it ad hoc and wraps it in an installer package (.pkg) that puts it in /Applications.
//! - Linux (Ubuntu/Debian): `stormcloud --package-deb DIR` builds a .deb with `dpkg-deb`.
//!
//! The Windows .msi is built by WiX Toolset from `src/installer/stormcloud.wxs` instead.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::package;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::package;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use anyhow::{Context, Result, bail};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;

/// Runs a packaging tool, failing with its stderr if it exits unsuccessfully.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_tool(cmd: &mut Command) -> Result<()> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let output = cmd.output().with_context(|| format!("could not run {program}"))?;
    if !output.status.success() {
        bail!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("could not set permissions on {}", path.display()))
}

/// Removes `path` if it exists, whether a file or a folder.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remove(path: &Path) -> Result<()> {
    let result = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("could not remove {}", path.display()))
        }
        _ => Ok(()),
    }
}
