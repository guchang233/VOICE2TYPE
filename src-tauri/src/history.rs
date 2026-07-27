use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

pub static HISTORY: OnceCell<Arc<Mutex<TranscriptionHistory>>> = OnceCell::new();

pub fn init(dir: PathBuf) {
    let _ = HISTORY.set(Arc::new(Mutex::new(TranscriptionHistory::new(dir))));
}

pub fn push(text: String) {
    if let Some(h) = HISTORY.get() {
        if let Ok(mut guard) = h.lock() {
            guard.push(text);
        }
    }
}

pub fn last() -> Option<String> {
    HISTORY.get()?.lock().ok()?.last()
}

pub fn get_all() -> Vec<String> {
    if let Some(h) = HISTORY.get() {
        if let Ok(guard) = h.lock() {
            return guard.entries.iter().cloned().collect();
        }
    }
    Vec::new()
}

/// 按索引（从 0 开始，0 = 最新）删除一条历史记录
pub fn remove(index: usize) -> bool {
    if let Some(h) = HISTORY.get() {
        if let Ok(mut guard) = h.lock() {
            return guard.remove(index);
        }
    }
    false
}

/// 清空所有历史记录
pub fn clear() {
    if let Some(h) = HISTORY.get() {
        if let Ok(mut guard) = h.lock() {
            guard.clear();
        }
    }
}

const MAX_ENTRIES: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HistoryFile {
    entries: Vec<String>,
}

pub struct TranscriptionHistory {
    path: PathBuf,
    entries: VecDeque<String>,
}

impl TranscriptionHistory {
    pub fn new(config_dir: PathBuf) -> Self {
        let path = config_dir.join("transcription_history.json");
        let entries = Self::load(&path);
        Self { path, entries }
    }

    fn load(path: &PathBuf) -> VecDeque<String> {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return VecDeque::new(),
        };
        let file: HistoryFile = match serde_json::from_str(&content) {
            Ok(f) => f,
            Err(_) => return VecDeque::new(),
        };
        file.entries.into_iter().collect()
    }

    pub fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.entries.front().map(|s| s.as_str()) == Some(text.as_str()) {
            return;
        }
        self.entries.push_front(text);
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_back();
        }
        let _ = self.save();
    }

    pub fn last(&self) -> Option<String> {
        self.entries.front().cloned()
    }

    pub fn entries(&self) -> Vec<String> {
        self.entries.iter().cloned().collect()
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        // VecDeque 没有 remove(idx)，先收集到 Vec 再重建
        let mut vec: Vec<String> = self.entries.iter().cloned().collect();
        vec.remove(index);
        self.entries = vec.into_iter().collect();
        let _ = self.save();
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = self.save();
    }

    fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = HistoryFile {
            entries: self.entries.iter().cloned().collect(),
        };
        fs::write(&self.path, serde_json::to_string_pretty(&file)?)?;
        Ok(())
    }
}
