//! A small SSH/SFTP server bound to 127.0.0.1 for skiff tests.
//!
//! It exists so no test ever needs a real server. Nothing here is shipped.

mod fixture;
mod server;
mod sftp_fs;

pub use fixture::{FixtureAuth, TestFixture};
pub use server::{ServerOptions, TestServer};
