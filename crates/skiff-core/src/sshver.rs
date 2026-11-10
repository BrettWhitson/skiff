//! The system OpenSSH client's version (askpass prompts need >= 8.4 for
//! `SSH_ASKPASS_REQUIRE`).

use std::process::Command;

/// Extracts `(major, minor)` from `ssh -V` output; anything that is not an
/// OpenSSH version string is an error.
/// Parses `OpenSSH_9.5p1, ...` / `OpenSSH_for_Windows_9.5p1, ...`.
pub fn parse_version(s: &str) -> Result<(u32, u32), String> {
    // Only the first word matters, e.g. `OpenSSH_9.5p1`.
    let first = s.trim().split([',', ' ']).next().unwrap_or("");
    let rest = first
        .strip_prefix("OpenSSH_for_Windows_")
        .or_else(|| first.strip_prefix("OpenSSH_"))
        .ok_or_else(|| format!("not an OpenSSH version: {s:?}"))?;
    // Split on non-digits: "9.5p1" yields "9", "5", "1".
    let mut it = rest.split(|c: char| !c.is_ascii_digit());
    let major = it.next().and_then(|m| m.parse().ok());
    let minor = it.next().and_then(|m| m.parse().ok());
    match (major, minor) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(format!("not an OpenSSH version: {s:?}")),
    }
}

/// Returns its `(major, minor)` version, or an error if it can't run or the
/// output isn't recognised.
/// Runs `program -V` (prints to stderr; no connection is made).
pub fn client_version(program: &str) -> Result<(u32, u32), String> {
    let out = Command::new(program).arg("-V").output().map_err(|e| format!("running `{program} -V`: {e}"))?;
    // Some builds print the version to stdout, so read both streams.
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    parse_version(&text)
}

/// Whether this OpenSSH honours `SSH_ASKPASS_REQUIRE` (added in 8.4), which
/// skiff needs to answer prompts through its own askpass helper.
pub fn supports_askpass_require(v: (u32, u32)) -> bool {
    v >= (8, 4)
}
