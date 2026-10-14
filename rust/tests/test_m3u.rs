use bongo_player::m3u::{
    keys, parse, read, split_headers, stream_id, with_headers, xtream_link, Headers, M3uChannel,
};

/// A list in iptv-org's form, without real addresses.
const LIST: &str = "#EXTM3U x-tvg-url=\"https://epg.example/guide.xml.gz\"\n\
#EXTINF:-1 tvg-id=\"Een.nl@SD\" tvg-logo=\"https://logos.example/een.png\" group-title=\"General;News\",Kanaal Een (1080p)\n\
https://a.example/een/index.m3u8\n\
#EXTINF:-1 tvg-id=\"\" group-title=\"Kids\",Kanaal Twee\n\
#EXTVLCOPT:http-user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64)\n\
#EXTVLCOPT:http-referrer=https://www.example.com/\n\
http://b.example/twee.m3u8\n";

fn one(text: &str) -> M3uChannel {
    let list = parse(text);
    assert_eq!(list.channels.len(), 1, "{text:?}");
    list.channels.into_iter().next().unwrap()
}

#[test]
fn reads_the_attributes_the_name_and_the_url() {
    let list = parse(LIST);
    assert_eq!(list.channels.len(), 2);
    let een = &list.channels[0];
    assert_eq!(een.name, "Kanaal Een (1080p)");
    assert_eq!(een.url, "https://a.example/een/index.m3u8");
    assert_eq!(een.tvg_id.as_deref(), Some("Een.nl@SD"));
    assert_eq!(een.logo.as_deref(), Some("https://logos.example/een.png"));
    assert_eq!(
        een.group.as_deref(),
        Some("General;News"),
        "several groups in one field"
    );
    assert_eq!(
        (een.user_agent.as_deref(), een.referrer.as_deref()),
        (None, None)
    );
    let twee = &list.channels[1];
    assert_eq!(twee.tvg_id, None, "an empty tvg-id is none");
    assert_eq!(twee.group.as_deref(), Some("Kids"));
}

#[test]
fn a_name_may_hold_commas_and_a_quoted_value_too() {
    let c = one("#EXTINF:-1 group-title=\"News, World\",Hello, World\nhttp://a.example/1\n");
    assert_eq!(c.group.as_deref(), Some("News, World"));
    assert_eq!(c.name, "Hello, World");
}

#[test]
fn extvlcopt_headers_go_to_the_next_url() {
    let twee = &parse(LIST).channels[1];
    assert_eq!(
        twee.user_agent.as_deref(),
        Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    );
    assert_eq!(twee.referrer.as_deref(), Some("https://www.example.com/"));
    // Before the #EXTINF, and spelled the HTTP way: they still wait for the URL.
    let c = one("#EXTVLCOPT:http-referer=https://r.example/\n#EXTINF:-1,Een\nhttp://a.example/1\n");
    assert_eq!(c.referrer.as_deref(), Some("https://r.example/"));
    // Used once: the channel after it has none.
    let list = parse("#EXTVLCOPT:http-user-agent=UA\nhttp://a.example/1\nhttp://a.example/2\n");
    assert_eq!(list.channels[0].user_agent.as_deref(), Some("UA"));
    assert_eq!(list.channels[1].user_agent, None);
}

#[test]
fn a_kodi_header_suffix_is_read_and_wins_over_extvlcopt() {
    let c = one("#EXTINF:-1,Een\n#EXTVLCOPT:http-user-agent=Vlc\nhttp://a.example/1.m3u8|User-Agent=Kodi%2F21&Referer=https%3A%2F%2Fk.example%2F\n");
    assert_eq!(c.url, "http://a.example/1.m3u8");
    assert_eq!(c.user_agent.as_deref(), Some("Kodi/21"));
    assert_eq!(c.referrer.as_deref(), Some("https://k.example/"));
}

#[test]
fn a_bom_crlf_and_no_header_are_fine() {
    let c = one("\u{feff}#EXTINF:-1 tvg-id=\"Een.nl\",Een\r\nhttp://a.example/1\r\n");
    assert_eq!(
        (c.name.as_str(), c.url.as_str()),
        ("Een", "http://a.example/1")
    );
    assert_eq!(c.tvg_id.as_deref(), Some("Een.nl"));
}

#[test]
fn broken_lines_are_dropped_or_named_after_their_url() {
    let list = parse(
        "#EXTM3U\n\
         #EXTINF:-1,No URL follows me\n\
         #EXTINF:-1,Twee\n\
         http://a.example/2\n\
         #EXTGRP:Unknown lines are skipped\n\
         rtmp://a.example/live/3\n\
         not a url\n\
         #EXTINF:-1 tvg-name=\"Vier\",\n\
         http://a.example/4\n",
    );
    let names: Vec<&str> = list.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Twee", "rtmp://a.example/live/3", "Vier"]);
}

#[test]
fn the_header_keeps_the_guide_url_for_round_two() {
    assert_eq!(
        parse(LIST).epg_url.as_deref(),
        Some("https://epg.example/guide.xml.gz")
    );
    let other = parse("#EXTM3U url-tvg=\"https://epg.example/b.xml\"\nhttp://a.example/1\n");
    assert_eq!(other.epg_url.as_deref(), Some("https://epg.example/b.xml"));
}

fn channel(name: &str, tvg_id: Option<&str>) -> M3uChannel {
    M3uChannel {
        name: name.into(),
        url: format!("http://a.example/{name}"),
        tvg_id: tvg_id.map(str::to_owned),
        ..M3uChannel::default()
    }
}

#[test]
fn a_unique_tvg_id_is_the_key_else_the_name_with_a_number() {
    let channels = [
        channel("Een", Some("Een.nl")),
        channel("Twee HD", Some("Twee.nl")),
        channel("Twee SD", Some("Twee.nl")), // the id twice: their names then
        channel("Drie", None),
        channel("Drie", None),
        channel("Drie", None),
    ];
    assert_eq!(
        keys(&channels),
        ["Een.nl", "Twee HD", "Twee SD", "Drie", "Drie#2", "Drie#3"]
    );
}

#[test]
fn a_name_that_looks_like_a_numbered_key_does_not_collide() {
    let channels = [channel("X", None), channel("X", None), channel("X#2", None)];
    assert_eq!(keys(&channels), ["X", "X#2", "X#2#2"]);
}

#[test]
fn stream_ids_are_pinned_and_positive() {
    // SHA-256's first 8 bytes as a number, shifted right by one bit (so never
    // negative): the same on every machine. Saved favorites hold these ids.
    assert_eq!(stream_id("NPO1.nl"), 3_499_708_883_161_207_265);
    assert_eq!(stream_id("Kanaal Een"), 6_767_733_321_925_303_917);
    assert_eq!(stream_id("Kanaal Een#2"), 5_326_501_074_998_566_453);
    assert!(stream_id("") > 0);
}

#[test]
fn the_stream_id_follows_the_key_not_the_place_or_the_address() {
    let before = "#EXTM3U\n#EXTINF:-1 tvg-id=\"Een.nl\",Een\nhttps://a.example/een.m3u8\n\
                  #EXTINF:-1,Twee\nhttps://a.example/twee.m3u8\n";
    // The provider put Twee first and moved Een to another server.
    let after = "#EXTM3U\n#EXTINF:-1,Twee\nhttps://b.example/twee.m3u8\n\
                 #EXTINF:-1 tvg-id=\"Een.nl\",Een\nhttps://c.example/een/index.m3u8\n";
    let id = |text: &str, name: &str| {
        read(text)
            .unwrap()
            .live
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .stream_id
    };
    assert_eq!(id(before, "Een"), id(after, "Een"));
    assert_eq!(id(before, "Twee"), id(after, "Twee"));
    assert_eq!(id(before, "Een"), stream_id("Een.nl"));
    assert_eq!(id(before, "Twee"), stream_id("Twee"));
}

#[test]
fn headers_ride_in_the_url_and_come_back_out() {
    let ua = "Mozilla/5.0 (X11; Linux), like Gecko & co";
    let referrer = "https://www.example.com/a?b=c:d";
    let url = with_headers("https://a.example/1.m3u8".into(), Some(ua), Some(referrer));
    assert!(
        url.starts_with("https://a.example/1.m3u8|User-Agent="),
        "{url}"
    );
    let (address, headers) = split_headers(&url);
    assert_eq!(address, "https://a.example/1.m3u8");
    assert_eq!(
        headers,
        Headers {
            user_agent: Some(ua.into()),
            referrer: Some(referrer.into())
        }
    );
    // None: the URL as it was, and nothing comes back out of it.
    assert_eq!(
        with_headers("http://a.example/2".into(), None, None),
        "http://a.example/2"
    );
    assert_eq!(
        split_headers("http://a.example/2"),
        ("http://a.example/2", Headers::default())
    );
    // Names in any case, Referrer spelled out, other headers ignored.
    let (_, h) = split_headers("http://a/1|user-agent=U&REFERRER=R&Cookie=c");
    assert_eq!(
        (h.user_agent.as_deref(), h.referrer.as_deref()),
        (Some("U"), Some("R"))
    );
}

#[test]
fn an_xtream_get_php_link_gives_its_credentials() {
    assert_eq!(
        xtream_link("http://srv.example.net:8080/get.php?username=bob&password=s%40cret&type=m3u_plus&output=ts")
            .as_deref(),
        Some("bob:s@cret@srv.example.net:8080")
    );
    assert_eq!(
        xtream_link(" http://srv.example.net/get.php?password=p&username=u ").as_deref(),
        Some("u:p@srv.example.net:80")
    );
    for other in [
        "http://srv.example.net/list.m3u?username=u&password=p",
        "http://srv.example.net/get.php?username=u",
        "http://srv.example.net/get.php?username=&password=p",
        "C:\\lists\\get.php",
        "not a url",
    ] {
        assert_eq!(xtream_link(other), None, "{other}");
    }
}

#[test]
fn read_gives_the_catalog_in_file_order_with_headers_in_the_urls() {
    let playlist = read(LIST).expect("a list");
    let names: Vec<&str> = playlist.live.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Kanaal Een (1080p)", "Kanaal Twee"]);
    let een = &playlist.live[0];
    assert_eq!(een.stream_id, stream_id("Een.nl@SD"));
    assert_eq!(een.category_id.as_deref(), Some("General;News"));
    assert_eq!(een.epg_channel_id.as_deref(), Some("Een.nl@SD"));
    assert_eq!((een.tv_archive, een.is_adult), (0, false));
    assert_eq!(een.stream_icon, None, "the logo is kept with the stream");
    let stream = &playlist.streams[&een.stream_id];
    assert_eq!(stream.url, "https://a.example/een/index.m3u8");
    assert_eq!(
        stream.logo.as_deref(),
        Some("https://logos.example/een.png")
    );
    let twee = &playlist.streams[&stream_id("Kanaal Twee")];
    let (address, headers) = split_headers(&twee.url);
    assert_eq!(address, "http://b.example/twee.m3u8");
    assert_eq!(
        headers.user_agent.as_deref(),
        Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    );
    assert_eq!(
        playlist.epg_url.as_deref(),
        Some("https://epg.example/guide.xml.gz")
    );
}

#[test]
fn an_html_page_or_a_list_without_channels_is_no_list() {
    for text in [
        "<!DOCTYPE html><html><body>Service Unavailable</body></html>",
        "\u{feff}  \n<html>\nhttp://a.example/1\n</html>",
        "",
        "#EXTM3U\n",
        "#EXTM3U\n#EXTINF:-1,No URL\n",
    ] {
        assert!(read(text).is_none(), "{text:?}");
    }
}

/// A list in iptv-org's form with `n` channels: a tvg-id that repeats now and
/// then, two groups, a logo each, and headers on every seventh.
fn big_list(n: usize) -> String {
    let mut text = String::from("#EXTM3U x-tvg-url=\"https://epg.example/guide.xml.gz\"\n");
    for i in 0..n {
        let id = if i % 10 == 0 {
            format!("Shared{}.xx", i / 100)
        } else {
            format!("Kanaal{i}.xx@SD")
        };
        text.push_str(&format!(
            "#EXTINF:-1 tvg-id=\"{id}\" tvg-logo=\"https://logos.example/{i}.png\" group-title=\"Groep {};News\",Kanaal {i} (1080p)\n",
            i % 183
        ));
        if i % 7 == 0 {
            text.push_str("#EXTVLCOPT:http-user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36\n");
            text.push_str("#EXTVLCOPT:http-referrer=https://www.example.com/\n");
        }
        text.push_str(&format!(
            "https://cdn{}.example.net/live/{i}/index.m3u8\n",
            i % 9
        ));
    }
    text
}

#[test]
fn eleven_thousand_channels_read_within_the_budget() {
    let text = big_list(11_262);
    assert!(
        text.len() > 2_000_000,
        "about iptv-org's size: {} bytes",
        text.len()
    );
    let t0 = std::time::Instant::now();
    let playlist = read(&text).expect("a list");
    let took = t0.elapsed().as_millis();
    let heap = playlist.heap_bytes();
    println!("read 11,262 channels in {took} ms, {} KB kept", heap / 1024);
    assert_eq!(playlist.live.len(), 11_262);
    assert_eq!(playlist.streams.len(), 11_262, "no two keys share an id");
    // The spec's budget is for a release build; a debug build is several times slower.
    let budget = if cfg!(debug_assertions) { 3_000 } else { 100 };
    assert!(took < budget, "{took} ms");
    assert!(heap < 10 * 1024 * 1024, "{heap} bytes");
}

/// The real list, measured by hand (Task 11):
/// `BONGO_M3U_SAMPLE=<file> cargo test --release --test test_m3u -- --ignored --nocapture`
#[test]
#[ignore]
fn a_real_list_read_and_measured() {
    let path = std::env::var_os("BONGO_M3U_SAMPLE").expect("BONGO_M3U_SAMPLE names a list file");
    let text = std::fs::read_to_string(path).unwrap();
    let t0 = std::time::Instant::now();
    let playlist = read(&text).expect("a list");
    println!(
        "{} channels in {} ms, {} KB kept, file {} KB",
        playlist.live.len(),
        t0.elapsed().as_millis(),
        playlist.heap_bytes() / 1024,
        text.len() / 1024
    );
}
