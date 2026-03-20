//! SQLite persistence layer for Gaia Light detections (via libsql / Turso).
//!
//! Stores every detection (animal/person/vehicle) with its bounding box,
//! confidence score, optional species classification, and metadata.

use std::path::Path;

use anyhow::{Context, Result};
use libsql::params;
use tracing::{debug, info};

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
    /// Which classifier model produced the species label.
    pub species_model: Option<String>,
    pub crop_path: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub processing_instance: String,
    /// URL of the capture node that recorded the clip.
    pub source_node: String,
    /// Re-identified individual ID (persons).
    pub individual_id: Option<i64>,
}

/// Lightweight wrapper around a libsql connection.
pub struct Database {
    conn: libsql::Connection,
}

impl Database {
    /// Open (or create) the SQLite database and initialise tables.
    pub async fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create DB directory {}", parent.display()))?;
        }

        let db = libsql::Builder::new_local(
            path.to_str().context("Non-UTF-8 database path")?,
        )
        .build()
        .await
        .with_context(|| format!("Cannot open DB at {}", path.display()))?;

        let conn = db.connect().context("Cannot connect to database")?;

        // Performance tuning
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous  = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size   = -4000;",
        )
        .await
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
                species_model       TEXT,
                crop_path           TEXT,
                latitude            REAL,
                longitude           REAL,
                processing_instance TEXT,
                source_node         TEXT NOT NULL DEFAULT '',
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
            );

            -- High-confidence animal detections with no species label,
            -- saved as training candidates for future classifier models.
            CREATE TABLE IF NOT EXISTS training_candidates (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp       TEXT NOT NULL,
                clip_filename   TEXT NOT NULL,
                frame_index     INTEGER NOT NULL,
                confidence      REAL NOT NULL,
                bbox_x1         REAL NOT NULL,
                bbox_y1         REAL NOT NULL,
                bbox_x2         REAL NOT NULL,
                bbox_y2         REAL NOT NULL,
                crop_path       TEXT,
                latitude        REAL,
                longitude       REAL,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_training_confidence
                ON training_candidates (confidence DESC);
            CREATE INDEX IF NOT EXISTS idx_training_created
                ON training_candidates (created_at);

            -- Identified individuals (person re-ID)
            CREATE TABLE IF NOT EXISTS individuals (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL DEFAULT '',
                embedding       BLOB NOT NULL,
                detection_count INTEGER NOT NULL DEFAULT 1,
                first_seen      TEXT NOT NULL DEFAULT (datetime('now')),
                last_seen       TEXT NOT NULL DEFAULT (datetime('now')),
                representative_crop TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_individuals_name
                ON individuals (name);",
        )
        .await
        .context("Cannot create schema")?;

        // ── Migrations for databases created before species_model existed ──
        let _ = conn
            .execute_batch("ALTER TABLE detections ADD COLUMN species_model TEXT;")
            .await;

        // Migration: add source_node column (capture node URL)
        let _ = conn
            .execute_batch(
                "ALTER TABLE detections ADD COLUMN source_node TEXT NOT NULL DEFAULT '';",
            )
            .await;

        // Migration: add individual_id column (person re-ID)
        let _ = conn
            .execute_batch(
                "ALTER TABLE detections ADD COLUMN individual_id INTEGER REFERENCES individuals(id);",
            )
            .await;

        info!("Database schema initialised at {}", path.display());
        Ok(Self { conn })
    }

    /// Insert a detection into the database.
    pub async fn insert_detection(&self, d: &DetectionRow) -> Result<i64> {
        let rows_changed = self.conn.execute(
            "INSERT INTO detections (
                timestamp, clip_filename, frame_index, detector_model,
                class, confidence, bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                species, species_confidence, species_model, crop_path,
                latitude, longitude, processing_instance, source_node,
                individual_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                      ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                d.timestamp.clone(),
                d.clip_filename.clone(),
                d.frame_index,
                d.detector_model.clone(),
                d.class.clone(),
                d.confidence,
                d.bbox_x1,
                d.bbox_y1,
                d.bbox_x2,
                d.bbox_y2,
                d.species.clone(),
                d.species_confidence,
                d.species_model.clone(),
                d.crop_path.clone(),
                d.latitude,
                d.longitude,
                d.processing_instance.clone(),
                d.source_node.clone(),
                d.individual_id,
            ],
        ).await?;

        // Get the last insert rowid
        let mut rows = self.conn.query("SELECT last_insert_rowid()", ()).await?;
        let id: i64 = rows
            .next()
            .await?
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(rows_changed as i64);

        debug!(
            "Inserted detection id={}: {} {} ({:.1}%) species={:?} in {}/{}",
            id,
            d.class,
            d.detector_model,
            d.confidence * 100.0,
            d.species,
            d.clip_filename,
            d.frame_index
        );

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
            params![date.to_string(), d.class.clone(), d.species.clone()],
        ).await?;

        Ok(id)
    }

    /// Insert a high-confidence animal detection (with no species label)
    /// as a training candidate for future classifier models.
    pub async fn insert_training_candidate(&self, d: &DetectionRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO training_candidates (
                timestamp, clip_filename, frame_index, confidence,
                bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                crop_path, latitude, longitude
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                d.timestamp.clone(),
                d.clip_filename.clone(),
                d.frame_index,
                d.confidence,
                d.bbox_x1,
                d.bbox_y1,
                d.bbox_x2,
                d.bbox_y2,
                d.crop_path.clone(),
                d.latitude,
                d.longitude,
            ],
        ).await?;
        Ok(())
    }

    /// Count how many training candidates are stored.
    pub async fn training_candidate_count(&self) -> Result<i64> {
        let mut rows = self.conn.query(
            "SELECT COUNT(*) FROM training_candidates",
            (),
        ).await?;
        let count = rows
            .next()
            .await?
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);
        Ok(count)
    }

    /// Record that a clip has been fully processed.
    pub async fn mark_clip_processed(
        &self,
        filename: &str,
        frame_count: usize,
        detection_count: usize,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO processed_clips
             (filename, frame_count, detection_count) VALUES (?1, ?2, ?3)",
            params![filename.to_string(), frame_count as i64, detection_count as i64],
        ).await?;
        debug!(
            "mark_clip_processed: {} ({} frames, {} detections)",
            filename, frame_count, detection_count
        );
        Ok(())
    }

    /// Check if a clip has already been processed.
    pub async fn is_clip_processed(&self, filename: &str) -> bool {
        let result = match self.conn
            .query(
                "SELECT 1 FROM processed_clips WHERE filename = ?1",
                params![filename.to_string()],
            )
            .await
        {
            Ok(mut rows) => matches!(rows.next().await, Ok(Some(_))),
            Err(_) => false,
        };
        if result {
            debug!("is_clip_processed({filename}): already processed");
        }
        result
    }

    /// Count detections within the last `seconds` seconds.
    pub async fn recent_detection_count(&self, seconds: u64) -> Result<usize> {
        let mut rows = self.conn.query(
            "SELECT COUNT(*) FROM detections
             WHERE created_at >= datetime('now', ?1)",
            params![format!("-{seconds} seconds")],
        ).await?;
        let count = rows
            .next()
            .await?
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);
        Ok(count as usize)
    }

    /// Get total detection count per class.
    pub async fn class_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut rows = self.conn.query(
            "SELECT class, COUNT(*) FROM detections GROUP BY class ORDER BY COUNT(*) DESC",
            (),
        ).await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push((row.get::<String>(0)?, row.get::<i64>(1)?));
        }
        Ok(out)
    }

    /// Get recent detections (for live status / web dashboard).
    pub async fn recent_detections(&self, limit: usize) -> Result<Vec<DetectionRow>> {
        let mut rows = self.conn.query(
            "SELECT timestamp, clip_filename, frame_index, detector_model,
                    class, confidence, bbox_x1, bbox_y1, bbox_x2, bbox_y2,
                    species, species_confidence, species_model, crop_path,
                    latitude, longitude, processing_instance,
                    COALESCE(source_node, ''), individual_id
             FROM detections
             ORDER BY created_at DESC
             LIMIT ?1",
            params![limit as i64],
        ).await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push(DetectionRow {
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
                species_model: row.get(12)?,
                crop_path: row.get(13)?,
                latitude: row.get(14)?,
                longitude: row.get(15)?,
                processing_instance: row.get(16)?,
                source_node: row.get(17)?,
                individual_id: row.get(18)?,
            });
        }

        Ok(out)
    }

    /// Get species counts for a given date range.
    pub async fn species_summary(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<(String, i64)>> {
        let mut rows = self.conn.query(
            "SELECT COALESCE(species, class), SUM(count)
             FROM daily_summary
             WHERE date >= ?1 AND date <= ?2
             GROUP BY COALESCE(species, class)
             ORDER BY SUM(count) DESC",
            params![from_date.to_string(), to_date.to_string()],
        ).await?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            out.push((row.get(0)?, row.get(1)?));
        }
        Ok(out)
    }

    // ── Person re-identification ─────────────────────────────────────

    /// Cosine similarity threshold for matching a person embedding to
    /// an existing individual.  Above this → same person.
    const REID_THRESHOLD: f64 = 0.65;

    /// Find the best matching individual for an embedding, or return
    /// `None` if no match exceeds the threshold.
    pub async fn find_matching_individual(&self, embedding: &[f32]) -> Result<Option<i64>> {
        let mut rows = self.conn.query(
            "SELECT id, embedding FROM individuals",
            (),
        ).await?;

        let mut best_id: Option<i64> = None;
        let mut best_sim: f64 = Self::REID_THRESHOLD;

        while let Some(row) = rows.next().await? {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let stored = bytes_to_f32(&blob);
            let sim = cosine_similarity(embedding, &stored);
            if sim > best_sim {
                best_sim = sim;
                best_id = Some(id);
            }
        }

        if let Some(id) = best_id {
            debug!("Re-ID match: individual {id} (similarity {best_sim:.3})");
        }
        Ok(best_id)
    }

    /// Create a new individual from an embedding.  Returns the new ID.
    pub async fn create_individual(
        &self,
        embedding: &[f32],
        crop_path: Option<&str>,
    ) -> Result<i64> {
        let blob = f32_to_bytes(embedding);
        self.conn.execute(
            "INSERT INTO individuals (embedding, representative_crop)
             VALUES (?1, ?2)",
            params![blob, crop_path.map(|s| s.to_string())],
        ).await?;
        let mut rows = self.conn.query("SELECT last_insert_rowid()", ()).await?;
        let id = rows
            .next()
            .await?
            .and_then(|r| r.get::<i64>(0).ok())
            .unwrap_or(0);
        info!("Created new individual id={id}");
        Ok(id)
    }

    /// Update the last_seen timestamp and detection_count for an individual.
    pub async fn touch_individual(&self, individual_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE individuals
             SET detection_count = detection_count + 1,
                 last_seen = datetime('now')
             WHERE id = ?1",
            params![individual_id],
        ).await?;
        Ok(())
    }

    /// Rename an individual.
    pub async fn rename_individual(&self, individual_id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE individuals SET name = ?1 WHERE id = ?2",
            params![name.to_string(), individual_id],
        ).await?;
        info!("Renamed individual {individual_id} to {name:?}");
        Ok(())
    }
}

// ── Embedding serialisation helpers ──────────────────────────────────────

/// Serialise `&[f32]` to little-endian bytes for BLOB storage.
fn f32_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialise little-endian bytes back to `Vec<f32>`.
fn bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let mut dot: f64 = 0.0;
    let mut na: f64 = 0.0;
    let mut nb: f64 = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-12 { 0.0 } else { dot / denom }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    async fn test_db() -> Database {
        Database::open(&PathBuf::from(":memory:")).await.unwrap()
    }

    #[tokio::test]
    async fn test_insert_and_count() {
        let db = test_db().await;
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
            species_model: Some("speciesnet".into()),
            crop_path: None,
            latitude: 42.0,
            longitude: -72.0,
            processing_instance: "test-01".into(),
            source_node: "http://localhost:8090".into(),
            individual_id: None,
        };

        let id = db.insert_detection(&row).await.unwrap();
        assert!(id > 0);

        let counts = db.class_counts().await.unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].0, "animal");
        assert_eq!(counts[0].1, 1);
    }

    #[tokio::test]
    async fn test_clip_processed() {
        let db = test_db().await;
        assert!(!db.is_clip_processed("clip.mp4").await);
        db.mark_clip_processed("clip.mp4", 60, 3).await.unwrap();
        assert!(db.is_clip_processed("clip.mp4").await);
    }
}
