//! SQLite read-only queries for the Gaia Light web dashboard (via libsql / Turso).
//!
//! Reads from the same `detections`, `daily_summary`, and
//! `processed_clips` tables written by the processing server.

use std::path::Path;

use libsql::params;

use crate::model::{
    ClassSummary, DailyCount, Individual, LiveStatus, PreviewInfo, SpeciesSummary,
    SystemInfo, TrainingCandidate, WebDetection,
};

// ── Connection helper ────────────────────────────────────────────────────────

async fn open(db_path: &Path) -> Result<libsql::Connection, libsql::Error> {
    let db = libsql::Builder::new_local(
        db_path.to_str().expect("Non-UTF-8 database path"),
    )
    .build()
    .await?;
    let conn = db.connect()?;
    conn.execute("PRAGMA busy_timeout = 3000;", ()).await?;
    Ok(conn)
}

/// Open a read-write connection (for rename_individual).
async fn open_rw(db_path: &Path) -> Result<libsql::Connection, libsql::Error> {
    let db = libsql::Builder::new_local(
        db_path.to_str().expect("Non-UTF-8 database path"),
    )
    .build()
    .await?;
    let conn = db.connect()?;
    conn.execute("PRAGMA busy_timeout = 3000;", ()).await?;
    Ok(conn)
}

// ── Recent detections (live feed) ────────────────────────────────────────────

pub async fn recent_detections(
    db_path: &Path,
    limit: u32,
    after_id: Option<i64>,
) -> Result<Vec<WebDetection>, libsql::Error> {
    let conn = open(db_path).await?;

    let mut out = Vec::new();
    match after_id {
        Some(id) => {
            let mut rows = conn
                .query(
                    "SELECT d.id, d.timestamp, d.clip_filename, d.frame_index,
                            d.detector_model, d.class, d.confidence,
                            d.bbox_x1, d.bbox_y1, d.bbox_x2, d.bbox_y2,
                            d.species, d.species_confidence, d.species_model, d.crop_path,
                            d.latitude, d.longitude, d.processing_instance,
                            d.created_at, COALESCE(d.source_node, ''),
                            d.individual_id, i.name
                     FROM detections d
                     LEFT JOIN individuals i ON d.individual_id = i.id
                     WHERE d.id > ?1
                     ORDER BY d.id DESC
                     LIMIT ?2",
                    params![id, limit as i64],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                out.push(read_web_detection(&row)?);
            }
        }
        None => {
            let mut rows = conn
                .query(
                    "SELECT d.id, d.timestamp, d.clip_filename, d.frame_index,
                            d.detector_model, d.class, d.confidence,
                            d.bbox_x1, d.bbox_y1, d.bbox_x2, d.bbox_y2,
                            d.species, d.species_confidence, d.species_model, d.crop_path,
                            d.latitude, d.longitude, d.processing_instance,
                            d.created_at, COALESCE(d.source_node, ''),
                            d.individual_id, i.name
                     FROM detections d
                     LEFT JOIN individuals i ON d.individual_id = i.id
                     ORDER BY d.id DESC
                     LIMIT ?1",
                    params![limit as i64],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                out.push(read_web_detection(&row)?);
            }
        }
    }
    Ok(out)
}

/// Helper: parse a WebDetection from a query row (22 columns).
fn read_web_detection(row: &libsql::Row) -> Result<WebDetection, libsql::Error> {
    Ok(WebDetection {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        clip_filename: row.get(2)?,
        frame_index: row.get(3)?,
        detector_model: row.get(4)?,
        class: row.get(5)?,
        confidence: row.get(6)?,
        bbox_x1: row.get(7)?,
        bbox_y1: row.get(8)?,
        bbox_x2: row.get(9)?,
        bbox_y2: row.get(10)?,
        species: row.get(11)?,
        species_confidence: row.get(12)?,
        species_model: row.get(13)?,
        crop_path: row.get(14)?,
        latitude: row.get(15)?,
        longitude: row.get(16)?,
        processing_instance: row.get(17)?,
        created_at: row.get(18)?,
        source_node: row.get(19)?,
        individual_id: row.get(20)?,
        individual_name: row.get(21)?,
    })
}

// ── Class breakdown ──────────────────────────────────────────────────────────

pub async fn class_counts(
    db_path: &Path,
) -> Result<Vec<ClassSummary>, libsql::Error> {
    let conn = open(db_path).await?;
    let mut rows = conn
        .query(
            "SELECT class, COUNT(*) FROM detections
             GROUP BY class ORDER BY COUNT(*) DESC",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(ClassSummary {
            class: row.get(0)?,
            count: row.get(1)?,
        });
    }
    Ok(out)
}

// ── Species ranking ──────────────────────────────────────────────────────────

pub async fn top_species(
    db_path: &Path,
    limit: u32,
) -> Result<Vec<SpeciesSummary>, libsql::Error> {
    let conn = open(db_path).await?;
    let mut rows = conn
        .query(
            "SELECT species, COUNT(*) as cnt, MAX(created_at) as last
             FROM detections
             WHERE species IS NOT NULL AND species != ''
             GROUP BY species
             ORDER BY cnt DESC
             LIMIT ?1",
            params![limit as i64],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(SpeciesSummary {
            species: row.get(0)?,
            count: row.get(1)?,
            last_seen: row.get(2)?,
        });
    }
    Ok(out)
}

// ── Daily counts ─────────────────────────────────────────────────────────────

pub async fn daily_counts(
    db_path: &Path,
    days: u32,
) -> Result<Vec<DailyCount>, libsql::Error> {
    let conn = open(db_path).await?;
    let mut rows = conn
        .query(
            "SELECT date, class, COALESCE(species, ''), SUM(count)
             FROM daily_summary
             WHERE date >= date('now', ?1)
             GROUP BY date, class
             ORDER BY date DESC",
            params![format!("-{days} days")],
        )
        .await?;

    // Aggregate per date
    let mut map: std::collections::BTreeMap<String, DailyCount> =
        std::collections::BTreeMap::new();

    while let Some(row) = rows.next().await? {
        let date: String = row.get(0)?;
        let class: String = row.get(1)?;
        let count: i64 = row.get(3)?;

        let entry = map.entry(date.clone()).or_insert_with(|| DailyCount {
            date,
            animals: 0,
            persons: 0,
            vehicles: 0,
            total: 0,
        });
        let c = count as u32;
        match class.as_str() {
            "animal" => entry.animals += c,
            "person" => entry.persons += c,
            "vehicle" => entry.vehicles += c,
            _ => {}
        }
        entry.total += c;
    }

    Ok(map.into_values().rev().collect())
}

// ── System info ──────────────────────────────────────────────────────────────

pub async fn system_info(db_path: &Path) -> Result<SystemInfo, libsql::Error> {
    let conn = open(db_path).await?;

    let total_detections: u64 = {
        let mut rows = conn.query("SELECT COUNT(*) FROM detections", ()).await?;
        rows.next().await?.map(|r| r.get::<u64>(0)).transpose()?.unwrap_or(0)
    };

    let total_animals: u64 = {
        let mut rows = conn
            .query("SELECT COUNT(*) FROM detections WHERE class = 'animal'", ())
            .await?;
        rows.next().await?.map(|r| r.get::<u64>(0)).transpose()?.unwrap_or(0)
    };

    let total_species: u32 = {
        let mut rows = conn
            .query(
                "SELECT COUNT(DISTINCT species) FROM detections
                 WHERE species IS NOT NULL AND species != ''",
                (),
            )
            .await?;
        rows.next().await?.map(|r| r.get::<u32>(0)).transpose()?.unwrap_or(0)
    };

    let clips_processed: u64 = {
        let mut rows = conn.query("SELECT COUNT(*) FROM processed_clips", ()).await?;
        rows.next().await?.map(|r| r.get::<u64>(0)).transpose()?.unwrap_or(0)
    };

    // DB file size
    let db_size_bytes = std::fs::metadata(db_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(SystemInfo {
        total_detections,
        total_animals,
        total_species,
        clips_processed,
        db_size_bytes,
    })
}

// ── Live status (from JSON file) ─────────────────────────────────────────────

pub fn read_live_status(data_dir: &Path) -> Option<LiveStatus> {
    let path = data_dir.join("live_status.json");
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

// ── Training candidates ──────────────────────────────────────────────────────

/// Fetch unclassified high-confidence animal crops for model training.
pub async fn training_candidates(
    db_path: &Path,
    limit: u32,
    offset: u32,
) -> Result<(Vec<TrainingCandidate>, u64), libsql::Error> {
    let conn = open(db_path).await?;

    let total: u64 = {
        let mut rows = conn
            .query("SELECT COUNT(*) FROM training_candidates", ())
            .await?;
        rows.next()
            .await?
            .map(|r| r.get::<u64>(0))
            .transpose()?
            .unwrap_or(0)
    };

    let mut rows = conn
        .query(
            "SELECT id, timestamp, clip_filename, frame_index, confidence,
                    bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                    crop_path, latitude, longitude, created_at
             FROM training_candidates
             ORDER BY confidence DESC, created_at DESC
             LIMIT ?1 OFFSET ?2",
            params![limit as i64, offset as i64],
        )
        .await?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(TrainingCandidate {
            id: row.get(0)?,
            timestamp: row.get(1)?,
            clip_filename: row.get(2)?,
            frame_index: row.get(3)?,
            confidence: row.get(4)?,
            bbox_x1: row.get(5)?,
            bbox_y1: row.get(6)?,
            bbox_x2: row.get(7)?,
            bbox_y2: row.get(8)?,
            crop_path: row.get(9)?,
            latitude: row.get(10)?,
            longitude: row.get(11)?,
            created_at: row.get(12)?,
        });
    }

    Ok((out, total))
}

// ── Preview info ─────────────────────────────────────────────────────────────

pub fn preview_info(data_dir: &Path) -> PreviewInfo {
    let path = data_dir.join("preview_latest.jpg");
    match std::fs::metadata(&path) {
        Ok(meta) => {
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            PreviewInfo {
                available: true,
                modified_ms,
            }
        }
        Err(_) => PreviewInfo {
            available: false,
            modified_ms: 0,
        },
    }
}

// ── Individuals (person re-ID) ───────────────────────────────────────────

/// List all known individuals, ordered by most recently seen.
pub async fn list_individuals(
    db_path: &Path,
) -> Result<Vec<Individual>, libsql::Error> {
    let conn = open(db_path).await?;

    // The individuals table may not exist yet (if the processing
    // container hasn't run the latest schema migration).  In that case,
    // return an empty list gracefully.
    let table_exists = conn
        .query("SELECT 1 FROM individuals LIMIT 0", ())
        .await
        .is_ok();
    if !table_exists {
        return Ok(Vec::new());
    }

    let mut rows = conn
        .query(
            "SELECT id, name, detection_count, first_seen, last_seen,
                    representative_crop
             FROM individuals
             ORDER BY last_seen DESC",
            (),
        )
        .await?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Individual {
            id: row.get(0)?,
            name: row.get::<String>(1).unwrap_or_default(),
            detection_count: row.get(2)?,
            first_seen: row.get(3)?,
            last_seen: row.get(4)?,
            representative_crop: row.get(5)?,
        });
    }

    Ok(out)
}

/// Rename an individual (write operation — opens read-write).
pub async fn rename_individual(
    db_path: &Path,
    individual_id: i64,
    new_name: &str,
) -> Result<(), libsql::Error> {
    let conn = open_rw(db_path).await?;
    conn.execute(
        "UPDATE individuals SET name = ?1 WHERE id = ?2",
        params![new_name.to_string(), individual_id],
    )
    .await?;
    Ok(())
}

/// Get detections for a specific individual.
pub async fn individual_detections(
    db_path: &Path,
    individual_id: i64,
    limit: u32,
) -> Result<Vec<WebDetection>, libsql::Error> {
    let conn = open(db_path).await?;

    let mut rows = conn
        .query(
            "SELECT d.id, d.timestamp, d.clip_filename, d.frame_index,
                    d.detector_model, d.class, d.confidence,
                    d.bbox_x1, d.bbox_y1, d.bbox_x2, d.bbox_y2,
                    d.species, d.species_confidence, d.species_model, d.crop_path,
                    d.latitude, d.longitude, d.processing_instance,
                    d.created_at, COALESCE(d.source_node, ''),
                    d.individual_id, i.name
             FROM detections d
             LEFT JOIN individuals i ON d.individual_id = i.id
             WHERE d.individual_id = ?1
             ORDER BY d.id DESC
             LIMIT ?2",
            params![individual_id, limit as i64],
        )
        .await?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(read_web_detection(&row)?);
    }

    Ok(out)
}
