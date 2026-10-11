//! Opt-in evaluation evidence, kept alongside History. Credentials and full browser URLs
//! never enter snapshots; clearing evaluation data invalidates outstanding writers.

use crate::db::{Database, DictionaryEntry};
use crate::frontmost::AppContext;
use crate::transcribe::TranscriptionConfig;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_EDIT_UTF16: usize = 16 * 1024;
const MAX_WORDS: usize = 2_048;
const MAX_EDIT_CELLS: usize = 1_000_000;
/// Per History row: preserve the original and the newest eight explicit comparisons.
const MAX_COMPARISONS: usize = 8;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowSettings {
    pub enabled: bool,
    pub cloud_analysis: bool,
    pub track_edits: bool,
    pub judges: Vec<WorkflowJudgeConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowJudgeConfig {
    pub provider: String,
    pub endpoint_url: String,
    pub model_name: String,
}

impl WorkflowSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.judges.len() > 3 {
            return Err("Workflow Insights supports at most three judges".into());
        }
        for judge in &self.judges {
            bounded_text(&judge.provider, 64, false)?;
            bounded_text(&judge.model_name, 256, false)?;
            bounded_text(&judge.endpoint_url, 2_048, false)?;
            let url =
                reqwest::Url::parse(&judge.endpoint_url).map_err(|_| "Invalid judge endpoint")?;
            let loopback = matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            );
            if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
                return Err("Judge endpoints require HTTPS or localhost HTTP".into());
            }
            if url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "Judge endpoints cannot contain credentials, query strings, or fragments"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowModel {
    pub source: String,
    pub provider: String,
    pub model_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowSnapshot {
    pub stt: WorkflowModel,
    pub llm: WorkflowModel,
    pub live: bool,
    /// Configured backups only. The winning provider is unavailable to this beta.
    pub configured_backups: Vec<WorkflowModel>,
    pub winning_provider: Option<String>,
    pub style: String,
    pub raw_text: bool,
    pub rule_id: Option<String>,
    pub system_prompt: String,
    pub languages: Vec<String>,
    pub dictionary_revision: String,
    /// The dictionary changed between capture and completion; provenance cannot identify
    /// one stable setup. Keep the evidence, but exclude it from comparisons by setup.
    #[serde(default)]
    pub dictionary_changed: bool,
    pub app_version: String,
    pub app_bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub website_host: Option<String>,
    pub preprocessing: String,
    pub input_kind: String,
    pub actual_microphone: Option<String>,
    pub sample_rate: Option<u32>,
    pub fallback: Option<bool>,
}

impl WorkflowSnapshot {
    pub fn from_config(
        config: &TranscriptionConfig,
        context: Option<&AppContext>,
        dictionary_revision: String,
    ) -> Self {
        let formatting = config.formatting_for(context);
        let stt = WorkflowModel {
            source: config.stt_source.clone(),
            provider: if config.stt_source == "local" {
                "local".into()
            } else {
                config.stt_provider.clone()
            },
            model_name: if config.stt_source == "local" {
                config.local_stt_model.clone()
            } else {
                config.model_name.clone()
            },
        };
        let llm = WorkflowModel {
            source: config.llm_source.clone(),
            provider: match config.llm_source.as_str() {
                "local" => "local".into(),
                "off" => "off".into(),
                _ => config.llm_provider.clone(),
            },
            model_name: match config.llm_source.as_str() {
                "local" => config.local_llm_model.clone(),
                "off" => String::new(),
                _ => config.llm_model_name.clone().unwrap_or_default(),
            },
        };
        Self {
            stt,
            llm,
            live: config.uses_live_transcription(),
            configured_backups: config
                .live_backup_configs()
                .into_iter()
                .map(|backup| WorkflowModel {
                    source: "cloud".into(),
                    provider: backup.stt_provider,
                    model_name: backup.model_name,
                })
                .collect(),
            winning_provider: None,
            style: formatting.style,
            raw_text: formatting.raw_text,
            rule_id: formatting.rule_id,
            system_prompt: formatting.system_prompt,
            languages: config.languages.clone(),
            dictionary_revision,
            dictionary_changed: false,
            app_version: env!("CARGO_PKG_VERSION").into(),
            app_bundle_id: context.map(|c| c.bundle_id.clone()),
            app_name: context.map(|c| c.name.clone()),
            website_host: context.and_then(|c| c.url.as_deref()).and_then(|url| {
                let host = crate::transcribe::host_of(url);
                if host.len() > 253 {
                    return None;
                }
                reqwest::Url::parse(&format!("https://{host}"))
                    .ok()?
                    .host_str()
                    .map(str::to_owned)
            }),
            preprocessing: "RNNoise, speech detection, 16 kHz PCM".into(),
            input_kind: "microphone".into(),
            actual_microphone: None,
            sample_rate: None,
            fallback: None,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkflowTimings {
    pub capture_ready_ms: Option<u64>,
    pub first_sample_ms: Option<u64>,
    pub audio_duration_ms: Option<u64>,
    pub stt_ms: Option<u64>,
    pub formatting_ms: Option<u64>,
    /// Time to requesting delivery; insertion into another app is unverified.
    pub delivery_requested_ms: Option<u64>,
    pub clipping_fraction: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunKind {
    Recording,
    Comparison,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub history_id: String,
    pub created_at: String,
    pub kind: WorkflowRunKind,
    pub label: String,
    pub snapshot: WorkflowSnapshot,
    pub timings: WorkflowTimings,
    pub raw_text: String,
    pub formatted_text: String,
    pub final_text: String,
    pub outcome: String,
}

impl WorkflowRun {
    pub fn new(
        history_id: impl Into<String>,
        kind: WorkflowRunKind,
        label: impl Into<String>,
        snapshot: WorkflowSnapshot,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            history_id: history_id.into(),
            created_at: now(),
            kind,
            label: label.into(),
            snapshot,
            timings: WorkflowTimings::default(),
            raw_text: String::new(),
            formatted_text: String::new(),
            final_text: String::new(),
            outcome: "completed".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowReferenceKind {
    Spoken,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFeedback {
    pub reference_kind: WorkflowReferenceKind,
    pub reference_text: String,
    pub category: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowEditStatus {
    Pending,
    Confirmed,
    Dismissed,
}

/// Only the passage inserted by OpenGlaido, never the surrounding destination field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEditObservation {
    pub history_id: String,
    pub original_text: String,
    pub edited_text: String,
    pub app_bundle_id: Option<String>,
    pub observed_at: String,
    pub status: WorkflowEditStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJudgeIdentity {
    /// Stable position within this three-judge analysis, without retaining endpoint URLs.
    pub slot: u8,
    pub provider: String,
    pub model_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowVerdict {
    Pass,
    Fail,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIssue {
    pub category: String,
    pub excerpt: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJudgement {
    pub id: String,
    pub analysis_id: String,
    pub run_id: String,
    pub reference_kind: WorkflowReferenceKind,
    pub reference_updated_at: String,
    pub judge: WorkflowJudgeIdentity,
    pub verdict: WorkflowVerdict,
    pub issues: Vec<WorkflowIssue>,
    pub explanation: String,
    pub created_at: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowWordError {
    pub edits: usize,
    pub reference_words: usize,
    pub hypothesis_words: usize,
    pub rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRunScore {
    pub run_id: String,
    pub word_error: Option<WorkflowWordError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowEntry {
    pub history_id: String,
    pub runs: Vec<WorkflowRun>,
    pub feedback: Vec<WorkflowFeedback>,
    pub judgements: Vec<WorkflowJudgement>,
    pub scores: Vec<WorkflowRunScore>,
    #[serde(default)]
    pub edit_observation: Option<WorkflowEditObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEventCount {
    pub category: String,
    pub count: u64,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn bounded_text(text: &str, limit: usize, allow_empty: bool) -> Result<(), String> {
    if text.len() > limit || (!allow_empty && text.trim().is_empty()) || text.contains('\0') {
        Err("Workflow evaluation text is empty or exceeds its size limit".into())
    } else {
        Ok(())
    }
}

fn kind_text(kind: WorkflowRunKind) -> &'static str {
    match kind {
        WorkflowRunKind::Recording => "recording",
        WorkflowRunKind::Comparison => "comparison",
    }
}
fn reference_text(kind: WorkflowReferenceKind) -> &'static str {
    match kind {
        WorkflowReferenceKind::Spoken => "spoken",
        WorkflowReferenceKind::Output => "output",
    }
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS workflow_state (id INTEGER PRIMARY KEY CHECK(id = 1), epoch INTEGER NOT NULL);
        INSERT OR IGNORE INTO workflow_state VALUES (1, 0);
        CREATE TABLE IF NOT EXISTS workflow_runs (
            id TEXT PRIMARY KEY, history_id TEXT NOT NULL REFERENCES history(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK(kind IN ('recording','comparison')), created_at TEXT NOT NULL, data TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS workflow_run_history ON workflow_runs(history_id, created_at, id);
        CREATE UNIQUE INDEX IF NOT EXISTS workflow_original ON workflow_runs(history_id) WHERE kind = 'recording';
        CREATE TABLE IF NOT EXISTS workflow_feedback (
            history_id TEXT NOT NULL REFERENCES history(id) ON DELETE CASCADE,
            reference_kind TEXT NOT NULL CHECK(reference_kind IN ('spoken','output')), updated_at TEXT NOT NULL, data TEXT NOT NULL,
            PRIMARY KEY(history_id, reference_kind));
        CREATE TABLE IF NOT EXISTS workflow_edit_observations (
            history_id TEXT PRIMARY KEY REFERENCES history(id) ON DELETE CASCADE,
            observed_at TEXT NOT NULL, data TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS workflow_judgements (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
            reference_kind TEXT NOT NULL, created_at TEXT NOT NULL, data TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS workflow_judgement_run ON workflow_judgements(run_id);
        CREATE TABLE IF NOT EXISTS workflow_events (category TEXT PRIMARY KEY, count INTEGER NOT NULL);"
    )
}

fn require_history(conn: &Connection, history_id: &str) -> Result<(), String> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM history WHERE id = ?1",
            [history_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match kind.as_deref() {
        Some("dictation") => Ok(()),
        Some(_) => Err("Workflow Insights supports dictations only".into()),
        None => Err("The recording was deleted or is unavailable".into()),
    }
}

fn require_epoch(conn: &Connection, epoch: u64) -> Result<(), String> {
    let current: u64 = conn
        .query_row("SELECT epoch FROM workflow_state WHERE id = 1", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    if current == epoch {
        Ok(())
    } else {
        Err("Workflow evaluation was cleared while this job was running".into())
    }
}

fn read_json_rows<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    history_id: &str,
) -> Result<Vec<T>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([history_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    })
    .collect()
}

fn read_entry(conn: &Connection, history_id: &str) -> Result<WorkflowEntry, String> {
    require_history(conn, history_id)?;
    let runs: Vec<WorkflowRun> = read_json_rows(
        conn,
        &format!("SELECT data FROM workflow_runs WHERE history_id = ?1 AND (kind = 'recording' OR id IN(SELECT id FROM workflow_runs WHERE history_id = ?1 AND kind = 'comparison' ORDER BY created_at DESC, id DESC LIMIT {MAX_COMPARISONS})) ORDER BY created_at, id"),
        history_id,
    )?;
    let feedback: Vec<WorkflowFeedback> = read_json_rows(
        conn,
        "SELECT data FROM workflow_feedback WHERE history_id = ?1 ORDER BY reference_kind",
        history_id,
    )?;
    let judgements = read_json_rows(conn, &format!("SELECT data FROM(SELECT j.data, j.created_at, j.id FROM workflow_judgements j JOIN workflow_runs r ON r.id = j.run_id WHERE r.history_id = ?1 AND (r.kind = 'recording' OR r.id IN(SELECT id FROM workflow_runs WHERE history_id = ?1 AND kind = 'comparison' ORDER BY created_at DESC, id DESC LIMIT {MAX_COMPARISONS})) ORDER BY j.created_at DESC, j.id DESC LIMIT 54) ORDER BY created_at, id"), history_id)?;
    Ok(WorkflowEntry {
        history_id: history_id.into(),
        runs,
        feedback,
        judgements,
        scores: Vec::new(),
        edit_observation: read_edit_observation(conn, history_id)?,
    })
}

fn read_edit_observation(
    conn: &Connection,
    history_id: &str,
) -> Result<Option<WorkflowEditObservation>, String> {
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM workflow_edit_observations WHERE history_id = ?1",
            [history_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    data.map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
        .transpose()
}

fn write_edit_observation(
    conn: &Connection,
    observation: &WorkflowEditObservation,
) -> Result<(), String> {
    let data = serde_json::to_string(observation).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO workflow_edit_observations(history_id,observed_at,data) VALUES(?1,?2,?3) ON CONFLICT(history_id) DO UPDATE SET observed_at=excluded.observed_at,data=excluded.data", params![observation.history_id, observation.observed_at, data]).map_err(|e| e.to_string())?;
    Ok(())
}

fn pending_edit(
    conn: &Connection,
    history_id: &str,
    expected_observed_at: &str,
) -> Result<WorkflowEditObservation, String> {
    require_history(conn, history_id)?;
    let observation = read_edit_observation(conn, history_id)?
        .ok_or("The suggested edit is no longer available")?;
    if observation.status != WorkflowEditStatus::Pending
        || observation.observed_at != expected_observed_at
    {
        return Err(
            "The suggested edit changed or was already reviewed. Reopen it before continuing."
                .into(),
        );
    }
    Ok(observation)
}

fn write_feedback(
    conn: &Connection,
    history_id: &str,
    feedback: &WorkflowFeedback,
) -> Result<(), String> {
    let mut feedback = feedback.clone();
    feedback.updated_at = now();
    let data = serde_json::to_string(&feedback).map_err(|e| e.to_string())?;
    let kind = reference_text(feedback.reference_kind);
    conn.execute("DELETE FROM workflow_judgements WHERE reference_kind = ?2 AND run_id IN(SELECT id FROM workflow_runs WHERE history_id = ?1)", params![history_id, kind]).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO workflow_feedback(history_id,reference_kind,updated_at,data) VALUES(?1,?2,?3,?4) ON CONFLICT(history_id,reference_kind) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data", params![history_id, kind, feedback.updated_at, data]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Bounded word edit work runs after releasing the connection used by normal dictation.
fn score_entry(mut entry: WorkflowEntry) -> WorkflowEntry {
    let spoken = entry
        .feedback
        .iter()
        .find(|f| f.reference_kind == WorkflowReferenceKind::Spoken);
    entry.scores = entry
        .runs
        .iter()
        .map(|run| WorkflowRunScore {
            run_id: run.id.clone(),
            word_error: if (run.raw_text.trim().is_empty() && run.outcome != "no_speech")
                || !matches!(
                    run.outcome.as_str(),
                    "completed"
                        | "formatting_fallback"
                        | "paste_failed"
                        | "clipboard_only"
                        | "snippet"
                        | "no_speech"
                ) {
                None
            } else {
                spoken.and_then(|f| word_error_score(&f.reference_text, &run.raw_text))
            },
        })
        .collect();
    entry
}

impl Database {
    pub fn workflow_epoch(&self) -> Result<u64, String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row("SELECT epoch FROM workflow_state WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())
    }

    pub fn dictionary_revision(&self) -> Result<String, String> {
        Ok(dictionary_revision(&self.get_dictionary()?))
    }

    /// Invalidates captures and analysis jobs on opt-out, preserving already saved evidence.
    pub fn workflow_invalidate_writers(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE workflow_state SET epoch = epoch + 1 WHERE id = 1",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn workflow_append_run(
        &self,
        run: &WorkflowRun,
        expected_epoch: u64,
    ) -> Result<(), String> {
        bounded_text(&run.id, 128, false)?;
        bounded_text(&run.history_id, 128, false)?;
        bounded_text(&run.label, 256, false)?;
        bounded_text(&run.outcome, 64, false)?;
        for text in [&run.raw_text, &run.formatted_text, &run.final_text] {
            bounded_text(text, MAX_TEXT_BYTES, true)?;
        }
        if run
            .timings
            .clipping_fraction
            .is_some_and(|f| !f.is_finite() || !(0.0..=1.0).contains(&f))
        {
            return Err("Invalid clipping fraction".into());
        }
        let data = serde_json::to_string(run).map_err(|e| e.to_string())?;
        bounded_text(&data, MAX_TEXT_BYTES * 4, false)?;
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        require_epoch(&tx, expected_epoch)?;
        require_history(&tx, &run.history_id)?;
        tx.execute("INSERT INTO workflow_runs (id, history_id, kind, created_at, data) VALUES (?1,?2,?3,?4,?5)", params![run.id, run.history_id, kind_text(run.kind), run.created_at, data]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM workflow_runs WHERE history_id = ?1 AND kind = 'comparison' AND id NOT IN(SELECT id FROM workflow_runs WHERE history_id = ?1 AND kind = 'comparison' ORDER BY created_at DESC, id DESC LIMIT ?2)", params![run.history_id, MAX_COMPARISONS as i64]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn workflow_entry(&self, history_id: &str) -> Result<WorkflowEntry, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let entry = read_entry(&conn, history_id)?;
        drop(conn);
        Ok(score_entry(entry))
    }

    pub fn workflow_list(&self, limit: usize) -> Result<Vec<WorkflowEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("SELECT h.id FROM history h WHERE EXISTS(SELECT 1 FROM workflow_runs r WHERE r.history_id = h.id) OR EXISTS(SELECT 1 FROM workflow_feedback f WHERE f.history_id = h.id) OR EXISTS(SELECT 1 FROM workflow_edit_observations e WHERE e.history_id = h.id) ORDER BY h.created_at DESC, h.id DESC LIMIT ?1").map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([limit.min(200) as i64], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let entries = ids
            .iter()
            .map(|id| read_entry(&conn, id))
            .collect::<Result<Vec<_>, String>>()?;
        drop(stmt);
        drop(conn);
        Ok(entries.into_iter().map(score_entry).collect())
    }

    /// Replaces only the selected confirmed reference. All old verdicts against it become invalid.
    pub fn workflow_set_feedback(
        &self,
        history_id: &str,
        feedback: &WorkflowFeedback,
    ) -> Result<WorkflowEntry, String> {
        bounded_text(&feedback.reference_text, MAX_TEXT_BYTES, false)?;
        bounded_text(&feedback.category, 64, false)?;
        if !matches!(
            feedback.category.as_str(),
            "correct" | "transcription" | "formatting" | "names" | "numbers" | "meaning" | "other"
        ) {
            return Err("Unknown workflow feedback category".into());
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        require_history(&tx, history_id)?;
        write_feedback(&tx, history_id, feedback)?;
        tx.commit().map_err(|e| e.to_string())?;
        let entry = read_entry(&conn, history_id)?;
        drop(conn);
        Ok(score_entry(entry))
    }

    /// Collection callers hold the current opt-in gate while making this bounded write.
    pub fn workflow_save_edit_observation(
        &self,
        history_id: &str,
        original: &str,
        edited: &str,
        app_bundle_id: Option<&str>,
        expected_epoch: u64,
    ) -> Result<WorkflowEntry, String> {
        bounded_text(history_id, 128, false)?;
        for text in [original, edited] {
            bounded_text(text, MAX_TEXT_BYTES, false)?;
            if text.encode_utf16().count() > MAX_EDIT_UTF16 {
                return Err("Suggested edit exceeds its text size limit".into());
            }
        }
        if let Some(app) = app_bundle_id {
            bounded_text(app, 256, false)?;
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        require_epoch(&tx, expected_epoch)?;
        require_history(&tx, history_id)?;
        if let Some(previous) = read_edit_observation(&tx, history_id)? {
            if previous.status != WorkflowEditStatus::Pending || previous.original_text != original
            {
                return Err(
                    "The suggested edit was already reviewed or belongs to a different paste"
                        .into(),
                );
            }
        }
        if original == edited {
            tx.execute(
                "DELETE FROM workflow_edit_observations WHERE history_id = ?1",
                [history_id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            write_edit_observation(
                &tx,
                &WorkflowEditObservation {
                    history_id: history_id.into(),
                    original_text: original.into(),
                    edited_text: edited.into(),
                    app_bundle_id: app_bundle_id.map(str::to_owned),
                    observed_at: now(),
                    status: WorkflowEditStatus::Pending,
                },
            )?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        let entry = read_entry(&conn, history_id)?;
        drop(conn);
        Ok(score_entry(entry))
    }

    /// Explicit confirmation updates desired output only; spoken accuracy is never inferred.
    pub fn workflow_confirm_edit(
        &self,
        history_id: &str,
        expected_observed_at: &str,
    ) -> Result<WorkflowEntry, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut observation = pending_edit(&tx, history_id, expected_observed_at)?;
        write_feedback(
            &tx,
            history_id,
            &WorkflowFeedback {
                reference_kind: WorkflowReferenceKind::Output,
                reference_text: observation.edited_text.clone(),
                category: "other".into(),
                updated_at: String::new(),
            },
        )?;
        observation.status = WorkflowEditStatus::Confirmed;
        write_edit_observation(&tx, &observation)?;
        tx.commit().map_err(|e| e.to_string())?;
        let entry = read_entry(&conn, history_id)?;
        drop(conn);
        Ok(score_entry(entry))
    }

    pub fn workflow_dismiss_edit(
        &self,
        history_id: &str,
        expected_observed_at: &str,
    ) -> Result<WorkflowEntry, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut observation = pending_edit(&tx, history_id, expected_observed_at)?;
        observation.status = WorkflowEditStatus::Dismissed;
        write_edit_observation(&tx, &observation)?;
        tx.commit().map_err(|e| e.to_string())?;
        let entry = read_entry(&conn, history_id)?;
        drop(conn);
        Ok(score_entry(entry))
    }

    pub fn workflow_append_judgements(
        &self,
        history_id: &str,
        kind: WorkflowReferenceKind,
        reference_updated_at: &str,
        judgements: &[WorkflowJudgement],
        expected_epoch: u64,
    ) -> Result<(), String> {
        if judgements.is_empty() || judgements.len() > 9 {
            return Err("Invalid number of judge assessments".into());
        }
        // Only a complete new jury can replace an old one. All three slots for each run
        // belong to one analysis, so repeated actions cannot mix juries or grow rows.
        let analysis_id = &judgements[0].analysis_id;
        let mut slots: HashMap<&str, HashSet<u8>> = HashMap::new();
        for judgement in judgements {
            if judgement.analysis_id != *analysis_id
                || !slots
                    .entry(&judgement.run_id)
                    .or_default()
                    .insert(judgement.judge.slot)
            {
                return Err("Judge assessments must belong to one complete jury".into());
            }
        }
        if slots
            .values()
            .any(|slots| slots != &HashSet::from([1, 2, 3]))
        {
            return Err("Each candidate requires all three judge slots".into());
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        require_epoch(&tx, expected_epoch)?;
        require_history(&tx, history_id)?;
        let current: Option<String> = tx.query_row("SELECT updated_at FROM workflow_feedback WHERE history_id = ?1 AND reference_kind = ?2", params![history_id, reference_text(kind)], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        if current.as_deref() != Some(reference_updated_at) {
            return Err("The confirmed reference changed while judging".into());
        }
        for run_id in slots.keys() {
            tx.execute(
                "DELETE FROM workflow_judgements WHERE run_id = ?1 AND reference_kind = ?2",
                params![run_id, reference_text(kind)],
            )
            .map_err(|e| e.to_string())?;
        }
        for judgement in judgements {
            if judgement.reference_kind != kind
                || judgement.reference_updated_at != reference_updated_at
            {
                return Err("Judge reference does not match".into());
            }
            let belongs: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE id = ?1 AND history_id = ?2)",
                    params![judgement.run_id, history_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !belongs {
                return Err("Judge candidate is unavailable".into());
            }
            bounded_text(&judgement.id, 128, false)?;
            bounded_text(&judgement.analysis_id, 128, false)?;
            bounded_text(&judgement.judge.provider, 64, false)?;
            bounded_text(&judgement.judge.model_name, 256, false)?;
            if !(1..=3).contains(&judgement.judge.slot) {
                return Err("Invalid workflow judge slot".into());
            }
            bounded_text(&judgement.explanation, 8_192, true)?;
            if judgement.issues.len() > 20 {
                return Err("Too many judge issues".into());
            }
            for issue in &judgement.issues {
                bounded_text(&issue.category, 64, false)?;
                bounded_text(&issue.excerpt, 4_096, true)?;
                bounded_text(&issue.explanation, 4_096, false)?;
            }
            let data = serde_json::to_string(judgement).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO workflow_judgements(id,run_id,reference_kind,created_at,data) VALUES(?1,?2,?3,?4,?5)", params![judgement.id, judgement.run_id, reference_text(kind), judgement.created_at, data]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn workflow_record_event(&self, category: &str) -> Result<(), String> {
        if !matches!(
            category,
            "no_speech" | "empty_stt" | "capture_failed" | "stt_failed" | "cancelled"
        ) {
            return Err("Unknown workflow event category".into());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO workflow_events(category,count) VALUES(?1,1) ON CONFLICT(category) DO UPDATE SET count = MIN(count + 1, 1000000000)", [category]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn workflow_events(&self) -> Result<Vec<WorkflowEventCount>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT category,count FROM workflow_events ORDER BY category")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkflowEventCount {
                    category: r.get(0)?,
                    count: r.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    pub fn workflow_clear(&self) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute_batch("UPDATE workflow_state SET epoch = epoch + 1 WHERE id = 1; DELETE FROM workflow_edit_observations; DELETE FROM workflow_judgements; DELETE FROM workflow_feedback; DELETE FROM workflow_runs; DELETE FROM workflow_events;").map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}

pub fn dictionary_revision(entries: &[DictionaryEntry]) -> String {
    let mut sorted = entries
        .iter()
        .map(|e| (&e.phrase, &e.replacement))
        .collect::<Vec<_>>();
    sorted.sort_unstable();
    let mut hash = Sha256::new();
    for (phrase, replacement) in sorted {
        hash.update((phrase.len() as u64).to_le_bytes());
        hash.update(phrase.as_bytes());
        hash.update((replacement.len() as u64).to_le_bytes());
        hash.update(replacement.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// Case-insensitive Unicode word edits, ignoring punctuation. CJK characters are separate
/// tokens. This measures transcription against confirmed speech, never desired formatting.
/// Long inputs are unscored instead of allocating unbounded quadratic work.
pub fn word_error_score(reference: &str, hypothesis: &str) -> Option<WorkflowWordError> {
    fn cjk(c: char) -> bool {
        matches!(c as u32, 0x3400..=0x9fff | 0xf900..=0xfaff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    }
    fn tokens(text: &str) -> Option<Vec<String>> {
        if text.len() > MAX_TEXT_BYTES {
            return None;
        }
        let mut words = Vec::new();
        let mut word = String::new();
        for c in text.chars().flat_map(char::to_lowercase) {
            if cjk(c) {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                words.push(c.to_string());
            } else if c.is_alphanumeric()
                || matches!(c as u32, 0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff)
            {
                word.push(c);
            } else if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            if words.len() > MAX_WORDS {
                return None;
            }
        }
        if !word.is_empty() {
            words.push(word);
        }
        (words.len() <= MAX_WORDS).then_some(words)
    }
    let reference = tokens(reference)?;
    let hypothesis = tokens(hypothesis)?;
    if reference.is_empty() || reference.len().checked_mul(hypothesis.len())? > MAX_EDIT_CELLS {
        return None;
    }
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0; hypothesis.len() + 1];
    for (i, reference_word) in reference.iter().enumerate() {
        current[0] = i + 1;
        for (j, hypothesis_word) in hypothesis.iter().enumerate() {
            current[j + 1] = (previous[j] + usize::from(reference_word != hypothesis_word))
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let edits = previous[hypothesis.len()];
    Some(WorkflowWordError {
        edits,
        reference_words: reference.len(),
        hypothesis_words: hypothesis.len(),
        rate: edits as f64 / reference.len() as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::HistoryEntry;

    fn database() -> Database {
        Database::new(
            std::env::temp_dir().join(format!("openglaido-workflow-{}", uuid::Uuid::new_v4())),
        )
        .unwrap()
    }

    fn recording(history_id: &str) -> WorkflowRun {
        let mut run = WorkflowRun::new(
            history_id,
            WorkflowRunKind::Recording,
            "Original recording",
            WorkflowSnapshot::from_config(
                &TranscriptionConfig::default(),
                None,
                "dictionary-v1".into(),
            ),
        );
        run.raw_text = "wrong name".into();
        run.formatted_text = "Wrong name.".into();
        run.final_text = "Wrong name.".into();
        run
    }

    fn feedback(kind: WorkflowReferenceKind, text: &str) -> WorkflowFeedback {
        WorkflowFeedback {
            reference_kind: kind,
            reference_text: text.into(),
            category: "names".into(),
            updated_at: "client-supplied-revision".into(),
        }
    }

    fn judgement(run: &WorkflowRun, feedback: &WorkflowFeedback) -> WorkflowJudgement {
        WorkflowJudgement {
            id: uuid::Uuid::new_v4().to_string(),
            analysis_id: uuid::Uuid::new_v4().to_string(),
            run_id: run.id.clone(),
            reference_kind: feedback.reference_kind,
            reference_updated_at: feedback.updated_at.clone(),
            judge: WorkflowJudgeIdentity {
                slot: 1,
                provider: "test".into(),
                model_name: "judge".into(),
            },
            verdict: WorkflowVerdict::Fail,
            issues: vec![WorkflowIssue {
                category: "names".into(),
                excerpt: "wrong".into(),
                explanation: "Name differs from reference".into(),
            }],
            explanation: "One incorrect name".into(),
            created_at: now(),
            elapsed_ms: 42,
        }
    }

    fn jury(run: &WorkflowRun, feedback: &WorkflowFeedback) -> Vec<WorkflowJudgement> {
        let analysis_id = uuid::Uuid::new_v4().to_string();
        (1..=3)
            .map(|slot| {
                let mut judge = judgement(run, feedback);
                judge.analysis_id = analysis_id.clone();
                judge.judge.slot = slot;
                judge
            })
            .collect()
    }

    fn clean_up(db: Database) {
        let path = db.app_dir.clone();
        drop(db);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn defaults_are_off_and_settings_reject_unsafe_endpoints() {
        let settings: WorkflowSettings = serde_json::from_str("{}").unwrap();
        assert!(
            !settings.enabled
                && !settings.cloud_analysis
                && !settings.track_edits
                && settings.judges.is_empty()
        );
        assert_eq!(WorkflowSettings::default(), settings);
        let judge = |endpoint: &str| WorkflowJudgeConfig {
            provider: "test".into(),
            endpoint_url: endpoint.into(),
            model_name: "judge".into(),
        };
        for endpoint in [
            "https://example.test/v1/chat/completions",
            "http://localhost:11434/v1/chat/completions",
            "http://127.0.0.1:1234/v1/chat/completions",
            "http://[::1]:11434/v1/chat/completions",
        ] {
            WorkflowSettings {
                judges: vec![judge(endpoint)],
                ..settings.clone()
            }
            .validate()
            .unwrap();
        }
        for endpoint in [
            "http://example.test/v1",
            "file:///private/key",
            "https://user:key@example.test/v1",
            "https://example.test/v1?key=secret",
            "https://example.test/v1#secret",
            "not a url",
        ] {
            assert!(
                WorkflowSettings {
                    judges: vec![judge(endpoint)],
                    ..settings.clone()
                }
                .validate()
                .is_err(),
                "{endpoint}"
            );
        }
        assert!(WorkflowSettings {
            judges: vec![judge("https://example.test"); 4],
            ..settings
        }
        .validate()
        .is_err());
        assert!(serde_json::from_str::<WorkflowJudgeConfig>(r#"{"provider":"test","endpoint_url":"https://example.test","model_name":"judge","api_key":"secret"}"#).is_err());
    }

    #[test]
    fn textbox_suggestions_are_pending_not_accuracy_and_latest_revision_requires_review() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        let first = db
            .workflow_save_edit_observation(
                "a",
                "Wrong name.",
                "Right name.",
                Some("com.editor"),
                epoch,
            )
            .unwrap();
        assert!(first.feedback.is_empty() && first.judgements.is_empty());
        assert!(first.scores[0].word_error.is_none());
        assert_eq!(
            first.edit_observation.as_ref().unwrap().status,
            WorkflowEditStatus::Pending
        );
        let version = first.edit_observation.unwrap().observed_at;
        let latest = db
            .workflow_save_edit_observation(
                "a",
                "Wrong name.",
                "Right name!",
                Some("com.editor"),
                epoch,
            )
            .unwrap();
        let latest = latest.edit_observation.unwrap();
        assert_ne!(latest.observed_at, version);
        assert_eq!(latest.edited_text, "Right name!");
        assert!(db.workflow_confirm_edit("a", &version).is_err());
        assert!(db.workflow_dismiss_edit("a", &version).is_err());
        assert_eq!(db.workflow_entry("a").unwrap().runs[0], run);
        assert!(db.workflow_list(20).unwrap()[0].edit_observation.is_some());
        clean_up(db);
    }

    #[test]
    fn confirming_textbox_edit_updates_only_output_and_invalidates_only_output_jury() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        db.workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let entry = db
            .workflow_set_feedback(
                "a",
                &feedback(WorkflowReferenceKind::Output, "Existing desired output"),
            )
            .unwrap();
        for reference in &entry.feedback {
            db.workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &jury(&run, reference),
                epoch,
            )
            .unwrap();
        }
        let entry = db
            .workflow_save_edit_observation("a", "Wrong name.", "Right name!", None, epoch)
            .unwrap();
        // Collection alone does not replace an already confirmed desired output.
        assert!(entry
            .feedback
            .iter()
            .any(|f| f.reference_text == "Existing desired output"));
        let version = entry.edit_observation.unwrap().observed_at;
        let confirmed = db.workflow_confirm_edit("a", &version).unwrap();
        assert_eq!(
            confirmed.edit_observation.as_ref().unwrap().status,
            WorkflowEditStatus::Confirmed
        );
        let output = confirmed
            .feedback
            .iter()
            .find(|f| f.reference_kind == WorkflowReferenceKind::Output)
            .unwrap();
        assert_eq!(output.reference_text, "Right name!");
        assert_eq!(output.category, "other");
        assert!(confirmed
            .feedback
            .iter()
            .any(|f| f.reference_kind == WorkflowReferenceKind::Spoken
                && f.reference_text == "right name"));
        assert_eq!(confirmed.judgements.len(), 3);
        assert!(confirmed
            .judgements
            .iter()
            .all(|j| j.reference_kind == WorkflowReferenceKind::Spoken));
        assert_eq!(confirmed.runs[0], run);
        assert!(db
            .workflow_save_edit_observation("a", "Wrong name.", "Late edit", None, epoch)
            .is_err());
        assert!(db
            .workflow_save_edit_observation("a", "Wrong name.", "Wrong name.", None, epoch)
            .is_err());
        assert!(db.workflow_confirm_edit("a", &version).is_err());
        clean_up(db);
    }

    #[test]
    fn dismiss_and_undo_do_not_create_references_or_accept_late_writers() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let first = db
            .workflow_save_edit_observation("a", "original", "edited", None, epoch)
            .unwrap();
        assert!(db
            .workflow_save_edit_observation("a", "another paste", "edited", None, epoch)
            .is_err());
        let reverted = db
            .workflow_save_edit_observation("a", "original", "original", None, epoch)
            .unwrap();
        assert!(reverted.edit_observation.is_none() && reverted.feedback.is_empty());
        assert!(db
            .workflow_confirm_edit("a", &first.edit_observation.unwrap().observed_at)
            .is_err());
        let entry = db
            .workflow_save_edit_observation("a", "original", "edited again", None, epoch)
            .unwrap();
        let version = entry.edit_observation.unwrap().observed_at;
        let dismissed = db.workflow_dismiss_edit("a", &version).unwrap();
        assert_eq!(
            dismissed.edit_observation.unwrap().status,
            WorkflowEditStatus::Dismissed
        );
        assert!(dismissed.feedback.is_empty());
        assert!(db
            .workflow_save_edit_observation("a", "original", "late edit", None, epoch)
            .is_err());
        assert!(db.workflow_confirm_edit("a", &version).is_err());
        clean_up(db);
    }

    #[test]
    fn textbox_suggestions_are_bounded_and_clear_deletion_optout_invalidate_writers() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        for edited in [
            String::new(),
            "\0".into(),
            "a".repeat(MAX_EDIT_UTF16 + 1),
            "💬".repeat(MAX_EDIT_UTF16 / 2 + 1),
            "💬".repeat(MAX_TEXT_BYTES),
        ] {
            assert!(db
                .workflow_save_edit_observation("a", "original", &edited, None, epoch)
                .is_err());
        }
        assert!(db
            .workflow_save_edit_observation("missing", "original", "edited", None, epoch)
            .is_err());
        db.insert_history(&HistoryEntry {
            kind: "command".into(),
            ..HistoryEntry::new("command")
        })
        .unwrap();
        assert!(db
            .workflow_save_edit_observation("command", "original", "edited", None, epoch)
            .is_err());
        let entry = db
            .workflow_save_edit_observation(
                "a",
                "original",
                "a".repeat(MAX_EDIT_UTF16).as_str(),
                None,
                epoch,
            )
            .unwrap();
        let version = entry.edit_observation.unwrap().observed_at;
        db.workflow_invalidate_writers().unwrap();
        assert!(db
            .workflow_save_edit_observation("a", "original", "late edit", None, epoch)
            .is_err());
        assert!(db.workflow_entry("a").unwrap().edit_observation.is_some());
        db.workflow_clear().unwrap();
        assert!(db.workflow_entry("a").unwrap().edit_observation.is_none());
        assert!(db.workflow_confirm_edit("a", &version).is_err());
        assert!(db
            .workflow_save_edit_observation("a", "original", "late edit", None, epoch)
            .is_err());
        let current = db.workflow_epoch().unwrap();
        db.workflow_save_edit_observation("a", "original", "edited", None, current)
            .unwrap();
        db.delete_history_entry("a").unwrap();
        let count: usize = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM workflow_edit_observations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        assert!(db
            .workflow_save_edit_observation("a", "original", "late edit", None, current)
            .is_err());
        clean_up(db);
    }

    #[test]
    fn snapshot_contains_allowlisted_settings_but_no_credentials_or_url_paths() {
        let mut config = TranscriptionConfig {
            api_key: "stt-secret".into(),
            llm_api_key: "llm-secret".into(),
            search_api_key: "search-secret".into(),
            endpoint_url: "https://user:password@example.test/private?key=url-secret".into(),
            ..TranscriptionConfig::default()
        };
        config.stt_backup_1_enabled = true;
        config.stt_backup_1_api_key = "backup-secret".into();
        config.model_name = "gpt-live-transcribe".into();
        config.languages = vec!["sl".into()];
        let context = AppContext {
            name: "Mail".into(),
            bundle_id: "com.apple.mail".into(),
            url: Some(
                "https://user:password@mail.google.com/private/customer?key=browser-secret".into(),
            ),
        };
        let snapshot =
            WorkflowSnapshot::from_config(&config, Some(&context), "dictionary-v1".into());
        assert!(!snapshot.dictionary_changed);
        let mut legacy = serde_json::to_value(&snapshot).unwrap();
        legacy.as_object_mut().unwrap().remove("dictionary_changed");
        assert!(
            !serde_json::from_value::<WorkflowSnapshot>(legacy)
                .unwrap()
                .dictionary_changed
        );
        assert_eq!(snapshot.rule_id.as_deref(), Some("email"));
        assert_eq!(snapshot.website_host.as_deref(), Some("mail.google.com"));
        assert_eq!(snapshot.languages, ["sl"]);
        assert!(snapshot.live);
        assert_eq!(snapshot.configured_backups.len(), 1);
        assert!(snapshot.winning_provider.is_none());
        let json = serde_json::to_string(&snapshot).unwrap();
        for forbidden in [
            "stt-secret",
            "llm-secret",
            "search-secret",
            "backup-secret",
            "url-secret",
            "browser-secret",
            "password",
            "customer",
            "api_key",
            "endpoint_url",
        ] {
            assert!(!json.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn migration_preserves_existing_history_and_runs_are_immutable() {
        let db = database();
        let path = db.app_dir.clone();
        db.insert_history(&HistoryEntry {
            text: "original".into(),
            ..HistoryEntry::new("a")
        })
        .unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        let mut changed = run.clone();
        changed.raw_text = "overwrite".into();
        assert!(db.workflow_append_run(&changed, epoch).is_err());
        assert!(db.workflow_append_run(&recording("a"), epoch).is_err());
        assert!(db
            .workflow_append_run(&recording("missing"), epoch)
            .is_err());
        db.insert_history(&HistoryEntry {
            kind: "command".into(),
            ..HistoryEntry::new("command")
        })
        .unwrap();
        assert!(db
            .workflow_append_run(&recording("command"), epoch)
            .is_err());
        drop(db);
        let db = Database::new(path).unwrap();
        assert_eq!(db.get_history_entry("a").unwrap().text, "original");
        assert_eq!(db.workflow_entry("a").unwrap().runs, [run]);
        assert_eq!(db.workflow_list(500).unwrap().len(), 1);
        assert!(db.workflow_list(0).unwrap().is_empty());
        clean_up(db);
    }

    #[test]
    fn confirmed_reference_kinds_are_isolated_and_changed_references_invalidate_judges() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        let entry = db
            .workflow_set_feedback(
                "a",
                &feedback(WorkflowReferenceKind::Output, "Preferred formatting"),
            )
            .unwrap();
        assert!(entry.scores[0].word_error.is_none());
        let output = entry.feedback[0].clone();
        let output_judgement = jury(&run, &output);
        db.workflow_append_judgements(
            "a",
            output.reference_kind,
            &output.updated_at,
            &output_judgement,
            epoch,
        )
        .unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        assert_eq!(entry.feedback.len(), 2);
        assert_eq!(entry.judgements, output_judgement);
        assert_eq!(entry.scores[0].word_error.as_ref().unwrap().edits, 1);
        let spoken = entry
            .feedback
            .iter()
            .find(|f| f.reference_kind == WorkflowReferenceKind::Spoken)
            .unwrap();
        assert_ne!(spoken.updated_at, "client-supplied-revision");
        let spoken_judgement = jury(&run, spoken);
        db.workflow_append_judgements(
            "a",
            spoken.reference_kind,
            &spoken.updated_at,
            &spoken_judgement,
            epoch,
        )
        .unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "wrong name"))
            .unwrap();
        assert_eq!(entry.scores[0].word_error.as_ref().unwrap().edits, 0);
        assert_eq!(entry.judgements, output_judgement);
        assert!(db
            .workflow_append_judgements(
                "a",
                spoken.reference_kind,
                &spoken.updated_at,
                &spoken_judgement,
                epoch
            )
            .is_err());
        assert!(db
            .workflow_set_feedback("missing", &feedback(WorkflowReferenceKind::Spoken, "text"))
            .is_err());
        assert!(db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, " "))
            .is_err());
        assert_eq!(db.workflow_entry("a").unwrap().runs[0], run);
        clean_up(db);
    }

    #[test]
    fn judgement_batches_are_atomic_and_cannot_reference_another_recording() {
        let db = database();
        let epoch = db.workflow_epoch().unwrap();
        for id in ["a", "b"] {
            db.insert_history(&HistoryEntry::new(id)).unwrap();
        }
        let run = recording("a");
        let other = recording("b");
        db.workflow_append_run(&run, epoch).unwrap();
        db.workflow_append_run(&other, epoch).unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let reference = &entry.feedback[0];
        let mut invalid = jury(&run, reference);
        let other_jury = jury(&other, reference);
        let analysis_id = invalid[0].analysis_id.clone();
        invalid.extend(other_jury.into_iter().map(|mut judge| {
            judge.analysis_id = analysis_id.clone();
            judge
        }));
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &invalid,
                epoch
            )
            .is_err());
        assert!(db.workflow_entry("a").unwrap().judgements.is_empty());
        let mut invalid_slot = judgement(&run, reference);
        invalid_slot.judge.slot = 0;
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                std::slice::from_ref(&invalid_slot),
                epoch
            )
            .is_err());
        let analysis_id = uuid::Uuid::new_v4().to_string();
        let judges = (1..=3)
            .map(|slot| {
                let mut assessment = judgement(&run, reference);
                assessment.analysis_id = analysis_id.clone();
                assessment.judge.slot = slot;
                assessment
            })
            .collect::<Vec<_>>();
        db.workflow_append_judgements(
            "a",
            reference.reference_kind,
            &reference.updated_at,
            &judges,
            epoch,
        )
        .unwrap();
        assert_eq!(
            db.workflow_entry("a")
                .unwrap()
                .judgements
                .iter()
                .map(|j| j.judge.slot)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        clean_up(db);
    }

    #[test]
    fn history_deletion_cascades_atomically_and_blocks_late_writes() {
        let db = database();
        let epoch = db.workflow_epoch().unwrap();
        db.insert_history(&HistoryEntry {
            audio_filename: Some("a.wav".into()),
            ..HistoryEntry::new("a")
        })
        .unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let reference = &entry.feedback[0];
        let assessment = jury(&run, reference);
        db.workflow_append_judgements(
            "a",
            reference.reference_kind,
            &reference.updated_at,
            &assessment,
            epoch,
        )
        .unwrap();
        std::fs::create_dir(db.app_dir.join("audio/a.wav")).unwrap();
        assert!(db.delete_history_entry("a").is_err());
        assert_eq!(db.workflow_epoch().unwrap(), epoch);
        assert_eq!(db.workflow_entry("a").unwrap().judgements.len(), 3);
        std::fs::remove_dir(db.app_dir.join("audio/a.wav")).unwrap();
        db.delete_history_entry("a").unwrap();
        assert_eq!(db.workflow_epoch().unwrap(), epoch + 1);
        assert!(db.workflow_entry("a").is_err());
        assert!(db.workflow_list(200).unwrap().is_empty());
        for table in ["workflow_runs", "workflow_feedback", "workflow_judgements"] {
            let count: i64 = db
                .conn
                .lock()
                .unwrap()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        assert!(db.workflow_append_run(&run, epoch).is_err());
        // A legacy completion may recreate History, but cannot resurrect old telemetry.
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        assert!(db.workflow_append_run(&run, epoch).is_err());
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &assessment,
                epoch
            )
            .is_err());
        clean_up(db);
    }

    #[test]
    fn clear_keeps_history_audio_and_dictionary_but_invalidates_outstanding_jobs() {
        let db = database();
        let epoch = db.workflow_epoch().unwrap();
        db.insert_history(&HistoryEntry {
            audio_filename: Some("a.wav".into()),
            ..HistoryEntry::new("a")
        })
        .unwrap();
        std::fs::write(db.app_dir.join("audio/a.wav"), b"original audio").unwrap();
        db.add_dictionary_entry("name", "proper").unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        db.workflow_record_event("no_speech").unwrap();
        db.workflow_record_event("no_speech").unwrap();
        assert_eq!(
            db.workflow_events().unwrap(),
            [WorkflowEventCount {
                category: "no_speech".into(),
                count: 2
            }]
        );
        assert!(db.workflow_record_event("arbitrary-private-event").is_err());
        db.workflow_clear().unwrap();
        assert_eq!(db.workflow_epoch().unwrap(), epoch + 1);
        assert!(db.workflow_list(200).unwrap().is_empty());
        assert!(db.workflow_events().unwrap().is_empty());
        assert!(db.get_history_entry("a").is_ok());
        assert_eq!(
            std::fs::read(db.app_dir.join("audio/a.wav")).unwrap(),
            b"original audio"
        );
        assert_eq!(db.get_dictionary().unwrap().len(), 1);
        assert!(db.workflow_append_run(&run, epoch).is_err());
        db.workflow_append_run(&run, epoch + 1).unwrap();
        clean_up(db);
    }

    #[test]
    fn opt_out_invalidates_writers_without_erasing_evidence_and_failures_are_unscored() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let run = recording("a");
        db.workflow_append_run(&run, epoch).unwrap();
        db.workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        db.workflow_record_event("capture_failed").unwrap();
        db.workflow_invalidate_writers().unwrap();
        assert_eq!(db.workflow_epoch().unwrap(), epoch + 1);
        assert_eq!(
            db.workflow_entry("a").unwrap().runs.as_slice(),
            std::slice::from_ref(&run)
        );
        assert_eq!(db.workflow_entry("a").unwrap().feedback.len(), 1);
        assert_eq!(db.workflow_events().unwrap()[0].count, 1);
        let mut replay = run.clone();
        replay.id = uuid::Uuid::new_v4().to_string();
        replay.kind = WorkflowRunKind::Comparison;
        replay.outcome = "stt_failed".into();
        replay.raw_text.clear();
        assert!(db.workflow_append_run(&replay, epoch).is_err());
        db.workflow_append_run(&replay, epoch + 1).unwrap();
        let entry = db.workflow_entry("a").unwrap();
        assert!(entry
            .scores
            .iter()
            .find(|s| s.run_id == replay.id)
            .unwrap()
            .word_error
            .is_none());
        assert!(entry
            .scores
            .iter()
            .find(|s| s.run_id == run.id)
            .unwrap()
            .word_error
            .is_some());
        let mut snippet = run.clone();
        snippet.id = uuid::Uuid::new_v4().to_string();
        snippet.kind = WorkflowRunKind::Comparison;
        snippet.outcome = "snippet".into();
        snippet.final_text = "A much longer intentional snippet expansion".into();
        db.workflow_append_run(&snippet, epoch + 1).unwrap();
        assert_eq!(
            db.workflow_entry("a")
                .unwrap()
                .scores
                .iter()
                .find(|s| s.run_id == snippet.id)
                .unwrap()
                .word_error
                .as_ref()
                .unwrap()
                .edits,
            1
        );
        let mut clipboard = snippet.clone();
        clipboard.id = uuid::Uuid::new_v4().to_string();
        clipboard.outcome = "clipboard_only".into();
        db.workflow_append_run(&clipboard, epoch + 1).unwrap();
        assert_eq!(
            db.workflow_entry("a")
                .unwrap()
                .scores
                .iter()
                .find(|s| s.run_id == clipboard.id)
                .unwrap()
                .word_error
                .as_ref()
                .unwrap()
                .edits,
            1
        );
        clean_up(db);
    }

    #[test]
    fn no_speech_replay_scores_missing_words_but_request_failures_remain_unknown() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let mut omission = recording("a");
        omission.kind = WorkflowRunKind::Comparison;
        omission.outcome = "no_speech".into();
        omission.raw_text.clear();
        omission.formatted_text.clear();
        omission.final_text.clear();
        db.workflow_append_run(&omission, epoch).unwrap();
        assert!(db.workflow_entry("a").unwrap().scores[0]
            .word_error
            .is_none());
        db.workflow_set_feedback(
            "a",
            &feedback(WorkflowReferenceKind::Output, "Preferred output"),
        )
        .unwrap();
        assert!(db.workflow_entry("a").unwrap().scores[0]
            .word_error
            .is_none());
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let score = entry.scores[0].word_error.as_ref().unwrap();
        assert_eq!(
            (
                score.edits,
                score.reference_words,
                score.hypothesis_words,
                score.rate
            ),
            (2, 2, 0, 1.0)
        );
        for outcome in ["stt_failed", "analysis_timeout", "cancelled", "completed"] {
            let mut failed = omission.clone();
            failed.id = uuid::Uuid::new_v4().to_string();
            failed.outcome = outcome.into();
            db.workflow_append_run(&failed, epoch).unwrap();
            assert!(db
                .workflow_entry("a")
                .unwrap()
                .scores
                .iter()
                .find(|s| s.run_id == failed.id)
                .unwrap()
                .word_error
                .is_none());
        }
        clean_up(db);
    }

    #[test]
    fn bounded_comparisons_preserve_original_and_cascade_evicted_judgements() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let original = recording("a");
        db.workflow_append_run(&original, epoch).unwrap();
        db.workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Output, "Right name."))
            .unwrap();
        for reference in &entry.feedback {
            db.workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &jury(&original, reference),
                epoch,
            )
            .unwrap();
        }
        for index in 0..20 {
            let mut comparison = original.clone();
            comparison.id = format!("comparison-{index:03}");
            comparison.kind = WorkflowRunKind::Comparison;
            comparison.created_at = format!("2030-01-01T00:00:{index:02}Z");
            db.workflow_append_run(&comparison, epoch).unwrap();
            for reference in &entry.feedback {
                db.workflow_append_judgements(
                    "a",
                    reference.reference_kind,
                    &reference.updated_at,
                    &jury(&comparison, reference),
                    epoch,
                )
                .unwrap();
            }
        }
        let entry = db.workflow_entry("a").unwrap();
        assert_eq!(entry.runs.len(), MAX_COMPARISONS + 1);
        assert_eq!(entry.judgements.len(), 54);
        assert_eq!(entry.runs[0], original);
        assert_eq!(entry.runs[1].id, "comparison-012");
        assert_eq!(entry.runs.last().unwrap().id, "comparison-019");
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM workflow_runs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM workflow_judgements", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            54
        );
        drop(conn);
        clean_up(db);
    }

    #[test]
    fn repeated_juries_replace_complete_batches_without_mixing_or_losing_previous_on_failure() {
        let db = database();
        db.insert_history(&HistoryEntry::new("a")).unwrap();
        let epoch = db.workflow_epoch().unwrap();
        let original = recording("a");
        db.workflow_append_run(&original, epoch).unwrap();
        let entry = db
            .workflow_set_feedback("a", &feedback(WorkflowReferenceKind::Spoken, "right name"))
            .unwrap();
        let reference = &entry.feedback[0];
        for _ in 0..12 {
            let latest = jury(&original, reference);
            db.workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &latest,
                epoch,
            )
            .unwrap();
            assert_eq!(db.workflow_entry("a").unwrap().judgements, latest);
        }
        let previous = db.workflow_entry("a").unwrap().judgements;
        let invalid = jury(&original, reference);
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &invalid[..2],
                epoch
            )
            .is_err());
        let mut invalid = jury(&original, reference);
        invalid[2].judge.slot = 1;
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &invalid,
                epoch
            )
            .is_err());
        let mut invalid = jury(&original, reference);
        invalid[2].reference_updated_at = "stale".into();
        assert!(db
            .workflow_append_judgements(
                "a",
                reference.reference_kind,
                &reference.updated_at,
                &invalid,
                epoch
            )
            .is_err());
        assert_eq!(db.workflow_entry("a").unwrap().judgements, previous);
        clean_up(db);
    }

    #[test]
    fn word_errors_handle_unicode_edits_empty_and_bounded_inputs() {
        let score = word_error_score("Živjo, svet!", "ŽIVJO svet").unwrap();
        assert_eq!(
            (score.edits, score.reference_words, score.hypothesis_words),
            (0, 2, 2)
        );
        assert_eq!(
            word_error_score("one two three", "one four").unwrap().edits,
            2
        );
        assert_eq!(word_error_score("one", "one two three").unwrap().rate, 2.0);
        assert_eq!(word_error_score("one two", "").unwrap().edits, 2);
        assert_eq!(word_error_score("你好世界", "你好世间").unwrap().edits, 1);
        assert_eq!(
            word_error_score("Καλημέρα κόσμε", "καλημέρα κόσμε")
                .unwrap()
                .edits,
            0
        );
        assert!(word_error_score("", "text").is_none());
        assert!(word_error_score("...", "text").is_none());
        assert!(word_error_score(&"a ".repeat(MAX_WORDS + 1), "a").is_none());
        assert!(word_error_score(&"a ".repeat(1_001), &"a ".repeat(1_001)).is_none());
        assert!(word_error_score("a", &"b".repeat(MAX_TEXT_BYTES + 1)).is_none());
    }

    #[test]
    fn dictionary_revision_is_stable_without_storing_terms() {
        let a = DictionaryEntry {
            id: "a".into(),
            phrase: "secret-term".into(),
            replacement: "replacement".into(),
        };
        let b = DictionaryEntry {
            id: "b".into(),
            phrase: "other-term".into(),
            replacement: "other".into(),
        };
        assert_eq!(
            dictionary_revision(&[a.clone(), b.clone()]),
            dictionary_revision(&[b.clone(), a.clone()])
        );
        assert_ne!(
            dictionary_revision(&[a.clone(), b]),
            dictionary_revision(std::slice::from_ref(&a))
        );
        let hash = dictionary_revision(&[a]);
        assert_eq!(hash.len(), 64);
        assert!(!hash.contains("secret"));
    }
}
