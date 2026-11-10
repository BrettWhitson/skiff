//! File-transfer tabs: two panes, each Local or any host.

use termax_core::hosts::HostEntry;

#[derive(Debug, Clone)]
pub enum Location {
    Local,
    Host(HostEntry),
}

impl Location {
    pub fn label(&self) -> &str {
        match self {
            Location::Local => "Local",
            Location::Host(h) => &h.name,
        }
    }
}

pub struct FilesTab {
    pub locations: [Location; 2],
}

impl FilesTab {
    pub fn new(left: Location, right: Location) -> Self {
        FilesTab { locations: [left, right] }
    }
}
