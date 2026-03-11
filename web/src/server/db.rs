//! SQLite read-only queries for the Gaia Light web dashboard.
//!
//! Reads from the same `detections`, `daily_summary`, and
//! `processed_clips` tables written by the processing server.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::model::{
    ClassSummary, DailyCount, Individual, LiveStatus, PreviewInfo, SpeciesSummary,
    SystemInfo, TrainingCandidate, WebDetection,
};

// ── Connection helper ────────────────────────────────────────────────────────

fn open(db_path: &Path) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    conn.execute_batch("PRAGMA busy_timeout = 3000;")?;
    Ok(conn)
}

// ── Recent detections (live feed) ────────────────────────────────────────────

pub fn recent_detections(
    db_path: &Path,
    limit: u32,
    after_id: Option<i64>,
) -> Result<Vec<WebDetection>, rusqlite::Error> {
    let conn = open(db_path)?;

    let (sql, row_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        match after_id {
            Some(id) => (
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
                 LIMIT ?2"
                    .into(),
                vec![Box::new(id), Box::new(limit)],
            ),
            None => (
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
                 LIMIT ?1"
                    .into(),
                vec![Box::new(limit)],
            ),
        };

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(row_params.iter()),
        |row| {
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
        },
    )?;

    rows.collect()
}

// ── Class breakdown ──────────────────────────────────────────────────────────

pub fn class_counts(
    db_path: &Path,
) -> Result<Vec<ClassSummary>, rusqlite::Error> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT class, COUNT(*) FROM detections
         GROUP BY class ORDER BY COUNT(*) DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ClassSummary {
            class: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    rows.collect()
}

// ── Species ranking ──────────────────────────────────────────────────────────

pub fn top_species(
    db_path: &Path,
    limit: u32,
) -> Result<Vec<SpeciesSummary>, rusqlite::Error> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT species, COUNT(*) as cnt, MAX(created_at) as last
         FROM detections
         WHERE species IS NOT NULL AND species != ''
         GROUP BY species
         ORDER BY cnt DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(SpeciesSummary {
            species: row.get(0)?,
            count: row.get(1)?,
            last_seen: row.get(2)?,
        })
    })?;
    rows.collect()
}

// ── Daily counts ─────────────────────────────────────────────────────────────

pub fn daily_counts(
    db_path: &Path,
    days: u32,
) -> Result<Vec<DailyCount>, rusqlite::Error> {
    let conn = open(db_path)?;
    let mut stmt = conn.prepare(
        "SELECT date, class, COALESCE(species, ''), SUM(count)
         FROM daily_summary
         WHERE date >= date('now', ?1)
         GROUP BY date, class
         ORDER BY date DESC",
    )?;
    let rows = stmt.query_map(
        params![format!("-{days} days")],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?;

    // Aggregate per date
    let mut map: std::collections::BTreeMap<String, DailyCount> =
        std::collections::BTreeMap::new();

    for row in rows {
        let (date, class, count) = row?;
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

pub fn system_info(db_path: &Path) -> Result<SystemInfo, rusqlite::Error> {
    let conn = open(db_path)?;

    let total_detections: u64 = conn.query_row(
        "SELECT COUNT(*) FROM detections",
        [],
        |r| r.get(0),
    )?;

    let total_animals: u64 = conn.query_row(
        "SELECT COUNT(*) FROM detections WHERE class = 'animal'",
        [],
        |r| r.get(0),
    )?;

    let total_species: u32 = conn.query_row(
        "SELECT COUNT(DISTINCT species) FROM detections
         WHERE species IS NOT NULL AND species != ''",
        [],
        |r| r.get(0),
    )?;

    let clips_processed: u64 = conn.query_row(
        "SELECT COUNT(*) FROM processed_clips",
        [],
        |r| r.get(0),
    )?;

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
pub fn training_candidates(
    db_path: &Path,
    limit: u32,
    offset: u32,
) -> Result<(Vec<TrainingCandidate>, u64), rusqlite::Error> {
    let conn = open(db_path)?;

    let total: u64 = conn.query_row(
        "SELECT COUNT(*) FROM training_candidates",
        [],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare_cached(
        "SELECT id, timestamp, clip_filename, frame_index, confidence,
                bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                crop_path, latitude, longitude, created_at
         FROM training_candidates
         ORDER BY confidence DESC, created_at DESC
         LIMIT ?1 OFFSET ?2",
    )?;

    let rows = stmt
        .query_map(params![limit, offset], |row| {
            Ok(TrainingCandidate {
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
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok((rows, total))
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
pub fn list_individuals(
    db_path: &Path,
) -> Result<Vec<Individual>, rusqlite::Error> {
    let conn = open(db_path)?;

    // The individuals table may not exist yet (if the processing
    // container hasn't run the latest schema migration).  In that case,
    // return an empty list gracefully.
    let table_exists: bool = conn
        .prepare("SELECT 1 FROM individuals LIMIT 0")
        .is_ok();
    if !table_exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT id, name, detection_count, first_seen, last_seen,
                representative_crop
         FROM individuals
         ORDER BY last_seen DESC",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(Individual {
                id: row.get(0)?,
                name: row.get::<_, String>(1).unwrap_or_default(),
                detection_count: row.get(2)?,
                first_seen: row.get(3)?,
                last_seen: row.get(4)?,
                representative_crop: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Rename an individual (write operation — opens read-write).
pub fn rename_individual(
    db_path: &Path,
    individual_id: i64,
    new_name: &str,
) -> Result<(), rusqlite::Error> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA busy_timeout = 3000;")?;
    conn.execute(
        "UPDATE individuals SET name = ?1 WHERE id = ?2",
        params![new_name, individual_id],
    )?;
    Ok(())
}

/// Get detections for a specific individual.
pub fn individual_detections(
    db_path: &Path,
    individual_id: i64,
    limit: u32,
) -> Result<Vec<WebDetection>, rusqlite::Error> {
    let conn = open(db_path)?;

    let mut stmt = conn.prepare(
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
    )?;

    let rows = stmt
        .query_map(params![individual_id, limit], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}
