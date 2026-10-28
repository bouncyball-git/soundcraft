//! Windows only: embed the app icon and version info (VERSIONINFO) into `soundcraft.exe`, so it
//! shows in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `SOUNDCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).
//!
//! Release builds with the MSVC toolchain also link without a `.pdb` (see `main`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/soundcraft.ico");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_REQUIRE_WINRES");
    // The release profile strips symbols, but with MSVC rustc still links with /DEBUG, which
    // writes soundcraft.pdb and points the .exe at it. /DEBUG:NONE (after it, so it wins) writes
    // none. Debug builds keep theirs for readable stack traces.
    if std::env::var("PROFILE").as_deref() == Ok("release") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEBUG:NONE");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/soundcraft.ico")
        .set("ProductName", "SoundCraft")
        .set("FileDescription", "SoundCraft digital audio workstation")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the SoundCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "soundcraft.exe")
        .set("InternalName", "soundcraft");
    if let Err(e) = res.compile() {
        if std::env::var_os("SOUNDCRAFT_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=soundcraft.exe built without icon/version resources: {e}");
    }
}
