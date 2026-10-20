//! Covers (vod/covers.rs, spec 2026-10-15 §4.4): the sizes, the texture LRU
//! with fake textures, fetching, keeping, decoding and dropping covers, the
//! disk cache's cap, and the placeholder's colour. Every server is local.

use bongo_player::vod::covers::{
    decode, file_stem, placeholder_tint, prune, CoverKey, CoverStore, CoverTexture, Shape,
    TextureLru, Uploader, AT_ONCE,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MB: usize = 1024 * 1024;

/// A texture that only says its size: no GPU, no memory.
#[derive(Debug, Clone)]
struct FakeTexture {
    name: String,
    size: [usize; 2],
    bytes: usize,
}

impl CoverTexture for FakeTexture {
    fn byte_size(&self) -> usize {
        self.bytes
    }
}

/// Makes fake textures and remembers their names.
#[derive(Clone, Default)]
struct FakeUploader {
    made: Arc<Mutex<Vec<String>>>,
}

impl Uploader for FakeUploader {
    type Texture = FakeTexture;

    fn upload(&self, name: String, image: egui::ColorImage) -> FakeTexture {
        self.made.lock().push(name.clone());
        let [w, h] = image.size;
        FakeTexture {
            name,
            size: image.size,
            bytes: w * h * 4,
        }
    }
}

fn fake(mb: usize) -> FakeTexture {
    FakeTexture {
        name: String::new(),
        size: [0, 0],
        bytes: mb * MB,
    }
}

fn key(n: u64) -> CoverKey {
    CoverKey {
        hash: n,
        size: [300, 450],
    }
}

fn picture(w: u32, h: u32, format: image::ImageOutputFormat) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    picture(w, h, image::ImageOutputFormat::Jpeg(80))
}

fn png(w: u32, h: u32) -> Vec<u8> {
    picture(w, h, image::ImageOutputFormat::Png)
}

#[test]
fn a_cover_is_made_in_the_first_size_at_least_as_big_as_its_tile() {
    assert_eq!(Shape::Poster.size_for([150.0, 225.0]), [200, 300]);
    assert_eq!(Shape::Poster.size_for([200.0, 300.0]), [200, 300]);
    assert_eq!(Shape::Poster.size_for([201.0, 301.0]), [300, 450]);
    assert_eq!(
        Shape::Poster.size_for([900.0, 1350.0]),
        [400, 600],
        "the largest"
    );
    assert_eq!(Shape::Landscape.size_for([400.0, 225.0]), [480, 270]);
    assert_eq!(Shape::Backdrop.size_for([1300.0, 700.0]), [1920, 1080]);
}

#[test]
fn the_textures_keep_96_mb_and_the_one_shown_longest_ago_goes_first() {
    let mut lru = TextureLru::new(96 * MB);
    lru.insert(key(1), fake(30));
    lru.insert(key(2), fake(30));
    lru.insert(key(3), fake(30));
    assert!(lru.touch(&key(1)), "shown again");
    lru.insert(key(4), fake(30));
    assert_eq!(lru.used(), 90 * MB);
    assert!(lru.peek(&key(2)).is_none(), "shown longest ago");
    assert!(
        lru.peek(&key(1)).is_some() && lru.peek(&key(3)).is_some() && lru.peek(&key(4)).is_some()
    );
    assert!(!lru.touch(&key(2)));
    // One too big for the cap alone stays alone.
    lru.insert(key(5), fake(100));
    assert_eq!(lru.len(), 1);
    assert_eq!(lru.used(), 100 * MB);
    // The same key again replaces its texture.
    lru.insert(key(5), fake(10));
    assert_eq!((lru.len(), lru.used()), (1, 10 * MB));
}

#[test]
fn jpeg_and_png_are_made_to_fill_the_size_and_anything_else_is_no_cover() {
    for bytes in [jpeg(600, 900), png(640, 900), jpeg(100, 100)] {
        let image = decode(&bytes, [300, 450]).unwrap();
        assert_eq!(image.size, [300, 450]);
    }
    assert!(decode(b"GIF89a....", [300, 450]).is_none());
    assert!(decode(b"<html>busy</html>", [300, 450]).is_none());
    assert!(
        decode(&[0xff, 0xd8, 0xff, 0x00], [300, 450]).is_none(),
        "a cut-off JPEG"
    );
    assert!(
        decode(&png(8193, 2), [300, 450]).is_none(),
        "a canvas too big"
    );
}

#[test]
fn a_title_without_a_cover_always_gets_its_own_colour() {
    let a = placeholder_tint("The Matrix");
    assert_eq!(a, placeholder_tint("The Matrix"));
    assert_ne!(a, placeholder_tint("Dark"));
    assert_eq!(a.a(), 255);
    assert!(
        a.r() < 160 && a.g() < 160 && a.b() < 160,
        "dark enough for white letters: {a:?}"
    );
}

#[test]
fn a_cover_file_is_named_after_a_hash_of_its_address() {
    let url = "http://h:80/images/u/p/1.jpg?token=secret";
    let stem = file_stem(url);
    assert_eq!(stem.len(), 16);
    assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(stem, file_stem(url));
    assert_ne!(stem, file_stem("http://h:80/images/u/p/2.jpg"));
}

/// Ends frames, each asking for these covers again, until `done` says so.
async fn frames_until(
    store: &mut CoverStore<FakeUploader>,
    ask: &[String],
    mut done: impl FnMut(&mut CoverStore<FakeUploader>) -> bool,
) {
    for _ in 0..400 {
        for url in ask {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        if done(store) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the covers never came");
}

fn shown(store: &mut CoverStore<FakeUploader>, url: &str) -> Option<FakeTexture> {
    store.cover(url, Shape::Poster, [300.0, 450.0]).cloned()
}

#[tokio::test]
async fn a_cover_is_fetched_once_kept_as_its_bytes_and_read_from_the_file_next_time() {
    let server = MockServer::start().await;
    Mock::given(path("/c.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(600, 900)))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/c.jpg?token=secret", server.uri());
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    assert!(
        shown(&mut store, &url).is_none(),
        "asked for, not there yet"
    );
    let ask = [url.clone()];
    frames_until(&mut store, &ask, |s| shown(s, &url).is_some()).await;
    let texture = shown(&mut store, &url).unwrap();
    assert_eq!(texture.size, [300, 450]);
    assert_eq!(
        texture.name,
        format!("cover {} 300x450", file_stem(&url)),
        "no address in it"
    );
    let kept = dir.path().join(format!("{}.jpg", file_stem(&url)));
    assert_eq!(
        std::fs::read(&kept).unwrap(),
        jpeg(600, 900),
        "the bytes as they came"
    );
    // Another session, another size: read from the file (expect(1) holds).
    let mut again = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    let big = [url.clone()];
    for _ in 0..400 {
        if again
            .cover(&big[0], Shape::Poster, [400.0, 600.0])
            .is_some()
        {
            break;
        }
        again.end_frame();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        again
            .cover(&big[0], Shape::Poster, [400.0, 600.0])
            .unwrap()
            .size,
        [400, 600]
    );
}

#[tokio::test]
async fn an_error_page_or_a_gif_is_no_cover_is_not_kept_and_is_not_asked_again() {
    let server = MockServer::start().await;
    Mock::given(path("/a.gif"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"GIF89a....".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/busy.jpg"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let urls = [
        format!("{}/a.gif", server.uri()),
        format!("{}/busy.jpg", server.uri()),
    ];
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    frames_until(&mut store, &urls, |s| {
        urls.iter()
            .all(|u| !s.is_loading(u, Shape::Poster, [300.0, 450.0]))
    })
    .await;
    for _ in 0..5 {
        for url in &urls {
            assert!(store.cover(url, Shape::Poster, [300.0, 450.0]).is_none());
        }
        store.end_frame();
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, |d| d.count()),
        0,
        "nothing kept"
    );
}

#[tokio::test]
async fn six_downloads_at_a_time_and_a_cover_that_leaves_the_screen_is_dropped() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(jpeg(60, 90))
                .set_delay(Duration::from_millis(600)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..10)
        .map(|i| format!("{}/{i}.jpg", server.uri()))
        .collect();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    for url in &urls {
        store.cover(url, Shape::Poster, [300.0, 450.0]);
    }
    store.end_frame();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(store.downloads_in_flight(), AT_ONCE);
    // The page scrolls on: no frame asks for these any more.
    store.end_frame();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.downloads_in_flight(), 0, "dropped, their places free");
    assert!(urls
        .iter()
        .all(|u| !store.is_loading(u, Shape::Poster, [300.0, 450.0])));
}

#[tokio::test]
async fn no_more_than_eight_textures_are_made_in_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..12)
        .map(|i| format!("http://covers.example/{i}.png"))
        .collect();
    for url in &urls {
        std::fs::write(
            dir.path().join(format!("{}.png", file_stem(url))),
            png(60, 90),
        )
        .unwrap();
    }
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    let frame = |store: &mut CoverStore<FakeUploader>| {
        for url in &urls {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
    };
    frame(&mut store);
    // Twelve small files are read and decoded well within this.
    tokio::time::sleep(Duration::from_millis(500)).await;
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 8, "one frame's worth");
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 12, "the rest in the next frame");
}

#[test]
fn the_folder_is_pruned_to_its_cap_the_oldest_files_first() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age_days) in [("old.jpg", 30u64), ("mid.jpg", 10), ("new.jpg", 1)] {
        let file = dir.path().join(name);
        std::fs::write(&file, vec![0u8; 1000]).unwrap();
        let written = now - Duration::from_secs(age_days * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(written)
            .unwrap();
    }
    assert_eq!(
        prune(dir.path(), 3000).unwrap(),
        (0, 3000),
        "within the cap"
    );
    assert_eq!(prune(dir.path(), 2500).unwrap(), (1, 2000));
    assert!(!dir.path().join("old.jpg").exists());
    assert!(dir.path().join("mid.jpg").exists() && dir.path().join("new.jpg").exists());
    assert_eq!(prune(&dir.path().join("missing"), 0).unwrap(), (0, 0));
}

#[tokio::test]
async fn a_kept_cover_needs_no_server_and_costs_its_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let url = "http://covers.example/kept.png".to_string();
    std::fs::write(
        dir.path().join(format!("{}.png", file_stem(&url))),
        png(60, 90),
    )
    .unwrap();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    assert_eq!(store.texture_bytes(), 0);
    frames_until(&mut store, std::slice::from_ref(&url), |s| {
        shown(s, &url).is_some()
    })
    .await;
    assert_eq!(store.texture_bytes(), 300 * 450 * 4);
}
