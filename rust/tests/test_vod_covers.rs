//! Covers (vod/covers.rs, spec 2026-10-15 §4.4): the sizes, the texture LRU
//! with fake textures, fetching, keeping, decoding and dropping covers, the
//! disk cache's cap, and the placeholder's colour. Every server is local.

use bongo_player::vod::covers::{
    decode, file_stem, placeholder_tint, prune, prune_in_background, url_hash, CoverKey,
    CoverStore, CoverTexture, Shape, TextureLru, Uploader, AT_ONCE, DISK_BYTES, READS_AT_ONCE,
    TEXTURE_BYTES, UPLOADS_PER_FRAME,
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

/// Makes fake textures and remembers their names. A `fat` one says every
/// texture costs 30 MB, whatever its picture: the cap is reached with a handful
/// of small files.
#[derive(Clone, Default)]
struct FakeUploader {
    made: Arc<Mutex<Vec<String>>>,
    fat: bool,
}

impl Uploader for FakeUploader {
    type Texture = FakeTexture;

    fn upload(&self, name: String, image: egui::ColorImage) -> FakeTexture {
        self.made.lock().push(name.clone());
        let [w, h] = image.size;
        FakeTexture {
            name,
            size: image.size,
            bytes: if self.fat { 30 * MB } else { w * h * 4 },
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

/// Polls until `ok` holds, for as long as a busy machine needs (`secs` at most):
/// a test waits for what it waits for, not for a time picked to suit a quick one.
async fn eventually(secs: u64, what: &str, mut ok: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !ok() {
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "never happened: {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// How long to rest after frame number `frame` of a wait that asks for covers
/// in every frame: 5 ms for the first 2 s, 50 ms after (20 s in 760 frames). A
/// store that forgot what was asked would start the work of every cover again in
/// every frame, and that work cannot be cancelled: the wait must not be a storm.
fn frame_gap(frame: usize) -> Duration {
    Duration::from_millis(if frame < 400 { 5 } else { 50 })
}

/// Waits until every task the store started has ended, its answer handed over.
/// Only for covers read from files: a download's connection outlives its answer
/// as a task of its own. The runtime of `#[tokio::test]` runs nothing else.
async fn the_store_is_idle() {
    let metrics = tokio::runtime::Handle::current().metrics();
    eventually(30, "the covers' tasks end", || {
        metrics.num_alive_tasks() == 0
    })
    .await;
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
fn the_sizes_are_the_spec_table_and_a_tile_has_to_fit_on_both_sides() {
    assert_eq!(Shape::Poster.sizes(), &[[200, 300], [300, 450], [400, 600]]);
    assert_eq!(
        Shape::Landscape.sizes(),
        &[[320, 180], [480, 270], [640, 360]]
    );
    assert_eq!(Shape::Backdrop.sizes(), &[[1280, 720], [1920, 1080]]);
    // Wide enough is not enough, nor is high enough.
    assert_eq!(Shape::Poster.size_for([150.0, 400.0]), [300, 450]);
    assert_eq!(Shape::Poster.size_for([250.0, 100.0]), [300, 450]);
    assert_eq!(Shape::Landscape.size_for([300.0, 200.0]), [480, 270]);
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
fn textures_that_fill_the_cap_exactly_are_not_over_it() {
    let mut lru = TextureLru::new(90 * MB);
    assert!(lru.is_empty());
    for n in 1..=3 {
        lru.insert(key(n), fake(30));
    }
    assert_eq!((lru.len(), lru.used()), (3, 90 * MB), "full is not over");
    assert!(!lru.is_empty());
    lru.insert(key(4), fake(1));
    assert_eq!((lru.len(), lru.used()), (3, 61 * MB), "the oldest went");
    assert!(lru.peek(&key(1)).is_none() && lru.peek(&key(4)).is_some());
}

#[test]
fn what_was_shown_in_order_goes_in_order_however_many_there_are() {
    // Touched one after the other, many at once: if a touch did not move the
    // clock they would all be the same age, and which one goes would be a toss-up.
    let mut lru = TextureLru::new(64 * MB);
    for n in 0..64 {
        lru.insert(key(n), fake(1));
    }
    for n in 0..64 {
        assert!(lru.touch(&key(n)), "{n} is there");
    }
    for n in 64..67 {
        lru.insert(key(n), fake(1));
    }
    for n in 0..3 {
        assert!(lru.peek(&key(n)).is_none(), "{n} was shown longest ago");
    }
    assert_eq!((lru.len(), lru.used()), (64, 64 * MB));
    for n in 3..67 {
        assert!(lru.peek(&key(n)).is_some(), "{n} stays");
    }
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
    // Over the limit and not a strip (a strip is turned down for its shape).
    assert!(
        decode(&png(8193, 600), [320, 180]).is_none(),
        "a canvas too big"
    );
}

#[test]
fn the_canvas_limit_is_8192_pixels_a_side_wide_or_high() {
    // None of these is a strip: they are turned down, or let through, for their canvas.
    let image = decode(&png(8192, 576), [320, 180]).unwrap();
    assert_eq!(image.size, [320, 180], "8192 wide is allowed");
    assert!(
        decode(&png(700, 8193), [300, 450]).is_none(),
        "8193 high is too high"
    );
}

#[test]
fn a_strip_is_no_cover_and_is_turned_down_before_a_pixel_is_read() {
    // 256 x 2 pixels (133 bytes) would be scaled up to 57,600 x 450 first (1 s
    // and 76 MB), 8192 x 2 (365 bytes) to 1,843,200 x 450 (a minute and a half, 2.4 GB).
    for (w, h) in [
        (256, 2),
        (2, 256),
        (8192, 2),
        (2, 8192),
        (8192, 1),
        (1, 8192),
    ] {
        assert!(decode(&png(w, h), [300, 450]).is_none(), "{w}x{h}");
    }
    // What fills a poster still does: a square, a banner, a landscape, one pixel.
    for (w, h) in [(1, 1), (900, 900), (2000, 500), (1920, 1080), (200, 300)] {
        let image = decode(&png(w, h), [300, 450]).unwrap_or_else(|| panic!("{w}x{h}"));
        assert_eq!(image.size, [300, 450], "{w}x{h}");
    }
}

#[test]
fn a_picture_is_a_strip_from_eight_times_the_area_of_its_size_on() {
    // Filling a 300 x 450 cover scales a picture by the larger of the two
    // ratios: the area it is scaled to is its shape's ratio to the size's shape.
    for (w, h, cover) in [
        (16, 3, true), // exactly 8: 16 x 450 = 8 x (3 x 300)
        (17, 3, false),
        (1, 12, true), // and the other way: 12 x 300 = 8 x (1 x 450)
        (1, 13, false),
    ] {
        assert_eq!(
            decode(&png(w, h), [300, 450]).is_some(),
            cover,
            "{w}x{h} for a poster"
        );
    }
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
fn a_title_keeps_its_colour_from_one_release_to_the_next() {
    // Ruling 43: the hue from the first two bytes of sha256(name), saturation
    // 0.45, value 0.35. One name for each sixth of the hue circle.
    for (name, [r, g, b]) in [
        ("Brazil", [89, 57, 49]),
        ("The Matrix", [69, 89, 49]),
        ("Dark", [49, 89, 60]),
        ("Cars", [49, 61, 89]),
        ("Avatar", [79, 49, 89]),
        ("Titanic", [89, 49, 88]),
        // Chosen because its red tells a hue of x / 65,536 from x / 65,535.
        ("Film 242", [60, 49, 89]),
    ] {
        assert_eq!(
            placeholder_tint(name),
            egui::Color32::from_rgb(r, g, b),
            "{name}"
        );
    }
}

#[test]
fn the_limits_are_the_specs() {
    // Spec 4.4 and 5: 6 downloads at a time, textures to 96 MB, the folder to
    // 500 MB; a screenful read from disk spreads over frames of 8 textures.
    assert_eq!(AT_ONCE, 6);
    assert_eq!(READS_AT_ONCE, 6);
    assert_eq!(UPLOADS_PER_FRAME, 8);
    assert_eq!(TEXTURE_BYTES, 96 * MB);
    assert_eq!(DISK_BYTES, 500 * MB as u64);
}

#[test]
fn the_apps_own_texture_costs_the_pixels_egui_keeps_and_has_the_name_it_was_given() {
    // egui's context is the app's `Uploader` and its handle the app's
    // `CoverTexture`; the LRU's 96 MB counts what they say. No GPU is needed.
    let ctx = egui::Context::default();
    let image = egui::ColorImage::new([30, 20], egui::Color32::RED);
    let texture = ctx.upload("cover 0123456789abcdef 30x20".to_string(), image);
    // The handle has a `byte_size` of its own, which a method call would pick;
    // the LRU is generic and calls the trait's.
    let cost = CoverTexture::byte_size(&texture);
    assert_eq!(cost, 30 * 20 * 4);
    assert_eq!(cost, texture.byte_size(), "the same as egui's own");
    let manager = ctx.tex_manager();
    let manager = manager.read();
    let meta = manager.meta(texture.id()).unwrap();
    assert_eq!(meta.bytes_used(), cost, "as egui counts it");
    assert_eq!(meta.name, "cover 0123456789abcdef 30x20");
    assert_eq!(meta.options, egui::TextureOptions::LINEAR, "smooth");
}

#[test]
fn a_picture_wider_than_the_cover_is_cut_at_the_middle_not_squeezed() {
    // Red | green | blue, 200 x 300 each. Filling 300 x 450 scales it to 900 x
    // 450 and keeps the middle 300: the green band, whole.
    let mut img = image::RgbImage::new(600, 300);
    for (x, _, pixel) in img.enumerate_pixels_mut() {
        *pixel = match x / 200 {
            0 => image::Rgb([220, 20, 20]),
            1 => image::Rgb([20, 220, 20]),
            _ => image::Rgb([20, 20, 220]),
        };
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageOutputFormat::Png)
        .unwrap();
    let image = decode(&out.into_inner(), [300, 450]).unwrap();
    for (x, y) in [(2, 5), (150, 225), (297, 440)] {
        let pixel = image.pixels[y * 300 + x];
        assert!(
            pixel.g() > 150 && pixel.r() < 80 && pixel.b() < 80,
            "({x}, {y}) is green, not squeezed: {pixel:?}"
        );
    }
}

#[test]
fn a_half_transparent_picture_is_made_premultiplied_as_egui_has_it() {
    let img = image::RgbaImage::from_pixel(60, 90, image::Rgba([200, 100, 50, 128]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageOutputFormat::Png)
        .unwrap();
    let image = decode(&out.into_inner(), [300, 450]).unwrap();
    let pixel = image.pixels[225 * 300 + 150];
    // egui's own conversion (it premultiplies in linear light: 200 becomes 147).
    let want = egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128);
    assert_eq!(pixel.a(), 128);
    for (got, expected) in [
        (pixel.r(), want.r()),
        (pixel.g(), want.g()),
        (pixel.b(), want.b()),
    ] {
        assert!(got.abs_diff(expected) <= 1, "{pixel:?} is not {want:?}");
    }
}

#[test]
fn a_small_picture_is_made_big_smoothly_not_in_blocks() {
    // Black | white, 2 x 3 pixels, to 300 x 450: between them there are greys.
    let img = image::RgbImage::from_fn(2, 3, |x, _| image::Rgb([if x == 0 { 0 } else { 255 }; 3]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageOutputFormat::Png)
        .unwrap();
    let image = decode(&out.into_inner(), [300, 450]).unwrap();
    let row = &image.pixels[225 * 300..226 * 300];
    assert!(
        row.iter().any(|p| (20..235).contains(&p.r())),
        "an edge of greys"
    );
}

#[test]
fn every_colour_type_a_portal_serves_is_made() {
    use image::DynamicImage as D;
    let (w, h) = (40, 60);
    let pngs = [
        D::ImageLuma8(image::GrayImage::from_pixel(w, h, image::Luma([90]))),
        D::ImageLumaA8(image::GrayAlphaImage::from_pixel(
            w,
            h,
            image::LumaA([90, 200]),
        )),
        D::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([1, 2, 3]))),
        D::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([1, 2, 3, 250]),
        )),
        D::ImageLuma16(image::ImageBuffer::from_pixel(
            w,
            h,
            image::Luma([40_000u16]),
        )),
        D::ImageRgb16(image::ImageBuffer::from_pixel(
            w,
            h,
            image::Rgb([1u16, 2, 3]),
        )),
        D::ImageRgba16(image::ImageBuffer::from_pixel(
            w,
            h,
            image::Rgba([1u16, 2, 3, 65_000]),
        )),
    ];
    for (n, picture) in pngs.into_iter().enumerate() {
        let mut out = std::io::Cursor::new(Vec::new());
        picture
            .write_to(&mut out, image::ImageOutputFormat::Png)
            .unwrap();
        let image = decode(&out.into_inner(), [200, 300]);
        assert_eq!(image.map(|i| i.size), Some([200, 300]), "PNG kind {n}");
    }
    let gray = D::ImageLuma8(image::GrayImage::from_pixel(w, h, image::Luma([90])));
    let mut out = std::io::Cursor::new(Vec::new());
    gray.write_to(&mut out, image::ImageOutputFormat::Jpeg(80))
        .unwrap();
    assert!(decode(&out.into_inner(), [200, 300]).is_some(), "gray JPEG");
}

/// CRC-32 of a PNG chunk: no crate for it, and a test of a hand-made file needs
/// it right.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// A zlib stream of one stored block: no compressor needed.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    assert!(raw.len() <= 65_535);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in raw {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    let mut out = vec![0x78, 0x01, 0x01];
    out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    out.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    out.extend_from_slice(raw);
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}

/// An 8 x 8 palette picture (8 bits, indices 0 to 2) with `chunks` between its
/// header and its pixels, so that a test can say what a file can say.
fn paletted_png(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(png_chunk(b"IHDR", &[0, 0, 0, 8, 0, 0, 0, 8, 8, 3, 0, 0, 0]));
    for (kind, data) in chunks {
        out.extend(png_chunk(kind, data));
    }
    let rows: Vec<u8> = (0..8u8)
        .flat_map(|y| std::iter::once(0).chain((0..8u8).map(move |x| (x + y) % 3)))
        .collect();
    out.extend(png_chunk(b"IDAT", &zlib_stored(&rows)));
    out.extend(png_chunk(b"IEND", &[]));
    out
}

fn palette(entries: usize) -> Vec<u8> {
    (0..entries * 3).map(|i| (i * 7) as u8).collect()
}

#[test]
fn a_palette_the_png_decoder_would_panic_on_is_no_cover() {
    // png 0.17.16 indexes past a PLTE chunk that is no multiple of 3 bytes or
    // holds more than 256 entries, and the release build aborts on a panic. In
    // a test the panic would fail it, here.
    for len in [0usize, 1, 2, 4, 7, 10, 769, 771, 1200] {
        let bytes = paletted_png(&[(b"PLTE", (0..len).map(|i| i as u8).collect())]);
        assert!(decode(&bytes, [300, 450]).is_none(), "PLTE of {len} bytes");
    }
    // What a palette is: 1 to 256 entries, with or without transparency.
    for entries in [1usize, 3, 255, 256] {
        let bytes = paletted_png(&[(b"PLTE", palette(entries))]);
        let image = decode(&bytes, [300, 450]).unwrap_or_else(|| panic!("{entries} entries"));
        assert_eq!(image.size, [300, 450]);
    }
    let with_alpha = paletted_png(&[(b"PLTE", palette(3)), (b"tRNS", vec![128, 0])]);
    assert!(decode(&with_alpha, [300, 450]).is_some());
    // The decoder stops at IEND and so does the look: what follows is not read.
    let mut trailing = paletted_png(&[(b"PLTE", palette(3))]);
    trailing.extend(png_chunk(b"PLTE", &[1, 2, 3, 4, 5, 6, 7]));
    assert!(decode(&trailing, [300, 450]).is_some(), "after IEND");
}

#[test]
fn a_colour_profile_that_is_inflated_whole_is_no_cover_when_it_is_big() {
    // iCCP: a name, a NUL, a method, a zlib stream. The png decoder inflates it
    // at once: 1 MB of PNG made 529 MB of it. Real profiles are a few KB.
    let profile = |zlib: Vec<u8>| {
        let mut data = b"sRGB\0\0".to_vec();
        data.extend(zlib);
        data
    };
    let with = |data: Vec<u8>| paletted_png(&[(b"iCCP", data), (b"PLTE", palette(3))]);
    let real = with(profile(zlib_stored(&[0; 3144])));
    assert!(decode(&real, [300, 450]).is_some(), "a real profile");
    // 6 bytes of name and method, 11 of zlib around the stored bytes.
    let at_the_limit = with(profile(zlib_stored(&vec![0; 64 * 1024 - 6 - 11])));
    assert!(decode(&at_the_limit, [300, 450]).is_some(), "64 KB");
    let over = with(profile(zlib_stored(&vec![0; 64 * 1024 - 6 - 11 + 1])));
    assert!(decode(&over, [300, 450]).is_none(), "64 KB and a byte");
}

/// `jpeg` with a comment segment after its start marker that holds `n` of the
/// byte pairs that start a scan.
fn with_fake_scans(jpeg: &[u8], n: usize) -> Vec<u8> {
    let payload: Vec<u8> = (0..n).flat_map(|_| [0xff, 0xda]).collect();
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xff, 0xfe]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

#[test]
fn a_jpeg_of_more_than_64_scans_is_no_cover() {
    // The decoder goes over every block of the frame in each scan: 22 ms at
    // 8192 x 8192, and 5,000 scans (295 KB) took it two minutes. A progressive
    // JPEG has about ten.
    let plain = jpeg(60, 90);
    let own = plain.windows(2).filter(|w| *w == [0xff, 0xda]).count();
    assert_eq!(own, 1, "a baseline JPEG is one scan");
    assert!(decode(&with_fake_scans(&plain, 63), [300, 450]).is_some());
    assert!(decode(&with_fake_scans(&plain, 64), [300, 450]).is_none());
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

#[test]
fn the_name_is_the_first_16_hex_digits_of_the_sha256_of_the_address() {
    // sha256("abc") = ba7816bf 8f01cfea 414140de 5dae2223 ...: files kept by an
    // earlier release are found by this name, and by no other.
    assert_eq!(file_stem("abc"), "ba7816bf8f01cfea");
    assert_eq!(url_hash("abc"), 0xba78_16bf_8f01_cfea);
}

#[tokio::test]
async fn a_cover_whose_name_starts_with_zeros_is_found_and_made_under_the_same_name() {
    // The hash of this address starts with two zero digits: written with fewer
    // than 16 digits it would be another name.
    let url = "http://covers.example/zero-429.png".to_string();
    assert_eq!(file_stem(&url), "00912cdb40a0dc30");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("00912cdb40a0dc30.png"), png(60, 90)).unwrap();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    frames_until(&mut store, std::slice::from_ref(&url), |s| {
        shown(s, &url).is_some()
    })
    .await;
    assert_eq!(
        shown(&mut store, &url).unwrap().name,
        "cover 00912cdb40a0dc30 300x450"
    );
}

/// Ends frames, each asking for these covers again, until `done` says so (20 s
/// at most).
async fn frames_until(
    store: &mut CoverStore<FakeUploader>,
    ask: &[String],
    done: impl FnMut(&mut CoverStore<FakeUploader>) -> bool,
) {
    frames_within(20, store, ask, done).await;
}

/// [`frames_until`] for `secs` at most.
async fn frames_within(
    secs: u64,
    store: &mut CoverStore<FakeUploader>,
    ask: &[String],
    mut done: impl FnMut(&mut CoverStore<FakeUploader>) -> bool,
) {
    let started = std::time::Instant::now();
    for frame in 0.. {
        for url in ask {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        if frame == 0 {
            // Nothing has had a chance to run yet: what was asked for is on its
            // way, or already there.
            for url in ask {
                let on_its_way = store.is_loading(url, Shape::Poster, [300.0, 450.0]);
                assert!(
                    on_its_way || shown(store, url).is_some(),
                    "asked for in a frame, on its way in the next: {url}"
                );
            }
        }
        if done(store) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(secs),
            "the covers never came"
        );
        tokio::time::sleep(frame_gap(frame)).await;
    }
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
    for frame in 0..760 {
        if again
            .cover(&big[0], Shape::Poster, [400.0, 600.0])
            .is_some()
        {
            break;
        }
        again.end_frame();
        tokio::time::sleep(frame_gap(frame)).await;
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
async fn the_covers_folder_is_made_when_the_first_cover_is_kept_and_a_png_stays_a_png() {
    let server = MockServer::start().await;
    Mock::given(path("/a.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(60, 90)))
        .mount(&server)
        .await;
    Mock::given(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png(60, 90)))
        .mount(&server)
        .await;
    // As at a first start: `cache\covers` is not there.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("cache").join("covers");
    let urls = [
        format!("{}/a.jpg", server.uri()),
        format!("{}/b", server.uri()),
    ];
    let mut store = CoverStore::new(dir.clone(), FakeUploader::default());
    frames_until(&mut store, &urls, |s| {
        urls.iter().all(|u| shown(s, u).is_some())
    })
    .await;
    assert!(dir.join(format!("{}.jpg", file_stem(&urls[0]))).is_file());
    assert!(
        dir.join(format!("{}.png", file_stem(&urls[1]))).is_file(),
        "named by what it is, not by its address"
    );
}

#[tokio::test]
async fn the_cover_of_an_adult_title_is_shown_and_never_kept_as_a_file() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(60, 90)))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (plain, adult) = (
        format!("{}/plain.jpg", server.uri()),
        format!("{}/adult.jpg", server.uri()),
    );
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    let both = |store: &mut CoverStore<FakeUploader>| {
        let plain_done = store.cover(&plain, Shape::Poster, [300.0, 450.0]).is_some();
        let adult_done = store
            .cover_in_memory(&adult, Shape::Poster, [300.0, 450.0])
            .is_some();
        store.end_frame();
        plain_done && adult_done
    };
    for frame in 0..760 {
        if both(&mut store) {
            break;
        }
        tokio::time::sleep(frame_gap(frame)).await;
    }
    assert!(both(&mut store), "both are shown");
    let files: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        files,
        [format!("{}.jpg", file_stem(&plain))],
        "the folder can be read by anyone: only the other one is kept"
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
            // Asked for again, it would be on its way at once.
            assert!(
                !store.is_loading(url, Shape::Poster, [300.0, 450.0]),
                "not asked again"
            );
        }
        store.end_frame();
    }
    // Nor after a frame that does not ask for them: the page scrolled away and back.
    store.end_frame();
    for url in &urls {
        assert!(store.cover(url, Shape::Poster, [300.0, 450.0]).is_none());
        assert!(!store.is_loading(url, Shape::Poster, [300.0, 450.0]));
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, |d| d.count()),
        0,
        "nothing kept"
    );
}

/// Asks for `url` until it is no longer on its way; whether a texture came.
async fn asked_until_settled(url: &str, dir: &std::path::Path) -> bool {
    let mut store = CoverStore::new(dir.to_path_buf(), FakeUploader::default());
    let ask = [url.to_string()];
    frames_until(&mut store, &ask, |s| {
        !s.is_loading(url, Shape::Poster, [300.0, 450.0])
    })
    .await;
    shown(&mut store, url).is_some()
}

#[tokio::test]
async fn an_error_status_with_a_picture_in_it_is_no_cover() {
    let server = MockServer::start().await;
    Mock::given(path("/gone.jpg"))
        .respond_with(ResponseTemplate::new(404).set_body_bytes(jpeg(60, 90)))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/gone.jpg", server.uri());
    assert!(!asked_until_settled(&url, dir.path()).await);
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, |d| d.count()),
        0,
        "nothing kept"
    );
}

#[tokio::test]
async fn a_body_that_starts_like_a_jpeg_and_does_not_decode_is_not_kept() {
    let server = MockServer::start().await;
    Mock::given(path("/cut.jpg"))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(vec![0xff, 0xd8, 0xff, 0x00, 1, 2, 3, 4]),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/cut.jpg", server.uri());
    assert!(!asked_until_settled(&url, dir.path()).await);
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, |d| d.count()),
        0,
        "a kept file is trusted next time"
    );
}

#[tokio::test]
async fn a_body_of_16_mb_is_a_cover_and_one_byte_more_is_not() {
    // A PNG ends at its IEND chunk: what follows is never read, so the padding
    // keeps it a picture. The size is what turns it down.
    let server = MockServer::start().await;
    for (name, len) in [("at", 16 * MB), ("over", 16 * MB + 1)] {
        let mut body = png(60, 90);
        body.resize(len, 0);
        Mock::given(path(format!("/{name}.png")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .expect(1)
            .mount(&server)
            .await;
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(asked_until_settled(&format!("{}/at.png", server.uri()), dir.path()).await);
    assert!(!asked_until_settled(&format!("{}/over.png", server.uri()), dir.path()).await);
}

#[tokio::test]
async fn a_body_that_never_ends_is_given_up_at_16_mb() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/never.png", listener.local_addr().unwrap());
    // A server that sends a PNG's signature and then zeros, until the client
    // hangs up (or 64 MB, so that a client that never does is no danger).
    let server = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if conn.read(&mut byte).unwrap_or(0) == 0 {
                return 0;
            }
            head.push(byte[0]);
        }
        let _ = conn.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\n",
        );
        let block = vec![0u8; 64 * 1024];
        let mut sent = 0;
        while sent < 64 * MB && conn.write_all(&block).is_ok() {
            sent += block.len();
        }
        sent
    });
    let dir = tempfile::tempdir().unwrap();
    assert!(!asked_until_settled(&url, dir.path()).await);
    // Not `server.join()` on this thread: it is the runtime's only one, and the
    // runtime is what closes the client's end of the connection, which is what
    // ends the server's writing. (That did hang, once, with a busy machine.)
    let sent = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || server.join().unwrap()),
    )
    .await
    .expect("the server saw the client hang up")
    .unwrap();
    assert!(
        (16 * MB..48 * MB).contains(&sent),
        "the client hung up after about 16 MB, the server had sent {sent} bytes"
    );
}

#[tokio::test]
async fn a_cover_behind_three_redirects_comes_and_one_behind_four_does_not() {
    let server = MockServer::start().await;
    for (name, hops) in [("three", 3), ("four", 4)] {
        for i in 0..hops {
            Mock::given(path(format!("/{name}/{i}")))
                .respond_with(
                    ResponseTemplate::new(302)
                        .insert_header("Location", format!("{}/{name}/{}", server.uri(), i + 1)),
                )
                .mount(&server)
                .await;
        }
        Mock::given(path(format!("/{name}/{hops}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(60, 90)))
            .mount(&server)
            .await;
    }
    let urls = [
        format!("{}/three/0", server.uri()),
        format!("{}/four/0", server.uri()),
    ];
    let dir = tempfile::tempdir().unwrap();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    frames_until(&mut store, &urls, |s| {
        urls.iter()
            .all(|u| !s.is_loading(u, Shape::Poster, [300.0, 450.0]))
    })
    .await;
    assert!(shown(&mut store, &urls[0]).is_some(), "three hops");
    assert!(shown(&mut store, &urls[1]).is_none(), "four hops");
    let asked: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(
        asked.contains(&"/four/3".to_string()) && !asked.contains(&"/four/4".to_string()),
        "the fourth hop was not followed: {asked:?}"
    );
}

#[tokio::test]
async fn a_redirect_to_another_server_gets_nothing_of_the_address() {
    // Where the picture is, and the portal's image host that sends the client on.
    let far = MockServer::start().await;
    Mock::given(path("/real.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(60, 90)))
        .mount(&far)
        .await;
    let near = MockServer::start().await;
    Mock::given(path("/images/ACCTNAME/SECRETPASS/1.jpg"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/real.jpg", far.uri())),
        )
        .mount(&near)
        .await;
    let host = near.uri().trim_start_matches("http://").to_string();
    let url =
        format!("http://USERX:PASSX@{host}/images/ACCTNAME/SECRETPASS/1.jpg?token=SECRETTOKEN");
    let dir = tempfile::tempdir().unwrap();
    assert!(asked_until_settled(&url, dir.path()).await, "followed");
    let first = near.received_requests().await.unwrap();
    assert!(
        first[0].headers.get("authorization").is_some(),
        "the first hop carries the account in the address"
    );
    let second = far.received_requests().await.unwrap();
    assert_eq!(second.len(), 1);
    assert!(second[0].headers.get("authorization").is_none(), "no login");
    assert!(second[0].headers.get("referer").is_none(), "no Referer");
    assert_eq!(second[0].url.query(), None, "no token");
    assert_eq!(
        second[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap(),
        bongo_player::APP_ID,
        "the app says who it is"
    );
}

#[tokio::test]
async fn a_cover_that_came_when_no_frame_asked_for_it_any_more_is_not_made() {
    let dir = tempfile::tempdir().unwrap();
    let url = "http://covers.example/late.png".to_string();
    std::fs::write(
        dir.path().join(format!("{}.png", file_stem(&url))),
        png(60, 90),
    )
    .unwrap();
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    store.cover(&url, Shape::Poster, [300.0, 450.0]);
    store.end_frame();
    // It is read and decoded while no frame asks for it.
    the_store_is_idle().await;
    store.end_frame();
    assert!(uploader.made.lock().is_empty(), "nobody asked for it");
    assert_eq!(store.texture_bytes(), 0);
    assert!(!store.is_loading(&url, Shape::Poster, [300.0, 450.0]));
}

#[tokio::test]
async fn six_downloads_at_a_time_and_a_cover_that_leaves_the_screen_is_dropped() {
    let server = MockServer::start().await;
    // So slow that no answer comes while the test looks.
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(jpeg(60, 90))
                .set_delay(Duration::from_secs(60)),
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
    // Six reach the server and hold their places; the other four wait for one.
    let mut asked = 0;
    for _ in 0..6000 {
        asked = server.received_requests().await.unwrap().len();
        if asked >= AT_ONCE && store.downloads_in_flight() == AT_ONCE {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!((asked, store.downloads_in_flight()), (AT_ONCE, AT_ONCE));
    // A seventh would be there by now.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.received_requests().await.unwrap().len(), AT_ONCE);
    // The page scrolls on: no frame asks for these any more.
    store.end_frame();
    // Well inside the client's own 15 s limit, which would free them by itself.
    eventually(10, "their places are free", || {
        store.downloads_in_flight() == 0
    })
    .await;
    assert!(urls
        .iter()
        .all(|u| !store.is_loading(u, Shape::Poster, [300.0, 450.0])));
}

#[tokio::test]
async fn a_kept_cover_comes_while_six_slow_downloads_hold_their_places() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(jpeg(60, 90))
                .set_delay(Duration::from_secs(60)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let kept = "http://covers.example/kept.png".to_string();
    std::fs::write(
        dir.path().join(format!("{}.png", file_stem(&kept))),
        png(60, 90),
    )
    .unwrap();
    let slow: Vec<String> = (0..AT_ONCE + 2)
        .map(|i| format!("{}/{i}.jpg", server.uri()))
        .collect();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    // The slow ones first, until six requests are at the server and hold every
    // place: asked in the same frame, the kept one would be read before they had
    // taken any. (Places taken for a moment, by the reads, do not count.)
    let mut reached = 0;
    for frame in 0..600 {
        for url in &slow {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        reached = server.received_requests().await.unwrap().len();
        if reached >= AT_ONCE && store.downloads_in_flight() == AT_ONCE {
            break;
        }
        tokio::time::sleep(frame_gap(frame)).await;
    }
    assert_eq!((reached, store.downloads_in_flight()), (AT_ONCE, AT_ONCE));
    let mut asks = slow.clone();
    asks.push(kept.clone());
    // Well inside the client's own 15 s limit, which would free the places by itself.
    frames_within(10, &mut store, &asks, |s| shown(s, &kept).is_some()).await;
    assert_eq!(
        store.downloads_in_flight(),
        AT_ONCE,
        "the slow ones still hold every place"
    );
}

#[tokio::test]
async fn kept_covers_are_read_side_by_side_not_one_after_the_other() {
    // A big cover (1000 x 1500: a second to decode in a debug build) is asked
    // for first, then five small ones. With places for six reads the small ones
    // are made before it; read one after the other, it would be the first.
    let dir = tempfile::tempdir().unwrap();
    let big = "http://covers.example/big.png".to_string();
    std::fs::write(
        dir.path().join(format!("{}.png", file_stem(&big))),
        png(1000, 1500),
    )
    .unwrap();
    let mut asks = vec![big.clone()];
    for i in 0..5 {
        let url = format!("http://covers.example/small{i}.png");
        std::fs::write(
            dir.path().join(format!("{}.png", file_stem(&url))),
            png(60, 90),
        )
        .unwrap();
        asks.push(url);
    }
    let uploader = FakeUploader::default();
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    // A frame every 50 ms (a second of decoding is 20 of them): the big cover's
    // work, started again in every frame by a store that forgot, would pile up.
    for frame in 0..400 {
        for url in &asks {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        if frame == 0 {
            assert!(
                asks.iter()
                    .all(|u| store.is_loading(u, Shape::Poster, [300.0, 450.0])),
                "asked for in a frame, on their way in the next"
            );
        }
        if uploader.made.lock().len() == asks.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        uploader.made.lock().last().unwrap(),
        &format!("cover {} 300x450", file_stem(&big)),
        "the big one was made last"
    );
}

#[tokio::test]
async fn bodies_that_came_are_decoded_six_at_a_time_not_all_at_once() {
    // Eight big pictures come from a server that answers at once. Each takes a
    // while to decode (1000 x 1500: a good part of a second in a debug build),
    // and every place for reads is taken for as long as six are being decoded.
    // Decoded without a place, a screenful of originals would all be in memory
    // together.
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jpeg(1000, 1500)))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let asks: Vec<String> = (0..READS_AT_ONCE + 2)
        .map(|i| format!("{}/{i}.jpg", server.uri()))
        .collect();
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    let (mut run, mut longest) = (0, 0);
    for frame in 0..2000 {
        for url in &asks {
            store.cover(url, Shape::Poster, [300.0, 450.0]);
        }
        store.end_frame();
        // From the moment every request has been at the server no file is being
        // looked for any more: what holds a place now is a decode.
        if server.received_requests().await.unwrap().len() == asks.len() {
            run = if store.reads_in_flight() == READS_AT_ONCE {
                run + 1
            } else {
                0
            };
            longest = longest.max(run);
        }
        if asks.iter().all(|u| shown(&mut store, u).is_some()) {
            break;
        }
        tokio::time::sleep(frame_gap(frame)).await;
    }
    assert!(
        longest >= 3,
        "six decodes at a time, for a while: {longest} frames in a row"
    );
    assert_eq!(store.reads_in_flight(), 0, "and the places are free again");
}

#[tokio::test]
async fn a_server_that_never_answers_is_given_up_on_after_15_seconds() {
    // The client's limit for a whole request (a body that trickles in, too) is
    // what frees the place of a download that never ends.
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(jpeg(60, 90))
                .set_delay(Duration::from_secs(120)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("{}/stuck.jpg", server.uri());
    let asks = [url.clone()];
    let mut store = CoverStore::new(dir.path().to_path_buf(), FakeUploader::default());
    frames_within(10, &mut store, &asks, |s| s.downloads_in_flight() == 1).await;
    // 15 s, and the time to notice it.
    frames_within(30, &mut store, &asks, |s| {
        !s.is_loading(&url, Shape::Poster, [300.0, 450.0])
    })
    .await;
    assert_eq!(store.downloads_in_flight(), 0, "its place is free again");
    assert!(shown(&mut store, &url).is_none(), "and it is no cover");
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
    // All twelve are read and decoded before a frame looks.
    the_store_is_idle().await;
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 8, "one frame's worth");
    frame(&mut store);
    assert_eq!(uploader.made.lock().len(), 12, "the rest in the next frame");
}

#[tokio::test]
async fn the_store_keeps_its_textures_to_96_mb_and_asks_again_for_one_it_let_go() {
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..5)
        .map(|i| format!("http://covers.example/{i}.png"))
        .collect();
    for url in &urls {
        std::fs::write(
            dir.path().join(format!("{}.png", file_stem(url))),
            png(60, 90),
        )
        .unwrap();
    }
    let uploader = FakeUploader {
        fat: true,
        ..Default::default()
    };
    let mut store = CoverStore::new(dir.path().to_path_buf(), uploader.clone());
    // Three of 30 MB fit in 96 MB. They are made one after the other, so that
    // the order they were shown in is known.
    for url in &urls[..3] {
        frames_until(&mut store, std::slice::from_ref(url), |s| {
            shown(s, url).is_some()
        })
        .await;
    }
    assert_eq!(store.texture_bytes(), 90 * MB);
    // The first is shown again, so the second is the one shown longest ago.
    assert!(shown(&mut store, &urls[0]).is_some());
    // A fourth does not fit: the second goes.
    frames_until(&mut store, &urls[3..4], |s| shown(s, &urls[3]).is_some()).await;
    assert_eq!(store.texture_bytes(), 90 * MB);
    assert_eq!(uploader.made.lock().len(), 4);
    assert!(
        [0, 2, 3]
            .iter()
            .all(|&i| shown(&mut store, &urls[i]).is_some()),
        "the others are still there"
    );
    assert!(shown(&mut store, &urls[1]).is_none(), "the second went");
    // Asked for again, it comes again from its file (its place was let go of
    // when its first texture was made), and the next oldest goes.
    frames_until(&mut store, &urls[1..2], |s| shown(s, &urls[1]).is_some()).await;
    assert_eq!(uploader.made.lock().len(), 5);
    assert_eq!(store.texture_bytes(), 90 * MB);
    assert!(!store.is_loading(&urls[1], Shape::Poster, [300.0, 450.0]));
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
    // A path that is no folder is an error to log, not an empty folder.
    assert!(prune(&dir.path().join("mid.jpg"), 0).is_err());
}

#[cfg(windows)]
#[test]
fn a_file_another_process_holds_open_stays_and_the_rest_still_goes() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age_days) in [("old.jpg", 30u64), ("mid.jpg", 10), ("new.jpg", 1)] {
        let file = dir.path().join(name);
        std::fs::write(&file, vec![0u8; 1000]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(now - Duration::from_secs(age_days * 86_400))
            .unwrap();
    }
    // A scanner, a backup: the oldest is open and shares nothing, so it cannot go.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.path().join("old.jpg"))
        .unwrap();
    assert_eq!(prune(dir.path(), 1000).unwrap(), (2, 1000));
    assert!(dir.path().join("old.jpg").exists(), "held");
    assert!(!dir.path().join("mid.jpg").exists() && !dir.path().join("new.jpg").exists());
    drop(held);
}

#[tokio::test]
async fn the_covers_folder_is_pruned_in_the_background_to_500_mb() {
    // Two files of 300 MB (their length: nothing is written) are 600: the older one goes.
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age_days) in [("old.jpg", 3u64), ("new.jpg", 1)] {
        let file = std::fs::File::create(dir.path().join(name)).unwrap();
        file.set_len(300 * MB as u64).unwrap();
        file.set_modified(now - Duration::from_secs(age_days * 86_400))
            .unwrap();
    }
    prune_in_background(dir.path().to_path_buf());
    eventually(30, "the older file goes", || {
        !dir.path().join("old.jpg").exists()
    })
    .await;
    // The job is over: the one that fits stays.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(dir.path().join("new.jpg").exists());
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
