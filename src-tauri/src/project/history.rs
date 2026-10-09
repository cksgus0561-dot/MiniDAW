//! Bounded document-only snapshots; Arc copies are cheap when a transaction is staged.
use super::schema::Project;
use std::{collections::VecDeque, sync::Arc};
#[derive(Clone)]
struct Entry {
    before: Arc<Project>,
    after: Arc<Project>,
    label: String,
    bytes: usize,
}
#[derive(Clone, Default)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    bytes: usize,
    group: Option<String>,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    pub undo: usize,
    pub redo: usize,
    pub bytes: usize,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}
impl History {
    pub fn record(&mut self, before: Project, after: Project, label: &str) {
        self.record_group(before, after, label, None);
    }
    pub fn record_group(
        &mut self,
        before: Project,
        after: Project,
        label: &str,
        group: Option<&str>,
    ) {
        if group.is_some() && self.group.as_deref() == group && self.redo.is_empty() {
            if let Some(entry) = self
                .undo
                .back_mut()
                .filter(|e| e.label == label && *e.after == before)
            {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                entry.after = Arc::new(after);
                entry.bytes = serde_json::to_vec(entry.before.as_ref()).map_or(0, |b| b.len())
                    + serde_json::to_vec(entry.after.as_ref()).map_or(0, |b| b.len());
                self.bytes += entry.bytes;
                return;
            }
        }
        self.group = group.map(str::to_owned);
        for entry in self.redo.drain(..) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
        let bytes = serde_json::to_vec(&before).map_or(0, |b| b.len())
            + serde_json::to_vec(&after).map_or(0, |b| b.len());
        self.bytes += bytes;
        self.undo.push_back(Entry {
            before: Arc::new(before),
            after: Arc::new(after),
            label: label.into(),
            bytes,
        });
        while self.undo.len() > 128 || (self.bytes > 32 * 1024 * 1024 && self.undo.len() > 1) {
            if let Some(e) = self.undo.pop_front() {
                self.bytes -= e.bytes;
            }
        }
    }
    pub fn undo(&mut self) -> Option<Project> {
        self.group = None;
        let e = self.undo.pop_back()?;
        let p = (*e.before).clone();
        self.redo.push(e);
        Some(p)
    }
    pub fn redo(&mut self) -> Option<Project> {
        self.group = None;
        let e = self.redo.pop()?;
        let p = (*e.after).clone();
        self.undo.push_back(e);
        Some(p)
    }
    pub fn view(&self) -> HistoryView {
        HistoryView {
            undo: self.undo.len(),
            redo: self.redo.len(),
            bytes: self.bytes,
            undo_label: self.undo.back().map(|e| e.label.clone()),
            redo_label: self.redo.last().map(|e| e.label.clone()),
        }
    }
}
pub fn musical_hash(p: &Project) -> String {
    use sha2::{Digest, Sha256};
    let mut p = p.clone();
    p.name.clear();
    for a in &mut p.assets {
        if a.path.original_absolute_path.is_some() {
            a.path.project_relative_path = None;
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&p).expect("validated document"))
    )
}
