//! Puts the app icon inside the Windows .exe, so Explorer, "Open with" and pinned taskbar buttons
//! show it. Building for Linux or macOS skips it.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon/icon.ico")
            .compile()
            .expect("the Windows resource compiler (rc.exe or windres) failed on the icon");
    }
}
