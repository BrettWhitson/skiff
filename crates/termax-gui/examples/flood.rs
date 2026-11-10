//! Prints a flood of mixed terminal output (colours, bold, wide characters,
//! long lines) for checking that the GUI terminal stays responsive.
//!
//! `cargo run -p termax-gui --release --example flood -- 50` prints ~50 MB.
//! Run it inside a termax terminal tab (e.g. one connected to
//! `termax-testserver --shell`) and type while it scrolls.

use std::io::{BufWriter, Write};

fn main() -> std::io::Result<()> {
    let mb: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(10);
    let limit = mb << 20;
    let mut out = BufWriter::new(std::io::stdout().lock());
    let mut written = 0usize;
    let mut i = 0u64;
    while written < limit {
        let c = 31 + i % 7;
        let mut line =
            format!("\x1b[{c}m{i:>9}\x1b[0m \x1b[1mbold\x1b[0m plain \u{4e2d}\u{6587} \x1b[4munderline\x1b[0m \x1b[7minverse\x1b[0m");
        if i.is_multiple_of(5) {
            line.push_str(&"\u{2500}".repeat(120));
        }
        line.push_str("\r\n");
        out.write_all(line.as_bytes())?;
        written += line.len();
        i += 1;
    }
    out.flush()
}
