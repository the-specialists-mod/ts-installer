//! Stubs for the Steam API, so the launcher starts without steam_api next to it.
//!
//! `.cargo/config.toml` points steamworks-sys at empty stand-ins for the library, so nothing
//! provides its functions at link time. This writes one stub per function exported by the real
//! library (assets/steam/exports.txt), each jumping through a slot in a table that `src/steam.rs`
//! fills once it has loaded the library.

use std::fmt::Write as _;

fn main() {
    println!("cargo:rerun-if-changed=assets/steam/exports.txt");
    println!("cargo::rustc-check-cfg=cfg(steam_client)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if os == "windows" {
        embed_icon();
    }
    // The targets with a library in assets/steam (and steamworks in Cargo.toml).
    if arch != "x86_64" || !(os == "linux" || os == "windows") {
        return;
    }
    println!("cargo::rustc-cfg=steam_client");
    let exports = std::fs::read_to_string("assets/steam/exports.txt").unwrap();
    let names: Vec<&str> = exports.lines().map(str::trim).filter(|l| !l.is_empty()).collect();

    let mut out = String::new();
    writeln!(out, "pub(super) static EXPORTS: [&str; {}] = [", names.len()).unwrap();
    for n in &names {
        writeln!(out, "    {n:?},").unwrap();
    }
    writeln!(out, "];").unwrap();
    writeln!(
        out,
        "pub(super) static SLOTS: [std::sync::atomic::AtomicUsize; {}] = [const {{ std::sync::atomic::AtomicUsize::new(0) }}; {}];",
        names.len(),
        names.len()
    )
    .unwrap();
    writeln!(out, "std::arch::global_asm!(\n    \".text\",").unwrap();
    for (i, n) in names.iter().enumerate() {
        writeln!(out, "    \".globl {n}\",").unwrap();
        if os == "linux" {
            writeln!(out, "    \".type {n}, @function\",").unwrap();
        }
        writeln!(out, "    \".p2align 3\",\n    \"{n}:\",\n    \"jmp qword ptr [rip + {{slots}} + {}]\",", i * 8).unwrap();
    }
    writeln!(out, "    slots = sym SLOTS,\n);").unwrap();
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("steam_stubs.rs");
    std::fs::write(path, out).unwrap();
}

/// Puts assets/icon/ts-launcher.ico in the executable as icon resource 1, which Explorer shows and
/// GPUI loads for the window and taskbar.
fn embed_icon() {
    println!("cargo:rerun-if-changed=assets/icon/ts-launcher.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon_with_id("assets/icon/ts-launcher.ico", "1")
        .set("ProductName", "The Specialists Launcher")
        .set("FileDescription", "The Specialists Launcher");
    res.compile().expect("embed the launcher icon");
}
