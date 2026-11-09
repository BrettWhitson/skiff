//! Test helper: the askpass program used by termax-core's own tests.
//! Front-ends call `termax_core::askpass::helper_main()` from their own binary.

fn main() {
    std::process::exit(termax_core::askpass::helper_main());
}
