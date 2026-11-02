//! Durable guest→host HQ track handoff queue.
//! Local files are always the source of truth. The queue survives leaving the
//! lobby so multi-hour podcasts can finish uploading later (or via cloud).

use crate::paths::app_data_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingTransfer {
    pub id: String,
    pub host_session_id: String,
    pub participant_id: String,
    pub invite: String,
    pub local_session_id: String,
    pub file_names: Vec<String>,
    /// Bytes already acknowledged per file (for resume).
    #[serde(default)]
    pub offsets: std::collections::HashMap<String, u64>,
    pub created_at: String,
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
    let json = serde_json::to_string_pretty(q).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub fn list_pending() -> Vec<PendingTransfer> {
    read_queue().items
}

pub fn enqueue(item: PendingTransfer) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(existing) = q.items.iter_mut().find(|i| i.id == item.id) {
        *existing = item;
    } else {
        q.items.push(item);
    }
    write_queue(&q)
}

pub fn update_offset(id: &str, file_name: &str, offset: u64) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.offsets.insert(file_name.to_string(), offset);
        item.last_error = None;
    }
    write_queue(&q)
}

pub fn set_error(id: &str, error: String) -> Result<(), String> {
    let mut q = read_queue();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.last_error = Some(error);
    }
    write_queue(&q)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut q = read_queue();
    q.items.retain(|i| i.id != id);
    write_queue(&q)
}
