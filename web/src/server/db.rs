//! SQLite read-only queries for the Gaia Light web dashboard.
//!
//! Reads from the same `detections`, `daily_summary`, and
//! `processed_clips` tables written by the processing server.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::model::{
    ClassSummary, DailyCount, LiveStatus, SpeciesSummary, SystemInfo,
    WebDetection,
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
                "SELECT id, timestamp, clip_filename, frame_index,
                        detector_model, class, confidence,
                        bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                        species, species_confidence, crop_path,
                        latitude, longitude, processing_instance,
                        created_at
                 FROM detections
                 WHERE id > ?1
                 ORDER BY id DESC
                 LIMIT ?2"
                    .into(),
                vec![Box::new(id), Box::new(limit)],
            ),
            None => (
                "SELECT id, timestamp, clip_filename, frame_index,
                        detector_model, class, confidence,
                        bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                        species, species_confidence, crop_path,
                        latitude, longitude, processing_instance,
                        created_at
                 FROM detections
                 ORDER BY id DESC
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
                crop_path: row.get(13)?,
                latitude: row.get(14)?,
                longitude: row.get(15)?,
                processing_instance: row.get(16)?,
                created_at: row.get(17)?,
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
