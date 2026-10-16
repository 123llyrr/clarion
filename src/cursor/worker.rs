//! Bounded cursor asset preparation. Only owned theme descriptions/pixels cross threads.
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

use calloop::channel::{Channel, SyncSender};
use smithay::input::pointer::CursorIcon;
use xcursor::parser::{parse_xcursor, Image};

const QUEUE: usize = 8;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_THEME_BYTES: usize = 1024 * 1024;
const MAX_TOC: usize = 4096;
const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct ThemeSpec {
    pub name: String,
    paths: Vec<PathBuf>,
}
impl ThemeSpec {
    pub fn new(name: &str) -> Self {
        // Environment reads do not consult the filesystem. Preserve xcursor0.3.11 search order.
        let variable = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
        let home = variable("HOME").map(PathBuf::from);
        let expand = |path: PathBuf| {
            if let Ok(tail) = path.strip_prefix("~") {
                home.as_ref().map(|home| home.join(tail))
            } else {
                Some(path)
            }
        };
        let mut paths = Vec::new();
        if let Some(search) = variable("XCURSOR_PATH") {
            paths.extend(
                search
                    .split(':')
                    .filter(|path| !path.is_empty())
                    .filter_map(|path| expand(PathBuf::from(path))),
            );
        } else {
            if let Some(data) = variable("XDG_DATA_HOME") {
                paths.extend(expand(PathBuf::from(data)));
            } else if let Some(home) = &home {
                paths.push(home.join(".local/share/icons"));
            }
            if let Some(home) = &home {
                paths.push(home.join(".icons"));
            }
            if let Some(data) = variable("XDG_DATA_DIRS") {
                paths.extend(
                    data.split(':')
                        .filter(|path| !path.is_empty())
                        .filter_map(|path| expand(PathBuf::from(path)))
                        .map(|path| path.join("icons")),
                );
            } else {
                paths.extend([
                    PathBuf::from("/usr/local/share/icons"),
                    PathBuf::from("/usr/share/icons"),
                ]);
            }
            paths.push(PathBuf::from("/usr/share/pixmaps"));
            if let Some(home) = &home {
                paths.push(home.join(".cursors"));
            }
            paths.push(PathBuf::from("/usr/share/cursors/xorg-x11"));
        }
        Self {
            name: name.to_owned(),
            paths,
        }
    }
}

pub(super) struct Request {
    pub epoch: u64,
    pub icon: CursorIcon,
    pub scale: i32,
    pub size: u32,
    pub theme: Arc<ThemeSpec>,
}
pub(crate) struct CursorLoadCompletion {
    pub(super) epoch: u64,
    pub(super) icon: CursorIcon,
    pub(super) scale: i32,
    pub(super) images: Result<Vec<Image>, String>,
}
pub(super) struct Worker {
    requests: mpsc::SyncSender<Request>,
    pub epoch: Arc<AtomicU64>,
}
impl Worker {
    pub fn new() -> (Self, Channel<CursorLoadCompletion>) {
        let (requests, rx) = mpsc::sync_channel::<Request>(QUEUE);
        let (tx, completions) = calloop::channel::sync_channel(QUEUE);
        let epoch = Arc::new(AtomicU64::new(1));
        let active_epoch = epoch.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("cursor-assets".into())
            .spawn(move || run(rx, tx, active_epoch))
        {
            warn!("cannot start cursor asset worker: {error}");
        }
        (Self { requests, epoch }, completions)
    }
    pub fn request(&self, request: Request) -> Result<(), mpsc::TrySendError<Request>> {
        self.requests.try_send(request)
    }
}
fn run(rx: mpsc::Receiver<Request>, tx: SyncSender<CursorLoadCompletion>, epoch: Arc<AtomicU64>) {
    while let Ok(request) = rx.recv() {
        let images = if request.epoch == epoch.load(Ordering::Acquire) {
            load(&request).map_err(|error| error.to_string())
        } else {
            Err("retired cursor theme".to_owned())
        };
        let images = if request.epoch == epoch.load(Ordering::Acquire) {
            images
        } else {
            Err("retired cursor theme".to_owned())
        };
        // Retired work still signals released capacity, so a full old-theme queue cannot leave
        // every new-theme key Deferred without a current completion to trigger retry.
        if tx
            .send(CursorLoadCompletion {
                epoch: request.epoch,
                icon: request.icon,
                scale: request.scale,
                images,
            })
            .is_err()
        {
            break;
        }
    }
}
fn read_regular(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    // A FIFO replacement cannot block open. NONBLOCK belongs to our independent descriptor.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cursor asset must be a regular file",
        ));
    }
    if metadata.len() > limit as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cursor asset exceeds byte limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cursor asset grew beyond byte limit",
        ));
    }
    Ok(bytes)
}
fn inherits(path: &Path) -> Option<String> {
    let bytes = read_regular(path, MAX_THEME_BYTES).ok()?;
    let content = std::str::from_utf8(&bytes).ok()?;
    // Match xcursor's first Inherits token, including its whitespace/separator behavior.
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("Inherits") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let token = rest
            .trim_start_matches(|ch: char| ch.is_whitespace() || ch == ';' || ch == ',')
            .split(|ch: char| ch.is_whitespace() || ch == ';' || ch == ',')
            .next()?;
        if !token.is_empty() {
            return Some(token.to_owned());
        }
    }
    None
}
fn find_icon(
    theme: &ThemeSpec,
    name: &str,
    icon: &str,
    visited: &mut HashSet<String>,
) -> Option<PathBuf> {
    if visited.len() >= 128 || !visited.insert(name.to_owned()) {
        return None;
    }
    let directories: Vec<_> = theme
        .paths
        .iter()
        .map(|path| path.join(name))
        .filter(|path| path.is_dir())
        .collect();
    for directory in &directories {
        let path = directory.join("cursors").join(icon);
        if path.is_file() {
            return Some(path);
        }
    }
    for directory in directories {
        let inherited = inherits(&directory.join("index.theme"))
            .or_else(|| (name != "default").then(|| "default".to_owned()));
        if let Some(inherited) = inherited {
            if let Some(path) = find_icon(theme, &inherited, icon, visited) {
                return Some(path);
            }
        }
    }
    None
}
fn load(request: &Request) -> io::Result<Vec<Image>> {
    for name in std::iter::once(request.icon.name()).chain(request.icon.alt_names().iter().copied())
    {
        let Some(path) = find_icon(
            &request.theme,
            &request.theme.name,
            name,
            &mut HashSet::new(),
        ) else {
            continue;
        };
        match read_regular(&path, MAX_FILE_BYTES).and_then(|bytes| decode(&bytes, request.size)) {
            Ok(images) => return Ok(images),
            Err(error) => {
                trace!(?path, %error, "cursor asset unavailable; trying alternate name");
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no valid cursor icon in theme",
    ))
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn word(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid("cursor offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("truncated cursor table"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}
pub(super) fn decode(bytes: &[u8], nominal_size: u32) -> io::Result<Vec<Image>> {
    if bytes.len() > MAX_FILE_BYTES || bytes.get(..4) != Some(b"Xcur".as_slice()) {
        return Err(invalid("invalid cursor header"));
    }
    let header = word(bytes, 4)? as usize;
    let entries = word(bytes, 12)? as usize;
    if header < 16 || entries == 0 || entries > MAX_TOC {
        return Err(invalid("invalid cursor table size"));
    }
    let table_end = entries
        .checked_mul(12)
        .and_then(|length| header.checked_add(length))
        .ok_or_else(|| invalid("cursor table overflow"))?;
    if table_end > bytes.len() {
        return Err(invalid("truncated cursor table"));
    }
    let mut image_count = 0usize;
    let mut decoded_bytes = 0usize;
    let mut duration = 0u64;
    for entry in (header..table_end).step_by(12) {
        if word(bytes, entry)? != 0xfffd_0002 {
            continue;
        }
        let offset = word(bytes, entry + 8)? as usize;
        if offset < table_end
            || word(bytes, offset)? != 36
            || word(bytes, offset + 4)? != 0xfffd_0002
            || word(bytes, offset + 12)? != 1
        {
            return Err(invalid("invalid cursor image chunk"));
        }
        let width = word(bytes, offset + 16)? as usize;
        let height = word(bytes, offset + 20)? as usize;
        if width == 0 || height == 0 || width > 0x7fff || height > 0x7fff {
            return Err(invalid("invalid cursor dimensions"));
        }
        if word(bytes, offset + 24)? as usize > width || word(bytes, offset + 28)? as usize > height
        {
            return Err(invalid("cursor hotspot outside image"));
        }
        let pixels = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| invalid("cursor pixel size overflow"))?;
        decoded_bytes = decoded_bytes
            .checked_add(pixels)
            .ok_or_else(|| invalid("cursor pixel size overflow"))?;
        if decoded_bytes > MAX_DECODED_BYTES
            || offset
                .checked_add(36)
                .and_then(|offset| offset.checked_add(pixels))
                .is_none_or(|end| end > bytes.len())
        {
            return Err(invalid("cursor decoded pixels exceed bounds"));
        }
        duration += u64::from(word(bytes, offset + 32)?);
        if duration > u64::from(u32::MAX) {
            return Err(invalid("cursor animation duration exceeds bounds"));
        }
        image_count += 1;
    }
    if image_count == 0 {
        return Err(invalid("cursor contains no images"));
    }
    let mut images = parse_xcursor(bytes).ok_or_else(|| invalid("invalid cursor image data"))?;
    let (width, height) = images
        .iter()
        .min_by_key(|image| nominal_size.abs_diff(image.size))
        .map(|image| (image.width, image.height))
        .ok_or_else(|| invalid("cursor contains no images"))?;
    images.retain(|image| image.width == width && image.height == height);
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header(entries: u32) -> Vec<u8> {
        [
            b"Xcur".as_slice(),
            &16u32.to_le_bytes(),
            &1u32.to_le_bytes(),
            &entries.to_le_bytes(),
        ]
        .concat()
    }
    fn image() -> Vec<u8> {
        let mut data = header(1);
        for word in [
            0xfffd_0002u32,
            24,
            28,
            36,
            0xfffd_0002,
            24,
            1,
            2,
            1,
            1,
            0,
            17,
        ] {
            data.extend(word.to_le_bytes());
        }
        data.extend([10, 20, 30, 255, 40, 50, 60, 128]);
        data
    }
    #[test]
    fn empty_image_table_and_huge_toc_are_errors() {
        assert!(decode(&header(0), 24).is_err());
        assert!(decode(&header(u32::MAX), 24).is_err());
        assert!(decode(&header(2), 24).is_err());
        let mut bytes = image();
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bytes, 24).is_err());
    }
    #[test]
    fn normal_rgba_hotspot_delay_and_nominal_size_are_preserved() {
        let images = decode(&image(), 24).unwrap();
        assert_eq!(
            (
                images[0].width,
                images[0].height,
                images[0].xhot,
                images[0].yhot,
                images[0].delay
            ),
            (2, 1, 1, 0, 17)
        );
        assert_eq!(images[0].pixels_rgba, [10, 20, 30, 255, 40, 50, 60, 128]);
        assert_eq!(images[0].pixels_argb, [255, 10, 20, 30, 128, 40, 50, 60]);
        assert!(decode(&image(), u32::MAX).is_ok());
    }
    #[test]
    fn malformed_pixel_extent_or_large_dimensions_never_allocate_parser_capacity() {
        let mut bytes = image();
        bytes.pop();
        assert!(decode(&bytes, 24).is_err());
        let mut bytes = image();
        bytes[44..48].copy_from_slice(&32767u32.to_le_bytes());
        assert!(decode(&bytes, 24).is_err());
        let mut bytes = image();
        bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bytes, 24).is_err());
    }
    #[test]
    fn theme_inheritance_token_matches_existing_search_semantics() {
        let dir = std::env::temp_dir().join(format!("clarion-cursor-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.theme");
        std::fs::write(&path, " Inherits=ignored\nInherits = ; , first;second\n").unwrap();
        assert_eq!(inherits(&path).as_deref(), Some("first"));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn regular_reader_rejects_special_and_oversize_payloads() {
        assert!(read_regular(Path::new("/dev/zero"), 32).is_err());
        let path =
            std::env::temp_dir().join(format!("clarion-cursor-bounded-{}", std::process::id()));
        std::fs::write(&path, [0u8; 33]).unwrap();
        assert!(read_regular(&path, 32).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
