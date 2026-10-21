//! The cover store's log lines (spec 2026-10-15 §4.4): "Een cover-adres met de
//! inloggegevens van de portal erin wordt nooit gelogd." This test is alone in
//! its file on purpose: it installs the process's one subscriber, so that the
//! lines written on the blocking pool are collected too, and that can only be a
//! test that has its binary to itself. (Sharing a binary, a scoped subscriber
//! lost a line in 19 of 20 runs: another test reached the call site first.)
//! Every address and secret below is made up, and every server is local.

use bongo_player::vod::covers::{CoverStore, CoverTexture, Shape, Uploader};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A texture that only says it is there.
struct NoTexture;

impl CoverTexture for NoTexture {
    fn byte_size(&self) -> usize {
        0
    }
}

struct NoUploader;

impl Uploader for NoUploader {
    type Texture = NoTexture;

    fn upload(&self, _name: String, _image: egui::ColorImage) -> NoTexture {
        NoTexture
    }
}

/// What a tracing subscriber writes, collected.
#[derive(Clone, Default)]
struct LogText(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogText {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogText {
    type Writer = LogText;

    fn make_writer(&'a self) -> LogText {
        self.clone()
    }
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageOutputFormat::Jpeg(80))
        .unwrap();
    out.into_inner()
}

/// Asks for these covers, frame after frame, until `done` holds: every 5 ms for
/// the first 2 s, every 50 ms after, 20 s at most.
async fn frames_until(
    store: &mut CoverStore<NoUploader>,
    urls: &[String],
    done: impl Fn(&mut CoverStore<NoUploader>) -> bool,
) {
    for frame in 0..760 {
        for url in urls {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        if frame == 0 {
            // Nothing has had a chance to run yet: what was asked for is on its way.
            for url in urls {
                assert!(
                    store.is_loading(url, Shape::Poster, [300.0, 450.0])
                        || store.cover(url, Shape::Poster, [300.0, 450.0]).is_some(),
                    "asked for in a frame, on its way in the next"
                );
            }
        }
        if done(store) {
            return;
        }
        let gap = if frame < 400 { 5 } else { 50 };
        tokio::time::sleep(Duration::from_millis(gap)).await;
    }
    panic!("the covers never settled");
}

#[tokio::test]
async fn the_log_names_a_covers_host_and_nothing_else_of_its_address() {
    let log = LogText::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(log.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let server = MockServer::start().await;
    Mock::given(path("/busy/ACCTNAME/SECRETPASS/1.jpg"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(path("/html/ACCTNAME/SECRETPASS/2.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"<html>nope</html>".to_vec()))
        .mount(&server)
        .await;
    Mock::given(path("/ok/ACCTNAME/SECRETPASS/4.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(60, 90)))
        .mount(&server)
        .await;
    let host = server.uri().trim_start_matches("http://").to_string();
    let urls = [
        format!("http://USERX:PASSX@{host}/busy/ACCTNAME/SECRETPASS/1.jpg?token=SECRETTOKEN"),
        format!("http://{host}/html/ACCTNAME/SECRETPASS/2.jpg?token=SECRETTOKEN"),
        // Nothing listens on port 1.
        "http://127.0.0.1:1/closed/ACCTNAME/SECRETPASS/3.jpg?token=SECRETTOKEN".to_string(),
        // The URL parser reads a backslash as a slash, so what follows is a path.
        format!("http://{host}\\BACKUSER\\BACKPASS\\5.jpg?token=SECRETTOKEN"),
    ];
    let dir = tempfile::tempdir().unwrap();
    let mut store = CoverStore::new(dir.path().to_path_buf(), NoUploader);
    frames_until(&mut store, &urls, |s| {
        urls.iter()
            .all(|u| !s.is_loading(u, Shape::Poster, [300.0, 450.0]))
    })
    .await;
    // A picture that comes but cannot be kept as a file (where the folder
    // should be there is a file): it shows, and the line says no more of it.
    let file = dir.path().join("not-a-folder");
    std::fs::write(&file, b"x").unwrap();
    let unkept = [format!(
        "http://USERX:PASSX@{host}/ok/ACCTNAME/SECRETPASS/4.jpg?token=SECRETTOKEN"
    )];
    let mut store = CoverStore::new(file.join("covers"), NoUploader);
    frames_until(&mut store, &unkept, |s| {
        s.cover(&unkept[0], Shape::Poster, [300.0, 450.0]).is_some()
    })
    .await;
    let text = String::from_utf8(log.0.lock().clone()).unwrap();
    assert_eq!(
        text.lines().filter(|l| l.contains("cover:")).count(),
        5,
        "a line for each cover that did not come, one for the file that was not kept: {text}"
    );
    assert!(
        text.contains(&host) && text.contains("127.0.0.1:1 "),
        "the hosts are named"
    );
    assert!(
        text.contains("could not be kept as a file"),
        "the blocking pool's line is seen"
    );
    for secret in [
        "SECRETTOKEN",
        "SECRETPASS",
        "ACCTNAME",
        "USERX",
        "PASSX",
        "BACKUSER",
        "BACKPASS",
        "token=",
        ".jpg",
        "not-a-folder",
    ] {
        assert!(!text.contains(secret), "a line holds part of an address");
    }
}
