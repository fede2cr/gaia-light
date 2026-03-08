# Gaia Light

A distributed camera-trap system split into networked containers.
Captures video from USB cameras, RTSP streams, or pre-recorded NVR
footage, runs MegaDetector + species classifiers on extracted frames,
and displays results on a real-time web dashboard.

## Architecture

```
┌──────────────────────┐         HTTP/REST          ┌──────────────────────────┐
│   CAPTURE SERVER     │ ◄────────────────────────► │   PROCESSING SERVER      │
│                      │                            │                          │
│  ffmpeg (V4L2/RTSP)  │  GET  /api/clips           │  HTTP client (polling)   │
│  → segmented MP4     │  GET  /api/clips/:name     │  ↓                       │
│  → axum HTTP server  │  DEL  /api/clips/:name     │  MegaDetector inference  │
│                      │  GET  /api/health          │  species classification  │
│  Container: ffmpeg + │                            │  SQLite DB writes ──────────┐
│  v4l-utils + Rust    │                            │  crop extraction         │  │
└──────────────────────┘                            └──────────────────────────┘  │
                                                                                  │
  ┌──────────────────────┐                                                        │
  │   VIDEO IMPORT       │          ┌──────────────────────────┐  reads SQLite    │
  │   (alternative)      │          │   WEB DASHBOARD          │ ◄────────────────┘
  │                      │          │                          │
  │  Scans NVR dir       │          │  Leptos SSR + WASM       │
  │  Symlinks + rename   │          │  Real-time detection feed│
  │  Same HTTP API       │          │  Live status panel       │
  │  mDNS discovery      │          │  Dark-themed responsive  │
  └──────────────────────┘          └──────────────────────────┘
```

### Crates

| Crate | Purpose |
|-------|---------|
| **common** (`gaia-light-common`) | Shared config, mDNS discovery, protocol types, classifier definitions |
| **capture** (`gaia-light-capture`) | Video capture (V4L2 / RTSP) + HTTP server serving MP4 clips |
| **processing** (`gaia-light-processing`) | MegaDetector + species classification, motion detection, DB, reporting |
| **web** (`gaia-light-web`) | Leptos web dashboard – live feed, detection list, settings |
| **video-import** (`gaia-light-video-import`) | Serves NVR recordings to processing via the standard capture API |

## Networking & Discovery

All containers use **`network_mode: host`** so they share the host's
network stack.  This enables **mDNS** (multicast DNS) discovery —
containers find each other automatically, even across machines on the
same LAN.

| Service | mDNS type | Default port |
|---------|-----------|-------------|
| Capture / Video Import | `_gaia-lt-cap._tcp.local.` | 8090 |
| Processing | `_gaia-lt-proc._tcp.local.` | — (no HTTP) |
| Web | `_gaia-lt-web._tcp.local.` | 8190 |

**How it works:**
- Capture (or video-import) registers as `_gaia-lt-cap._tcp.local.`
  with a sequential instance name (e.g. `capture-01`)
- Processing browses for capture peers every 60 s, polls each for new clips
- If mDNS finds no peers, processing falls back to `CAPTURE_SERVER_URL`

## Configuration

All crates read the same `KEY=VALUE` config file (default:
`/etc/gaia/gaia-light.conf`).  **Environment variables override** file values.

### Capture

| Key | Default | Description |
|-----|---------|-------------|
| `VIDEO_DEVICE` | — | V4L2 device path (e.g. `/dev/video0`) |
| `RTSP_STREAMS` | — | Comma-separated RTSP URLs (fallback when no V4L2) |
| `SEGMENT_LENGTH` | `60` | Video segment length in seconds |
| `CAPTURE_FPS` | `1` | Frames per second (0 = native) |
| `CAPTURE_WIDTH` | `0` | Resolution width (0 = native) |
| `CAPTURE_HEIGHT` | `0` | Resolution height (0 = native) |
| `RECS_DIR` | `/data` | Base data directory |
| `CAPTURE_LISTEN_ADDR` | `0.0.0.0:8090` | HTTP listen address |
| `DISK_USAGE_MAX` | `95` | Max disk usage % before pausing capture |

### Processing

| Key | Default | Description |
|-----|---------|-------------|
| `CONFIDENCE` | `0.5` | Minimum MegaDetector confidence |
| `SPECIES_CONFIDENCE` | `0.1` | Minimum species-classifier confidence |
| `MAX_FRAMES_PER_CLIP` | `0` | Max frames per clip (0 = all) |
| `MOTION_THRESHOLD` | `1.5` | MAD threshold for motion detection |
| `MODEL_DIR` | `/models` | Directory containing ONNX models |
| `CLASSIFIERS` | `ai4g-amazon-v2` | Comma-separated classifier slugs |
| `DB_PATH` | `/data/detections.db` | SQLite database path |
| `CAPTURE_SERVER_URL` | `http://localhost:8090` | Fallback capture URL |
| `POLL_INTERVAL_SECS` | `10` | Seconds between polling cycles |
| `LATITUDE` | `-1` | GPS latitude |
| `LONGITUDE` | `-1` | GPS longitude |

## Building

```bash
# Full workspace
cargo build --release

# Individual crates
cargo build --release -p gaia-light-capture
cargo build --release -p gaia-light-processing
cargo build --release -p gaia-light-video-import

# Web dashboard (requires cargo-leptos + wasm32 target)
cargo install cargo-leptos
rustup target add wasm32-unknown-unknown
cargo leptos build --release
```

## Container Images

```bash
# Capture
podman build -f capture/Containerfile -t gaia-light-capture .

# Processing
podman build -f processing/Containerfile -t gaia-light-processing .

# Web dashboard
podman build -f web/Containerfile -t gaia-light-web .

# Video import (for NVR recordings)
podman build -f video-import/Containerfile -t gaia-light-video-import .
```

---

## Video Import

The **video-import** container is a specialised capture node that serves
pre-recorded security-camera footage (e.g. from an NVR) to the standard
processing node — **no code changes to processing are required**.

### How it works

1. **Scans** `IMPORT_DIR` for `.mp4` files inside date sub-directories
   (e.g. `2025-08-11/`)
2. **Creates symlinks** in a flat `StreamData/` directory, renaming files
   to the standard capture filename format:

   ```
   Source:  2025-08-11/RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4
   Link:   StreamData/2025-08-11-camera-front-yard-070122_070131.mp4
   ```

3. **Registers on mDNS** as `_gaia-lt-cap._tcp.local.` — processing nodes
   discover it automatically alongside any real capture nodes
4. **Serves the same HTTP API** (`GET /api/clips`, `GET /api/clips/:name`,
   `DELETE /api/clips/:name`, `GET /api/health`)
5. When processing **DELETEs** a clip, the symlink is **moved to `processed/`**
   instead of deleted — original NVR files are never modified
6. **Periodic re-scan** (default 30 s) picks up new files as the NVR records

### Filename parsing

The scanner looks for three consecutive underscore-separated tokens where
the first is 8 digits (`YYYYMMDD`) and the next two are 6 digits each
(`HHMMSS`).  Any prefix before the date is ignored, so it handles various
NVR naming conventions.  Files that don't match are served with their
original name.

### Configuration

Video-import is configured entirely through **environment variables**
(it does not read `gaia-light.conf`):

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `IMPORT_DIR` | **yes** | — | NVR directory with date sub-directories |
| `CAMERA_NAME` | **yes** | — | Camera identifier (used in renamed filenames) |
| `LISTEN_ADDR` | no | `0.0.0.0:8090` | HTTP listen address |
| `DATA_DIR` | no | `/data` | Working directory for symlinks and processed tracking |
| `SCAN_INTERVAL_SECS` | no | `30` | Seconds between import re-scans |

### Running

```bash
podman run --rm --network=host \
  -v /path/to/nvr/recordings:/import:ro \
  -v video-import-data:/data \
  -e IMPORT_DIR=/import \
  -e CAMERA_NAME=front-yard \
  gaia-light-video-import
```

Multiple cameras can be served by running multiple instances — each
registers a unique mDNS name (`capture-01`, `capture-02`, …) and the
processing node discovers all of them:

```bash
# Camera 1
podman run -d --network=host \
  -v /nvr/front-yard:/import:ro \
  -v import-front:/data \
  -e IMPORT_DIR=/import \
  -e CAMERA_NAME=front-yard \
  -e LISTEN_ADDR=0.0.0.0:8090 \
  gaia-light-video-import

# Camera 2
podman run -d --network=host \
  -v /nvr/back-garden:/import:ro \
  -v import-back:/data \
  -e IMPORT_DIR=/import \
  -e CAMERA_NAME=back-garden \
  -e LISTEN_ADDR=0.0.0.0:8091 \
  gaia-light-video-import
```

### Expected directory layout

```
/import/                          ← IMPORT_DIR (read-only mount)
├── 2025-08-10/
│   ├── RecM05_20250810_180000_180010_0_ABC123_DEF456.mp4
│   └── RecM05_20250810_180010_180020_0_ABC123_DEF456.mp4
├── 2025-08-11/
│   ├── RecM05_20250811_070122_070131_0_7D1E82100_2B3591.mp4
│   └── …
└── …

/data/                            ← DATA_DIR (persistent volume)
├── StreamData/                   ← symlinks served to processing
│   ├── 2025-08-11-camera-front-yard-070122_070131.mp4 → /import/…
│   └── …
└── processed/                    ← consumed symlinks (for dedup)
    ├── 2025-08-10-camera-front-yard-180000_180010.mp4 → /import/…
    └── …
```
