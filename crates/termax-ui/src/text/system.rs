//! Well-known system font locations, per platform.

use std::path::PathBuf;

use super::FontKind;

fn in_dirs(dirs: &[PathBuf], names: &[&str]) -> Vec<PathBuf> {
    names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).collect()
}

fn font_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        vec![windir.join("Fonts")]
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")]
    } else {
        ["/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/TTF", "/usr/share/fonts/dejavu", "/usr/share/fonts/truetype"]
            .iter()
            .map(PathBuf::from)
            .collect()
    }
}

/// Primary-font candidates for `kind`, best first.
pub fn candidates(kind: FontKind) -> Vec<PathBuf> {
    let names: &[&str] = match (kind, cfg!(windows), cfg!(target_os = "macos")) {
        (FontKind::Ui, true, _) => &["segoeui.ttf", "arial.ttf"],
        (FontKind::Mono, true, _) => &["consola.ttf", "cour.ttf"],
        (FontKind::Ui, _, true) => &["Helvetica.ttc", "Geneva.ttf"],
        (FontKind::Mono, _, true) => &["Menlo.ttc", "Monaco.ttf"],
        (FontKind::Ui, _, _) => &["DejaVuSans.ttf"],
        (FontKind::Mono, _, _) => &["DejaVuSansMono.ttf"],
    };
    in_dirs(&font_dirs(), names)
}

/// Extra fonts tried for characters the primary lacks (symbols, arrows).
pub fn fallbacks() -> Vec<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["seguisym.ttf"] } else { &[] };
    in_dirs(&font_dirs(), names)
}
