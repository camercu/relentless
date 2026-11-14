//! Sets `cfg(os_clock)` on targets whose `std` can read the time.
//!
//! `wasm*-unknown-unknown` (wasm32 and wasm64) has no OS clock:
//! `std::time::Instant::now` and `SystemTime::now` build there but panic.
//! Code that reads them is gated on `os_clock`, so on that target it is
//! compiled out instead.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(os_clock)");
    let family = std::env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let wasm = family.split(',').any(|f| f == "wasm");
    if !(wasm && os == "unknown") {
        println!("cargo::rustc-cfg=os_clock");
    }
}
