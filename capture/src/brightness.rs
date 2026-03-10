//! Brightness detection and V4L2 camera control.
//!
//! Periodically grabs a single frame from the camera, computes its
//! mean luminance, and – for supported cameras – automatically adjusts
//! V4L2 controls to compensate for low-light conditions.
//!
//! ## Supported cameras
//!
//! | Camera card name (v4l2)              | Low-light control              |
//! |--------------------------------------|--------------------------------|
//! | **Arducam 1080P Low Light**          | `backlight_compensation=1`     |
//!
//! For unknown cameras the module still measures brightness and exposes
//! the reading via the HTTP API, but does **not** modify any controls.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use tracing::{debug, info, warn};

// ── Tuning ───────────────────────────────────────────────────────────

/// Default mean-luma threshold (0–255) below which a frame is
/// considered "too dark".
pub const DEFAULT_BRIGHTNESS_THRESHOLD: f64 = 20.0;

/// How often (seconds) the probe thread grabs a test frame.
pub const DEFAULT_PROBE_INTERVAL_SECS: u64 = 60;

// ── Shared state ─────────────────────────────────────────────────────

/// Thread-safe brightness state shared with the HTTP server.
#[derive(Debug)]
pub struct BrightnessState {
    /// Last measured mean luma × 100 (e.g. 1234 = 12.34).
    pub luma_centipct: AtomicU32,
    /// `true` when the last probe was below the darkness threshold.
    pub is_dark: AtomicBool,
    /// `true` when backlight_compensation (or equivalent) is active.
    pub low_light_active: AtomicBool,
}

impl BrightnessState {
    pub fn new() -> Self {
        Self {
            luma_centipct: AtomicU32::new(0),
            is_dark: AtomicBool::new(false),
            low_light_active: AtomicBool::new(false),
        }
    }

    /// Mean luma on 0–255 scale as a float.
    pub fn mean_luma(&self) -> f64 {
        self.luma_centipct.load(Ordering::Relaxed) as f64 / 100.0
    }
}

// ── Camera identification ────────────────────────────────────────────

/// Known camera models and how to handle their low-light control.
#[derive(Debug, Clone, PartialEq)]
pub enum CameraKind {
    /// Arducam 1080P Low Light – uses `backlight_compensation`.
    ArducamLowLight,
    /// Any other V4L2 camera – brightness is measured but no automatic
    /// control is applied.
    Unknown(String),
}

impl CameraKind {
    pub fn display_name(&self) -> &str {
        match self {
            CameraKind::ArducamLowLight => "Arducam 1080P Low Light",
            CameraKind::Unknown(name) => name.as_str(),
        }
    }
}

/// Query `v4l2-ctl --info` to determine the camera model.
pub fn identify_camera(device: &str) -> CameraKind {
    let output = Command::new("v4l2-ctl")
        .args(["--device", device, "--info"])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            debug!("v4l2-ctl --info for {device}:\n{text}");

            // The "Card type" line looks like:
            //   Card type     : Arducam 1080P Low Light
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("\tCard type") {
                    let card = rest.trim_start_matches(|c: char| c == ' ' || c == ':');
                    let card = card.trim();
                    info!("Detected camera card: \"{card}\" on {device}");

                    if card.contains("Arducam") && card.contains("Low Light") {
                        return CameraKind::ArducamLowLight;
                    }
                    return CameraKind::Unknown(card.to_string());
                }
            }

            warn!("Could not parse camera card type from v4l2-ctl --info");
            CameraKind::Unknown("unknown".to_string())
        }
        Err(e) => {
            warn!("v4l2-ctl --info failed for {device}: {e}");
            CameraKind::Unknown("unknown".to_string())
        }
    }
}

// ── Brightness measurement ───────────────────────────────────────────

/// Grab a single JPEG frame from the V4L2 device using ffmpeg and
/// return the mean luma (0–255).
///
/// Returns `None` if frame capture or analysis fails.
pub fn probe_brightness(device: &str, tmp_dir: &Path) -> Option<f64> {
    let probe_path = tmp_dir.join("_brightness_probe.jpg");

    // Remove stale probe
    let _ = std::fs::remove_file(&probe_path);

    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "error",
            "-nostdin",
            "-f", "v4l2",
            "-i", device,
            "-frames:v", "1",
            "-q:v", "2",
            "-y",
        ])
        .arg(probe_path.as_os_str())
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            debug!("Brightness probe ffmpeg exited with {s}");
            return None;
        }
        Err(e) => {
            debug!("Brightness probe ffmpeg failed: {e}");
            return None;
        }
    }

    let mean = compute_mean_luma(&probe_path);
    let _ = std::fs::remove_file(&probe_path);
    mean
}

/// Compute mean luma from a JPEG file on disk.
fn compute_mean_luma(path: &Path) -> Option<f64> {
    let img = image::open(path).ok()?;
    let grey = img.to_luma8();
    let count = grey.len() as u64;
    if count == 0 {
        return None;
    }
    let sum: u64 = grey.iter().map(|&p| p as u64).sum();
    Some(sum as f64 / count as f64)
}

// ── V4L2 control ─────────────────────────────────────────────────────

/// Set a V4L2 integer control on the given device.
pub fn set_v4l2_ctrl(device: &str, control: &str, value: i32) -> bool {
    let arg = format!("{control}={value}");
    let result = Command::new("v4l2-ctl")
        .args(["--device", device, "--set-ctrl", &arg])
        .output();

    match result {
        Ok(out) if out.status.success() => {
            info!("v4l2-ctl: set {control}={value} on {device}");
            true
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            warn!("v4l2-ctl --set-ctrl {arg} failed: {stderr}");
            false
        }
        Err(e) => {
            warn!("Cannot run v4l2-ctl: {e}");
            false
        }
    }
}

/// Get the current value of a V4L2 integer control.
pub fn get_v4l2_ctrl(device: &str, control: &str) -> Option<i32> {
    let result = Command::new("v4l2-ctl")
        .args(["--device", device, "--get-ctrl", control])
        .output();

    match result {
        Ok(out) if out.status.success() => {
            // Output like: "backlight_compensation: 0"
            let text = String::from_utf8_lossy(&out.stdout);
            text.split(':')
                .nth(1)
                .and_then(|v| v.trim().parse::<i32>().ok())
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            debug!("v4l2-ctl --get-ctrl {control} failed: {stderr}");
            None
        }
        Err(e) => {
            debug!("Cannot run v4l2-ctl: {e}");
            None
        }
    }
}

/// Apply the appropriate low-light compensation for the given camera kind.
///
/// Returns `true` if a control was actually changed.
pub fn apply_low_light(device: &str, kind: &CameraKind, enable: bool) -> bool {
    match kind {
        CameraKind::ArducamLowLight => {
            let val = if enable { 1 } else { 0 };
            set_v4l2_ctrl(device, "backlight_compensation", val)
        }
        CameraKind::Unknown(_) => {
            debug!("No low-light control for camera: {}", kind.display_name());
            false
        }
    }
}

// ── Probe loop (runs in its own thread) ──────────────────────────────

/// Run the periodic brightness probe.  Call from a dedicated thread.
///
/// `shutdown` is polled every second; the probe fires every
/// `interval_secs` seconds.
pub fn probe_loop(
    device: String,
    tmp_dir: std::path::PathBuf,
    threshold: f64,
    interval_secs: u64,
    camera_kind: CameraKind,
    state: Arc<BrightnessState>,
    shutdown: Arc<AtomicBool>,
) {
    info!(
        "Brightness probe starting: device={device}, camera={}, \
         threshold={threshold:.1}, interval={interval_secs}s",
        camera_kind.display_name()
    );

    // Initial probe right away
    run_single_probe(&device, &tmp_dir, threshold, &camera_kind, &state);

    let mut elapsed = 0u64;
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_secs(1));
        elapsed += 1;
        if elapsed >= interval_secs {
            elapsed = 0;
            run_single_probe(&device, &tmp_dir, threshold, &camera_kind, &state);
        }
    }

    debug!("Brightness probe thread exiting");
}

fn run_single_probe(
    device: &str,
    tmp_dir: &Path,
    threshold: f64,
    camera_kind: &CameraKind,
    state: &BrightnessState,
) {
    match probe_brightness(device, tmp_dir) {
        Some(luma) => {
            state
                .luma_centipct
                .store((luma * 100.0) as u32, Ordering::Relaxed);

            let was_dark = state.is_dark.load(Ordering::Relaxed);
            let now_dark = luma < threshold;
            state.is_dark.store(now_dark, Ordering::Relaxed);

            if now_dark && !was_dark {
                info!(
                    "Frame is DARK (mean luma {luma:.1} < threshold {threshold:.1}) — \
                     enabling low-light compensation"
                );
                let ok = apply_low_light(device, camera_kind, true);
                state.low_light_active.store(ok, Ordering::Relaxed);
            } else if !now_dark && was_dark {
                info!(
                    "Frame is BRIGHT (mean luma {luma:.1} >= threshold {threshold:.1}) — \
                     disabling low-light compensation"
                );
                apply_low_light(device, camera_kind, false);
                state.low_light_active.store(false, Ordering::Relaxed);
            } else {
                debug!(
                    "Brightness probe: luma={luma:.1}, dark={now_dark}, low_light={}",
                    state.low_light_active.load(Ordering::Relaxed),
                );
            }
        }
        None => {
            debug!("Brightness probe: could not capture frame");
        }
    }
}
