//! Shared engine for the skiff front-ends: configuration, host list, terminal
//! sessions and file-system helpers. Contains no UI code.
//!
//! Both the terminal UI (`skiff-ssh`, binary `skiff`) and the desktop GUI
//! (`skiff-gui`) are thin layers over this crate. SSH itself is done by the
//! system OpenSSH client: interactive sessions run `ssh` in a pseudo-terminal,
//! and SFTP/port forwards spawn `ssh` as a subprocess.
//!
//! Main modules:
//! - `config`, `state`, `history`, `theme`, `migrate`: skiff's own files
//!   (`config.toml`, `state.toml`, connection history, colour themes) and
//!   their upgrade path.
//! - `sshconfig`, `hosts`, `hostform`: reading and minimally editing
//!   `~/.ssh/config`, building the host list, and the add/edit host form.
//! - `cli`: command-line argument parsing shared by both front-ends.
//! - `terminal`, `termx`: interactive SSH tabs and terminal helpers (search,
//!   paste confirmation, mouse reporting).
//! - `fs`: local and SFTP file access and transfers.
//! - `askpass`, `sshver`: answering ssh's prompts from the UI, and checking the
//!   OpenSSH version supports it.
//! - `forward`, `status`: port forwards and opt-in reachability probes.

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
