//! Links Skiff's icon into the Windows .exe so Explorer, the taskbar and
//! shortcuts show it. Only for MSVC targets, whose linker takes the
//! prebuilt `assets/icon/skiff.res` (made by `gen_icons`) directly; every
//! other target, and a build without the file (e.g. from a packaged
//! crate), links as before.

use std::env;
use std::path::PathBuf;

/// Passes the icon resource to the linker for the binaries when it applies.
fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let res = manifest.join("../../assets/icon/skiff.res");
    let msvc = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if res.exists() {
        println!("cargo:rerun-if-changed={}", res.display());
        if msvc {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
    } else {
        println!("cargo:rerun-if-changed=build.rs");
    }
}
