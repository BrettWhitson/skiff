//! A small SSH/SFTP server bound to 127.0.0.1 for skiff tests.
//!
//! It exists so no test ever needs a real server. Nothing here is shipped.
//!
//! - `server`: [`TestServer`], an in-process russh server on its own thread and
//!   tokio runtime. It offers key or password auth for one user, an SFTP
//!   subsystem, an optional echoing shell, echo/`exit N` exec commands and local
//!   (`ssh -L`) forwards to 127.0.0.1 only.
//! - `sftp_fs`: the SFTP handler that serves one directory as `/` and refuses
//!   any path that would escape it.
//! - `fixture`: [`TestFixture`], a temp directory with generated keys, an empty
//!   ssh config, a known_hosts entry and a running server, plus the `ssh`
//!   options that keep the system client away from the user's own config, keys
//!   and agent.
//!
//! skiff-core, skiff-ssh (the TUI) and skiff-gui use this crate as a
//! dev-dependency; the `skiff-testserver` binary runs the same fixture by hand.

mod fixture;
mod server;
mod sftp_fs;

pub use fixture::{FixtureAuth, TestFixture};
pub use server::{ServerOptions, TestServer};
