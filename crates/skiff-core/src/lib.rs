//! Shared engine for skiff front-ends: configuration, host list, terminal
//! sessions and file-system helpers. Contains no UI code.

pub mod askpass;
pub mod cli;
pub mod config;
pub mod forward;
pub mod fs;
pub mod history;
pub mod hostform;
pub mod hosts;
pub mod migrate;
pub mod sshconfig;
pub mod sshver;
pub mod state;
pub mod status;
pub mod terminal;
pub mod termx;
pub mod theme;
