use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::ImageReader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{now_ms, HistoryEntry, HistoryKind};

pub(super) const TEXT_LIMIT: usize = 1024 * 1024;
pub(super) const IMAGE_LIMIT: usize = 16 * 1024 * 1024;
const BYTE_LIMIT: usize = 64 * 1024 * 1024;
const ENTRY_LIMIT: usize = 80;
/// The metadata file inside the history directory. Payloads sit beside it, one
/// `<id>.bin` per entry, so a restart reads only what it owns.
const INDEX: &str = "index.json";

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
    /// `None` keeps the history in memory only (tests, or no home directory).
    state_dir: Option<PathBuf>,
}

/// `$XDG_STATE_HOME/clarion-shell/clipboard`, else `~/.local/state/...`.
/// Clipboard data is personal, so it lives in the private state tree rather
/// than the cache or config one.
pub(super) fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))?;
    Some(base.join("clarion-shell").join("clipboard"))
}

/// One entry as it is written to `index.json`. The payload itself lives in the
/// blob beside it; the derived fields (text, thumbnail, dimensions) are rebuilt
/// from that blob on load, so only the facts the session decides are stored.
#[derive(Serialize, Deserialize)]
struct StoredEntry {
    id: String,
    /// `text`, `image` or `screenshot`.
    kind: String,
    #[serde(default)]
    pinned: bool,
    created_at: u64,
    #[serde(default)]
    file_path: Option<PathBuf>,
    #[serde(default)]
    mime: String,
}

impl History {
    /// In-memory history: the tests' default, and the shape a caller takes when
    /// it does not want clipboard data on disk.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_state(None)
    }

    /// A history that reads its state directory at construction and writes it
    /// back after every change.
    pub fn with_state(state_dir: Option<PathBuf>) -> Self {
        let mut history = Self {
            entries: HashMap::new(),
            payloads: HashMap::new(),
            bytes: 0,
            paused: false,
            state_dir,
        };
        history.load();
        history
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
        let changed = self.insert_inner(payload);
        if changed {
            self.persist();
        }
        changed
    }

    /// The insert itself, without the disk write: loading the saved history
    /// walks through here to rebuild text, thumbnails and dimensions from the
    /// stored payloads.
    fn insert_inner(&mut self, payload: Payload) -> bool {
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
        // A `text/uri-list` of local files is a file copy: keep the URIs in the
        // payload (so they can be pasted back into a file manager) and show the
        // plain paths. A URI list of anything else stays an ordinary text entry.
        let paths = if payload.mime == "text/uri-list" {
            uri_paths(&text)
        } else {
            None
        };
        let prefix = if image {
            "image"
        } else if paths.is_some() {
            "files"
        } else {
            "text"
        };
        let text = match paths {
            Some(ref paths) => paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n"),
            None => text,
        };
        let id = format!(
            "{}:{:x}",
            prefix,
            Sha256::digest(payload.bytes.as_slice())
        );
        let existing = self.entries.get(&id);
        let pinned = existing.is_some_and(|entry| entry.pinned);
        let screenshot = payload.screenshot
            || existing.is_some_and(|entry| entry.kind == HistoryKind::Screenshot);
        let (thumbnail, width, height) = if image {
            match existing
                .and_then(|entry| {
                    entry
                        .thumbnail_png
                        .clone()
                        .zip(entry.width.zip(entry.height))
                        .map(|(png, (width, height))| (png, width, height))
                })
                .or_else(|| thumbnail(&payload.bytes))
            {
                Some((thumbnail, width, height)) => (Some(thumbnail), Some(width), Some(height)),
                None => return false,
            }
        } else {
            (None, None, None)
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
                self.remove(&oldest);
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
                } else if paths.is_some() {
                    HistoryKind::Files
                } else {
                    HistoryKind::Text
                },
                text,
                thumbnail_png: thumbnail,
                pinned,
                file_path: payload
                    .file_path
                    .clone()
                    .or_else(|| paths.as_ref().and_then(|paths| paths.first().cloned()))
                    .or_else(|| {
                        self.entries
                            .get(&id)
                            .and_then(|entry| entry.file_path.clone())
                    }),
                created_at: now_ms(),
                mime: payload.mime.clone(),
                bytes: payload.bytes.len(),
                width,
                height,
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
        let before = self.entries.get(id).map(|entry| entry.pinned);
        if let Some(entry) = self.entries.get_mut(id) {
            entry.pinned = !entry.pinned;
        }
        if self.entries.get(id).map(|entry| entry.pinned) != before {
            self.persist();
        }
    }
    pub fn delete(&mut self, id: &str) {
        if self.remove(id) {
            self.persist();
        }
    }
    fn remove(&mut self, id: &str) -> bool {
        let removed = self.entries.remove(id).is_some();
        if let Some(payload) = self.payloads.remove(id) {
            self.bytes = self.bytes.saturating_sub(payload.bytes.len());
        }
        removed
    }
    pub fn clear(&mut self) {
        let ids: Vec<_> = self
            .entries
            .values()
            .filter(|entry| !entry.pinned)
            .map(|entry| entry.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.remove(&id);
        }
        self.persist();
    }

    /// Rebuild the session's history from the state directory. A missing or
    /// unreadable index simply starts empty; an entry whose payload vanished or
    /// no longer validates is skipped so the rest of the list survives.
    fn load(&mut self) {
        let Some(dir) = self.state_dir.clone() else {
            return;
        };
        let Ok(index) = std::fs::read(dir.join(INDEX)) else {
            return;
        };
        let Ok(stored) = serde_json::from_slice::<Vec<StoredEntry>>(&index) else {
            return;
        };
        for stored in stored {
            let Ok(bytes) = std::fs::read(blob_path(&dir, &stored.id)) else {
                continue;
            };
            let image = matches!(stored.kind.as_str(), "image" | "screenshot");
            let mime = if stored.mime.is_empty() {
                if image {
                    "image/png".to_owned()
                } else if stored.kind == "files" {
                    "text/uri-list".to_owned()
                } else {
                    "text/plain".to_owned()
                }
            } else {
                stored.mime
            };
            let payload = Payload {
                mime,
                bytes: Arc::new(bytes),
                screenshot: stored.kind == "screenshot",
                file_path: stored.file_path,
            };
            if !self.insert_inner(payload) {
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&stored.id) {
                entry.pinned = stored.pinned;
                entry.created_at = stored.created_at;
            }
        }
    }

    /// Write the index and any payload blob the directory does not have yet,
    /// then drop blobs the session no longer keeps. Files are private (0600)
    /// inside a private directory (0700), and the index is replaced through a
    /// temporary file so a crash never leaves it half-written.
    fn persist(&self) {
        let Some(dir) = &self.state_dir else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let mut stored = Vec::with_capacity(self.entries.len());
        let mut live = HashSet::with_capacity(self.entries.len() + 1);
        for entry in self.entries.values() {
            stored.push(StoredEntry {
                id: entry.id.clone(),
                kind: match entry.kind {
                    HistoryKind::Text => "text",
                    HistoryKind::Image => "image",
                    HistoryKind::Screenshot => "screenshot",
                    HistoryKind::Files => "files",
                }
                .to_owned(),
                pinned: entry.pinned,
                created_at: entry.created_at,
                file_path: entry.file_path.clone(),
                mime: entry.mime.clone(),
            });
            let path = blob_path(dir, &entry.id);
            live.insert(path.clone());
            if let Some(payload) = self.payloads.get(&entry.id) {
                if !path.exists() {
                    write_private(&path, &payload.bytes);
                }
            }
        }
        live.insert(dir.join(INDEX));
        if let Ok(json) = serde_json::to_vec(&stored) {
            write_private(&dir.join(INDEX), &json);
        }
        if let Ok(files) = std::fs::read_dir(dir) {
            for file in files.flatten() {
                let path = file.path();
                if !live.contains(&path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }
}

/// The blob file for one entry. Ids are `<kind>:<digest>`, so the colon is the
/// only character that needs replacing for a portable file name.
fn blob_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.bin", id.replace(':', "-")))
}

/// Replace one private file atomically: write the sibling temporary file with
/// mode 0600, then rename it over the target.
fn write_private(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
    else {
        return;
    };
    if file.write_all(bytes).is_err() || file.sync_all().is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    drop(file);
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The local paths behind a `text/uri-list`, or `None` when any entry is not a
/// local `file://` URI. Comments (`#`) and blank lines are skipped, as the
/// format allows.
pub(super) fn uri_paths(list: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in list.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let rest = line.strip_prefix("file://")?;
        // `file:///path` and `file://localhost/path` are both local.
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode(rest)?;
        let path = PathBuf::from(decoded);
        if !path.is_absolute() {
            return None;
        }
        paths.push(path);
    }
    (!paths.is_empty()).then_some(paths)
}

/// The minimal percent-decoding `text/uri-list` needs for file names with
/// spaces or non-ASCII bytes.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub(super) fn thumbnail(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
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
    Some((output.into_inner(), width, height))
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

    fn file_list(uris: &str) -> Payload {
        Payload {
            mime: "text/uri-list".into(),
            bytes: Arc::new(uris.as_bytes().to_vec()),
            screenshot: false,
            file_path: None,
        }
    }

    #[test]
    fn local_uri_lists_become_file_entries() {
        let mut history = History::new();
        assert!(history.insert(file_list(
            "file:///tmp/one%20two.txt\r\n# comment\r\nfile:///tmp/three.png\r\n"
        )));
        let entry = &history.snapshot()[0];
        assert_eq!(entry.kind, HistoryKind::Files);
        assert_eq!(entry.text, "/tmp/one two.txt\n/tmp/three.png");
        assert_eq!(
            entry.file_path.as_deref(),
            Some(Path::new("/tmp/one two.txt"))
        );
        assert_eq!(entry.mime, "text/uri-list");
        // The payload keeps the original list so a paste restores the URIs.
        assert_eq!(
            history.payload(&entry.id).unwrap().bytes.as_slice(),
            b"file:///tmp/one%20two.txt\r\n# comment\r\nfile:///tmp/three.png\r\n"
        );

        // A link is not a file: it stays an ordinary text capture.
        assert!(history.insert(file_list("https://example.com/a\r\n")));
        let link = history
            .snapshot()
            .into_iter()
            .find(|entry| entry.kind == HistoryKind::Text)
            .expect("the link stays text");
        assert_eq!(link.text.trim(), "https://example.com/a");
        assert!(link.file_path.is_none());
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 8, 7, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn history_survives_a_restart_with_pins_and_metadata() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("clipboard");
        let mut history = History::with_state(Some(dir.clone()));
        assert!(history.insert(text("hello")));
        assert!(history.insert(text("world")));
        assert!(history.insert(Payload {
            mime: "image/png".into(),
            bytes: Arc::new(png(4, 2)),
            screenshot: true,
            file_path: Some(PathBuf::from("/tmp/shot.png")),
        }));
        assert!(history.insert(file_list("file:///tmp/copied.txt\r\n")));
        let hello = history
            .snapshot()
            .into_iter()
            .find(|entry| entry.text == "hello")
            .unwrap();
        history.pin(&hello.id);

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "the history directory is private");
        let index = std::fs::metadata(dir.join(INDEX))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(index, 0o600, "the index is readable by its owner only");

        let entries = History::with_state(Some(dir.clone())).snapshot();
        assert_eq!(entries.len(), 4, "text and screenshot entries come back");
        assert_eq!(entries[0].text, "hello");
        assert!(entries[0].pinned, "pins survive a restart");
        assert_eq!(
            entries[0].created_at, hello.created_at,
            "the capture time survives"
        );
        let shot = entries
            .iter()
            .find(|entry| entry.kind == HistoryKind::Screenshot)
            .expect("the screenshot comes back as a screenshot");
        assert_eq!((shot.width, shot.height), (Some(4), Some(2)));
        assert_eq!(shot.file_path.as_deref(), Some(Path::new("/tmp/shot.png")));
        assert_eq!(shot.mime, "image/png");
        assert!(shot.thumbnail_png.is_some(), "the thumbnail is rebuilt");
        let files = entries
            .iter()
            .find(|entry| entry.kind == HistoryKind::Files)
            .expect("the file list comes back as files");
        assert_eq!(files.text, "/tmp/copied.txt");
        assert_eq!(files.mime, "text/uri-list");

        let mut reopened = History::with_state(Some(dir.clone()));
        let world = entries
            .iter()
            .find(|entry| entry.text == "world")
            .unwrap()
            .id
            .clone();
        reopened.delete(&world);
        assert_eq!(
            History::with_state(Some(dir)).snapshot().len(),
            3,
            "a delete reaches the next session"
        );
    }

    #[test]
    fn cleared_entries_stay_gone_after_a_restart() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("clipboard");
        let mut history = History::with_state(Some(dir.clone()));
        assert!(history.insert(text("keep")));
        assert!(history.insert(text("drop")));
        let keep = history
            .snapshot()
            .into_iter()
            .find(|entry| entry.text == "keep")
            .unwrap()
            .id;
        history.pin(&keep);
        history.clear();

        let entries = History::with_state(Some(dir.clone())).snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "keep");
        let blobs = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|file| file.file_name().to_string_lossy().ends_with(".bin"))
            .count();
        assert_eq!(blobs, 1, "the dropped payload leaves no blob behind");
    }
}
