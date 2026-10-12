//! The guide (`g`): one column per channel with catch-up, live and catch-up in
//! one list, NLZIET style. `GuideView` keeps the guide's state for the whole
//! session; `show` draws one frame and hands what the user did to app.rs as a
//! [`GuideAction`], as settings_ui.rs does.

use crate::catalog::GuideChannel;
use crate::epg::EpgEntry;
use crate::guide::store::{filter_channels, EpgStore, FilterResult};
use crate::guide::time::{self, Playable};
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Where the now-line sits, as a share of a column's rows from the top.
const NOW_LINE: f32 = 0.35;

/// Nu & straks, or Primetime (19:00 on the chosen day).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    NowAndNext,
    Primetime,
}

/// What the user did in the guide.
#[derive(Debug, Clone, PartialEq)]
pub enum GuideAction {
    /// Enter, or a click on a programme: app.rs plays it, or says why not.
    Play {
        channel: GuideChannel,
        entry: EpgEntry,
        playable: Playable,
    },
    Close,
    Help,
}

/// ↑ ↓ Home End in a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStep {
    Up,
    Down,
    First,
    Last,
}

/// What one frame of the guide looks at: worked out at the top of `show`, and
/// what keys, wheel and clicks act on.
pub struct Page<'a> {
    pub channels: &'a [GuideChannel],
    pub store: &'a EpgStore,
    /// Indices into `channels` of the ones shown: all, or what the filter left.
    pub shown: &'a [usize],
    /// Columns side by side, and rows in a column.
    pub cols: usize,
    pub rows: usize,
    pub now: DateTime<Utc>,
    pub today: NaiveDate,
}

impl Page<'_> {
    /// The row of the now-line.
    pub fn line(&self) -> usize {
        (self.rows as f32 * NOW_LINE) as usize
    }

    /// The channel in column `col` of a page that starts at shown index `start`.
    pub fn channel(&self, start: usize, col: usize) -> Option<&GuideChannel> {
        self.shown
            .get(start + col)
            .and_then(|&i| self.channels.get(i))
    }
}

/// What a filter result was worked out for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FilterKey {
    text: String,
    day: NaiveDate,
    store_revision: u64,
    channels_revision: u64,
}

/// The guide's state, kept for the whole session: it opens as it was left.
pub struct GuideView {
    day: NaiveDate,
    /// The chosen day's bounds (local midnight to local midnight, as UTC) and
    /// its 19:00, worked out by `set_day` when the day changes: a frame
    /// converts no local time, which is an OS call each.
    day_bounds: (DateTime<Utc>, DateTime<Utc>),
    primetime_at: DateTime<Utc>,
    mode: Mode,
    /// Index into the shown channels of the leftmost column.
    page_start: usize,
    /// The cursor's column, counted within the page.
    selected: usize,
    /// Per catch-up id: the programme the cursor is on (an index into its EPG).
    cursor: HashMap<i64, usize>,
    /// Per catch-up id: when that programme starts. A new copy of the EPG moves
    /// the indexes; the start finds the programme again.
    cursor_at: HashMap<i64, DateTime<Utc>>,
    /// Per catch-up id: the entry in the top row, fractional while the wheel
    /// turns; before the day's first programme the rows stay empty.
    top: HashMap<i64, f32>,
    /// Per catch-up id: when the copy of its EPG was fetched that `cursor` and
    /// `top` were worked out on. A column without an entry lines up as soon as
    /// it shows with programmes on the day (after `realign`, or when its EPG
    /// comes late); one whose entry is not the store's copy any more has to
    /// line up again, or keep its place (see `moved`).
    aligned: HashMap<i64, DateTime<Utc>>,
    /// Columns the user moved or scrolled in since they were lined up: on a new
    /// copy of their EPG they keep their programme instead of lining up again.
    moved: HashSet<i64>,
    filter: String,
    filtered: Option<(FilterKey, FilterResult)>,
    /// The date list while it is open: the day it highlights.
    date_pick: Option<NaiveDate>,
}

impl GuideView {
    pub fn new(today: NaiveDate) -> Self {
        let mut view = Self {
            day: today,
            // Placeholders: `set_day` below works the real ones out.
            day_bounds: (DateTime::UNIX_EPOCH, DateTime::UNIX_EPOCH),
            primetime_at: DateTime::UNIX_EPOCH,
            mode: Mode::NowAndNext,
            page_start: 0,
            selected: 0,
            cursor: HashMap::new(),
            cursor_at: HashMap::new(),
            top: HashMap::new(),
            aligned: HashMap::new(),
            moved: HashSet::new(),
            filter: String::new(),
            filtered: None,
            date_pick: None,
        };
        view.set_day(today);
        view
    }

    /// Every change of the chosen day goes through here: the day's bounds and
    /// its 19:00 are worked out once and kept (three time-zone conversions).
    fn set_day(&mut self, day: NaiveDate) {
        self.day = day;
        self.day_bounds = time::day_bounds(day);
        self.primetime_at = time::at_hour(day, time::PRIMETIME_HOUR);
    }

    /// The programmes of the chosen day in a channel's listing: the bounds
    /// kept for the day, no time-zone work (draw and key code use this).
    pub fn day_range(&self, entries: &[EpgEntry]) -> Range<usize> {
        let (from, to) = self.day_bounds;
        time::slice_between(entries, from, to)
    }

    /// The guide opens: in Nu & straks it lines up on now (today) again; the
    /// rest is as it was left.
    pub fn opened(&mut self, today: NaiveDate) {
        self.date_pick = None;
        if self.mode == Mode::NowAndNext {
            self.set_day(today);
            self.realign();
        }
    }

    /// Every column lines up again as soon as it shows with programmes.
    pub fn realign(&mut self) {
        self.aligned.clear();
        self.moved.clear();
    }

    /// `n`, or a click on "Nu & straks": today, lined up on now.
    pub fn now_and_next(&mut self, today: NaiveDate) {
        self.mode = Mode::NowAndNext;
        self.set_day(today);
        self.realign();
        tracing::info!("guide: now and next");
    }

    /// `p`, or a click on "Primetime": 19:00 on the chosen day.
    pub fn primetime(&mut self) {
        self.mode = Mode::Primetime;
        self.realign();
        tracing::info!("guide: primetime on {}", self.day);
    }

    /// A day from the date list: its primetime.
    pub fn pick_day(&mut self, day: NaiveDate) {
        self.set_day(day);
        self.date_pick = None;
        self.primetime();
    }

    /// In Nu & straks the guide follows today past midnight.
    pub fn follow_today(&mut self, today: NaiveDate) {
        if self.mode == Mode::NowAndNext && self.day != today {
            self.set_day(today);
            self.realign();
        }
    }

    /// `d`, or a click on the date: the list opens on the chosen day.
    pub fn open_dates(&mut self) {
        self.date_pick = Some(self.day);
    }

    /// ↑ or ↓ in the date list (`days` in order, the earliest first).
    pub fn move_date(&mut self, days: &[NaiveDate], later: bool) {
        let Some(current) = self.date_pick else {
            return;
        };
        let i = days.iter().position(|d| *d == current).unwrap_or(0);
        let j = if later {
            (i + 1).min(days.len().saturating_sub(1))
        } else {
            i.saturating_sub(1)
        };
        if let Some(day) = days.get(j) {
            self.date_pick = Some(*day);
        }
    }

    /// The moment the columns line up on.
    fn target_time(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self.mode {
            Mode::NowAndNext => now,
            Mode::Primetime => self.primetime_at,
        }
    }

    /// Works the filter's result out again when its text, the day, the store or
    /// the channel list changed. They are compared where they lie, so a frame
    /// in which nothing changed allocates nothing. No filter: no result.
    pub fn refresh_filter(
        &mut self,
        channels: &[GuideChannel],
        store: &EpgStore,
        channels_revision: u64,
    ) {
        if self.filter.trim().is_empty() {
            self.filtered = None;
            return;
        }
        let fresh = self.filtered.as_ref().is_some_and(|(key, _)| {
            key.text == self.filter
                && key.day == self.day
                && key.store_revision == store.revision()
                && key.channels_revision == channels_revision
        });
        if fresh {
            return;
        }
        let day = self.day;
        let (from, to) = self.day_bounds;
        let result = filter_channels(channels, store, &self.filter, |e| {
            time::slice_between(e, from, to)
        });
        let key = FilterKey {
            text: self.filter.clone(),
            day,
            store_revision: store.revision(),
            channels_revision,
        };
        self.filtered = Some((key, result));
    }

    /// The filter's result, while a filter is set.
    pub fn filtered(&self) -> Option<&FilterResult> {
        self.filtered.as_ref().map(|(_, result)| result)
    }

    /// Lines up every column of the page that has programmes on the day and is
    /// not lined up on the copy of its EPG the store holds: its cursor goes to
    /// the target (now, 19:00, or the filter's next match) and that row to the
    /// now-line. Those are the columns not lined up since the last `realign`,
    /// the ones whose EPG comes late, and the ones whose EPG a newer copy
    /// replaced, because the old copy's indexes mean nothing in the new one. A
    /// column the user moved in is the exception to the last: it keeps its
    /// programme, on the same row of the screen.
    pub fn align(&mut self, page: &Page, matches: Option<&HashMap<i64, Vec<usize>>>) {
        let t = self.target_time(page.now);
        for col in 0..page.cols {
            let Some(ch) = page.channel(self.page_start, col) else {
                break;
            };
            let id = ch.archive_id;
            let Some(epg) = page.store.get(id) else {
                continue;
            };
            let seen = self.aligned.get(&id).copied();
            if seen == Some(epg.fetched_at) {
                continue;
            }
            let entries = epg.entries();
            let range = self.day_range(entries);
            let kept = seen.is_some()
                && self.moved.contains(&id)
                && self.keep_place(id, entries, &range, page);
            if !kept {
                let target = match matches.and_then(|m| m.get(&id)) {
                    Some(hits) => time::match_target(entries, hits, t),
                    None => time::target_index(entries, range, t),
                };
                if !self.line_up(id, entries, target, page.line()) {
                    // The new copy has no programme on the day to stand on.
                    if seen.is_some() {
                        self.forget(id);
                    }
                    continue;
                }
            }
            self.aligned.insert(id, epg.fetched_at);
        }
    }

    /// Puts column `id` on `target`, an index into `entries`, and that row on
    /// the now-line (`line` rows from the top). False, and nothing changed,
    /// when there is no such programme.
    fn line_up(
        &mut self,
        id: i64,
        entries: &[EpgEntry],
        target: Option<usize>,
        line: usize,
    ) -> bool {
        let Some(target) = target else {
            return false;
        };
        if !self.set_cursor(id, entries, target) {
            return false;
        }
        self.top.insert(id, target as f32 - line as f32);
        self.moved.remove(&id);
        true
    }

    /// A newer copy of the EPG of a column the user moved in: the cursor goes
    /// to the programme running at the start it was on, else the first one
    /// after it, and the column scrolls by as many rows as the cursor moved, so
    /// the cursor stays on the same row of the screen. False when the new copy
    /// has nothing on the day to keep a place in.
    fn keep_place(
        &mut self,
        id: i64,
        entries: &[EpgEntry],
        range: &Range<usize>,
        page: &Page,
    ) -> bool {
        let (Some(&old), Some(&at), Some(&top)) = (
            self.cursor.get(&id),
            self.cursor_at.get(&id),
            self.top.get(&id),
        ) else {
            return false;
        };
        if range.is_empty() {
            return false;
        }
        let new = entries
            .partition_point(|e| e.end <= at)
            .clamp(range.start, range.end - 1);
        if !self.set_cursor(id, entries, new) {
            return false;
        }
        let top = top + (new as f32 - old as f32);
        self.top.insert(id, clamp_top(top, range, page));
        true
    }

    /// Drops what a column kept for a copy of its EPG that is gone.
    fn forget(&mut self, id: i64) {
        self.cursor.remove(&id);
        self.cursor_at.remove(&id);
        self.top.remove(&id);
        self.aligned.remove(&id);
        self.moved.remove(&id);
    }

    /// Puts the cursor of column `id` on programme `i` of `entries` and
    /// remembers when it starts. False, and nothing changed, when `entries` has
    /// no programme `i` (a list of matches worked out on an older copy).
    fn set_cursor(&mut self, id: i64, entries: &[EpgEntry], i: usize) -> bool {
        let Some(entry) = entries.get(i) else {
            return false;
        };
        self.cursor.insert(id, i);
        self.cursor_at.insert(id, entry.start);
        true
    }

    /// The user put the cursor on programme `i` of `entries`: a key (see
    /// `move_row`) or a click on it. The column counts as moved in, so a newer
    /// copy of its EPG keeps the programme instead of lining the column up.
    fn put_cursor(&mut self, id: i64, entries: &[EpgEntry], i: usize) {
        if self.set_cursor(id, entries, i) {
            self.moved.insert(id);
        }
    }

    /// Keeps the page and the cursor's column within the shown channels, which
    /// change under the guide with the filter, the lock or the catalog.
    pub fn clamp(&mut self, page: &Page) {
        let n = page.shown.len();
        self.page_start = self.page_start.min(n.saturating_sub(page.cols));
        let on_page = n.saturating_sub(self.page_start).min(page.cols);
        self.selected = self.selected.min(on_page.saturating_sub(1));
    }

    /// ← or →: the next column; at the edge the page moves one column.
    pub fn move_column(&mut self, page: &Page, right: bool) {
        let n = page.shown.len();
        if right {
            if self.selected + 1 < page.cols && self.page_start + self.selected + 1 < n {
                self.selected += 1;
            } else if self.page_start + page.cols < n {
                self.page_start += 1;
            }
        } else if self.selected > 0 {
            self.selected -= 1;
        } else if self.page_start > 0 {
            self.page_start -= 1;
        }
    }

    /// PgUp or PgDn, ‹ or ›: a page of channels, never past the last full page.
    pub fn move_page(&mut self, page: &Page, forward: bool) {
        let last = page.shown.len().saturating_sub(page.cols);
        self.page_start = if forward {
            (self.page_start + page.cols).min(last)
        } else {
            self.page_start.saturating_sub(page.cols)
        };
    }

    /// ↑ ↓ Home End in the cursor's column, within the day; the column scrolls
    /// along so the cursor stays in view.
    pub fn move_row(&mut self, page: &Page, step: RowStep) {
        let Some(ch) = page.channel(self.page_start, self.selected) else {
            return;
        };
        let id = ch.archive_id;
        let Some(epg) = page.store.get(id) else {
            return;
        };
        let entries = epg.entries();
        let range = self.day_range(entries);
        if range.is_empty() {
            return;
        }
        let last = range.end - 1;
        let current = self
            .cursor
            .get(&id)
            .copied()
            .unwrap_or(range.start)
            .clamp(range.start, last);
        let next = match step {
            RowStep::Up => current.saturating_sub(1).max(range.start),
            RowStep::Down => (current + 1).min(last),
            RowStep::First => range.start,
            RowStep::Last => last,
        };
        self.put_cursor(id, entries, next);
        let rows = page.rows.max(1) as f32;
        let top = self.top_of(id, &range, page);
        let wanted = if (next as f32) < top {
            next as f32
        } else if next as f32 > top + rows - 1.0 {
            next as f32 - rows + 1.0
        } else {
            top
        };
        self.top.insert(id, clamp_top(wanted, &range, page));
    }

    /// The wheel over column `col`: `rows` rows later (or earlier, negative).
    pub fn scroll(&mut self, page: &Page, col: usize, rows: f32) {
        let Some(ch) = page.channel(self.page_start, col) else {
            return;
        };
        let id = ch.archive_id;
        let Some(epg) = page.store.get(id) else {
            return;
        };
        let range = self.day_range(epg.entries());
        if range.is_empty() {
            return;
        }
        let top = self.top_of(id, &range, page) + rows;
        self.top.insert(id, clamp_top(top, &range, page));
        self.moved.insert(id);
    }

    /// A column's top row: as kept, else the day's first programme on the
    /// now-line.
    pub fn top_of(&self, id: i64, range: &Range<usize>, page: &Page) -> f32 {
        self.top
            .get(&id)
            .copied()
            .unwrap_or(range.start as f32 - page.line() as f32)
    }

    /// The programme under the cursor, to play.
    pub fn play_selected(&self, page: &Page) -> Option<GuideAction> {
        let ch = page.channel(self.page_start, self.selected)?;
        let i = *self.cursor.get(&ch.archive_id)?;
        let epg = page.store.get(ch.archive_id)?;
        if !self.day_range(epg.entries()).contains(&i) {
            return None;
        }
        self.play(page, ch, i)
    }

    /// Programme `i` of `ch`, to play.
    pub fn play(&self, page: &Page, ch: &GuideChannel, i: usize) -> Option<GuideAction> {
        let epg = page.store.get(ch.archive_id)?;
        let entry = epg.entries().get(i)?.clone();
        let playable = time::playable(
            &entry,
            ch.archive_days,
            epg.trusts_flag_for(&entry),
            page.now,
        );
        Some(GuideAction::Play {
            channel: ch.clone(),
            entry,
            playable,
        })
    }

    /// Esc: closes the date list; else wipes the filter; else closes the guide.
    pub fn escape(&mut self) -> Option<GuideAction> {
        if self.date_pick.take().is_some() {
            return None;
        }
        if !self.filter.is_empty() {
            self.filter.clear();
            self.realign();
            return None;
        }
        Some(GuideAction::Close)
    }
}

/// A column's top row, kept so the day's first programme never sinks below
/// the now-line and its last never rises above it.
fn clamp_top(top: f32, range: &Range<usize>, page: &Page) -> f32 {
    let line = page.line() as f32;
    let min = range.start as f32 - line;
    let max = (range.end as f32 - 1.0 - line).max(min);
    top.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epg::Epg;
    use crate::guide::store::ChannelEpg;
    use chrono::{Duration, TimeZone};

    /// The tests' "now": 12:00 UTC, the afternoon in Europe.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn channel(id: i64) -> GuideChannel {
        GuideChannel {
            name: format!("NL | ZENDER {id}"),
            live_id: id - 100,
            archive_id: id,
            archive_days: 7,
            logo: None,
            adult: false,
        }
    }

    /// A programme on every hour from 36 hours before `now()` to 36 after it;
    /// "P0" starts at `now()`.
    fn hourly() -> Epg {
        Epg::new(
            (-36..36)
                .map(|h| {
                    let start = now() + Duration::hours(h);
                    EpgEntry {
                        title: format!("P{h}"),
                        start,
                        end: start + Duration::hours(1),
                        has_archive: None,
                    }
                })
                .collect(),
        )
    }

    /// `hourly` as a later fetch gives it: the first six programmes are over
    /// and gone from the portal's listing, so every index moves by six.
    fn without_first_six() -> Epg {
        Epg::new(hourly().entries()[6..].to_vec())
    }

    /// That listing as a copy fetched five minutes after the fixture's.
    fn later_copy() -> ChannelEpg {
        ChannelEpg::new(without_first_six(), now() + Duration::minutes(5))
    }

    struct Fixture {
        channels: Vec<GuideChannel>,
        store: EpgStore,
        shown: Vec<usize>,
    }

    impl Fixture {
        /// `n` channels from 601 on, each with `hourly` EPG except those in `without`.
        fn new(n: i64, without: &[i64]) -> Self {
            let channels: Vec<GuideChannel> = (601..601 + n).map(channel).collect();
            let mut store = EpgStore::default();
            for c in &channels {
                if !without.contains(&c.archive_id) {
                    store.insert(c.archive_id, ChannelEpg::new(hourly(), now()));
                }
            }
            let shown = (0..channels.len()).collect();
            Self {
                channels,
                store,
                shown,
            }
        }

        /// Three columns of ten rows.
        fn page(&self) -> Page<'_> {
            Page {
                channels: &self.channels,
                store: &self.store,
                shown: &self.shown,
                cols: 3,
                rows: 10,
                now: now(),
                today: time::day_of(now()),
            }
        }

        /// The index of the programme on now in a channel's EPG.
        fn live(&self, id: i64) -> usize {
            let entries = self.store.get(id).unwrap().entries();
            entries
                .iter()
                .position(|e| e.start <= now() && now() < e.end)
                .unwrap()
        }
    }

    #[test]
    fn on_opening_every_column_lines_up_on_what_runs_now() {
        let f = Fixture::new(5, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.opened(page.today);
        v.align(&page, None);
        for id in [601, 602, 603] {
            let live = f.live(id);
            assert_eq!(v.cursor[&id], live);
            assert_eq!(
                v.top[&id],
                live as f32 - page.line() as f32,
                "the live row sits on the now-line"
            );
        }
        assert!(
            !v.cursor.contains_key(&604),
            "a column off the page waits until it shows"
        );
    }

    #[test]
    fn a_column_whose_epg_lands_late_lines_up_when_it_lands() {
        let mut f = Fixture::new(3, &[602]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        assert!(!v.cursor.contains_key(&602));
        f.store.insert(602, ChannelEpg::new(hourly(), now()));
        v.align(&f.page(), None);
        assert_eq!(v.cursor[&602], f.live(602));
    }

    #[test]
    fn after_lining_up_each_column_scrolls_on_its_own() {
        let f = Fixture::new(3, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.align(&page, None);
        let before = v.top[&601];
        v.scroll(&page, 0, -2.0);
        assert_eq!(v.top[&601], before - 2.0);
        assert_eq!(v.top[&602], before, "the other columns stay");
        v.move_column(&page, true);
        v.align(&page, None);
        assert_eq!(
            v.top[&601],
            before - 2.0,
            "moving to another column lines nothing up again"
        );
    }

    #[test]
    fn now_and_next_follows_the_day_past_midnight() {
        let f = Fixture::new(3, &[]);
        let today = time::day_of(now());
        let yesterday = today.pred_opt().unwrap();
        let mut v = GuideView::new(yesterday);
        v.align(&f.page(), None);
        v.follow_today(today);
        assert_eq!(v.day, today);
        v.align(&f.page(), None);
        assert_eq!(v.cursor[&601], f.live(601), "lined up on now again");
        // In Primetime a chosen day stays.
        let mut p = GuideView::new(today);
        p.pick_day(yesterday);
        p.follow_today(today);
        assert_eq!(p.day, yesterday);
    }

    #[test]
    fn primetime_lines_up_on_seven_in_the_evening_of_the_day() {
        let f = Fixture::new(3, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.primetime();
        v.align(&page, None);
        let entries = f.store.get(601).unwrap().entries();
        let seven = time::at_hour(page.today, time::PRIMETIME_HOUR);
        let e = &entries[v.cursor[&601]];
        assert!(
            e.start <= seven && seven < e.end,
            "{} runs at 19:00",
            e.title
        );
    }

    #[test]
    fn picking_another_day_goes_to_its_primetime() {
        let f = Fixture::new(3, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        let yesterday = page.today.pred_opt().unwrap();
        v.open_dates();
        v.move_date(&[yesterday, page.today], false);
        assert_eq!(v.date_pick, Some(yesterday));
        v.pick_day(yesterday);
        assert_eq!(
            (v.day, v.mode, v.date_pick),
            (yesterday, Mode::Primetime, None)
        );
        v.align(&page, None);
        let e = &f.store.get(601).unwrap().entries()[v.cursor[&601]];
        let seven = time::at_hour(yesterday, time::PRIMETIME_HOUR);
        assert!(e.start <= seven && seven < e.end);
    }

    #[test]
    fn up_and_down_move_the_cursor_and_keep_it_in_view() {
        let f = Fixture::new(3, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.align(&page, None);
        let live = v.cursor[&601];
        v.move_row(&page, RowStep::Down);
        assert_eq!(v.cursor[&601], live + 1);
        for _ in 0..20 {
            v.move_row(&page, RowStep::Down);
        }
        let (cur, top) = (v.cursor[&601] as f32, v.top[&601]);
        assert!(
            cur >= top && cur <= top + page.rows as f32 - 1.0,
            "the cursor stays in view"
        );
        let range = time::day_slice(f.store.get(601).unwrap().entries(), page.today);
        v.move_row(&page, RowStep::First);
        assert_eq!(v.cursor[&601], range.start);
        v.move_row(&page, RowStep::Up);
        assert_eq!(v.cursor[&601], range.start, "not before the day's first");
        v.move_row(&page, RowStep::Last);
        assert_eq!(v.cursor[&601], range.end - 1);
    }

    #[test]
    fn left_right_and_the_pages_stay_within_the_channels() {
        let f = Fixture::new(7, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.move_column(&page, true);
        v.move_column(&page, true);
        assert_eq!((v.page_start, v.selected), (0, 2));
        v.move_column(&page, true);
        assert_eq!(
            (v.page_start, v.selected),
            (1, 2),
            "at the edge the page moves"
        );
        v.move_page(&page, true);
        assert_eq!(v.page_start, 4, "a page on, never past the last full page");
        v.move_page(&page, true);
        assert_eq!(v.page_start, 4);
        v.move_page(&page, false);
        v.move_page(&page, false);
        assert_eq!(v.page_start, 0);
        for _ in 0..3 {
            v.move_column(&page, false);
        }
        assert_eq!((v.page_start, v.selected), (0, 0));
    }

    #[test]
    fn enter_plays_what_the_cursor_is_on() {
        let f = Fixture::new(3, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        v.align(&page, None);
        match v.play_selected(&page) {
            Some(GuideAction::Play {
                channel,
                entry,
                playable,
            }) => {
                assert_eq!(channel.archive_id, 601);
                assert!(entry.start <= now() && now() < entry.end);
                assert_eq!(playable, Playable::Live);
            }
            other => panic!("{other:?}"),
        }
        v.move_row(&page, RowStep::Up);
        assert!(matches!(
            v.play_selected(&page),
            Some(GuideAction::Play {
                playable: Playable::Replay,
                ..
            })
        ));
        v.move_row(&page, RowStep::Down);
        v.move_row(&page, RowStep::Down);
        assert!(matches!(
            v.play_selected(&page),
            Some(GuideAction::Play {
                playable: Playable::Future,
                ..
            })
        ));
    }

    #[test]
    fn escape_closes_the_date_list_then_wipes_the_filter_then_closes_the_guide() {
        let mut v = GuideView::new(time::day_of(now()));
        v.open_dates();
        v.filter = "nos".into();
        assert_eq!(v.escape(), None);
        assert_eq!(v.date_pick, None);
        assert_eq!(v.escape(), None);
        assert!(v.filter.is_empty());
        assert_eq!(v.escape(), Some(GuideAction::Close));
    }

    #[test]
    fn with_a_filter_a_column_lines_up_on_its_match() {
        let f = Fixture::new(2, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        let live = f.live(601);
        let mut matches = HashMap::new();
        matches.insert(601, vec![live + 2]);
        v.align(&page, Some(&matches));
        assert_eq!(v.cursor[&601], live + 2);
        assert_eq!(
            v.cursor[&602],
            f.live(602),
            "a column found by its name lines up as usual"
        );
    }

    #[test]
    fn the_page_and_the_cursor_stay_valid_when_the_list_shrinks() {
        let mut v = GuideView::new(time::day_of(now()));
        v.page_start = 4;
        v.selected = 2;
        let fewer = Fixture {
            shown: vec![0, 1],
            ..Fixture::new(7, &[])
        };
        v.clamp(&fewer.page());
        assert_eq!((v.page_start, v.selected), (0, 1));
        let none = Fixture {
            shown: vec![],
            ..Fixture::new(7, &[])
        };
        v.clamp(&none.page());
        assert_eq!((v.page_start, v.selected), (0, 0));
    }

    #[test]
    fn the_filter_result_is_worked_out_again_only_when_something_changes() {
        let f = Fixture::new(3, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.filter = "zender 602".into();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert_eq!(v.filtered().unwrap().channels, [1]);
        let kept = v.filtered().unwrap().channels.as_ptr();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert_eq!(
            v.filtered().unwrap().channels.as_ptr(),
            kept,
            "the same inputs: not worked out again"
        );
        v.filter.clear();
        v.refresh_filter(&f.channels, &f.store, 2);
        assert!(v.filtered().is_none());
    }

    #[test]
    fn the_kept_day_bounds_follow_every_change_of_day() {
        let f = Fixture::new(1, &[]);
        let entries = f.store.get(601).unwrap().entries();
        let today = time::day_of(now());
        let yesterday = today.pred_opt().unwrap();
        let tomorrow = today.succ_opt().unwrap();
        // The check means something only while the days differ in the listing.
        let (a, b, c) = (
            time::day_slice(entries, yesterday),
            time::day_slice(entries, today),
            time::day_slice(entries, tomorrow),
        );
        assert!(a != b && b != c, "the fixture tells the days apart");
        let mut v = GuideView::new(today);
        assert_eq!(v.day_range(entries), b, "new");
        v.pick_day(yesterday);
        assert_eq!(v.day_range(entries), a, "a day from the date list");
        v.now_and_next(today);
        assert_eq!(v.day_range(entries), b, "Nu & straks");
        v.follow_today(tomorrow);
        assert_eq!(v.day, tomorrow);
        assert_eq!(v.day_range(entries), c, "past midnight");
        v.opened(today);
        assert_eq!(v.day_range(entries), b, "the guide opens");
    }

    #[test]
    fn a_new_copy_of_a_columns_epg_lines_it_up_again() {
        let mut f = Fixture::new(1, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        assert_eq!(v.cursor[&601], f.live(601));
        // A fetch lands: the first six programmes are gone, every index moves.
        f.store.insert(601, later_copy());
        let page = f.page();
        v.align(&page, None);
        let live = f.live(601);
        assert_eq!(v.cursor[&601], live, "the live row of the new copy");
        assert_eq!(
            v.top[&601],
            live as f32 - page.line() as f32,
            "on the now-line"
        );
    }

    #[test]
    fn a_column_the_user_moved_in_keeps_its_programme_on_a_new_copy() {
        let mut f = Fixture::new(1, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        v.move_row(&f.page(), RowStep::Down);
        v.move_row(&f.page(), RowStep::Down);
        let old = v.cursor[&601];
        let title = f.store.get(601).unwrap().entries()[old].title.clone();
        let row = old as f32 - v.top[&601];
        f.store.insert(601, later_copy());
        v.align(&f.page(), None);
        let entries = f.store.get(601).unwrap().entries();
        assert_eq!(entries[v.cursor[&601]].title, title, "the same programme");
        assert_eq!(v.cursor[&601], old - 6, "six programmes fewer before it");
        assert_eq!(
            v.cursor[&601] as f32 - v.top[&601],
            row,
            "on the same row of the screen"
        );
        assert_ne!(v.cursor[&601], f.live(601), "not lined up on now again");
    }

    #[test]
    fn a_scrolled_column_keeps_its_place_and_another_columns_copy_leaves_it_alone() {
        let mut f = Fixture::new(2, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        v.scroll(&f.page(), 0, -2.0);
        let (cursor, top) = (v.cursor[&601], v.top[&601]);
        // 602 gets a newer copy, 601 still has the one it was lined up on.
        f.store.insert(602, later_copy());
        v.align(&f.page(), None);
        assert_eq!(
            (v.cursor[&601], v.top[&601]),
            (cursor, top),
            "the other column's copy does not touch this one"
        );
        assert_eq!(
            v.cursor[&602],
            f.live(602),
            "602, not moved in, lines up again"
        );
        // Now 601 gets one: the wheel moved it in, so the same programmes
        // stay on the same rows.
        f.store.insert(601, later_copy());
        v.align(&f.page(), None);
        assert_eq!(v.cursor[&601], cursor - 6);
        assert_eq!(v.top[&601], top - 6.0);
    }

    #[test]
    fn a_shorter_or_empty_new_copy_leaves_no_stale_index_behind() {
        let mut f = Fixture::new(1, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        v.move_row(&f.page(), RowStep::Last);
        let at = now() + Duration::minutes(5);
        // A copy that ends long before the programme the cursor was on.
        let short = hourly().entries()[30..40].to_vec();
        f.store.insert(601, ChannelEpg::new(Epg::new(short), at));
        v.align(&f.page(), None);
        let entries = f.store.get(601).unwrap().entries();
        let range = v.day_range(entries);
        assert!(range.end <= entries.len());
        assert_eq!(
            v.cursor[&601],
            range.end - 1,
            "the last programme of the day the new copy has"
        );
        // A copy without programmes: nothing of the old one is kept.
        let empty = ChannelEpg::new(Epg::new(Vec::new()), at + Duration::minutes(5));
        f.store.insert(601, empty);
        v.align(&f.page(), None);
        assert!(!v.cursor.contains_key(&601) && !v.top.contains_key(&601));
        assert_eq!(v.play_selected(&f.page()), None);
        v.move_row(&f.page(), RowStep::Down);
        // The programmes come back: the column lines up as on a first copy.
        let back = ChannelEpg::new(hourly(), at + Duration::minutes(10));
        f.store.insert(601, back);
        v.align(&f.page(), None);
        assert_eq!(v.cursor[&601], f.live(601));
    }

    #[test]
    fn a_new_copy_without_the_programme_the_cursor_was_on_moves_it_to_the_next_one_of_the_day() {
        let mut f = Fixture::new(1, &[]);
        let mut v = GuideView::new(time::day_of(now()));
        v.align(&f.page(), None);
        v.move_row(&f.page(), RowStep::Down);
        v.move_row(&f.page(), RowStep::Down);
        // A copy fetched `minutes` after the fixture's: the first six
        // programmes are gone, and so are those named in `gone`.
        let without = |gone: &[&str], minutes: i64| {
            let kept = without_first_six()
                .entries()
                .iter()
                .filter(|e| !gone.contains(&e.title.as_str()))
                .cloned()
                .collect();
            ChannelEpg::new(Epg::new(kept), now() + Duration::minutes(minutes))
        };
        let title = |f: &Fixture, v: &GuideView| {
            f.store.get(601).unwrap().entries()[v.cursor[&601]]
                .title
                .clone()
        };
        assert_eq!(title(&f, &v), "P2");
        f.store.insert(601, without(&["P2"], 5));
        v.align(&f.page(), None);
        assert_eq!(title(&f, &v), "P3", "the first one after it");
        // On the day's last programme the first one after it is tomorrow's:
        // the cursor stays on the day.
        v.move_row(&f.page(), RowStep::Last);
        let last = title(&f, &v);
        f.store.insert(601, without(&["P2", last.as_str()], 10));
        v.align(&f.page(), None);
        let entries = f.store.get(601).unwrap().entries();
        let range = v.day_range(entries);
        assert_eq!(
            v.cursor[&601],
            range.end - 1,
            "the last programme of the day that is left"
        );
        assert_ne!(entries[v.cursor[&601]].title, last);
    }

    #[test]
    fn a_match_list_from_an_older_copy_cannot_put_the_cursor_outside_the_listing() {
        let f = Fixture::new(1, &[]);
        let page = f.page();
        let mut v = GuideView::new(page.today);
        // Matches worked out on a longer copy: programme 500 is not there.
        let mut matches = HashMap::new();
        matches.insert(601, vec![500]);
        v.align(&page, Some(&matches));
        assert!(
            !v.cursor.contains_key(&601),
            "no such programme: nothing to stand on"
        );
        v.align(&page, None);
        assert_eq!(
            v.cursor[&601],
            f.live(601),
            "lined up once it is asked right"
        );
    }

    #[test]
    fn the_filter_result_is_worked_out_again_when_any_part_of_its_key_changes() {
        let mut f = Fixture::new(3, &[]);
        let today = time::day_of(now());
        let mut v = GuideView::new(today);
        v.filter = "special".into();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert!(v.filtered().unwrap().channels.is_empty());
        // The store: 602 gets a newer copy with "Special" on now.
        let mut entries = hourly().entries().to_vec();
        entries[36].title = "Special".into();
        let copy = ChannelEpg::new(Epg::new(entries), now() + Duration::minutes(5));
        f.store.insert(602, copy);
        v.refresh_filter(&f.channels, &f.store, 1);
        let found = v.filtered().unwrap();
        assert_eq!(found.channels, [1], "the store changed");
        assert_eq!(found.matches[&602], [36]);
        // The text.
        v.filter = "zender 601".into();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert_eq!(v.filtered().unwrap().channels, [0], "the text changed");
        // The day: "Special" is on today, not on yesterday.
        v.filter = "special".into();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert_eq!(v.filtered().unwrap().channels, [1]);
        v.pick_day(today.pred_opt().unwrap());
        v.refresh_filter(&f.channels, &f.store, 1);
        assert!(v.filtered().unwrap().channels.is_empty(), "the day changed");
        // The channel list: its revision moves when it changes.
        v.filter = "zender".into();
        v.refresh_filter(&f.channels, &f.store, 1);
        assert_eq!(v.filtered().unwrap().channels, [0, 1, 2]);
        v.refresh_filter(&f.channels[..2], &f.store, 2);
        assert_eq!(
            v.filtered().unwrap().channels,
            [0, 1],
            "the channels changed"
        );
    }

    #[test]
    fn play_trusts_a_zero_for_has_archive_only_for_a_programme_over_at_the_fetch() {
        let mut f = Fixture::new(1, &[601]);
        let at = |hours: i64| now() + Duration::hours(hours);
        let entry = |title: &str, from: i64, to: i64, has_archive: Option<bool>| EpgEntry {
            title: title.into(),
            start: at(from),
            end: at(to),
            has_archive,
        };
        // The portal says 1 for the first, 0 for the others. The copy was
        // fetched three hours ago: the third was still running then.
        let epg = Epg::new(vec![
            entry("Kept", -6, -5, Some(true)),
            entry("Gone", -5, -4, Some(false)),
            entry("Running at the fetch", -4, -1, Some(false)),
        ]);
        f.store.insert(601, ChannelEpg::new(epg, at(-3)));
        let page = f.page();
        let v = GuideView::new(page.today);
        let playable = |i: usize| match v.play(&page, &f.channels[0], i) {
            Some(GuideAction::Play { playable, .. }) => playable,
            other => panic!("{other:?}"),
        };
        assert_eq!(playable(0), Playable::Replay);
        assert_eq!(playable(1), Playable::Unavailable, "a 0 over at the fetch");
        assert_eq!(
            playable(2),
            Playable::Replay,
            "a 0 for a programme that was still running says nothing"
        );
    }
}
