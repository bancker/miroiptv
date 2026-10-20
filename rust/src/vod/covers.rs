//! Covers, backdrops and stills (spec 2026-10-15 §4.4). Only what a page asks
//! for in a frame is fetched: what is on screen and what comes next. A cover is
//! read from its file when it is kept, else downloaded (6 at a time); one that
//! a frame no longer asks for before it comes is dropped. What came is kept as
//! the bytes it was, in `<16 hex of sha256(address)>.jpg` or `.png`, decoded
//! on tokio's blocking pool into one of a few fixed sizes, and kept as a
//! texture: 96 MB of them at most, the one shown longest ago going first. The
//! folder is kept to 500 MB by `prune`, the oldest files first.
//!
//! An address may hold the portal's account: it is never logged (only its
//! host), never a file's name and never a texture's (a hash). A title without
//! a cover shows its name on a colour of its own (`placeholder_tint`).

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Downloads at the same time.
pub const AT_ONCE: usize = 6;
/// Covers read from their files and decoded at the same time: beside the
/// downloads, so a slow server never holds up a kept cover.
pub const READS_AT_ONCE: usize = 6;
/// The textures' bytes together (width × height × 4).
pub const TEXTURE_BYTES: usize = 96 * 1024 * 1024;
/// The covers folder's bytes, kept by `prune`.
pub const DISK_BYTES: u64 = 500 * 1024 * 1024;
/// Textures made per frame: a screenful read from the disk spreads over a few
/// frames instead of making one long one.
pub const UPLOADS_PER_FRAME: usize = 8;
/// A body bigger than this is no cover.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// A canvas wider or taller than this is no cover.
const MAX_CANVAS: u32 = 8192;

/// What a picture is drawn as, and so the sizes it is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A standing cover, 2:3.
    Poster,
    /// A lying tile or an episode's still, 16:9.
    Landscape,
    /// A page's background, 16:9.
    Backdrop,
}

impl Shape {
    /// The sizes it is made in, smallest first (spec §4.4's table).
    pub fn sizes(self) -> &'static [[u32; 2]] {
        match self {
            Shape::Poster => &[[200, 300], [300, 450], [400, 600]],
            Shape::Landscape => &[[320, 180], [480, 270], [640, 360]],
            Shape::Backdrop => &[[1280, 720], [1920, 1080]],
        }
    }

    /// The first size at least as big as a tile of `shown` pixels (physical
    /// pixels: points times the screen's scale), the largest for a bigger one.
    /// A window of another size so finds its covers made already.
    pub fn size_for(self, shown: [f32; 2]) -> [u32; 2] {
        let sizes = self.sizes();
        sizes
            .iter()
            .copied()
            .find(|[w, h]| *w as f32 >= shown[0] && *h as f32 >= shown[1])
            .unwrap_or(sizes[sizes.len() - 1])
    }
}

/// A cover at one size: the hash of its address (its file's name too) and the
/// size it is made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverKey {
    pub hash: u64,
    pub size: [u32; 2],
}

/// The first 8 bytes of the SHA-256 of `url`.
pub fn url_hash(url: &str) -> u64 {
    let digest = ring::digest::digest(&ring::digest::SHA256, url.as_bytes());
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest.as_ref()[..8]);
    u64::from_be_bytes(first)
}

/// The name a cover's file has before its extension: 16 hex digits of the
/// SHA-256 of its address.
pub fn file_stem(url: &str) -> String {
    format!("{:016x}", url_hash(url))
}

/// The colour of a title without a cover: its hue from the first bytes of the
/// SHA-256 of its name, so a title always has the same one; muted and dark
/// enough for white letters.
pub fn placeholder_tint(name: &str) -> egui::Color32 {
    let digest = ring::digest::digest(&ring::digest::SHA256, name.as_bytes());
    let b = digest.as_ref();
    let hue = f32::from(u16::from_be_bytes([b[0], b[1]])) / 65_536.0;
    hsv(hue, 0.45, 0.35)
}

/// A colour by its hue (0 to 1), saturation and value, as the screen shows it.
fn hsv(h: f32, s: f32, v: f32) -> egui::Color32 {
    let sector = (h * 6.0).floor();
    let f = h * 6.0 - sector;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match sector as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let byte = |x: f32| (x * 255.0).round() as u8;
    egui::Color32::from_rgb(byte(r), byte(g), byte(b))
}

/// A texture as the covers keep it: its size in bytes is what it costs.
pub trait CoverTexture {
    fn byte_size(&self) -> usize;
}

impl CoverTexture for egui::TextureHandle {
    fn byte_size(&self) -> usize {
        let [w, h] = self.size();
        w * h * 4
    }
}

/// What makes a texture of a decoded cover: egui's context in the app, a fake
/// in the tests, which have no GPU.
pub trait Uploader {
    type Texture: CoverTexture;
    /// A texture of `image`, called `name` (a hash, never an address).
    fn upload(&self, name: String, image: egui::ColorImage) -> Self::Texture;
}

impl Uploader for egui::Context {
    type Texture = egui::TextureHandle;

    fn upload(&self, name: String, image: egui::ColorImage) -> egui::TextureHandle {
        self.load_texture(name, image, egui::TextureOptions::LINEAR)
    }
}

/// The textures of the covers shown, `cap` bytes together at most: the one
/// shown longest ago goes first.
pub struct TextureLru<T> {
    cap: usize,
    used: usize,
    clock: u64,
    entries: HashMap<CoverKey, (T, u64)>,
}

impl<T: CoverTexture> TextureLru<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    /// Marks `key`'s texture as shown now; whether there is one.
    pub fn touch(&mut self, key: &CoverKey) -> bool {
        self.clock += 1;
        match self.entries.get_mut(key) {
            Some((_, shown)) => {
                *shown = self.clock;
                true
            }
            None => false,
        }
    }

    /// `key`'s texture, as it is.
    pub fn peek(&self, key: &CoverKey) -> Option<&T> {
        self.entries.get(key).map(|(t, _)| t)
    }

    /// Keeps `texture` as shown now. Those shown longest ago go until it fits
    /// (one bigger than the cap alone stays alone).
    pub fn insert(&mut self, key: CoverKey, texture: T) {
        self.clock += 1;
        let bytes = texture.byte_size();
        if let Some((old, _)) = self.entries.insert(key, (texture, self.clock)) {
            self.used -= old.byte_size();
        }
        self.used += bytes;
        while self.used > self.cap && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, (_, shown))| *shown)
                .map(|(k, _)| *k);
            let Some(oldest) = oldest else { break };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.used -= gone.byte_size();
            }
        }
    }

    /// The bytes kept.
    pub fn used(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

enum Slot {
    /// On its way; dropped when a frame no longer asks for it.
    Loading(tokio::task::AbortHandle),
    /// No cover at this address: not asked again this session.
    Missing,
}

/// What a cover's task came back with.
struct Arrived {
    key: CoverKey,
    image: Option<egui::ColorImage>,
}

/// The covers of the pages that show.
pub struct CoverStore<U: Uploader> {
    dir: PathBuf,
    uploader: U,
    /// Built when the first cover is asked for: a session that never opens
    /// the thuisbioscoop pays nothing for it.
    client: Option<reqwest::Client>,
    client_failed: bool,
    textures: TextureLru<U::Texture>,
    slots: HashMap<CoverKey, Slot>,
    /// Asked for in this frame.
    wanted: HashSet<CoverKey>,
    downloads: Arc<Semaphore>,
    reads: Arc<Semaphore>,
    done: Arc<Mutex<Vec<Arrived>>>,
    /// Decoded, waiting for a frame with room to make its texture.
    decoded: Vec<Arrived>,
}

impl<U: Uploader> CoverStore<U> {
    /// Covers kept as files in `dir`, made into textures by `uploader`.
    pub fn new(dir: PathBuf, uploader: U) -> Self {
        Self {
            dir,
            uploader,
            client: None,
            client_failed: false,
            textures: TextureLru::new(TEXTURE_BYTES),
            slots: HashMap::new(),
            wanted: HashSet::new(),
            downloads: Arc::new(Semaphore::new(AT_ONCE)),
            reads: Arc::new(Semaphore::new(READS_AT_ONCE)),
            done: Arc::new(Mutex::new(Vec::new())),
            decoded: Vec::new(),
        }
    }

    /// The texture of the cover at `url`, made for a tile of `shown` pixels,
    /// once it is there; asked for when it is not. None while it comes and for
    /// no cover: the tile shows its placeholder, of the same size.
    pub fn cover(&mut self, url: &str, shape: Shape, shown: [f32; 2]) -> Option<&U::Texture> {
        let key = CoverKey {
            hash: url_hash(url),
            size: shape.size_for(shown),
        };
        self.wanted.insert(key);
        if self.textures.touch(&key) {
            return self.textures.peek(&key);
        }
        if !self.slots.contains_key(&key) {
            self.start(key, url);
        }
        None
    }

    /// Whether the cover at `url` for a tile of `shown` pixels is on its way.
    pub fn is_loading(&self, url: &str, shape: Shape, shown: [f32; 2]) -> bool {
        let key = CoverKey {
            hash: url_hash(url),
            size: shape.size_for(shown),
        };
        matches!(self.slots.get(&key), Some(Slot::Loading(_)))
    }

    /// Downloads on their way now.
    pub fn downloads_in_flight(&self) -> usize {
        AT_ONCE - self.downloads.available_permits()
    }

    /// The textures' bytes.
    pub fn texture_bytes(&self) -> usize {
        self.textures.used()
    }

    /// After a frame has asked for its covers: what it did not ask for and is
    /// still on its way is dropped, and what came is made into textures, at
    /// most [`UPLOADS_PER_FRAME`].
    pub fn end_frame(&mut self) {
        let wanted = std::mem::take(&mut self.wanted);
        self.slots.retain(|key, slot| match slot {
            Slot::Loading(task) if !wanted.contains(key) => {
                task.abort();
                false
            }
            _ => true,
        });
        self.decoded.extend(std::mem::take(&mut *self.done.lock()));
        let mut made = 0;
        let mut later = Vec::new();
        for arrived in std::mem::take(&mut self.decoded) {
            // Dropped while it came: of no use any more.
            if !matches!(self.slots.get(&arrived.key), Some(Slot::Loading(_))) {
                continue;
            }
            match arrived.image {
                None => {
                    self.slots.insert(arrived.key, Slot::Missing);
                }
                Some(_) if made >= UPLOADS_PER_FRAME => later.push(arrived),
                Some(image) => {
                    let [w, h] = arrived.key.size;
                    let name = format!("cover {:016x} {w}x{h}", arrived.key.hash);
                    let texture = self.uploader.upload(name, image);
                    self.textures.insert(arrived.key, texture);
                    self.slots.remove(&arrived.key);
                    made += 1;
                }
            }
        }
        self.decoded = later;
    }

    /// Reads or fetches the cover in the background.
    fn start(&mut self, key: CoverKey, url: &str) {
        let Some(client) = self.http_client() else {
            self.slots.insert(key, Slot::Missing);
            return;
        };
        let (url, dir, done) = (url.to_owned(), self.dir.clone(), self.done.clone());
        let (downloads, reads) = (self.downloads.clone(), self.reads.clone());
        let task = tokio::spawn(async move {
            let image = load_cover(&client, &url, &dir, key, &downloads, &reads).await;
            done.lock().push(Arrived { key, image });
        });
        self.slots.insert(key, Slot::Loading(task.abort_handle()));
    }

    /// The HTTP client, built on first use; none when it cannot be built
    /// (warned once): the placeholders show.
    fn http_client(&mut self) -> Option<reqwest::Client> {
        if self.client.is_none() && !self.client_failed {
            // A cover address may hold a token: no more than 3 redirects, and
            // none hands it on as `Referer`.
            let built = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(crate::APP_ID)
                .redirect(reqwest::redirect::Policy::limited(3))
                .referer(false)
                .build();
            match built {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    self.client_failed = true;
                    tracing::warn!("cover: no HTTP client ({e}) - the placeholders show");
                }
            }
        }
        self.client.clone()
    }
}

/// The cover from its file when that decodes, else from the web (kept as a
/// file only once it decodes), made in `key.size`. None for no cover.
async fn load_cover(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
    key: CoverKey,
    downloads: &Semaphore,
    reads: &Semaphore,
) -> Option<egui::ColorImage> {
    let stem = format!("{:016x}", key.hash);
    {
        let _place = reads.acquire().await.ok()?;
        let (dir, stem) = (dir.to_path_buf(), stem.clone());
        let kept = tokio::task::spawn_blocking(move || read_kept(&dir, &stem, key.size))
            .await
            .ok()
            .flatten();
        if kept.is_some() {
            return kept;
        }
    }
    let host = crate::guide::logos::host_of(url).to_owned();
    let bytes = {
        let _place = downloads.acquire().await.ok()?;
        download(client, url).await
    };
    let Some(bytes) = bytes else {
        tracing::debug!("cover: {host} could not be fetched - the placeholder shows");
        return None;
    };
    let dir = dir.to_path_buf();
    let image = tokio::task::spawn_blocking(move || keep(&bytes, &dir, &stem, key.size))
        .await
        .ok()
        .flatten();
    if image.is_none() {
        tracing::debug!("cover: {host} is no JPEG or PNG - the placeholder shows");
    }
    image
}

/// The cover in a kept file (`<stem>.jpg` or `<stem>.png`), made in `size`;
/// none when there is none or it does not decode.
fn read_kept(dir: &Path, stem: &str, size: [u32; 2]) -> Option<egui::ColorImage> {
    ["jpg", "png"].iter().find_map(|ext| {
        let bytes = std::fs::read(dir.join(format!("{stem}.{ext}"))).ok()?;
        decode(&bytes, size)
    })
}

/// Decodes a downloaded body and keeps it as a file, but only when it decodes:
/// a kept file is trusted next time. A file that cannot be written costs the
/// next session a download, not this cover.
fn keep(bytes: &[u8], dir: &Path, stem: &str, size: [u32; 2]) -> Option<egui::ColorImage> {
    let (_, ext) = format_of(bytes)?;
    let image = decode(bytes, size)?;
    let _ = std::fs::create_dir_all(dir);
    if let Err(e) = std::fs::write(dir.join(format!("{stem}.{ext}")), bytes) {
        tracing::debug!("cover: could not be kept as a file ({e})");
    }
    Some(image)
}

/// JPEG or PNG, by its first bytes; none for anything else.
fn format_of(bytes: &[u8]) -> Option<(image::ImageFormat, &'static str)> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some((image::ImageFormat::Jpeg, "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some((image::ImageFormat::Png, "png"))
    } else {
        None
    }
}

/// A JPEG or PNG made to fill `size` exactly (cut to its shape at the middle);
/// none for anything else, and for a canvas over `MAX_CANVAS` either way.
pub fn decode(bytes: &[u8], size: [u32; 2]) -> Option<egui::ColorImage> {
    let (format, _) = format_of(bytes)?;
    let mut reader = image::io::Reader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(MAX_CANVAS);
    limits.max_image_height = Some(MAX_CANVAS);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let filled = image.resize_to_fill(size[0], size[1], image::imageops::FilterType::CatmullRom);
    let rgba = filled.into_rgba8();
    let dims = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        dims,
        rgba.as_raw(),
    ))
}

/// `url`'s body, at most [`MAX_BYTES`]; none on an error status, a failure,
/// or more.
async fn download(client: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_BYTES {
            return None;
        }
    }
    Some(bytes)
}

/// Removes the oldest files of `dir` (by when they were written) until it
/// holds `cap` bytes at most; how many went and the bytes left. A missing
/// folder holds nothing.
pub fn prune(dir: &Path, cap: u64) -> std::io::Result<(usize, u64)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, meta.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(written, _, _)| *written);
    let mut removed = 0;
    for (_, len, path) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    Ok((removed, total))
}

/// [`prune`] of `dir` to [`DISK_BYTES`] on the blocking pool, with a line in
/// the log when it removed any: the app runs it once at a start.
pub fn prune_in_background(dir: PathBuf) {
    tokio::task::spawn_blocking(move || match prune(&dir, DISK_BYTES) {
        Ok((0, _)) => {}
        Ok((n, left)) => tracing::info!(
            "covers: {n} oldest files removed, {} MB left",
            left / (1024 * 1024)
        ),
        Err(e) => tracing::warn!("covers: folder not pruned ({e})"),
    });
}
