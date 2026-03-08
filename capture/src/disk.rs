//! Lightweight disk-usage helper (Linux / macOS).
//!
//! Calls `df` on the target path and parses the output.  This avoids
//! pulling in `nix` or `libc` just for `statvfs`.

use std::path::Path;
use std::process::Command;

/// Return the disk usage percentage (0–100) for the filesystem that
/// contains `path`.  Returns `None` when the check cannot be performed.
pub fn usage_pct(path: &Path) -> Option<f64> {
    let output = Command::new("df")
        .args(["--output=pcent"])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(|l| l.trim().trim_end_matches('%').trim().parse::<f64>().ok())
}
