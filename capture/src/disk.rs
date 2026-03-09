//! Lightweight disk-usage helper (Linux / macOS).
//!
//! Calls `df` on the target path and parses the output.  This avoids
//! pulling in `nix` or `libc` just for `statvfs`.

use std::path::Path;
use std::process::Command;

/// Human-readable disk space summary for the filesystem containing `path`.
///
/// Returns a formatted string like:
///   "Used 12.3 GB / 29.1 GB (42%), 16.8 GB available"
pub fn summary(path: &Path) -> Option<String> {
    let output = Command::new("df")
        .args(["-B1", "--output=used,avail,size"])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 3 {
        return None;
    }
    let used: f64 = cols[0].parse().ok()?;
    let avail: f64 = cols[1].parse().ok()?;
    let total: f64 = cols[2].parse().ok()?;
    let pct = if total > 0.0 { used / total * 100.0 } else { 0.0 };

    Some(format!(
        "Used {:.1} GB / {:.1} GB ({:.1}%), {:.1} GB available",
        used / 1e9,
        total / 1e9,
        pct,
        avail / 1e9,
    ))
}

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
