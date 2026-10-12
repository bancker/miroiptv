//! Channel logos for the guide's column headers: fetched only for the columns
//! on screen, kept as files (only what decodes), decoded (PNG only), made small
//! and uploaded once as textures. Draws nothing itself.

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
/// Logos downloaded at the same time. A logo read from its kept file does not
/// count.
const AT_ONCE: usize = 2;

enum Slot {
    /// Has to be downloaded, and both places are taken: a later `texture` call
    /// starts it when one is free.
    Waiting,
    /// Being read from its file or fetched, and decoded.
    Loading,
    /// Decoded, not uploaded yet.
    Decoded(egui::ColorImage),
    Ready(egui::TextureHandle),
    /// No logo: the header shows the name. Not tried again this session.
    Missing,
}

/// A logo that came.
struct Arrived {
    url: String,
    /// Its image, when there is one to show.
    image: Option<egui::ColorImage>,
    /// Whether it held one of the `AT_ONCE` places while it was on its way.
    counted: bool,
}

pub struct LogoStore {
    dir: PathBuf,
    /// Built when the first logo is asked for, not before: a session that
    /// never opens the guide pays nothing for it.
    client: Option<reqwest::Client>,
    /// The client could not be built (warned once): no logo comes then.
    client_failed: bool,
    slots: HashMap<String, Slot>,
    /// Downloads on their way: they hold the `AT_ONCE` places.
    loading: usize,
    done: Arc<Mutex<Vec<Arrived>>>,
}

impl LogoStore {
    /// Logos kept as files in `dir`. Nothing is built or read yet.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            client: None,
            client_failed: false,
            slots: HashMap::new(),
            loading: 0,
            done: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The texture of `url`'s logo once it is there. A URL not seen before is
    /// asked for: a logo whose file is kept is read at once, one that has to
    /// be downloaded takes one of two places (the others are asked for in a
    /// later frame). `None` while loading, and for no logo.
    pub fn texture(&mut self, ctx: &egui::Context, url: &str) -> Option<&egui::TextureHandle> {
        match self.slots.get(url) {
            None => {
                self.request(url);
                return None;
            }
            Some(Slot::Waiting) => {
                self.start_waiting(url);
                return None;
            }
            Some(_) => {}
        }
        if matches!(self.slots.get(url), Some(Slot::Decoded(_))) {
            if let Some(Slot::Decoded(image)) = self.slots.remove(url) {
                // Named after the file, not the URL: a URL may carry a token.
                let name = format!("logo {}", file_name(url));
                let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                self.slots.insert(url.to_owned(), Slot::Ready(texture));
            }
        }
        match self.slots.get(url) {
            Some(Slot::Ready(texture)) => Some(texture),
            _ => None,
        }
    }

    /// Whether `url`'s logo is still on its way (the header shows a dot): being
    /// read from its file or downloaded.
    pub fn is_loading(&self, url: &str) -> bool {
        matches!(self.slots.get(url), Some(Slot::Loading))
    }

    /// Takes in the logos that came; once per frame.
    pub fn drain(&mut self) {
        for Arrived {
            url,
            image,
            counted,
        } in std::mem::take(&mut *self.done.lock())
        {
            if counted {
                self.loading = self.loading.saturating_sub(1);
            }
            self.slots
                .insert(url, image.map_or(Slot::Missing, Slot::Decoded));
        }
    }

    /// A URL seen for the first time. One stat tells whether its file is kept,
    /// once per URL: a download that has to wait is remembered as `Waiting`
    /// and not looked at again. Only a download takes one of the `AT_ONCE`
    /// places, so the logos kept as files all start in the frame that asks for
    /// them (a warm cache must not appear two by two).
    fn request(&mut self, url: &str) {
        let file = self.dir.join(file_name(url));
        if file.exists() {
            self.start(url, file, false);
        } else if self.loading < AT_ONCE {
            self.start(url, file, true);
        } else {
            self.slots.insert(url.to_owned(), Slot::Waiting);
        }
    }

    /// A download that waited goes as soon as a place is free.
    fn start_waiting(&mut self, url: &str) {
        if self.loading < AT_ONCE {
            let file = self.dir.join(file_name(url));
            self.start(url, file, true);
        }
    }

    /// Reads or fetches `url`'s logo in the background. `counted`: it holds one
    /// of the `AT_ONCE` places until it arrives.
    fn start(&mut self, url: &str, file: PathBuf, counted: bool) {
        let Some(client) = self.http_client() else {
            self.slots.insert(url.to_owned(), Slot::Missing);
            return;
        };
        self.slots.insert(url.to_owned(), Slot::Loading);
        if counted {
            self.loading += 1;
        }
        let url = url.to_owned();
        let done = self.done.clone();
        tokio::spawn(async move {
            let image = load_logo(&client, &url, &file).await;
            done.lock().push(Arrived {
                url,
                image,
                counted,
            });
        });
    }

    /// The HTTP client, built on first use. `None` when it cannot be built
    /// (warned once): the names show instead of logos.
    fn http_client(&mut self) -> Option<reqwest::Client> {
        if self.client.is_none() && !self.client_failed {
            // A logo URL may carry a token: no more than 3 redirects, and none
            // hands the URL on as `Referer`.
            let built = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(crate::APP_ID)
                .redirect(reqwest::redirect::Policy::limited(3))
                .referer(false)
                .build();
            match built {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    self.client_failed = true;
                    tracing::warn!("logo: no HTTP client ({e}) - the names show");
                }
            }
        }
        self.client.clone()
    }
}

/// The logo from its kept file when that decodes, else from the web (kept as a
/// file only once it decodes); made small. `None` when there is none to show.
/// The file work and the decoding run on the blocking pool, off the async
/// workers.
async fn load_logo(client: &reqwest::Client, url: &str, file: &Path) -> Option<egui::ColorImage> {
    let host = host_of(url);
    let kept = file.to_path_buf();
    let mut image = blocking(move || read_kept(&kept)).await;
    if image.is_none() {
        // No file, or one that is no logo (an earlier session kept a bad
        // body): fetch it, and the good body replaces the file.
        let Some(bytes) = download(client, url).await else {
            tracing::info!("logo: {host} could not be fetched - the name shows");
            return None;
        };
        let (path, name) = (file.to_path_buf(), host.to_owned());
        image = blocking(move || keep(&bytes, &path, &name)).await;
    }
    match &image {
        Some(i) => tracing::info!("logo: {host} ready ({}x{})", i.size[0], i.size[1]),
        None => tracing::info!("logo: {host} is no PNG - the name shows"),
    }
    image
}

/// Runs `work` on the blocking pool. `None` when it gave none or did not
/// finish.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok().flatten()
}

/// The logo in a kept file; `None` when there is no file or it does not decode.
fn read_kept(file: &Path) -> Option<egui::ColorImage> {
    decode(&std::fs::read(file).ok()?)
}

/// Decodes a downloaded body and keeps it as `file`, but only when it decodes:
/// a kept file is trusted next time, so a bad body (an error page served with
/// 200, a GIF, a cut-off download) must never become one. A file that cannot be
/// written costs the next session a download, not this logo.
fn keep(bytes: &[u8], file: &Path, host: &str) -> Option<egui::ColorImage> {
    let image = decode(bytes)?;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(file, bytes) {
        tracing::warn!("logo: {host} could not be kept as a file ({e})");
    }
    Some(image)
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

    /// Takes results in until `url` is no longer on its way.
    async fn settled(logos: &mut LogoStore, url: &str) {
        for _ in 0..400 {
            logos.drain();
            if !logos.is_loading(url) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(!logos.is_loading(url), "the answer for {url} never came");
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
            .expect(1)
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
        settled(&mut logos, &gif).await;
        assert!(
            !dir.path().join(file_name(&gif)).exists(),
            "a body that is no PNG is not kept"
        );
        assert!(
            logos.texture(&ctx, &gif).is_none(),
            "a GIF shows the name instead"
        );
        // A GIF is asked for once in a session, not again by the `texture` call
        // above (`expect(1)`, checked as the server goes). Were it asked again,
        // this waits for the answer, so that request has arrived by then.
        settled(&mut logos, &gif).await;

        // A new session reads the file: the server is asked once in all
        // (`expect(1)` is checked when the server goes).
        let mut again = LogoStore::new(dir.path().to_path_buf());
        again.texture(&ctx, &url);
        assert_eq!(ready(&mut again, &ctx, &url).await, [64, 32]);
    }

    #[tokio::test]
    async fn a_kept_file_that_is_no_logo_is_fetched_again_and_replaced() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/npo1.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let url = format!("{}/npo1.png", server.uri());
        // An error page served with 200 was kept once, as whatever came was.
        let file = dir.path().join(file_name(&url));
        std::fs::write(&file, b"<html>not a logo</html>").unwrap();

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        logos.texture(&ctx, &url);
        assert_eq!(ready(&mut logos, &ctx, &url).await, [64, 32]);
        assert_eq!(
            std::fs::read(&file).unwrap(),
            png(64, 32),
            "the file now holds the PNG"
        );
    }

    #[tokio::test]
    async fn kept_logos_are_all_started_in_the_frame_that_asks_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let urls: Vec<String> = (1..=3)
            .map(|n| format!("http://logos.example/{n}.png"))
            .collect();
        for url in &urls {
            std::fs::write(dir.path().join(file_name(url)), png(64, 32)).unwrap();
        }

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        // One frame: every column asks.
        for url in &urls {
            assert!(logos.texture(&ctx, url).is_none());
        }
        for url in &urls {
            assert!(logos.is_loading(url), "{url} waits for a later frame");
        }
        for url in &urls {
            assert_eq!(ready(&mut logos, &ctx, url).await, [64, 32]);
        }
    }

    #[tokio::test]
    async fn downloads_stay_two_at_a_time_and_a_kept_logo_does_not_wait_behind_them() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let [a, b, c, kept] = ["a", "b", "c", "kept"].map(|n| format!("{}/{n}.png", server.uri()));
        std::fs::write(dir.path().join(file_name(&kept)), png(32, 16)).unwrap();

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        for url in [&a, &b, &c, &kept] {
            logos.texture(&ctx, url);
        }
        assert!(logos.is_loading(&a) && logos.is_loading(&b));
        assert!(!logos.is_loading(&c), "a third download waits for a place");
        assert!(
            matches!(logos.slots.get(&c), Some(Slot::Waiting)),
            "it is remembered, so its file is not looked at again every frame"
        );
        assert!(logos.is_loading(&kept), "a kept logo needs no place");
        assert_eq!(ready(&mut logos, &ctx, &kept).await, [32, 16]);
        assert_eq!(ready(&mut logos, &ctx, &a).await, [64, 32]);
        assert_eq!(ready(&mut logos, &ctx, &b).await, [64, 32]);
        // The two places are free again: the third goes now.
        logos.texture(&ctx, &c);
        assert!(logos.is_loading(&c));
        assert_eq!(ready(&mut logos, &ctx, &c).await, [64, 32]);
    }

    /// `from` answers with a redirect to `to`.
    fn redirect(from: &str, to: &str) -> wiremock::Mock {
        wiremock::Mock::given(wiremock::matchers::path(from))
            .respond_with(wiremock::ResponseTemplate::new(302).insert_header("Location", to))
    }

    #[tokio::test]
    async fn a_redirect_does_not_hand_the_logo_url_on_as_referer() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        redirect("/old.png", "/new.png").mount(&server).await;
        Mock::given(path("/new.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        // A token in the query: where the logo is sent on, it must not follow.
        let url = format!("{}/old.png?token=secret", server.uri());

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        logos.texture(&ctx, &url);
        assert_eq!(ready(&mut logos, &ctx, &url).await, [64, 32]);
        let seen = server.received_requests().await.unwrap();
        let second = seen.iter().find(|r| r.url.path() == "/new.png");
        let second = second.expect("the redirect was followed");
        assert!(
            second.headers.get("referer").is_none(),
            "{:?}",
            second.headers
        );
    }

    #[tokio::test]
    async fn a_logo_url_is_followed_through_three_redirects_and_no_further() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        for (from, to) in [
            ("/r1", "/r2"),
            ("/r2", "/r3"),
            ("/r3", "/r4"),
            ("/r4", "/logo.png"),
        ] {
            redirect(from, to).mount(&server).await;
        }
        Mock::given(path("/logo.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let three = format!("{}/r2", server.uri());
        let four = format!("{}/r1", server.uri());

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        logos.texture(&ctx, &three);
        logos.texture(&ctx, &four);
        assert_eq!(ready(&mut logos, &ctx, &three).await, [64, 32]);
        settled(&mut logos, &four).await;
        assert!(
            logos.texture(&ctx, &four).is_none(),
            "four redirects are one too many"
        );
    }

    #[tokio::test]
    async fn a_texture_is_not_named_after_its_url() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        // A token in the URL stays out of egui's structures.
        let url = "http://logos.example/a.png?token=secret";
        std::fs::write(dir.path().join(file_name(url)), png(64, 32)).unwrap();

        let mut logos = LogoStore::new(dir.path().to_path_buf());
        logos.texture(&ctx, url);
        ready(&mut logos, &ctx, url).await;
        let name = logos.texture(&ctx, url).unwrap().name();
        assert_eq!(name, format!("logo {}", file_name(url)));
        assert!(!name.contains("secret"), "{name}");
    }

    #[tokio::test]
    async fn a_cache_that_cannot_be_written_still_shows_the_logo() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/npo1.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png(64, 32)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let url = format!("{}/npo1.png", server.uri());
        // A file where the cache folder should be: nothing can be kept under it.
        let blocked = dir.path().join("logos");
        std::fs::write(&blocked, b"a file, not a folder").unwrap();

        let mut logos = LogoStore::new(blocked);
        logos.texture(&ctx, &url);
        assert_eq!(ready(&mut logos, &ctx, &url).await, [64, 32]);
    }

    #[tokio::test]
    async fn the_http_client_is_built_when_the_first_logo_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut logos = LogoStore::new(dir.path().to_path_buf());
        assert!(logos.client.is_none(), "a store nobody asks builds nothing");
        logos.texture(&ctx, "http://logos.example/a.png");
        assert!(logos.client.is_some());
    }
}
