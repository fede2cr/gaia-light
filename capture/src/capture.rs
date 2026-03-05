//! Video capture – spawns `ffmpeg` as child processes.
//!
//! Supports two input modes:
//!   1. **USB / V4L2 camera** (`VIDEO_DEVICE=/dev/video0`) – the default
//!      for gaia-light, same physical cameras used by RMS and gaia-gmn.
//!   2. **RTSP streams** (`RTSP_STREAMS=rtsp://…`) – fallback for IP cams.
//!
//! Each source gets its own ffmpeg child that records segmented MP4 files
//! to the data directory.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use gaia_light_common::config::Config;

/// Opaque handle that owns the recording child process(es).
pub struct CaptureHandle {
    children: Vec<Child>,
}

impl CaptureHandle {
    pub fn kill(&mut self) -> Result<()> {
        for child in &mut self.children {
            let _ = child.kill();
        }
        Ok(())
    }

    /// Returns `Some(msg)` if any child has exited, `None` if all alive.
    pub fn check_alive(&mut self) -> Option<String> {
        for (i, child) in self.children.iter_mut().enumerate() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Some(format!(
                        "Capture child {} exited with {}",
                        i, status
                    ));
                }
                Ok(None) => {} // still running
                Err(e) => {
                    return Some(format!(
                        "Cannot check capture child {}: {e}",
                        i
                    ));
                }
            }
        }
        None
    }
}

/// Start the video capture pipeline according to the config.
///
/// Preference order:
///   1. `VIDEO_DEVICE` – local USB camera via V4L2
///   2. `RTSP_STREAMS` – one or more IP camera URLs
pub fn start(config: &Config) -> Result<CaptureHandle> {
    std::fs::create_dir_all(config.stream_data_dir())
        .context("Cannot create StreamData directory")?;

    if let Some(ref dev) = config.video_device {
        start_v4l2(config, dev)
    } else if !config.rtsp_streams.is_empty() {
        start_rtsp(config)
    } else {
        anyhow::bail!(
            "No camera configured. Set VIDEO_DEVICE=/dev/videoN \
             (or RTSP_STREAMS=rtsp://…) in gaia-light.conf"
        );
    }
}

// ── USB / V4L2 via ffmpeg ────────────────────────────────────────────────

fn start_v4l2(config: &Config, device: &str) -> Result<CaptureHandle> {
    let output_pattern = config
        .stream_data_dir()
        .join("%F-camera-v4l2-%H%M%S.mp4");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);

    // V4L2 input — try native MJPEG if the camera supports it to
    // avoid expensive YUV→H.264 transcoding.
    if probe_native_mjpeg(device) {
        info!("Camera {device} supports native MJPEG");
        cmd.args(["-input_format", "mjpeg"]);
    }

    cmd.args(["-f", "v4l2"]);

    // Resolution (before -i so it's a demuxer option)
    if config.capture_width > 0 && config.capture_height > 0 {
        cmd.args([
            "-video_size",
            &format!("{}x{}", config.capture_width, config.capture_height),
        ]);
    }

    // Framerate (demuxer option when set before -i)
    if config.capture_fps > 0 {
        cmd.args(["-framerate", &config.capture_fps.to_string()]);
    }

    cmd.args(["-i", device]);

    // Re-encode to H.264 — V4L2 raw/MJPEG can't be put straight into MP4.
    cmd.args([
        "-c:v", "libx264",
        "-preset", "ultrafast",
        "-an",
        // Segment muxer
        "-f", "segment",
        "-segment_format", "mp4",
        "-segment_time", &config.segment_length.to_string(),
        "-reset_timestamps", "1",
        "-strftime", "1",
    ]);
    cmd.arg(output_pattern.to_str().unwrap());
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());

    info!(
        "Spawning ffmpeg for V4L2 device {device} \
         (segment={}s, fps={}, {}x{})",
        config.segment_length,
        config.capture_fps,
        config.capture_width,
        config.capture_height,
    );

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn ffmpeg for V4L2 device {device}"))?;

    // Drain stderr in a background thread
    drain_stderr(&mut child, "ffmpeg-v4l2");

    // Give ffmpeg a moment to fail on bad config
    std::thread::sleep(std::time::Duration::from_millis(500));
    match child.try_wait() {
        Ok(Some(status)) => {
            anyhow::bail!(
                "ffmpeg for {device} exited immediately with {status} — \
                 check VIDEO_DEVICE in gaia-light.conf \
                 (is the device mounted into the container?)"
            );
        }
        Ok(None) => {} // still running
        Err(e) => warn!("Cannot check ffmpeg status: {e}"),
    }

    info!(
        "ffmpeg V4L2 capture started (pid={}, device={device})",
        child.id()
    );
    Ok(CaptureHandle {
        children: vec![child],
    })
}

/// Probe whether the V4L2 device supports native MJPEG output.
fn probe_native_mjpeg(device: &str) -> bool {
    let result = Command::new("v4l2-ctl")
        .args(["--device", device, "--list-formats"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    match result {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_uppercase();
            text.contains("MJPG") || text.contains("MJPEG")
        }
        Err(e) => {
            debug!("v4l2-ctl probe failed: {e}");
            false
        }
    }
}

// ── RTSP via ffmpeg ──────────────────────────────────────────────────────

fn start_rtsp(config: &Config) -> Result<CaptureHandle> {
    let mut children = Vec::new();

    for (i, url) in config.rtsp_streams.iter().enumerate() {
        let stream_idx = i + 1;
        let output_pattern = config
            .stream_data_dir()
            .join(format!("%F-camera-RTSP_{stream_idx}-%H%M%S.mp4"));

        let timeout_args = if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
            vec!["-timeout".to_string(), "10000000".to_string()]
        } else if url.contains("://") {
            vec!["-rw_timeout".to_string(), "10000000".to_string()]
        } else {
            vec![]
        };

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        for arg in &timeout_args {
            cmd.arg(arg);
        }
        cmd.args(["-i", url]);

        // Optional FPS filter
        if config.capture_fps > 0 {
            cmd.args(["-vf", &format!("fps={}", config.capture_fps)]);
        }

        // Optional resolution scaling
        if config.capture_width > 0 && config.capture_height > 0 {
            cmd.args([
                "-s",
                &format!("{}x{}", config.capture_width, config.capture_height),
            ]);
        }

        cmd.args([
            // Remove audio – we only care about video
            "-an",
            // Codec: copy (passthrough) where possible; re-encode if filters
            // are active. When -vf is set ffmpeg already forces re-encode.
            "-c:v",
            if config.capture_fps > 0 || (config.capture_width > 0 && config.capture_height > 0) {
                "libx264"
            } else {
                "copy"
            },
            // Segment muxer
            "-f",
            "segment",
            "-segment_format",
            "mp4",
            "-segment_time",
            &config.segment_length.to_string(),
            "-reset_timestamps",
            "1",
            "-strftime",
            "1",
        ]);
        cmd.arg(output_pattern.to_str().unwrap());
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());

        info!(
            "Spawning ffmpeg for camera stream {stream_idx}: {url} \
             (segment={}s, fps={}, {}x{})",
            config.segment_length,
            config.capture_fps,
            config.capture_width,
            config.capture_height,
        );

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn ffmpeg for stream {stream_idx}: {url}"))?;

        // Drain stderr in a background thread
        drain_stderr(&mut child, &format!("ffmpeg-cam{stream_idx}"));

        // Give ffmpeg a moment to fail on bad URL/config
        std::thread::sleep(std::time::Duration::from_millis(500));
        match child.try_wait() {
            Ok(Some(status)) => {
                anyhow::bail!(
                    "ffmpeg for stream {stream_idx} ({url}) exited immediately \
                     with {status} — check RTSP_STREAMS in gaia.conf"
                );
            }
            Ok(None) => {} // still running
            Err(e) => warn!("Cannot check ffmpeg status: {e}"),
        }

        info!(
            "ffmpeg camera capture started (pid={}, stream={stream_idx})",
            child.id()
        );
        children.push(child);
    }

    Ok(CaptureHandle { children })
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Spawn a thread that drains stderr from an ffmpeg child so the pipe
/// buffer doesn't fill up and block the process.
fn drain_stderr(child: &mut Child, tag: &str) {
    if let Some(stderr) = child.stderr.take() {
        let tag = tag.to_string();
        std::thread::Builder::new()
            .name(tag.clone())
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) if l.is_empty() => {}
                        Ok(l) => warn!("[{tag}] {l}"),
                        Err(_) => break,
                    }
                }
                debug!("{tag} stderr stream ended");
            })
            .ok();
    }
}
