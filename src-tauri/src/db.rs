use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub id: String,
    pub text: String,
    pub raw_text: String,
    pub duration_ms: i64,
    pub audio_filename: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DictionaryEntry {
    pub id: String,
    pub phrase: String,
    pub replacement: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnippetEntry {
    pub id: String,
    pub trigger: String,
    pub content: String,
}

pub struct Database {
    conn: Mutex<Connection>,
    pub app_dir: PathBuf,
}

impl Database {
    pub fn new(app_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
        let audio_dir = app_dir.join("audio");
        std::fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;

        let db_path = app_dir.join("openglaido.db");
        let conn = Connection::open(db_path).map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                id TEXT PRIMARY KEY,
                text TEXT NOT NULL,
                raw_text TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                audio_filename TEXT,
                created_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS dictionary (
                id TEXT PRIMARY KEY,
                phrase TEXT NOT NULL UNIQUE,
                replacement TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS snippets (
                id TEXT PRIMARY KEY,
                trigger TEXT NOT NULL UNIQUE,
                content TEXT NOT NULL
            )",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(Self {
            conn: Mutex::new(conn),
            app_dir,
        })
    }

    pub fn insert_history(
        &self,
        id: &str,
        text: &str,
        raw_text: &str,
        duration_ms: i64,
        audio_filename: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO history (id, text, raw_text, duration_ms, audio_filename, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, text, raw_text, duration_ms, audio_filename, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, text, raw_text, duration_ms, audio_filename, created_at FROM history ORDER BY created_at DESC LIMIT ?1")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    raw_text: row.get(2)?,
                    duration_ms: row.get(3)?,
                    audio_filename: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.map_err(|e| e.to_string())?);
        }
        Ok(entries)
    }

    pub fn get_history_entry(&self, id: &str) -> Result<HistoryEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, text, raw_text, duration_ms, audio_filename, created_at FROM history WHERE id = ?1",
            params![id],
            |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    raw_text: row.get(2)?,
                    duration_ms: row.get(3)?,
                    audio_filename: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    }

    pub fn delete_history_entry(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM history WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // --- Dictionary ---
    pub fn get_dictionary(&self) -> Result<Vec<DictionaryEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, phrase, replacement FROM dictionary ORDER BY phrase ASC")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                Ok(DictionaryEntry {
                    id: row.get(0)?,
                    phrase: row.get(1)?,
                    replacement: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.map_err(|e| e.to_string())?);
        }
        Ok(entries)
    }

    pub fn add_dictionary_entry(&self, phrase: &str, replacement: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR REPLACE INTO dictionary (id, phrase, replacement) VALUES (?1, ?2, ?3)",
            params![id, phrase, replacement],
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn delete_dictionary_entry(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM dictionary WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // --- Snippets ---
    pub fn get_snippets(&self) -> Result<Vec<SnippetEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, trigger, content FROM snippets ORDER BY trigger ASC")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                Ok(SnippetEntry {
                    id: row.get(0)?,
                    trigger: row.get(1)?,
                    content: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.map_err(|e| e.to_string())?);
        }
        Ok(entries)
    }

    pub fn add_snippet(&self, trigger: &str, content: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR REPLACE INTO snippets (id, trigger, content) VALUES (?1, ?2, ?3)",
            params![id, trigger, content],
        )
        .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn delete_snippet(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM snippets WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn match_snippet(&self, text: &str) -> Option<String> {
        let trimmed = text.trim().trim_matches(|c: char| c.is_ascii_punctuation()).trim().to_lowercase();
        if trimmed.is_empty() {
            return None;
        }

        if let Ok(snippets) = self.get_snippets() {
            for s in snippets {
                let s_trig = s.trigger.trim().trim_matches(|c: char| c.is_ascii_punctuation()).trim().to_lowercase();
                if s_trig == trimmed {
                    return Some(s.content);
                }
            }
        }
        None
    }
}
