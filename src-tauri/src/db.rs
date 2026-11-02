use crate::models::{SessionRecord, StudioMode};
use crate::paths::db_path;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open() -> Result<Self, String> {
        let path = db_path()?;
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                mode TEXT NOT NULL,
                frame_rate INTEGER NOT NULL,
                resolution TEXT NOT NULL,
                created_at TEXT NOT NULL,
                duration_secs REAL NOT NULL DEFAULT 0,
                participants_json TEXT NOT NULL DEFAULT '[]',
                has_video INTEGER NOT NULL DEFAULT 1,
                local_path TEXT NOT NULL,
                drive_uploaded INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn upsert_session(&self, session: &SessionRecord) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mode = match session.mode {
            StudioMode::AudioOnly => "audio_only",
            StudioMode::AudioVideo => "audio_video",
        };
        conn.execute(
            r#"
            INSERT INTO sessions (
                id, name, mode, frame_rate, resolution, created_at, duration_secs,
                participants_json, has_video, local_path, drive_uploaded
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
            ON CONFLICT(id) DO UPDATE SET
                name=excluded.name,
                duration_secs=excluded.duration_secs,
                participants_json=excluded.participants_json,
                drive_uploaded=excluded.drive_uploaded
            "#,
            params![
                session.id,
                session.name,
                mode,
                session.frame_rate as i64,
                session.resolution,
                session.created_at.to_rfc3339(),
                session.duration_secs,
                serde_json::to_string(&session.participants).map_err(|e| e.to_string())?,
                session.has_video as i64,
                session.local_path,
                session.drive_uploaded as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, name, mode, frame_rate, resolution, created_at, duration_secs,
                       participants_json, has_video, local_path, drive_uploaded
                FROM sessions
                ORDER BY created_at DESC
                "#,
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let mode_str: String = row.get(2)?;
                let created: String = row.get(5)?;
                let participants_json: String = row.get(7)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    mode_str,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    created,
                    row.get::<_, f64>(6)?,
                    participants_json,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                name,
                mode_str,
                frame_rate,
                resolution,
                created,
                duration_secs,
                participants_json,
                has_video,
                local_path,
                drive_uploaded,
            ) = row.map_err(|e| e.to_string())?;

            let mode = if mode_str == "audio_only" {
                StudioMode::AudioOnly
            } else {
                StudioMode::AudioVideo
            };
            let created_at: DateTime<Utc> = created
                .parse()
                .unwrap_or_else(|_| Utc::now());
            let participants = serde_json::from_str(&participants_json).unwrap_or_default();

            out.push(SessionRecord {
                id,
                name,
                mode,
                frame_rate: frame_rate as u32,
                resolution,
                created_at,
                duration_secs,
                participants,
                has_video: has_video != 0,
                local_path,
                drive_uploaded: drive_uploaded != 0,
            });
        }
        Ok(out)
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, String> {
        Ok(self
            .list_sessions()?
            .into_iter()
            .find(|s| s.id == id))
    }

    pub fn mark_drive_uploaded(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sessions SET drive_uploaded = 1 WHERE id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
