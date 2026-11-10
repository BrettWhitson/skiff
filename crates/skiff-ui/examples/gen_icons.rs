//! Regenerates Skiff's app icons from the mark in `skiff_ui::logo`:
//! `cargo run -p skiff-ui --release --example gen_icons`.
//!
//! Writes, under the repository's `assets/`:
//! - `icon/skiff.svg` (master) and `icon/skiff-small.svg` (16–32 px variant),
//! - `icon/png/skiff-<n>.png` for 16…1024,
//! - `icon/skiff.ico` (16–256), `icon/skiff.icns` (16–1024),
//! - `icon/skiff.res`, the Windows resource the `skiff-gui` and `skiff`
//!   build scripts link so the .exe files carry the icon,
//! - `linux/icons/hicolor/<n>x<n>/apps/skiff.png` and `scalable/apps/skiff.svg`.

use std::fs;
use std::path::{Path, PathBuf};

use skiff_ui::logo;

/// Sizes of the standalone PNGs.
const PNG_SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];
/// Sizes packed into the .ico and the .exe resource.
const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
/// Sizes in the Linux hicolor theme.
const HICOLOR_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// CRC-32 (IEEE) of `data`, as PNG chunks need.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Encodes straight RGBA pixels as a PNG.
fn png(size: u32, rgba: &[u8]) -> Vec<u8> {
    /// Appends one chunk: length, type, data, CRC over type and data.
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let mut ihdr = vec![];
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
    // Each row starts with filter 0 (none).
    let mut raw = Vec::with_capacity(rgba.len() + size as usize);
    for row in rgba.chunks((size * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 10));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A 32-bit BMP icon image (BITMAPINFOHEADER, bottom-up BGRA, empty AND
/// mask), the form small .ico entries use for the widest compatibility.
fn bmp_icon(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    let le32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    le32(&mut out, 40);
    le32(&mut out, size);
    le32(&mut out, size * 2); // XOR image + AND mask
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    for _ in 0..6 {
        le32(&mut out, 0);
    }
    for y in (0..size).rev() {
        for x in 0..size {
            let p = &rgba[((y * size + x) * 4) as usize..][..4];
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    let mask_row = size.div_ceil(32) * 4;
    out.resize(out.len() + (mask_row * size) as usize, 0);
    out
}

/// One icon image: its size and encoded bytes (BMP below 256, else PNG).
fn ico_images() -> Vec<(u32, Vec<u8>)> {
    ICO_SIZES
        .iter()
        .map(|&s| {
            let px = logo::rgba(s);
            (s, if s >= 256 { png(s, &px) } else { bmp_icon(s, &px) })
        })
        .collect()
}

/// The 16-byte ICONDIRENTRY / 14-byte GRPICONDIRENTRY head shared by .ico
/// and the resource: width, height (0 = 256), colours, planes, bit count, bytes.
fn dir_entry_head(size: u32, len: usize) -> Vec<u8> {
    let dim = if size >= 256 { 0 } else { size as u8 };
    let mut e = vec![dim, dim, 0, 0];
    e.extend_from_slice(&1u16.to_le_bytes());
    e.extend_from_slice(&32u16.to_le_bytes());
    e.extend_from_slice(&(len as u32).to_le_bytes());
    e
}

/// A multi-size .ico file.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, data) in images {
        out.extend_from_slice(&dir_entry_head(*size, data.len()));
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

/// A compiled Windows resource (.res) holding the icon as group 1, which
/// Explorer shows for the .exe. MSVC's linker takes .res files directly.
fn res(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    /// Appends one resource: header with numeric type and name, then the
    /// data, each DWORD-aligned.
    fn entry(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
        let le32 = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        le32(out, data.len() as u32);
        le32(out, 32); // header size
        out.extend_from_slice(&[0xff, 0xff]);
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&[0xff, 0xff]);
        out.extend_from_slice(&name.to_le_bytes());
        le32(out, 0); // data version
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0x0409u16.to_le_bytes()); // en-US
        le32(out, 0); // version
        le32(out, 0); // characteristics
        out.extend_from_slice(data);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    let mut out = vec![];
    // The leading empty entry that marks a 32-bit .res file.
    entry(&mut out, 0, 0, 0, &[]);
    let mut group = vec![0, 0, 1, 0];
    group.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (i, (size, data)) in images.iter().enumerate() {
        let id = i as u16 + 1;
        entry(&mut out, 3, id, 0x1010, data); // RT_ICON
        group.extend_from_slice(&dir_entry_head(*size, data.len()));
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(&mut out, 14, 1, 0x1030, &group); // RT_GROUP_ICON
    out
}

/// A macOS .icns with PNG entries from 16 to 1024 px.
fn icns() -> Vec<u8> {
    // (type, pixel size): 1x and 2x slots.
    let slots: [(&[u8; 4], u32); 10] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic13", 256),
        (b"ic09", 512),
        (b"ic14", 512),
        (b"ic10", 1024),
    ];
    let mut body = vec![];
    for (kind, size) in slots {
        let data = png(size, &logo::rgba(size));
        body.extend_from_slice(kind);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Writes `data` to `path`, creating folders, and says so.
fn write(path: &Path, data: &[u8]) {
    fs::create_dir_all(path.parent().expect("has a parent")).expect("create folder");
    fs::write(path, data).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    println!("{:>8}  {}", data.len(), path.display());
}

/// Generates every icon file.
fn main() {
    let assets: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let icon = assets.join("icon");
    write(&icon.join("skiff.svg"), logo::svg(false).as_bytes());
    write(&icon.join("skiff-small.svg"), logo::svg(true).as_bytes());
    for s in PNG_SIZES {
        write(&icon.join(format!("png/skiff-{s}.png")), &png(s, &logo::rgba(s)));
    }
    let images = ico_images();
    write(&icon.join("skiff.ico"), &ico(&images));
    write(&icon.join("skiff.res"), &res(&images));
    write(&icon.join("skiff.icns"), &icns());
    let hicolor = assets.join("linux/icons/hicolor");
    for s in HICOLOR_SIZES {
        write(&hicolor.join(format!("{s}x{s}/apps/skiff.png")), &png(s, &logo::rgba(s)));
    }
    write(&hicolor.join("scalable/apps/skiff.svg"), logo::svg(false).as_bytes());
}
