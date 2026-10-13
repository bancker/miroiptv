//! The guide's EPG in a file, so a restart has the whole guide at once: "het
//! kan zijn dat je net de app hebt afgesloten met de hele gids er al in".

use super::store::ChannelEpg;
use crate::epg::{Epg, EpgEntry};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    channels: Vec<CachedChannel>,
}

#[derive(Serialize, Deserialize)]
struct CachedChannel {
    id: i64,
    fetched_at: i64,
    /// `[start, end, title, has_archive]`: epoch seconds, and 1, 0 or null.
    entries: Vec<(i64, i64, String, Option<u8>)>,
}

/// The cache file of the portal at `host`:`port`: `epg-<host>_<port>.json`,
/// every character but letters, digits, '.' and '-' turned into '_'. The user
/// name and password never go into it.
pub fn file_name(host: &str, port: u16) -> String {
    let host: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("epg-{host}_{port}.json")
}

/// A channel to write: its catch-up id, its EPG, and how far back its
/// catch-up reaches (0: the portal does not say).
pub struct ToSave {
    pub id: i64,
    pub epg: ChannelEpg,
    pub archive_days: u32,
}

/// Writes the file at `path`: a temporary file next to it, renamed over it, so
/// a crash halfway leaves the old one. Programmes that ended before catch-up
/// reaches (plus a day; 14 days when unknown) are left out. The caller leaves
/// adult channels out. Answers how many channels and programmes went in, and
/// the file's size.
pub fn save(
    path: &Path,
    channels: &[ToSave],
    now: DateTime<Utc>,
) -> std::io::Result<(usize, usize, u64)> {
    let mut programmes = 0;
    let channels: Vec<CachedChannel> = channels
        .iter()
        .map(|c| {
            let days = if c.archive_days == 0 {
                14
            } else {
                i64::from(c.archive_days) + 1
            };
            // A window reaching back past the first date there is leaves
            // nothing out; a plain subtraction would panic (the release build
            // aborts on a panic).
            let oldest = now.checked_sub_signed(Duration::days(days));
            let entries: Vec<(i64, i64, String, Option<u8>)> = c
                .epg
                .entries()
                .iter()
                .filter(|e| oldest.is_none_or(|oldest| e.end > oldest))
                .map(|e| {
                    (
                        e.start.timestamp(),
                        e.end.timestamp(),
                        e.title.clone(),
                        e.has_archive.map(u8::from),
                    )
                })
                .collect();
            programmes += entries.len();
            CachedChannel {
                id: c.id,
                fetched_at: c.epg.fetched_at.timestamp(),
                entries,
            }
        })
        .collect();
    let count = channels.len();
    let bytes = serde_json::to_vec(&CacheFile {
        version: VERSION,
        channels,
    })
    .map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        // A write that failed halfway, or a rename that failed, leaves no
        // temporary file behind: the next write starts afresh.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok((count, programmes, bytes.len() as u64))
}

/// What the file holds: per catch-up id, the EPG as it was fetched then. A
/// missing file is an empty cache; a broken one, or one of another version,
/// an error (the caller logs it and writes a new one in time).
///
/// A fetch time later than `now` is taken as `now`: the clock was wrong when
/// the file was written, or has stepped back since. Left as it is, that stamp
/// would pin the channel. Its age would be negative, so `EpgStore::needs_fetch`
/// would never ask for it again, and `EpgStore::insert` would turn down every
/// honest fetch as older than the copy it keeps.
pub fn load(path: &Path, now: DateTime<Utc>) -> Result<Vec<(i64, ChannelEpg)>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let file: CacheFile = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("version {}", file.version));
    }
    Ok(file
        .channels
        .into_iter()
        .filter_map(|c| {
            let fetched_at = Utc.timestamp_opt(c.fetched_at, 0).single()?.min(now);
            let entries = c
                .entries
                .into_iter()
                .filter_map(|(start, end, title, kept)| {
                    Some(EpgEntry {
                        title,
                        start: Utc.timestamp_opt(start, 0).single()?,
                        end: Utc.timestamp_opt(end, 0).single()?,
                        has_archive: kept.map(|k| k != 0),
                    })
                })
                .collect();
            Some((c.id, ChannelEpg::new(Epg::new(entries), fetched_at)))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap() + Duration::hours(h)
    }

    fn entry(title: &str, from: i64, to: i64, kept: Option<bool>) -> EpgEntry {
        EpgEntry {
            title: title.into(),
            start: t(from),
            end: t(to),
            has_archive: kept,
        }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache").join(file_name("127.0.0.1", 18080));
        let epg = Epg::new(vec![
            entry("NOS Journaal", -2, -1, Some(true)),
            entry("Nu", -1, 1, None),
            entry("Straks", 1, 2, Some(false)),
        ]);
        let channels = vec![
            ToSave {
                id: 601,
                epg: ChannelEpg::new(epg, t(0)),
                archive_days: 7,
            },
            ToSave {
                id: 602,
                epg: ChannelEpg::new(Epg::default(), t(-3)),
                archive_days: 0,
            },
        ];
        let (n, programmes, _bytes) = save(&path, &channels, t(0)).unwrap();
        assert_eq!((n, programmes), (2, 3));
        let mut back = load(&path, t(0)).unwrap();
        back.sort_by_key(|(id, _)| *id);
        assert_eq!(back.len(), 2);
        let (id, c) = &back[0];
        assert_eq!(*id, 601);
        assert_eq!(c.fetched_at, t(0));
        assert_eq!(c.entries(), channels[0].epg.entries());
        assert!(c.trusts_flag_for(&c.entries()[0]), "the 1 came back");
        assert_eq!(back[1].1.fetched_at, t(-3));
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "no temporary file is left behind");
    }

    #[test]
    fn a_write_that_fails_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file goes: the temporary file is written, the
        // rename over the folder fails.
        let path = dir.path().join("epg.json");
        std::fs::create_dir(&path).unwrap();
        let channel = ToSave {
            id: 601,
            epg: ChannelEpg::new(Epg::new(vec![entry("Nu", -1, 1, None)]), t(0)),
            archive_days: 7,
        };
        assert!(save(&path, &[channel], t(0)).is_err());
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "{} is left behind", tmp.display());
    }

    #[test]
    fn programmes_older_than_catch_up_are_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("epg.json");
        let old = entry("Oud", -24 * 9, -24 * 9 + 1, None);
        let recent = entry("Recent", -24 * 7, -24 * 7 + 1, None);
        let kept = |archive_days: u32| {
            let epg = Epg::new(vec![old.clone(), recent.clone()]);
            let channel = ToSave {
                id: 601,
                epg: ChannelEpg::new(epg, t(0)),
                archive_days,
            };
            save(&path, &[channel], t(0)).unwrap();
            load(&path, t(0)).unwrap()[0]
                .1
                .entries()
                .iter()
                .map(|e| e.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(7), ["Recent"], "7 days of catch-up and a day to spare");
        assert_eq!(kept(0), ["Oud", "Recent"], "unknown: 14 days");
    }

    #[test]
    fn a_catch_up_window_wider_than_dates_go_back_keeps_everything() {
        // The portal's `tv_archive_duration` is an i32: a "forever" such as
        // 99999999 days reaches back past the first date chrono has. The
        // subtraction must not panic (the release build aborts on a panic).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("epg.json");
        let epg = Epg::new(vec![entry("Oud", -24 * 400, -24 * 400 + 1, None)]);
        for archive_days in [99_999_999, i32::MAX as u32, u32::MAX] {
            let channel = ToSave {
                id: 601,
                epg: ChannelEpg::new(epg.clone(), t(0)),
                archive_days,
            };
            let (_, programmes, _) = save(&path, &[channel], t(0)).unwrap();
            assert_eq!(programmes, 1, "{archive_days} days reach back past it all");
        }
    }

    #[test]
    fn the_file_name_holds_the_portal_and_nothing_else() {
        assert_eq!(file_name("192.168.1.5", 8080), "epg-192.168.1.5_8080.json");
        let odd = file_name("evil/../host:x\\y", 80);
        assert!(
            !odd.contains('/') && !odd.contains('\\') && !odd.contains(':'),
            "{odd}"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_cache_and_a_broken_one_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("none.json"), t(0))
            .unwrap()
            .is_empty());
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, b"{not json").unwrap();
        assert!(load(&broken, t(0)).is_err());
        let other = dir.path().join("other.json");
        std::fs::write(&other, br#"{"version":99,"channels":[]}"#).unwrap();
        assert!(load(&other, t(0)).unwrap_err().contains("99"));
    }

    #[test]
    fn a_stamp_from_the_future_is_taken_as_now() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("epg.json");
        let channel = ToSave {
            id: 601,
            epg: ChannelEpg::new(Epg::new(vec![entry("Nu", -1, 1, None)]), t(5)),
            archive_days: 7,
        };
        save(&path, &[channel], t(5)).unwrap();
        assert_eq!(load(&path, t(0)).unwrap()[0].1.fetched_at, t(0));
    }
}
