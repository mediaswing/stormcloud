//! Embeds the app icon in the Windows exe, so Explorer, the taskbar and the Start menu shortcut
//! show it. Other platforms get the icon from their package (see `src/platform`).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/stormcloud.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon/stormcloud.ico")
            .compile()
            .expect("could not embed the icon in the exe");
    }
}
