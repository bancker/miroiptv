//! The audio tracks of what plays, read from mpv's `track-list`; which one `a`
//! picks next; and when a choice made with `a` or `s` goes back to mpv's own.
//!
//! mpv's `cycle audio` passes through "no audio", and an `aid` set at runtime
//! outlives the file (measured on the vendored libmpv, `tests/test_mpv_smoke.rs`):
//! one press of `a` on a one-track stream (Bloomberg on an M3U list) made that
//! stream and every later one silent. `a` now picks the next track itself,
//! round to the first, and a choice is put back to `auto` when another stream
//! loads; a reload of the same stream keeps it.

/// One audio track of mpv's `track-list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    /// mpv's id of the track, what `aid` takes.
    pub id: i64,
    pub selected: bool,
    pub lang: Option<String>,
    pub title: Option<String>,
}

impl AudioTrack {
    /// How the toast names the track: "eng", "eng (Dolby)", "Dolby", or nothing.
    pub fn label(&self) -> String {
        match (&self.lang, &self.title) {
            (Some(lang), Some(title)) => format!("{lang} ({title})"),
            (Some(lang), None) => lang.clone(),
            (None, Some(title)) => title.clone(),
            (None, None) => String::new(),
        }
    }
}

/// The audio tracks of `track-list`, in mpv's order, read through `read`
/// (`Mpv::get_property_string`): `track-list/count`, then `track-list/<n>/type`,
/// `id`, `selected`, `lang` and `title` of each entry. A track without a
/// readable id is left out; an absent or empty `lang` or `title` is `None`.
pub fn audio_tracks(read: impl Fn(&str) -> Option<String>) -> Vec<AudioTrack> {
    let count: usize = read("track-list/count")
        .and_then(|c| c.trim().parse().ok())
        .unwrap_or(0);
    (0..count)
        .filter(|n| read(&format!("track-list/{n}/type")).as_deref() == Some("audio"))
        .filter_map(|n| {
            let id = read(&format!("track-list/{n}/id"))?.trim().parse().ok()?;
            Some(AudioTrack {
                id,
                selected: read(&format!("track-list/{n}/selected")).as_deref() == Some("yes"),
                lang: read(&format!("track-list/{n}/lang")).filter(|s| !s.is_empty()),
                title: read(&format!("track-list/{n}/title")).filter(|s| !s.is_empty()),
            })
        })
        .collect()
}

/// What `a` does with the audio tracks of what plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// No audio track: nothing to pick.
    None,
    /// The one track there is plays already: kept.
    OnlyOne,
    /// Select this one, a position in the list: the track after the selected
    /// one, the first after the last, the first when none is selected.
    Next(usize),
}

/// The track `a` selects, never "no audio".
pub fn pick_next(tracks: &[AudioTrack]) -> Pick {
    if tracks.is_empty() {
        return Pick::None;
    }
    match tracks.iter().position(|t| t.selected) {
        Some(_) if tracks.len() == 1 => Pick::OnlyOne,
        Some(i) => Pick::Next((i + 1) % tracks.len()),
        None => Pick::Next(0),
    }
}

/// Whether `a` or `s` chose a track for the stream that plays. mpv keeps such
/// a choice for every later file, so the load of another stream puts `aid` and
/// `sid` back to `auto` first (`load` says so, once); a reload of the same
/// stream keeps the choice. `Debug` for the tests only: it holds the URL of
/// the last load, an Xtream account's in it.
#[derive(Default)]
#[cfg_attr(test, derive(Debug))]
pub struct TrackChoice {
    chosen: bool,
    url: Option<String>,
}

impl TrackChoice {
    pub fn new() -> Self {
        Self::default()
    }

    /// `a` or `s` changed a track of what plays.
    pub fn chosen(&mut self) {
        self.chosen = true;
    }

    /// `url` loads now: true when a choice is to be put back before it.
    pub fn load(&mut self, url: &str) -> bool {
        let reset = self.chosen && self.url.as_deref() != Some(url);
        if reset {
            self.chosen = false;
        }
        self.url = Some(url.to_owned());
        reset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mpv's properties as a test gives them: `None` for one mpv has not.
    fn reader<'a>(entries: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    fn track(id: i64, selected: bool) -> AudioTrack {
        AudioTrack {
            id,
            selected,
            lang: None,
            title: None,
        }
    }

    #[test]
    fn a_stream_without_audio_has_no_track_to_pick() {
        let read = reader(&[
            ("track-list/count", "2"),
            ("track-list/0/type", "video"),
            ("track-list/0/id", "1"),
            ("track-list/1/type", "sub"),
            ("track-list/1/id", "1"),
        ]);
        assert!(audio_tracks(read).is_empty());
        assert_eq!(pick_next(&[]), Pick::None);
        // Nothing loaded: no count either.
        assert!(audio_tracks(reader(&[])).is_empty());
    }

    #[test]
    fn the_one_track_that_plays_is_kept() {
        assert_eq!(pick_next(&[track(1, true)]), Pick::OnlyOne);
    }

    #[test]
    fn an_only_track_that_does_not_play_is_selected() {
        assert_eq!(pick_next(&[track(1, false)]), Pick::Next(0));
    }

    #[test]
    fn the_next_track_round_to_the_first_never_no_audio() {
        let three = |selected: Option<usize>| -> Vec<AudioTrack> {
            (0..3)
                .map(|i| track(i as i64 + 1, selected == Some(i)))
                .collect()
        };
        assert_eq!(pick_next(&three(Some(0))), Pick::Next(1));
        assert_eq!(pick_next(&three(Some(1))), Pick::Next(2));
        assert_eq!(
            pick_next(&three(Some(2))),
            Pick::Next(0),
            "round to the first"
        );
        assert_eq!(
            pick_next(&three(None)),
            Pick::Next(0),
            "none selected: the first"
        );
    }

    #[test]
    fn the_audio_tracks_are_read_in_order_with_their_language_and_title() {
        let read = reader(&[
            ("track-list/count", "4"),
            ("track-list/0/type", "video"),
            ("track-list/0/id", "1"),
            ("track-list/1/type", "audio"),
            ("track-list/1/id", "1"),
            ("track-list/1/selected", "yes"),
            ("track-list/1/lang", "eng"),
            ("track-list/2/type", "audio"),
            ("track-list/2/id", "2"),
            ("track-list/2/selected", "no"),
            ("track-list/2/lang", "deu"),
            ("track-list/2/title", "Dolby"),
            ("track-list/3/type", "audio"),
            ("track-list/3/id", "3"),
            ("track-list/3/selected", "no"),
            ("track-list/3/title", ""),
        ]);
        let tracks = audio_tracks(read);
        assert_eq!(
            tracks,
            [
                AudioTrack {
                    id: 1,
                    selected: true,
                    lang: Some("eng".into()),
                    title: None,
                },
                AudioTrack {
                    id: 2,
                    selected: false,
                    lang: Some("deu".into()),
                    title: Some("Dolby".into()),
                },
                AudioTrack {
                    id: 3,
                    selected: false,
                    lang: None,
                    title: None,
                },
            ]
        );
        assert_eq!(tracks[0].label(), "eng");
        assert_eq!(tracks[1].label(), "deu (Dolby)");
        assert_eq!(tracks[2].label(), "");
        assert_eq!(pick_next(&tracks), Pick::Next(1));
    }

    #[test]
    fn a_track_without_a_readable_id_is_left_out() {
        let read = reader(&[
            ("track-list/count", "2"),
            ("track-list/0/type", "audio"),
            ("track-list/0/id", "x"),
            ("track-list/1/type", "audio"),
            ("track-list/1/id", " 7 "),
            ("track-list/1/selected", "yes"),
        ]);
        assert_eq!(audio_tracks(read), [track(7, true)]);
    }

    #[test]
    fn a_title_alone_names_the_track() {
        let t = AudioTrack {
            id: 1,
            selected: true,
            lang: None,
            title: Some("Commentary".into()),
        };
        assert_eq!(t.label(), "Commentary");
    }

    #[test]
    fn a_choice_goes_back_for_another_stream_not_for_a_reload() {
        let mut choice = TrackChoice::new();
        assert!(!choice.load("http://a/1"), "nothing chosen yet");
        choice.chosen();
        assert!(
            !choice.load("http://a/1"),
            "a reload of the same stream keeps it"
        );
        assert!(choice.load("http://a/2"), "another stream puts it back");
        assert!(!choice.load("http://a/3"), "and once is enough");
        assert!(
            !choice.load("http://a/1"),
            "the first stream is another stream now"
        );
    }

    #[test]
    fn a_choice_made_before_any_load_goes_back_at_the_first() {
        let mut choice = TrackChoice::new();
        choice.chosen();
        assert!(choice.load("http://a/1"));
    }
}
