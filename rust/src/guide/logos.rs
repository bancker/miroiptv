//! Channel logos for the guide's column headers: fetched only for the columns
//! on screen, kept as files, decoded (PNG only), made small and uploaded once
//! as textures. Draws nothing itself.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Logos are made to fit this box (width, height in pixels).
pub const LOGO_BOX: [u32; 2] = [128, 48];
/// A logo bigger than this is no logo.
const MAX_BYTES: usize = 512 * 1024;
/// A logo whose canvas is wider or taller than this (pixels) is no logo.
const MAX_CANVAS: u32 = 4096;
/// Logos fetched at the same time.
const AT_ONCE: usize = 2;

enum Slot {
    /// Being fetched and decoded.
    Loading,
    /// Decoded, not uploaded yet.
    Decoded(egui::ColorImage),
    Ready(egui::TextureHandle),
    /// No logo: the header shows the name. Not tried again this session.
    Missing,
}

/// A logo that came: its URL, and its image when there is one to show.
type Arrived = (String, Option<egui::ColorImage>);

pub struct LogoStore {
    dir: PathBuf,
    client: reqwest::Client,
    slots: HashMap<String, Slot>,
    loading: usize,
    done: Arc<Mutex<Vec<Arrived>>>,
}

impl LogoStore {
    /// Logos kept as files in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(crate::APP_ID)
            .build()
            .unwrap_or_default();
        Self {
            dir,
            client,
            slots: HashMap::new(),
            loading: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The texture of `url`'s logo once it is there. A URL not seen before is
    /// asked for (at most two at a time; the others are asked for in a later
    /// frame). `None` while loading, and for no logo.
    pub fn texture(&mut self, ctx: &egui::Context, url: &str) -> Option<&egui::TextureHandle> {
        if !self.slots.contains_key(url) {
            if self.loading < AT_ONCE {
                self.request(url);
            }
            return None;
        }
        if matches!(self.slots.get(url), Some(Slot::Decoded(_))) {
            if let Some(Slot::Decoded(image)) = self.slots.remove(url) {
                let texture =
                    ctx.load_texture(format!("logo {url}"), image, egui::TextureOptions::LINEAR);
                self.slots.insert(url.to_owned(), Slot::Ready(texture));
            }
        }
        match self.slots.get(url) {
            Some(Slot::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    /// Whether `url`'s logo is still on its way (the header shows a dot).
    pub fn is_loading(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Loading))
    }

    /// Takes in the logos that came; once per frame.
    pub fn drain(&mut self) {
        for (url, image) in std::mem::take(&mut *self.done.lock()) {
            self.loading = self.loading.saturating_sub(1);
            self.slots
                .insert(url, image.map_or(Slot::Missing, Slot::Decoded));
        }
    }

    fn request(&mut self, url: &str) {
        self.slots.insert(url.to_owned(), Slot::Loading);
        self.loading += 1;
        let url = url.to_owned();
        let file = self.dir.join(file_name(&url));
        let (client, done) = (self.client.clone(), self.done.clone());
        tokio::spawn(async move {
            let image = load_logo(&client, &url, &file).await;
            done.lock().push((url, image));
        });
    }
}

/// The logo from its file, else from the web (then kept as a file); decoded
/// and made small. `None` when there is none to show.
async fn load_logo(client: &reqwest::Client, url: &str, file: &Path) -> Option<egui::ColorImage> {
    let host = host_of(url);
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(_) => {
            let Some(bytes) = download(client, url).await else {
                tracing::info!("logo: {host} could not be fetched - the name shows");
                return None;
            };
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(file, &bytes);
            bytes
        }
    };
    let image = decode(&bytes);
    match &image {
        Some(i) => tracing::info!("logo: {host} ready ({}x{})", i.size[0], i.size[1]),
        None => tracing::info!("logo: {host} is no PNG - the name shows"),
    }
    image
}

/// `url`'s body, at most [`MAX_BYTES`]; `None` on an error status, a failure,
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

/// A PNG made to fit [`LOGO_BOX`] (a smaller one keeps its size); `None` for
/// anything else, and for a canvas over `MAX_CANVAS` in either direction.
pub fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    // `MAX_BYTES` limits the file, not the canvas: a small PNG can claim
    // hundreds of megabytes of pixels. The decoder checks the claim before it
    // allocates them.
    let mut reader =
        image::io::Reader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(MAX_CANVAS);
    limits.max_image_height = Some(MAX_CANVAS);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    // Only shrink: `thumbnail` also scales a smaller image UP to fill the box
    // (64x32 would come out 96x48), but a logo that fits keeps its size, and
    // the header never enlarges a texture either.
    let fitted = if image.width() <= LOGO_BOX[0] && image.height() <= LOGO_BOX[1] {
        image
    } else {
        image.thumbnail(LOGO_BOX[0], LOGO_BOX[1])
    };
    let small = fitted.into_rgba8();
    let size = [small.width() as usize, small.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        small.as_raw(),
    ))
}

/// The file a logo is kept in: 16 hex digits of its URL's SHA-256.
pub fn file_name(url: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, url.as_bytes());
    let hex: String = digest.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{hex}.img")
}

/// The host (and port) of a URL, for the log: a logo URL may carry a token.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit('@').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageOutputFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_png_is_made_to_fit_the_header() {
        assert_eq!(decode(&png(256, 96)).unwrap().size, [128, 48]);
        let tall = decode(&png(40, 160)).unwrap();
        assert!(tall.size[0] <= 128 && tall.size[1] <= 48, "{:?}", tall.size);
    }

    #[test]
    fn a_png_that_fits_keeps_its_size() {
        assert_eq!(decode(&png(64, 32)).unwrap().size, [64, 32]);
        assert_eq!(decode(&png(128, 48)).unwrap().size, [128, 48]);
    }

    #[test]
    fn a_png_with_a_huge_canvas_is_no_logo() {
        assert!(decode(&png(4097, 2)).is_none());
    }

    #[test]
    fn anything_but_a_png_is_no_logo() {
        assert!(decode(b"GIF89a....").is_none());
        assert!(decode(b"\x89PNG\r\n\x1a\nbroken").is_none());
        assert!(decode(b"").is_none());
    }

    #[test]
    fn a_logo_file_is_named_after_its_url() {
        let a = file_name("http://logos.example/npo1.png");
        assert_eq!(a.len(), 16 + ".img".len());
        assert!(a.ends_with(".img"));
        assert_eq!(a, file_name("http://logos.example/npo1.png"));
        assert_ne!(a, file_name("http://logos.example/npo2.png"));
    }

    #[test]
    fn only_the_host_of_a_logo_url_is_logged() {
        assert_eq!(
            host_of("http://user:pw@logos.example:8080/a.png?t=1"),
            "logos.example:8080"
        );
        assert_eq!(host_of("https://cdn.example/x/y.png"), "cdn.example");
        assert_eq!(host_of("no-scheme/x.png"), "no-scheme");
    }

    /// Takes results in until `url`'s texture is there; its size.
    async fn ready(logos: &mut LogoStore, ctx: &egui::Context, url: &str) -> [usize; 2] {
        for _ in 0..400 {
            logos.drain();
            if let Some(t) = logos.texture(ctx, url) {
                return t.size();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the logo never came");
    }

    #[tokio::test]
    async fn a_logo_is_fetched_once_kept_as_a_file_and_read_from_it_next_time() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/npo1.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(path("/npo1.gif"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"GIF89a".to_vec()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let url = format!("{}/npo1.png", server.uri());
        let gif = format!("{}/npo1.gif", server.uri());

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        assert!(
            logos.texture(&ctx, &url).is_none(),
            "asked for, not there yet"
        );
        assert!(logos.is_loading(&url));
        assert!(logos.texture(&ctx, &gif).is_none());
        assert_eq!(ready(&mut logos, &ctx, &url).await, [64, 32]);
        for _ in 0..400 {
            logos.drain();
            if !logos.is_loading(&gif) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            logos.texture(&ctx, &gif).is_none(),
            "a GIF shows the name instead"
        );

        // A new session reads the file: the server is asked once in all
        // (`expect(1)` is checked when the server goes).
        let mut again = LogoStore::new(dir.path().to_path_buf());
        again.texture(&ctx, &url);
        assert_eq!(ready(&mut again, &ctx, &url).await, [64, 32]);
    }
}
