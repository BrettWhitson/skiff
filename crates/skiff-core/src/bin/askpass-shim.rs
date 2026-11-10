//! Test helper: the askpass program used by skiff-core's own tests.
//! Front-ends call `skiff_core::askpass::helper_main()` from their own binary.

fn main() {
    std::process::exit(skiff_core::askpass::helper_main());
}
