//! Durable guest→host HQ track handoff queue.
//! Local files are always the source of truth. The queue survives leaving the
//! lobby so multi-hour podcasts can finish uploading later (or via cloud).

use crate::paths::app_data_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTransfer {
    pub id: String,
    pub host_session_id: String,
    pub participant_id: String,
    pub invite: String,
    pub local_session_id: String,
    pub file_names: Vec<String>,
    /// Known file sizes at enqueue time (for UI % + ETA).
    #[serde(default)]
    pub file_sizes: HashMap<String, u64>,
    /// Bytes already acknowledged per file (for resume).
    #[serde(default)]
    pub offsets: HashMap<String, u64>,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QueueFile {
    items: Vec<PendingTransfer>,
}

fn queue_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("pending_transfers.json"))
}

fn read_queue() -> QueueFile {
    let Ok(path) = queue_path() else {
        return QueueFile::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_queue(q: &QueueFile) -> Result<(), String> {
    let path = queue_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(q).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn list_pending() -> Vec<PendingTransfer> {
    read_queue().items
}

/// Insert or refresh a queue row without wiping resume offsets.
pub fn enqueue(item: PendingTransfer) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(existing) = q.items.iter_mut().find(|i| i.id == item.id) {
        // Keep progress cursors unless the new item explicitly carries newer ones.
        let mut merged_offsets = existing.offsets.clone();
        for (k, v) in item.offsets {
            let prev = merged_offsets.get(&k).copied().unwrap_or(0);
            if v >= prev {
                merged_offsets.insert(k, v);
            }
        }
        let mut merged_sizes = existing.file_sizes.clone();
        for (k, v) in item.file_sizes {
            merged_sizes.insert(k, v);
        }
        existing.host_session_id = item.host_session_id;
        existing.participant_id = item.participant_id;
        if !item.invite.trim().is_empty() {
            existing.invite = item.invite;
        }
        existing.local_session_id = item.local_session_id;
        if !item.file_names.is_empty() {
            existing.file_names = item.file_names;
        }
        existing.file_sizes = merged_sizes;
        existing.offsets = merged_offsets;
        existing.updated_at = Some(now_rfc3339());
        // Don't clear last_error here — caller may set a fresh one after a failed try.
        if item.last_error.is_some() {
            existing.last_error = item.last_error;
        }
    } else {
        let mut item = item;
        if item.updated_at.is_none() {
            item.updated_at = Some(item.created_at.clone());
        }
        q.items.push(item);
    }
    write_queue(&q)
}

pub fn update_offset(id: &str, file_name: &str, offset: u64) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        let prev = item.offsets.get(file_name).copied().unwrap_or(0);
        if offset >= prev {
            item.offsets.insert(file_name.to_string(), offset);
        }
        item.last_error = None;
        item.updated_at = Some(now_rfc3339());
    }
    write_queue(&q)
}

pub fn mark_attempt(id: &str) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.attempts = item.attempts.saturating_add(1);
        item.updated_at = Some(now_rfc3339());
        item.last_error = None;
    }
    write_queue(&q)
}

pub fn set_error(id: &str, error: String) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.last_error = Some(error);
        item.updated_at = Some(now_rfc3339());
    }
    write_queue(&q)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut q = read_queue();
    q.items.retain(|i| i.id != id);
    write_queue(&q)
}

impl PendingTransfer {
    #[allow(dead_code)]
    pub fn bytes_done(&self) -> u64 {
        self.offsets.values().copied().sum()
    }

    #[allow(dead_code)]
    pub fn bytes_total(&self) -> u64 {
        if !self.file_sizes.is_empty() {
            return self.file_sizes.values().copied().sum::<u64>().max(1);
        }
        // Fallback: at least show progress within known offsets.
        self.bytes_done().max(1)
    }
}
