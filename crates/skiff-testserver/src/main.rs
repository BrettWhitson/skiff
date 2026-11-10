//! `skiff-testserver [--root DIR] [--password PW] [--shell]`
//!
//! Runs the local test server on 127.0.0.1 until Enter is pressed, and prints
//! the ssh options needed to reach it. For manual testing of skiff only.

use skiff_testserver::{FixtureAuth, TestFixture};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let auth = match value("--password") {
        Some(p) => FixtureAuth::Password(p),
        None => FixtureAuth::Key,
    };
    let fx = if args.iter().any(|a| a == "--shell") { TestFixture::with_shell(auth)? } else { TestFixture::new(auth)? };
    if let Some(src) = value("--root") {
        for entry in std::fs::read_dir(&src)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), fx.root().join(entry.file_name()))?;
            }
        }
    }
    println!("serving {} as / on 127.0.0.1", fx.root().display());
    println!("ssh {} {}", shell_join(&fx.ssh_options()), fx.destination());
    println!("press Enter to stop");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(())
}

fn shell_join(args: &[String]) -> String {
    args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}
