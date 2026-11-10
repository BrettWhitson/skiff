//! Well-known system font locations, per platform.

use std::path::{Path, PathBuf};

use super::FontKind;

fn in_dirs(dirs: &[PathBuf], names: &[&str]) -> Vec<PathBuf> {
    names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).collect()
}

/// The directories system fonts are installed in.
pub fn font_dirs() -> Vec<PathBuf> {
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

/// Files larger than this (big CJK fonts) aren't read while scanning.
const MAX_SCAN_BYTES: u64 = 32 << 20;

/// Where installed fonts live, for listing them: the system folders plus
/// the per-user ones (Windows' per-user install, `~/Library/Fonts`,
/// `~/.local/share/fonts`). Scanned recursively.
pub fn scan_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dirs = if cfg!(windows) {
        let mut d = font_dirs();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            d.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
        d
    } else if cfg!(target_os = "macos") {
        let mut d = font_dirs();
        d.extend(home.map(|h| h.join("Library").join("Fonts")));
        d
    } else {
        let mut d = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(h) = home {
            d.push(h.join(".local").join("share").join("fonts"));
            d.push(h.join(".fonts"));
        }
        d
    };
    dirs.dedup();
    dirs
}

/// How deep `scan_fonts` looks below each folder.
const SCAN_DEPTH: usize = 4;

/// `(name, path, monospace)` of the fonts (`.ttf`, `.otf`, and the first
/// face of `.ttc`) in `dirs` and their subfolders, sorted by name.
/// Unreadable or invalid files are skipped.
pub fn scan_all(dirs: &[PathBuf]) -> Vec<(String, PathBuf, bool)> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < SCAN_DEPTH {
                    walk(&p, depth + 1, out);
                }
            } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["ttf", "otf", "ttc"].iter().any(|x| e.eq_ignore_ascii_case(x))) {
                out.push(p);
            }
        }
    }
    let mut files = vec![];
    for d in dirs {
        walk(d, 0, &mut files);
    }
    files.sort();
    files.dedup();
    let mut out: Vec<(String, PathBuf, bool)> = files.into_iter().filter_map(|p| font_info(&p).map(|(n, m)| (n, p, m))).collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.1.cmp(&b.1)));
    out
}

/// `(name, path)` of the fonts in `dirs` (see `scan_all`); only monospace
/// ones with `mono_only`.
pub fn scan_fonts(dirs: &[PathBuf], mono_only: bool) -> Vec<(String, PathBuf)> {
    scan_all(dirs).into_iter().filter(|(_, _, m)| *m || !mono_only).map(|(n, p, _)| (n, p)).collect()
}

/// A font file's full name and whether it is monospace (`i`, `M` and `0`
/// advance within 1% of each other).
fn font_info(path: &Path) -> Option<(String, bool)> {
    if std::fs::metadata(path).ok()?.len() > MAX_SCAN_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let face = ttf_parser::Face::parse(&data, 0).ok()?;
    let name = |id| face.names().into_iter().filter(|n| n.name_id == id && n.is_unicode()).find_map(|n| n.to_string());
    let name = name(ttf_parser::name_id::FULL_NAME).or_else(|| name(ttf_parser::name_id::FAMILY))?;
    let adv = |c| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).map(f32::from);
    let (a, b, c) = (adv('i')?, adv('M')?, adv('0')?);
    let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));
    Some((name, hi > 0.0 && (hi - lo) / hi <= 0.01))
}
