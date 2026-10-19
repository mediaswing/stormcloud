//! StormCloud: a console for managing Microsoft Entra ID, in the style of a classic MMC snap-in.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod auth;
mod bulk;
mod config;
mod db;
mod dialogs;
mod exchange;
mod graph;
mod logging;
mod model;
mod ops;
mod platform;
mod sheets;
mod update;

use std::process::ExitCode;

const USAGE: &str = "Graphical Cloud Management

Usage:
  stormcloud                     Start the application
  stormcloud --version           Print the version
  stormcloud --help              Print this help
  stormcloud --package-macos DIR Build an installer package (.pkg) of the app in DIR (macOS)
  stormcloud --package-deb DIR   Build a .deb package in DIR (Linux)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_gui(),
        Some("--version" | "-V") => {
            println!("stormcloud {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        #[cfg(target_os = "macos")]
        Some("--package-macos") => package(args.get(1)),
        #[cfg(target_os = "linux")]
        Some("--package-deb") => package(args.get(1)),
        Some(other) => {
            eprintln!("Unknown option: {other}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn package(dir: Option<&String>) -> ExitCode {
    let dir = std::path::PathBuf::from(dir.map(String::as_str).unwrap_or("dist"));
    match platform::package(&dir) {
        Ok(path) => {
            println!("Created {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Packaging failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_gui() -> ExitCode {
    let settings = config::Settings::load().unwrap_or_default();
    logging::init(logging::parse_level(&settings.log_level));
    log::info!(
        "StormCloud {} starting on {} {}; settings: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        config::settings_path().display()
    );
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Graphical Cloud Management — Microsoft Entra ID")
            // Matches the .desktop file name so Linux desktops attach the launcher to the window.
            .with_app_id("stormcloud")
            .with_inner_size([1280.0, 780.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/stormcloud.png"))
                    .expect("the bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    let result = eframe::run_native("StormCloud", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))));
    let code = match &result {
        Ok(()) => {
            log::info!("StormCloud exited");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::error!("StormCloud exited with error: {e}");
            ExitCode::FAILURE
        }
    };
    log::logger().flush();
    code
}
