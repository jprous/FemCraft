//! Embeds the application icon (`assets/icon/femcraft.ico`) into femcraft.exe on Windows, so
//! Explorer, shortcuts and the taskbar show it. Regenerate the icon with
//! `assets/icon/make-icon.ps1`.

use std::path::PathBuf;

fn main() {
    let ico = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon/femcraft.ico");
    println!("cargo:rerun-if-changed={}", ico.display());

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = ico.canonicalize().expect("assets/icon/femcraft.ico exists");
    // Resource script with an absolute, forward-slash path (accepted by windres and rc.exe).
    let path = ico.display().to_string().replace('\\', "/");
    let path = path.trim_start_matches("//?/");
    let rc = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("femcraft.rc");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write femcraft.rc");
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .expect("embed the application icon");
}
