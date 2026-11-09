//! Shared engine for termax front-ends: configuration, host list, terminal
//! sessions and file-system helpers. Contains no UI code.

pub mod config;
pub mod fs;
pub mod hosts;
pub mod sshconfig;
pub mod state;
pub mod terminal;
pub mod theme;
