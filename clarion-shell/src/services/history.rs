use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use image::ImageReader;
use sha2::{Digest, Sha256};

use super::{now_ms, HistoryEntry, HistoryKind};

pub(super) const TEXT_LIMIT: usize = 1024 * 1024;
pub(super) const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
const BYTE_LIMIT: usize = 64 * 1024 * 1024;
const ENTRY_LIMIT: usize = 80;

#[derive(Clone)]
pub(super) struct Payload {
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
    pub screenshot: bool,
    pub file_path: Option<std::path::PathBuf>,
}
pub(super) struct History {
    entries: HashMap<String, HistoryEntry>,
    payloads: HashMap<String, Payload>,
    bytes: usize,
    pub paused: bool,
}

impl History {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            payloads: HashMap::new(),
            bytes: 0,
            paused: false,
        }
    }
    pub fn snapshot(&self) -> Vec<HistoryEntry> {
        let mut entries: Vec<_> = self.entries.values().cloned().collect();
        entries.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        entries
    }
    pub fn insert(&mut self, payload: Payload) -> bool {
        if self.paused || payload.bytes.is_empty() {
            return false;
        }
        let image = payload.mime == "image/png";
        if payload.bytes.len() > if image { IMAGE_LIMIT } else { TEXT_LIMIT } {
            return false;
        }
        let text = if image {
            String::new()
        } else {
            let Ok(text) = std::str::from_utf8(&payload.bytes) else {
                return false;
            };
            text.to_owned()
        };
        let id = format!(
            "{}:{:x}",
            if image { "image" } else { "text" },
            Sha256::digest(payload.bytes.as_slice())
        );
        let existing = self.entries.get(&id);
        let pinned = existing.is_some_and(|entry| entry.pinned);
        let screenshot = payload.screenshot
            || existing.is_some_and(|entry| entry.kind == HistoryKind::Screenshot);
        let thumbnail = if image {
            match existing
                .and_then(|entry| entry.thumbnail_png.clone())
                .or_else(|| thumbnail(&payload.bytes))
            {
                Some(thumbnail) => Some(thumbnail),
                None => return false,
            }
        } else {
            None
        };
        if existing.is_none() {
            while self.entries.len() >= ENTRY_LIMIT || self.bytes + payload.bytes.len() > BYTE_LIMIT
            {
                let oldest = self
                    .entries
                    .values()
                    .filter(|entry| !entry.pinned)
                    .min_by_key(|entry| entry.created_at)
                    .map(|entry| entry.id.clone());
                let Some(oldest) = oldest else {
                    return false;
                };
                self.delete(&oldest);
            }
            self.bytes += payload.bytes.len();
            self.payloads.insert(id.clone(), payload.clone());
        }
        self.entries.insert(
            id.clone(),
            HistoryEntry {
                id: id.clone(),
                kind: if screenshot {
                    HistoryKind::Screenshot
                } else if image {
                    HistoryKind::Image
                } else {
                    HistoryKind::Text
                },
                text,
                thumbnail_png: thumbnail,
                pinned,
                file_path: payload.file_path.clone().or_else(|| {
                    self.entries
                        .get(&id)
                        .and_then(|entry| entry.file_path.clone())
                }),
                created_at: now_ms(),
            },
        );
        true
    }
    pub fn payload(&self, id: &str) -> Option<Payload> {
        let mut payload = self.payloads.get(id)?.clone();
        payload.screenshot = self.entries.get(id)?.kind == HistoryKind::Screenshot;
        Some(payload)
    }
    pub fn pin(&mut self, id: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            entry.pinned = !entry.pinned;
        }
    }
    pub fn delete(&mut self, id: &str) {
        self.entries.remove(id);
        if let Some(payload) = self.payloads.remove(id) {
            self.bytes -= payload.bytes.len();
        }
    }
    pub fn clear(&mut self) {
        let ids: Vec<_> = self
            .entries
            .values()
            .filter(|entry| !entry.pinned)
            .map(|entry| entry.id.clone())
            .collect();
        for id in ids {
            self.delete(&id);
        }
    }
}

pub(super) fn thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() > IMAGE_LIMIT || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let (width, height) = reader.into_dimensions().ok()?;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 40 * 1024 * 1024
    {
        return None;
    }
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .ok()?
        .thumbnail(320, 320);
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(value: &str) -> Payload {
        Payload {
            mime: "text/plain".into(),
            bytes: Arc::new(value.as_bytes().to_vec()),
            screenshot: false,
            file_path: None,
        }
    }
    #[test]
    fn history_clear_recopy_pin_and_limits() {
        let mut history = History::new();
        assert!(history.insert(text("hello")));
        assert!(history.insert(text("hello")));
        assert_eq!(history.snapshot().len(), 1);
        history.clear();
        assert!(history.snapshot().is_empty());
        assert!(history.insert(text("hello")));
        let id = history.snapshot()[0].id.clone();
        history.pin(&id);
        history.clear();
        assert_eq!(history.snapshot().len(), 1);
        history.paused = true;
        assert!(!history.insert(text("private")));
        history.paused = false;
        for i in 0..90 {
            history.insert(text(&i.to_string()));
        }
        assert_eq!(history.snapshot().len(), 80);
        assert!(history.payload(&id).is_some());
        history.delete(&id);
        assert!(history.payload(&id).is_none());
    }
    #[test]
    fn invalid_and_oversize_inputs_do_not_enter_history() {
        let mut history = History::new();
        assert!(!history.insert(Payload {
            mime: "text/plain".into(),
            bytes: Arc::new(vec![0xff]),
            screenshot: false,
            file_path: None
        }));
        assert!(!history.insert(Payload {
            mime: "text/plain".into(),
            bytes: Arc::new(vec![b'x'; TEXT_LIMIT + 1]),
            screenshot: false,
            file_path: None
        }));
        assert!(thumbnail(b"not png").is_none());
    }
}
