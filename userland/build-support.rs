// Shared build script for user programs: link with userland/link.ld.
// Each program's build.rs includes this file.

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let script = dir.join("..").join("link.ld");
    println!("cargo:rustc-link-arg-bins=-T{}", script.display());
    println!("cargo:rustc-link-arg-bins=-znoexecstack");
    println!("cargo:rustc-link-arg-bins=-no-pie");
    // No RELRO: it would put read-only and writable data on one page.
    println!("cargo:rustc-link-arg-bins=-znorelro");
    println!("cargo:rerun-if-changed={}", script.display());
}
