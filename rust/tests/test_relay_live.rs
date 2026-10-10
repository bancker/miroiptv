//! Manual check of the relay against a real portal (needs network + creds):
//!
//!   BONGO_LIVE_URL=http://host:port/live/<user>/<pass>/<id>.m3u8 \
//!     cargo test --test test_relay_live -- --ignored --nocapture
use bongo_player::relay::{Relay, RelayConfig};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn relays_a_real_portal_stream_without_interruption() {
    let Ok(upstream) = std::env::var("BONGO_LIVE_URL") else {
        eprintln!("BONGO_LIVE_URL not set - skipped");
        return;
    };
    // Show the relay's own log (session start, first bytes, low-cushion warnings).
    let _ = tracing_subscriber::fmt()
        .with_env_filter("bongo_player=info")
        .with_test_writer()
        .try_init();
    let seconds: usize = std::env::var("BONGO_LIVE_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let relay = Relay::start(RelayConfig::default()).unwrap();
    let local = relay.local_url(&upstream);
    let rest = local.trim_start_matches("http://");
    let (host, path) = rest.split_at(rest.find('/').unwrap());

    let t0 = Instant::now();
    let mut conn = tokio::net::TcpStream::connect(host).await.unwrap();
    conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = vec![0u8; 64 * 1024];
    let mut first_byte = None;
    let mut per_5s = vec![0usize; seconds / 5];
    let mut ended = None;
    // Segments arrive as bursts (a ~10 s segment downloads in ~1 s), so quiet
    // stretches of a segment plus a poll interval are normal; the relay itself
    // warns in the log when its cushion runs low.
    let mut last_data = Instant::now();
    let mut max_gap = Duration::ZERO;
    while t0.elapsed() < Duration::from_secs(seconds as u64) {
        match tokio::time::timeout(Duration::from_secs(15), conn.read(&mut buf)).await {
            Ok(Ok(0)) => {
                ended = Some("relay closed the stream");
                break;
            }
            Ok(Ok(n)) => {
                first_byte.get_or_insert(t0.elapsed());
                max_gap = max_gap.max(last_data.elapsed());
                last_data = Instant::now();
                let slot = (t0.elapsed().as_secs() as usize / 5).min(per_5s.len() - 1);
                per_5s[slot] += n;
            }
            Ok(Err(_)) => {
                ended = Some("read error");
                break;
            }
            Err(_) => {
                ended = Some("no data for 15 s");
                break;
            }
        }
    }
    println!("first byte after {:?}", first_byte.unwrap());
    for (i, b) in per_5s.iter().enumerate() {
        println!(
            "t={:2}-{:2}s  {:6.2} Mbit/s",
            i * 5,
            i * 5 + 5,
            *b as f64 * 8.0 / 5.0 / 1e6
        );
    }
    let total: usize = per_5s.iter().sum();
    println!(
        "largest gap between data: {max_gap:?}, total {:.1} MB",
        total as f64 / 1e6
    );
    assert_eq!(ended, None, "stream interrupted");
    assert!(total > 5_000_000, "too little data: {total} bytes");
}
