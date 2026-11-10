//! The system OpenSSH client's version (askpass prompts need >= 8.4 for
//! `SSH_ASKPASS_REQUIRE`).

use std::process::Command;

/// Parses `OpenSSH_9.5p1, ...` / `OpenSSH_for_Windows_9.5p1, ...`.
pub fn parse_version(s: &str) -> Result<(u32, u32), String> {
    let first = s.trim().split([',', ' ']).next().unwrap_or("");
    let rest = first
        .strip_prefix("OpenSSH_for_Windows_")
        .or_else(|| first.strip_prefix("OpenSSH_"))
        .ok_or_else(|| format!("not an OpenSSH version: {s:?}"))?;
    let mut it = rest.split(|c: char| !c.is_ascii_digit());
    let major = it.next().and_then(|m| m.parse().ok());
    let minor = it.next().and_then(|m| m.parse().ok());
    match (major, minor) {
        (Some(a), Some(b)) => Ok((a, b)),
        _ => Err(format!("not an OpenSSH version: {s:?}")),
    }
}

/// Runs `program -V` (prints to stderr; no connection is made).
pub fn client_version(program: &str) -> Result<(u32, u32), String> {
    let out = Command::new(program).arg("-V").output().map_err(|e| format!("running `{program} -V`: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    parse_version(&text)
}

pub fn supports_askpass_require(v: (u32, u32)) -> bool {
    v >= (8, 4)
}
