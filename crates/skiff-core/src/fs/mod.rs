//! File access for local and remote (SFTP) locations, and transfers between
//! any two of them.

pub mod connect;
pub mod endpoint;
pub mod path;
pub mod transfer;

pub use path::{FileEntry, human_size, join, parent};
