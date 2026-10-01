use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct HistoryEntry {
    pub id: String,
    pub text: String,
    pub raw_text: String,
    pub duration_ms: i64,
    pub audio_filename: Option<String>,
    pub created_at: String,
    pub kind: String,   // "dictation" | "command"
    pub status: String, // "ok" | "failed" | "running"
    pub error: Option<String>,
    pub app_name: Option<String>,
    pub app_bundle_id: Option<String>,
    pub website: Option<String>,
    pub answer: Option<String>, // command answer (markdown)
    pub sources: Vec<Source>,   // stored as a JSON array
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Source {
    pub title: String,
    pub url: String,
}

impl HistoryEntry {
    /// An "ok" dictation row created now; set the rest with struct update syntax.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: String::new(),
            raw_text: String::new(),
            duration_ms: 0,
            audio_filename: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            kind: "dictation".to_string(),
            status: "ok".to_string(),
            error: None,
            app_name: None,
            app_bundle_id: None,
            website: None,
            answer: None,
            sources: Vec::new(),
        }
    }
}

const HISTORY_COLUMNS: &str = "id, text, raw_text, duration_ms, audio_filename, created_at, kind, status, error, app_name, app_bundle_id, answer, sources, website";

/// Columns added after the first release, ALTERed onto older databases.
const HISTORY_ADDED_COLUMNS: [(&str, &str); 8] = [
    ("kind", "TEXT NOT NULL DEFAULT 'dictation'"),
    ("status", "TEXT NOT NULL DEFAULT 'ok'"),
    ("error", "TEXT"),
    ("app_name", "TEXT"),
    ("app_bundle_id", "TEXT"),
    ("answer", "TEXT"),
    ("sources", "TEXT"),
    ("website", "TEXT"),
];

fn migrate_history(conn: &Connection) -> Result<()> {
    let existing = conn
        .prepare("SELECT name FROM pragma_table_info('history')")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    for (name, definition) in HISTORY_ADDED_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            conn.execute(&format!("ALTER TABLE history ADD COLUMN {name} {definition}"), [])?;
        }
    }
    Ok(())
}

fn history_row(row: &rusqlite::Row) -> Result<HistoryEntry> {
    let sources: Option<String> = row.get(12)?;
    Ok(HistoryEntry {
        id: row.get(0)?,
        text: row.get(1)?,
        raw_text: row.get(2)?,
        duration_ms: row.get(3)?,
        audio_filename: row.get(4)?,
        created_at: row.get(5)?,
        kind: row.get(6)?,
        status: row.get(7)?,
        error: row.get(8)?,
        app_name: row.get(9)?,
        app_bundle_id: row.get(10)?,
        website: row.get(13)?,
        answer: row.get(11)?,
        sources: sources.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
    })
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
    retries: Mutex<HashSet<String>>,
    pub app_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct HistoryCursor {
    pub created_at: String,
    pub id: String,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct HistoryStats {
    pub words: u64,
    pub duration_ms: i64,
    pub streak: u64,
}

pub struct HistoryRetryGuard<'a> {
    db: &'a Database,
    id: String,
}

impl Drop for HistoryRetryGuard<'_> {
    fn drop(&mut self) {
        self.db.retries.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
    }
}

// Match JavaScript's /\s+/ word splitting, including BOM but excluding NEXT LINE.
fn word_count(text: &str) -> u64 {
    text.split(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
        .filter(|word| !word.is_empty()).count() as u64
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
        migrate_history(&conn).map_err(|e| e.to_string())?;
        conn.execute("CREATE INDEX IF NOT EXISTS history_order ON history(created_at DESC, id DESC)", [])
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
            retries: Mutex::new(HashSet::new()),
            app_dir,
        })
    }

    pub fn insert_history(&self, e: &HistoryEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sources = serde_json::to_string(&e.sources).map_err(|e| e.to_string())?;
        conn.execute(
            &format!("INSERT INTO history ({HISTORY_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
            params![
                e.id, e.text, e.raw_text, e.duration_ms, e.audio_filename, e.created_at, e.kind, e.status, e.error,
                e.app_name, e.app_bundle_id, e.answer, sources, e.website
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Overwrites every column but `id` and `created_at`.
    pub fn update_history(&self, e: &HistoryEntry) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let sources = serde_json::to_string(&e.sources).map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "UPDATE history SET text = ?2, raw_text = ?3, duration_ms = ?4, audio_filename = ?5, kind = ?6,
                 status = ?7, error = ?8, app_name = ?9, app_bundle_id = ?10, answer = ?11, sources = ?12, website = ?13 WHERE id = ?1",
                params![
                    e.id, e.text, e.raw_text, e.duration_ms, e.audio_filename, e.kind, e.status, e.error, e.app_name,
                    e.app_bundle_id, e.answer, sources, e.website
                ],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err(format!("No history entry {}", e.id));
        }
        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryEntry>, String> {
        self.get_history_page("", None, limit)
    }

    /// Search the whole database, including app names, preserving Unicode case folding.
    pub fn search_history(&self, query: &str, limit: usize) -> Result<Vec<HistoryEntry>, String> {
        self.get_history_page(query, None, limit)
    }

    pub fn get_history_page(&self, query: &str, before: Option<&HistoryCursor>, limit: usize) -> Result<Vec<HistoryEntry>, String> {
        if limit == 0 { return Ok(Vec::new()); }
        let limit = limit.min(500);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {HISTORY_COLUMNS} FROM history
             WHERE ?1 IS NULL OR (created_at, id) < (?1, ?2)
             ORDER BY created_at DESC, id DESC LIMIT ?3"
        )).map_err(|e| e.to_string())?;
        let query = query.trim().to_lowercase();
        let rows = stmt.query_map(params![
            before.map(|c| &c.created_at), before.map(|c| &c.id),
            if query.is_empty() { limit as i64 } else { -1 },
        ], history_row).map_err(|e| e.to_string())?;
        // ponytail: stream matching rows for Unicode search; add a Unicode FTS index if scans become slow.
        let mut entries = Vec::new();
        for row in rows {
            let row = row.map_err(|e| e.to_string())?;
            if query.is_empty() || [row.text.as_str(), row.raw_text.as_str(), row.answer.as_deref().unwrap_or_default(), row.app_name.as_deref().unwrap_or_default()]
                .iter().any(|text| text.to_lowercase().contains(&query)) {
                entries.push(row);
                if entries.len() == limit { break; }
            }
        }
        Ok(entries)
    }

    pub fn history_stats(&self) -> Result<HistoryStats, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("SELECT text, duration_ms, created_at FROM history WHERE kind != 'command' AND status = 'ok'")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
            .map_err(|e| e.to_string())?;
        let mut stats = HistoryStats::default();
        let mut days = HashSet::new();
        for row in rows {
            let (text, duration, created_at) = row.map_err(|e| e.to_string())?;
            stats.words += word_count(&text);
            stats.duration_ms += duration;
            if let Ok(date) = chrono::DateTime::parse_from_rfc3339(&created_at) {
                days.insert(date.with_timezone(&chrono::Local).date_naive());
            }
        }
        let mut day = chrono::Local::now().date_naive();
        while days.contains(&day) {
            stats.streak += 1;
            match day.pred_opt() { Some(previous) => day = previous, None => break }
        }
        Ok(stats)
    }

    /// One paid retry per entry at a time, released on success, failure or cancellation.
    pub fn begin_retry(&self, id: &str) -> Result<HistoryRetryGuard<'_>, String> {
        let mut retries = self.retries.lock().map_err(|e| e.to_string())?;
        if retries.contains(id) || self.get_history_entry(id)?.status == "running" {
            return Err("This recording is already being processed".into());
        }
        retries.insert(id.to_owned());
        Ok(HistoryRetryGuard { db: self, id: id.to_owned() })
    }

    /// Apps dictated into since `since` (RFC 3339), most used first: (bundle id, name).
    pub fn recent_apps(&self, since: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT app_bundle_id, MAX(app_name), COUNT(*) AS n FROM history
                 WHERE app_bundle_id IS NOT NULL AND app_bundle_id != '' AND created_at >= ?1
                 GROUP BY app_bundle_id ORDER BY n DESC LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_>>().map_err(|e| e.to_string())
    }

    pub fn get_history_entry(&self, id: &str) -> Result<HistoryEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(&format!("SELECT {HISTORY_COLUMNS} FROM history WHERE id = ?1"), params![id], history_row)
            .map_err(|e| e.to_string())
    }

    /// Rows left "running" by a quit or crash become failed records (Retry keeps working).
    pub fn mark_interrupted(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE history SET status = 'failed', error = 'Interrupted: OpenGlaido quit before this finished' WHERE status = 'running'",
            [],
        )
        .map_err(|e| e.to_string())
    }

    pub fn delete_history_entry(&self, id: &str) -> Result<(), String> {
        let retries = self.retries.lock().map_err(|e| e.to_string())?;
        if retries.contains(id) { return Err("This recording is already being processed".into()); }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let transaction = conn.transaction().map_err(|e| e.to_string())?;
        let filename: Option<String> = transaction.query_row("SELECT audio_filename FROM history WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        // Stage the row deletion, but commit only after audio removal succeeds. NotFound is already deleted.
        transaction.execute("DELETE FROM history WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
        if let Some(filename) = filename {
            let path = std::path::Path::new(&filename);
            if path.components().count() != 1 || !matches!(path.components().next(), Some(std::path::Component::Normal(_))) {
                return Err("Invalid recording filename".into());
            }
            match std::fs::remove_file(self.app_dir.join("audio").join(path)) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(format!("Couldn't delete the recording audio: {e}")),
            }
        }
        transaction.commit().map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("openglaido-db-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn old_history_table_gains_new_columns() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let old = Connection::open(dir.join("openglaido.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE history (id TEXT PRIMARY KEY, text TEXT NOT NULL, raw_text TEXT NOT NULL,
                duration_ms INTEGER NOT NULL, audio_filename TEXT, created_at TEXT NOT NULL);
             INSERT INTO history VALUES ('a', 'hi', 'hi', 1200, 'a.wav', '2026-01-01T00:00:00+00:00');",
        )
        .unwrap();
        drop(old);

        let db = Database::new(dir.clone()).unwrap();
        drop(db);
        let db = Database::new(dir.clone()).unwrap(); // second open: nothing left to migrate
        let entry = db.get_history_entry("a").unwrap();
        assert_eq!((entry.kind.as_str(), entry.status.as_str()), ("dictation", "ok"));
        assert_eq!((entry.text.as_str(), entry.duration_ms), ("hi", 1200));
        assert!(entry.error.is_none() && entry.app_name.is_none() && entry.website.is_none() && entry.answer.is_none() && entry.sources.is_empty());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn insert_update_search_round_trip() {
        let dir = temp_dir();
        let db = Database::new(dir.clone()).unwrap();
        let mut entry = HistoryEntry {
            text: "open 100% of_it".into(),
            kind: "command".into(),
            status: "running".into(),
            app_name: Some("Mail".into()),
            app_bundle_id: Some("com.apple.mail".into()),
            ..HistoryEntry::new("c")
        };
        db.insert_history(&entry).unwrap();
        db.insert_history(&HistoryEntry { text: "open 1000 ofxit".into(), ..HistoryEntry::new("d") }).unwrap();
        assert_eq!(db.get_history_entry("c").unwrap(), entry);

        entry.status = "ok".into();
        entry.answer = Some("**done**".into());
        entry.sources = vec![Source { title: "Docs".into(), url: "https://example.com".into() }];
        db.update_history(&entry).unwrap();
        assert_eq!(db.get_history_entry("c").unwrap(), entry);
        assert!(db.update_history(&HistoryEntry::new("missing")).is_err());

        // LIKE wildcards in the query are literal.
        let ids = |q| db.search_history(q, 10).unwrap().into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids("100%"), ["c"]);
        assert_eq!(ids("OF_IT"), ["c"]);
        assert_eq!(ids("open").len(), 2);
        assert!(ids("nothing").is_empty());
        assert_eq!(db.get_history(1).unwrap().len(), 1);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn complete_history_pages_unicode_search_and_stats_survive_five_hundred_rows() {
        let dir = temp_dir();
        let db = Database::new(dir.clone()).unwrap();
        let now = chrono::Local::now();
        for n in 0..605 {
            let date = now - chrono::Duration::days((n / 300) as i64);
            db.insert_history(&HistoryEntry {
                text: "one\t two\u{feff}three".into(), duration_ms: 1200,
                created_at: date.to_rfc3339(),
                app_name: (n == 604).then(|| "ŽIVJO Mail".into()),
                website: Some("https://mail.example.test/inbox".into()),
                ..HistoryEntry::new(format!("row-{n:04}"))
            }).unwrap();
        }
        let mut ids = HashSet::new();
        let mut cursor = None;
        loop {
            let page = db.get_history_page("", cursor.as_ref(), 100).unwrap();
            if page.is_empty() { break; }
            for entry in &page { assert!(ids.insert(entry.id.clone())); }
            let last = page.last().unwrap();
            cursor = Some(HistoryCursor { created_at: last.created_at.clone(), id: last.id.clone() });
        }
        assert_eq!(ids.len(), 605);
        assert_eq!(db.search_history("živjo", 10).unwrap()[0].id, "row-0604");
        assert_eq!(db.get_history_entry("row-0604").unwrap().website.as_deref(), Some("https://mail.example.test/inbox"));
        db.insert_history(&HistoryEntry { text: "ignored words".into(), kind: "command".into(), ..HistoryEntry::new("command") }).unwrap();
        db.insert_history(&HistoryEntry { text: "ignored words".into(), status: "failed".into(), ..HistoryEntry::new("failed") }).unwrap();
        assert_eq!(db.history_stats().unwrap(), HistoryStats { words: 1815, duration_ms: 726000, streak: 3 });
        assert_eq!(word_count("a\u{85}b c"), 2);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_audio_deletion_keeps_history_and_retries_are_exclusive() {
        let dir = temp_dir();
        let db = Database::new(dir.clone()).unwrap();
        db.insert_history(&HistoryEntry { audio_filename: Some("record.wav".into()), ..HistoryEntry::new("record") }).unwrap();
        std::fs::create_dir(dir.join("audio/record.wav")).unwrap();
        assert!(db.delete_history_entry("record").is_err());
        assert!(db.get_history_entry("record").is_ok());
        let retry = db.begin_retry("record").unwrap();
        assert!(db.begin_retry("record").is_err());
        assert!(db.delete_history_entry("record").is_err());
        drop(retry);
        drop(db.begin_retry("record").unwrap());
        std::fs::remove_dir(dir.join("audio/record.wav")).unwrap();
        db.delete_history_entry("record").unwrap(); // Missing audio already meets the deletion guarantee.
        assert!(db.get_history_entry("record").is_err());
        db.insert_history(&HistoryEntry { audio_filename: Some("real.wav".into()), ..HistoryEntry::new("real") }).unwrap();
        std::fs::write(dir.join("audio/real.wav"), b"recording").unwrap();
        db.delete_history_entry("real").unwrap();
        assert!(!dir.join("audio/real.wav").exists());
        db.insert_history(&HistoryEntry { status: "running".into(), ..HistoryEntry::new("running") }).unwrap();
        assert!(db.begin_retry("running").is_err());
        assert!(db.begin_retry("missing").is_err());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
