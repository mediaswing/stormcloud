//! Packaging for macOS: `Graphical Cloud Management.app` inside an installer package (.pkg).

use super::{remove, run_tool, set_mode};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "Graphical Cloud Management";
/// The bundle's name before the rename, removed when upgrading (see `postinstall_script`).
const OLD_APP_NAME: &str = "Graphical Cloud Manager";
/// Reverse-DNS identifier for the bundle and the installer package. macOS keys preferences and
/// permissions on it, and `pkgutil` records the installation under it, so never change it.
const BUNDLE_ID: &str = "io.github.mediaswing.stormcloud";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMPONENT_PKG: &str = "stormcloud-app.pkg";

const CODESIGN: &str = "/usr/bin/codesign";
const PKGBUILD: &str = "/usr/bin/pkgbuild";
const PRODUCTBUILD: &str = "/usr/bin/productbuild";

/// Builds `Graphical Cloud Management.app` from the running executable, signs it ad hoc and writes an
/// installer package for it to `out_dir`. Returns the package's path. The bundle is left in
/// `out_dir/root` for trying it out.
pub fn package(out_dir: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not locate the running executable")?;
    fs::create_dir_all(out_dir).with_context(|| format!("could not create {}", out_dir.display()))?;

    // pkgbuild installs everything in the root folder, so it holds only the bundle.
    let root = out_dir.join("root");
    remove(&root)?;
    let app = root.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    let macos_dir = contents.join("MacOS");
    fs::create_dir_all(&macos_dir)?;
    fs::create_dir_all(contents.join("Resources"))?;

    let bin = macos_dir.join("stormcloud");
    fs::copy(&exe, &bin).context("could not copy the executable into the bundle")?;
    set_mode(&bin, 0o755)?;
    fs::write(contents.join("Info.plist"), info_plist())?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;
    // The GPL and, as the fonts are compiled into the binary, their licence travel with it, as
    // does the credit the icon's CC BY 4.0 licence requires.
    fs::write(contents.join("Resources/LICENSE.txt"), include_str!("../../LICENSE"))?;
    fs::write(contents.join("Resources/fonts-OFL.txt"), include_str!("../../assets/fonts/OFL.txt"))?;
    fs::write(contents.join("Resources/icon-CREDITS.txt"), include_str!("../../assets/icon/CREDITS.txt"))?;
    fs::write(contents.join("Resources/stormcloud.icns"), include_bytes!("../../assets/icon/stormcloud.icns"))?;

    // Ad hoc signature ("-" identity): Apple Silicon refuses to run unsigned arm64 code, and the
    // bundle's seal must cover Info.plist. The hardened runtime is enabled for good measure.
    run_tool(
        Command::new(CODESIGN)
            .args(["--force", "--sign", "-", "--timestamp=none", "--options", "runtime"])
            .arg(&app),
    )?;
    run_tool(Command::new(CODESIGN).args(["--verify", "--strict", "--verbose=2"]).arg(&app))?;

    // The component package installs the bundle; the product package around it gives the
    // installer its title and refuses Intel Macs and macOS versions the app can't run on.
    let work = out_dir.join("pkg-work");
    remove(&work)?;
    fs::create_dir_all(&work)?;
    let component_plist = work.join("component.plist");
    fs::write(&component_plist, component_plist_xml())?;
    let distribution = work.join("distribution.xml");
    fs::write(&distribution, distribution_xml())?;
    let scripts = work.join("scripts");
    fs::create_dir_all(&scripts)?;
    let postinstall = scripts.join("postinstall");
    fs::write(&postinstall, postinstall_script())?;
    set_mode(&postinstall, 0o755)?;

    run_tool(
        Command::new(PKGBUILD)
            .arg("--root")
            .arg(&root)
            .arg("--component-plist")
            .arg(&component_plist)
            .arg("--scripts")
            .arg(&scripts)
            .args(["--identifier", BUNDLE_ID, "--version", VERSION])
            .args(["--install-location", "/Applications"])
            .arg(work.join(COMPONENT_PKG)),
    )?;

    let pkg = out_dir.join(format!("stormcloud-{VERSION}-macos-{}.pkg", std::env::consts::ARCH));
    remove(&pkg)?;
    run_tool(
        Command::new(PRODUCTBUILD)
            .arg("--distribution")
            .arg(&distribution)
            .arg("--package-path")
            .arg(&work)
            .arg(&pkg),
    )?;
    remove(&work)?;
    Ok(pkg)
}

/// Versions before the rename to Graphical Cloud Management were installed as
/// `Graphical Cloud Manager.app`. The installer leaves files from earlier versions alone, so this
/// removes that bundle, but only if it is this app. `$2` is the folder the bundle went into.
fn postinstall_script() -> String {
    format!(
        r#"#!/bin/sh
old="$2/{OLD_APP_NAME}.app"
if [ -d "$old" ] && [ "$(/usr/bin/defaults read "$old/Contents/Info" CFBundleIdentifier 2>/dev/null)" = "{BUNDLE_ID}" ]; then
    /bin/rm -rf "$old"
fi
exit 0
"#
    )
}

/// Without this, pkgbuild marks the bundle relocatable, and the installer would update a copy
/// found anywhere else on the Mac instead of putting it in /Applications.
fn component_plist_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
    <dict>
        <key>BundleHasStrictIdentifier</key><true/>
        <key>BundleIsRelocatable</key><false/>
        <key>BundleIsVersionChecked</key><false/>
        <key>BundleOverwriteAction</key><string>upgrade</string>
        <key>RootRelativeBundlePath</key><string>{APP_NAME}.app</string>
    </dict>
</array>
</plist>
"#
    )
}

fn distribution_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<installer-gui-script minSpecVersion="2">
    <title>{APP_NAME}</title>
    <options customize="never" require-scripts="false" hostArchitectures="arm64"/>
    <domains enable_localSystem="true"/>
    <volume-check>
        <allowed-os-versions><os-version min="11.0"/></allowed-os-versions>
    </volume-check>
    <choices-outline>
        <line choice="{BUNDLE_ID}"/>
    </choices-outline>
    <choice id="{BUNDLE_ID}" visible="false">
        <pkg-ref id="{BUNDLE_ID}"/>
    </choice>
    <pkg-ref id="{BUNDLE_ID}" version="{VERSION}" onConclusion="none">{COMPONENT_PKG}</pkg-ref>
</installer-gui-script>
"#
    )
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>{APP_NAME}</string>
    <key>CFBundleDisplayName</key><string>{APP_NAME}</string>
    <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
    <key>CFBundleExecutable</key><string>stormcloud</string>
    <key>CFBundleIconFile</key><string>stormcloud</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>{VERSION}</string>
    <key>CFBundleVersion</key><string>{VERSION}</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSArchitecturePriority</key><array><string>arm64</string></array>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
"#
    )
}
