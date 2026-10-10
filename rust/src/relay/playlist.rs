//! HLS media-playlist parsing for the relay. Pure: no I/O.

use reqwest::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub seq: u64,
    pub duration: f64,
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    pub target_duration: f64,
    pub segments: Vec<Segment>,
    pub ended: bool,
}

/// Parse a media playlist. Segments are numbered from `#EXT-X-MEDIA-SEQUENCE`:
/// those numbers are what keeps the relay's output continuous across reloads
/// and refusals. `None` if the text is not a playlist (e.g. an error page).
pub fn parse(text: &str) -> Option<Playlist> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next()? != "#EXTM3U" {
        return None;
    }
    let mut first_seq = 0;
    let mut target_duration = 0.0;
    let mut duration = None;
    let mut segments = Vec::new();
    let mut ended = false;
    for line in lines {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            first_seq = v.trim().parse().ok()?;
        } else if let Some(v) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            target_duration = v.trim().parse().ok()?;
        } else if let Some(v) = line.strip_prefix("#EXTINF:") {
            duration = v.split(',').next().and_then(|d| d.trim().parse().ok());
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if !line.starts_with('#') {
            segments.push(Segment {
                seq: first_seq + segments.len() as u64,
                duration: duration.take().unwrap_or(target_duration),
                uri: line.to_string(),
            });
        }
    }
    Some(Playlist {
        target_duration,
        segments,
        ended,
    })
}

/// The playlist text with every URI made absolute against `base` (where it
/// was fetched from), so it can be served from somewhere else.
pub fn absolutize(text: &str, base: &Url) -> String {
    let resolve = |uri: &str| {
        base.join(uri)
            .map(String::from)
            .unwrap_or_else(|_| uri.to_string())
    };
    let mut out = String::with_capacity(text.len() * 2);
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !line.starts_with('#') {
            out += &resolve(line);
        } else if let Some(at) = line.find("URI=\"") {
            // A tag pointing at a file: a key, an init segment, a variant.
            let (head, rest) = line.split_at(at + 5);
            match rest.find('"') {
                Some(end) => {
                    out += head;
                    out += &resolve(&rest[..end]);
                    out += &rest[end..];
                }
                None => out += line,
            }
        } else {
            out += line;
        }
        out.push('\n');
    }
    out
}

impl Playlist {
    /// Sequence number a fresh session starts at: `behind` segments before
    /// the live edge (or the first one if the playlist is shorter).
    pub fn start_seq(&self, behind: usize) -> Option<u64> {
        let i = self.segments.len().checked_sub(1)?;
        Some(self.segments[i.saturating_sub(behind.saturating_sub(1))].seq)
    }

    /// Segments from sequence `next` on, in order: what is still to fetch.
    pub fn from(&self, next: u64) -> &[Segment] {
        &self.segments[self.segments.partition_point(|s| s.seq < next)..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of the real portal's live playlists (tokens shortened).
    const LIVE: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-MEDIA-SEQUENCE:4703\n\
        #EXT-X-ALLOW-CACHE:YES\n#EXT-X-TARGETDURATION:12\n\
        #EXTINF:8.640000,\n/hls/aa/981950_4703.ts\n\
        #EXTINF:11.600000,\n/hls/bb/981950_4704.ts\n\
        #EXTINF:8.400000,\n/hls/cc/981950_4705.ts\n";

    fn seqs(segs: &[Segment]) -> Vec<u64> {
        segs.iter().map(|s| s.seq).collect()
    }

    #[test]
    fn numbers_segments_from_the_media_sequence() {
        let p = parse(LIVE).unwrap();
        assert_eq!(p.target_duration, 12.0);
        assert!(!p.ended);
        assert_eq!(
            p.segments[1],
            Segment {
                seq: 4704,
                duration: 11.6,
                uri: "/hls/bb/981950_4704.ts".into()
            }
        );
        assert_eq!(seqs(&p.segments), [4703, 4704, 4705]);
    }

    #[test]
    fn tolerates_crlf_and_blank_lines() {
        let p = parse(&LIVE.replace('\n', "\r\n\r\n")).unwrap();
        assert_eq!(seqs(&p.segments), [4703, 4704, 4705]);
        assert_eq!(p.segments[0].uri, "/hls/aa/981950_4703.ts");
    }

    #[test]
    fn rejects_what_is_not_a_playlist() {
        assert_eq!(parse("<html>509 Bandwidth Limit Exceeded</html>"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn endlist_marks_a_finished_playlist() {
        let p = parse(&format!("{LIVE}#EXT-X-ENDLIST\n")).unwrap();
        assert!(p.ended);
    }

    #[test]
    fn starts_two_segments_behind_the_live_edge() {
        // Two segments of cushion: the start downloads at full speed, then
        // portal hiccups are ridden out from that buffer.
        let p = parse(LIVE).unwrap();
        assert_eq!(p.start_seq(2), Some(4704));
        assert_eq!(
            p.start_seq(10),
            Some(4703),
            "short playlist: from the first"
        );
    }

    #[test]
    fn absolutize_resolves_segment_uris_against_the_playlist_url() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:60\n#EXT-X-PLAYLIST-TYPE:VOD\n\
            #EXTINF:60.0,\n/ts/aa/0.ts?token=t1\n\
            #EXTINF:60.0,\r\nhttp://edge.example/ts/aa/1.ts?token=t1\n#EXT-X-ENDLIST\n";
        let base = Url::parse("http://portal.example:8080/timeshift/x.m3u8?token=t0").unwrap();
        let out = absolutize(text, &base);
        assert!(out.contains("\nhttp://portal.example:8080/ts/aa/0.ts?token=t1\n"));
        assert!(out.contains("\nhttp://edge.example/ts/aa/1.ts?token=t1\n"));
        assert!(out.contains("\n#EXT-X-PLAYLIST-TYPE:VOD\n"), "tags kept");
        assert!(out.ends_with("\n#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn absolutize_resolves_uri_attributes_too() {
        let text = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1\n#EXTINF:6,\ns.ts\n";
        let base = Url::parse("http://portal.example/a/b.m3u8").unwrap();
        let out = absolutize(text, &base);
        assert!(out.contains("URI=\"http://portal.example/a/key.bin\",IV=0x1\n"));
        assert!(out.contains("\nhttp://portal.example/a/s.ts\n"));
    }

    #[test]
    fn from_never_hands_out_a_segment_twice() {
        let p = parse(LIVE).unwrap();
        assert_eq!(seqs(p.from(4704)), [4704, 4705]);
        assert!(p.from(4706).is_empty(), "nothing new yet");
        assert_eq!(seqs(p.from(4700)), [4703, 4704, 4705], "fell behind: all");
    }
}
