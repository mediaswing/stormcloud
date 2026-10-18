//! Packaging for Linux: a .deb for Ubuntu and Debian.

use super::{remove, run_tool, set_mode};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DPKG_DEB: &str = "/usr/bin/dpkg-deb";

/// What eframe/winit/wgpu load at run time. They are `dlopen`ed, so the linker can't see them.
/// `ca-certificates` holds the roots rustls checks Microsoft's certificates against.
const DEB_DEPENDS: &str = "libc6, libgcc-s1, ca-certificates, libxkbcommon0, libxkbcommon-x11-0, \
                           libwayland-client0, libwayland-egl1, libx11-6, libxcursor1, libxrandr2, \
                           libxi6, libegl1, libgl1";
/// `xdg-desktop-portal` shows the Open and Save dialogs for CSV import and export.
const DEB_RECOMMENDS: &str = "libvulkan1, mesa-vulkan-drivers, at-spi2-core, xdg-desktop-portal";

const DESKTOP_ENTRY: &str = "[Desktop Entry]
Type=Application
Name=Graphical Cloud Management
GenericName=Entra ID Console
Comment=Console for managing Microsoft Entra ID
Exec=stormcloud
Icon=stormcloud
Terminal=false
Categories=System;Network;
StartupWMClass=stormcloud
";

const COPYRIGHT: &str = "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: StormCloud
Source: https://github.com/mediaswing/stormcloud

Files: *
Copyright: 2026 The Graphical Cloud Management contributors
License: GPL-3.0-or-later
 On Debian systems, the full text of the GNU General Public License
 version 3 can be found in /usr/share/common-licenses/GPL-3.

Files: assets/fonts/*
Copyright: Google LLC
License: OFL-1.1
 The Google Sans fonts are built into the program.
 See /usr/share/doc/stormcloud/fonts-OFL.txt.

Files: assets/icon/*
Copyright: IconMarket
License: CC-BY-4.0
 The safe icon from the Casino pack (https://icon-icons.com/pack/casino/4016),
 installed as SVG and as a PNG made from it. See
 /usr/share/doc/stormcloud/icon-CREDITS.txt and
 https://creativecommons.org/licenses/by/4.0/.
";

/// Builds `stormcloud_<version>_<arch>.deb` from the running executable in `out_dir` and
/// returns its path.
pub fn package(out_dir: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not locate the running executable")?;
    fs::create_dir_all(out_dir).with_context(|| format!("could not create {}", out_dir.display()))?;
    let arch = deb_arch()?;
    let name = format!("stormcloud_{VERSION}_{arch}");
    let stage = out_dir.join(format!("{name}-stage"));
    remove(&stage)?;
    let deb = out_dir.join(format!("{name}.deb"));
    let result = stage_and_build(&exe, &stage, &deb, arch);
    remove(&stage)?;
    result.map(|()| deb)
}

fn stage_and_build(exe: &Path, stage: &Path, deb: &Path, arch: &str) -> Result<()> {
    let files: [(&str, u32, Vec<u8>); 7] = [
        ("usr/bin/stormcloud", 0o755, fs::read(exe).context("could not read the executable")?),
        ("usr/share/applications/stormcloud.desktop", 0o644, DESKTOP_ENTRY.into()),
        (
            "usr/share/icons/hicolor/256x256/apps/stormcloud.png",
            0o644,
            include_bytes!("../../assets/icon/stormcloud.png").to_vec(),
        ),
        (
            "usr/share/icons/hicolor/scalable/apps/stormcloud.svg",
            0o644,
            include_bytes!("../../assets/icon/stormcloud.svg").to_vec(),
        ),
        ("usr/share/doc/stormcloud/copyright", 0o644, COPYRIGHT.into()),
        (
            "usr/share/doc/stormcloud/fonts-OFL.txt",
            0o644,
            include_bytes!("../../assets/fonts/OFL.txt").to_vec(),
        ),
        (
            "usr/share/doc/stormcloud/icon-CREDITS.txt",
            0o644,
            include_bytes!("../../assets/icon/CREDITS.txt").to_vec(),
        ),
    ];
    let mut size = 0;
    for (rel, mode, bytes) in &files {
        let path = stage.join(rel);
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
        fs::write(&path, bytes).with_context(|| format!("could not write {}", path.display()))?;
        set_mode(&path, *mode)?;
        size += bytes.len() as u64;
    }
    // dpkg-deb rejects packages whose folders aren't 0755.
    for dir in [
        "DEBIAN",
        "usr",
        "usr/bin",
        "usr/share",
        "usr/share/applications",
        "usr/share/doc",
        "usr/share/doc/stormcloud",
        "usr/share/icons",
        "usr/share/icons/hicolor",
        "usr/share/icons/hicolor/256x256",
        "usr/share/icons/hicolor/256x256/apps",
        "usr/share/icons/hicolor/scalable",
        "usr/share/icons/hicolor/scalable/apps",
    ] {
        let path = stage.join(dir);
        fs::create_dir_all(&path)?;
        set_mode(&path, 0o755)?;
    }
    let control = stage.join("DEBIAN/control");
    fs::write(&control, control_file(arch, size.div_ceil(1024)))?;
    set_mode(&control, 0o644)?;

    run_tool(Command::new(DPKG_DEB).args(["--root-owner-group", "--build"]).arg(stage).arg(deb))
}

fn deb_arch() -> Result<&'static str> {
    Ok(match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        "x86" => "i386",
        other => bail!("no Debian architecture name known for {other}"),
    })
}

fn control_file(arch: &str, installed_size_kib: u64) -> String {
    format!(
        "Package: stormcloud\n\
         Version: {VERSION}\n\
         Architecture: {arch}\n\
         Maintainer: wrichards <mediaswing@outlook.com>\n\
         Installed-Size: {installed_size_kib}\n\
         Depends: {DEB_DEPENDS}\n\
         Recommends: {DEB_RECOMMENDS}\n\
         Section: admin\n\
         Priority: optional\n\
         Homepage: https://github.com/mediaswing/stormcloud\n\
         Description: Console for managing Microsoft Entra ID\n \
         A desktop console for Entra ID users, groups, devices and licences,\n \
         laid out like a classic MMC snap-in. Talks to Microsoft Graph directly.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packages the test binary and, where dpkg is installed (as on the release runner), checks
    /// that dpkg accepts the result.
    #[test]
    fn builds_a_deb() {
        if !Path::new(DPKG_DEB).exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("stormcloud-deb-test-{}", std::process::id()));
        let deb = package(&dir).unwrap();
        let info = Command::new(DPKG_DEB).arg("--info").arg(&deb).output().unwrap();
        assert!(info.status.success());
        assert!(String::from_utf8_lossy(&info.stdout).contains("Package: stormcloud"));
        let contents = Command::new(DPKG_DEB).arg("--contents").arg(&deb).output().unwrap();
        let listing = String::from_utf8_lossy(&contents.stdout);
        for path in [
            "./usr/bin/stormcloud",
            "./usr/share/applications/stormcloud.desktop",
            "./usr/share/icons/hicolor/256x256/apps/stormcloud.png",
        ] {
            assert!(listing.contains(path), "{path} missing from:\n{listing}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
