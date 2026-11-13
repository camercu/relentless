//! Sets `cfg(os_clock)` on targets whose `std` can read the time.
//!
//! `wasm32-unknown-unknown` has no OS clock: `std::time::Instant::now` and
//! `SystemTime::now` build there but panic. Code that reads them is gated on
//! `os_clock`, so on that target it is compiled out instead.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(os_clock)");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !(arch == "wasm32" && os == "unknown") {
        println!("cargo::rustc-cfg=os_clock");
    }
}
