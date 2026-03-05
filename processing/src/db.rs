//! SQLite persistence layer for Gaia Light detections.
//!
//! Stores every detection (animal/person/vehicle) with its bounding box,
//! confidence score, optional species classification, and metadata.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

/// A single detection to insert into the database.
#[derive(Debug, Clone)]
pub struct DetectionRow {
    pub timestamp: String,
    pub clip_filename: String,
    pub frame_index: i64,
    pub detector_model: String,
    pub class: String,
    pub confidence: f64,
    pub bbox_x1: f64,
    pub bbox_y1: f64,
    pub bbox_x2: f64,
    pub bbox_y2: f64,
    pub species: Option<String>,
    pub species_confidence: Option<f64>,
    pub crop_path: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub processing_instance: String,
}

/// Lightweight wrapper around a SQLite connection.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the SQLite database and initialise tables.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create DB directory {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Cannot open DB at {}", path.display()))?;

        // Performance tuning
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous  = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size   = -4000;",
        )
        .context("Cannot set PRAGMAs")?;

        // Schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS detections (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp           TEXT NOT NULL,
                clip_filename       TEXT NOT NULL,
                frame_index         INTEGER NOT NULL,
                detector_model      TEXT NOT NULL,
                class               TEXT NOT NULL,
                confidence          REAL NOT NULL,
                bbox_x1             REAL NOT NULL,
                bbox_y1             REAL NOT NULL,
                bbox_x2             REAL NOT NULL,
                bbox_y2             REAL NOT NULL,
                species             TEXT,
                species_confidence  REAL,
                crop_path           TEXT,
                latitude            REAL,
                longitude           REAL,
                processing_instance TEXT,
                created_at          TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_detections_timestamp
                ON detections (timestamp);
            CREATE INDEX IF NOT EXISTS idx_detections_class
                ON detections (class);
            CREATE INDEX IF NOT EXISTS idx_detections_species
                ON detections (species);
            CREATE INDEX IF NOT EXISTS idx_detections_created
                ON detections (created_at);

            -- Summary table: per-day, per-class counts for fast dashboards
            CREATE TABLE IF NOT EXISTS daily_summary (
                date        TEXT NOT NULL,
                class       TEXT NOT NULL,
                species     TEXT,
                count       INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, class, species)
            );

            -- Processing state: track which clips have been processed
            CREATE TABLE IF NOT EXISTS processed_clips (
                filename    TEXT PRIMARY KEY,
                processed_at TEXT NOT NULL DEFAULT (datetime('now')),
                frame_count INTEGER,
                detection_count INTEGER
            );",
        )
        .context("Cannot create schema")?;

        info!("Database schema initialised at {}", path.display());
        Ok(Self { conn })
    }

    /// Insert a detection into the database.
    pub fn insert_detection(&self, d: &DetectionRow) -> Result<i64> {
        let id = self.conn.execute(
            "INSERT INTO detections (
                timestamp, clip_filename, frame_index, detector_model,
                class, confidence, bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                species, species_confidence, crop_path,
                latitude, longitude, processing_instance
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                      ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                d.timestamp,
                d.clip_filename,
                d.frame_index,
                d.detector_model,
                d.class,
                d.confidence,
                d.bbox_x1,
                d.bbox_y1,
                d.bbox_x2,
                d.bbox_y2,
                d.species,
                d.species_confidence,
                d.crop_path,
                d.latitude,
                d.longitude,
                d.processing_instance,
            ],
        )?;

        // Update daily summary
        let date = if d.timestamp.len() >= 10 {
            &d.timestamp[..10]
        } else {
            &d.timestamp
        };

        self.conn.execute(
            "INSERT INTO daily_summary (date, class, species, count)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT (date, class, species) DO UPDATE
             SET count = count + 1",
            rusqlite::params![date, d.class, d.species],
        )?;

        Ok(id as i64)
    }

    /// Record that a clip has been fully processed.
    pub fn mark_clip_processed(
        &self,
        filename: &str,
        frame_count: usize,
        detection_count: usize,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO processed_clips
             (filename, frame_count, detection_count) VALUES (?1, ?2, ?3)",
            rusqlite::params![filename, frame_count as i64, detection_count as i64],
        )?;
        Ok(())
    }

    /// Check if a clip has already been processed.
    pub fn is_clip_processed(&self, filename: &str) -> bool {
        self.conn
            .prepare_cached(
                "SELECT 1 FROM processed_clips WHERE filename = ?1",
            )
            .and_then(|mut stmt| {
                stmt.query_row(rusqlite::params![filename], |_| Ok(true))
            })
            .unwrap_or(false)
    }

    /// Count detections within the last `seconds` seconds.
    pub fn recent_detection_count(&self, seconds: u64) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM detections
             WHERE created_at >= datetime('now', ?1)",
            rusqlite::params![format!("-{seconds} seconds")],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get total detection count per class.
    pub fn class_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT class, COUNT(*) FROM detections GROUP BY class ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get recent detections (for live status / web dashboard).
    pub fn recent_detections(&self, limit: usize) -> Result<Vec<DetectionRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT timestamp, clip_filename, frame_index, detector_model,
                    class, confidence, bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                    species, species_confidence, crop_path,
                    latitude, longitude, processing_instance
             FROM detections
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(DetectionRow {
                    timestamp: row.get(0)?,
                    clip_filename: row.get(1)?,
                    frame_index: row.get(2)?,
                    detector_model: row.get(3)?,
                    class: row.get(4)?,
                    confidence: row.get(5)?,
                    bbox_x1: row.get(6)?,
                    bbox_y1: row.get(7)?,
                    bbox_x2: row.get(8)?,
                    bbox_y2: row.get(9)?,
                    species: row.get(10)?,
                    species_confidence: row.get(11)?,
                    crop_path: row.get(12)?,
                    latitude: row.get(13)?,
                    longitude: row.get(14)?,
                    processing_instance: row.get(15)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Get species counts for a given date range.
    pub fn species_summary(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT COALESCE(species, class), SUM(count)
             FROM daily_summary
             WHERE date >= ?1 AND date <= ?2
             GROUP BY COALESCE(species, class)
             ORDER BY SUM(count) DESC",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![from_date, to_date], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_db() -> Database {
        Database::open(&PathBuf::from(":memory:")).unwrap()
    }

    #[test]
    fn test_insert_and_count() {
        let db = test_db();
        let row = DetectionRow {
            timestamp: "2026-03-04T12:00:00Z".into(),
            clip_filename: "test.mp4".into(),
            frame_index: 0,
            detector_model: "megadetector-v6".into(),
            class: "animal".into(),
            confidence: 0.95,
            bbox_x1: 0.1,
            bbox_y1: 0.1,
            bbox_x2: 0.5,
            bbox_y2: 0.5,
            species: Some("deer".into()),
            species_confidence: Some(0.8),
            crop_path: None,
            latitude: 42.0,
            longitude: -72.0,
            processing_instance: "test-01".into(),
        };

        let id = db.insert_detection(&row).unwrap();
        assert!(id > 0);

        let counts = db.class_counts().unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].0, "animal");
        assert_eq!(counts[0].1, 1);
    }

    #[test]
    fn test_clip_processed() {
        let db = test_db();
        assert!(!db.is_clip_processed("clip.mp4"));
        db.mark_clip_processed("clip.mp4", 60, 3).unwrap();
        assert!(db.is_clip_processed("clip.mp4"));
    }
}
