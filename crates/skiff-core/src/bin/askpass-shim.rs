//! Test helper: the askpass program used by skiff-core's own tests.
//! Front-ends call `skiff_core::askpass::helper_main()` from their own binary.

/// Runs the askpass helper and exits with its status.
fn main() {
    std::process::exit(skiff_core::askpass::helper_main());
}
