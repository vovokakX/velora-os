// Link like the other user programs (see userland/build-support.rs).
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let script = dir.join("../../userland/link.ld");
    println!("cargo:rustc-link-arg-bins=-T{}", script.display());
    println!("cargo:rustc-link-arg-bins=-znoexecstack");
    println!("cargo:rustc-link-arg-bins=-no-pie");
    println!("cargo:rustc-link-arg-bins=-znorelro");
    println!("cargo:rerun-if-changed={}", script.display());
}
