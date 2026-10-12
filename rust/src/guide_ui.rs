//! The guide (`g`): one column per channel with catch-up, live and catch-up in
//! one list, NLZIET style. `GuideView` keeps the guide's state for the whole
//! session; `show` draws one frame and hands what the user did to app.rs as a
//! [`GuideAction`], as settings_ui.rs does.

use crate::catalog::GuideChannel;
use crate::epg::EpgEntry;
use crate::guide::logos::LogoStore;
use crate::guide::store::{filter_channels, EpgStore, FilterResult};
use crate::guide::time::{self, Playable};
use crate::i18n::{self, Lang, T};
use chrono::{DateTime, NaiveDate, Utc};
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};
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
    /// `0..channels.len()`, made for the channel list `all_for`: what shows
    /// without a filter, so a frame does not build it.
    all: Vec<usize>,
    all_for: Option<u64>,
    /// The date list's days, and the store revision, channel list and day
    /// they were worked out for.
    days: Vec<NaiveDate>,
    days_for: Option<(u64, u64, NaiveDate)>,
    /// The filter box had the keyboard in the frame before: what it typed, and
    /// the Enter or Esc that left it, is not the guide's.
    filter_had_focus: bool,
    /// The keyboard went into the filter box by `f` or a click. It gets there
    /// no other way: a Tab that puts it there is undone.
    filter_wanted: bool,
    /// Index into the guide's channel list of the leftmost column, and how many
    /// columns show: where the fetcher starts.
    visible: (usize, usize),
    /// Texts that change now and then, made when they do.
    labels: Labels,
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
            all: Vec::new(),
            all_for: None,
            days: Vec::new(),
            days_for: None,
            filter_had_focus: false,
            filter_wanted: false,
            visible: (0, 0),
            labels: Labels::default(),
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
        // The box does not keep the keyboard over a close and an open.
        self.filter_had_focus = false;
        self.filter_wanted = false;
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

/// The filter box: the one widget in the guide that takes the keyboard.
pub fn filter_id() -> egui::Id {
    egui::Id::new("__guide_filter__")
}

/// A click target the keyboard never reaches: the guide's keys are its own,
/// and a focused widget would take Enter or Space for a click.
const CLICK: Sense = Sense {
    click: true,
    drag: false,
    focusable: false,
};

const COLUMN_W: f32 = 230.0;
const COLUMN_GAP: f32 = 8.0;
const ROW_H: f32 = 46.0;
const HEADER_H: f32 = 58.0;
const TOP_BAR_H: f32 = 60.0;
const FOOTER_H: f32 = 24.0;
const MARGIN: f32 = 16.0;
/// Days the date list shows at once, and the height of each.
const DATE_ROWS: usize = 10;
const DATE_ROW_H: f32 = 30.0;

const BG: Color32 = Color32::from_rgb(14, 14, 18);
const PANEL: Color32 = Color32::from_rgb(32, 32, 40);
const PANEL_HOVER: Color32 = Color32::from_rgb(44, 44, 54);
const ROW_BG: Color32 = Color32::from_rgb(22, 22, 28);
const ROW_HOVER: Color32 = Color32::from_rgb(30, 30, 38);
const ROW_LIVE: Color32 = Color32::from_rgb(28, 58, 30);
const ROW_LIVE_HOVER: Color32 = Color32::from_rgb(34, 70, 38);
const ROW_SELECTED: Color32 = Color32::from_rgb(40, 55, 80);
const ACCENT: Color32 = Color32::from_rgb(255, 80, 140);
const LIVE_GREEN: Color32 = Color32::from_rgb(120, 230, 100);
const REPLAY_BLUE: Color32 = Color32::from_rgb(140, 180, 220);
const MATCH_YELLOW: Color32 = Color32::from_rgb(230, 220, 110);

/// What the guide shows but app.rs owns.
pub struct GuideData<'a> {
    pub lang: Lang,
    pub channels: &'a [GuideChannel],
    /// Counts every new channel list (catalog, lock): what is worked out from
    /// the list is worked out again when it moves.
    pub channels_revision: u64,
    pub store: &'a EpgStore,
    /// Catch-up ids whose EPG is being fetched: their header shows a dot.
    pub fetching: &'a HashSet<i64>,
    /// What plays, for "playing: …" in the top bar.
    pub playing: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub today: NaiveDate,
    /// The guide reads the keyboard (not while the help is over it).
    pub keys: bool,
}

/// The keys of one frame, as the guide uses them.
struct Keys {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    page_up: bool,
    page_down: bool,
    home: bool,
    end: bool,
    enter: bool,
    esc: bool,
    now_next: bool,
    primetime: bool,
    dates: bool,
    filter: bool,
    close: bool,
    help: bool,
}

fn read_keys(ctx: &egui::Context) -> Keys {
    ctx.input(|i| Keys {
        left: i.key_pressed(Key::ArrowLeft),
        right: i.key_pressed(Key::ArrowRight),
        up: i.key_pressed(Key::ArrowUp),
        down: i.key_pressed(Key::ArrowDown),
        page_up: i.key_pressed(Key::PageUp),
        page_down: i.key_pressed(Key::PageDown),
        home: i.key_pressed(Key::Home),
        end: i.key_pressed(Key::End),
        enter: i.key_pressed(Key::Enter),
        esc: i.key_pressed(Key::Escape),
        now_next: i.key_pressed(Key::N),
        primetime: i.key_pressed(Key::P),
        dates: i.key_pressed(Key::D),
        filter: i.key_pressed(Key::F),
        close: i.key_pressed(Key::G),
        help: i
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
    })
}

/// Texts that change now and then: made when they change, not per frame.
#[derive(Default)]
struct Labels {
    day: Option<(NaiveDate, NaiveDate, Lang, String)>,
    range: Option<((usize, usize, usize, Lang), String)>,
    playing: Option<(String, Lang, String)>,
}

impl Labels {
    fn day(&mut self, lang: Lang, day: NaiveDate, today: NaiveDate) -> &str {
        if !matches!(&self.day, Some((d, t, l, _)) if *d == day && *t == today && *l == lang) {
            self.day = Some((day, today, lang, i18n::day_label(lang, day, today)));
        }
        self.day.as_ref().map_or("", |(_, _, _, s)| s.as_str())
    }

    fn range(&mut self, lang: Lang, from: usize, to: usize, of: usize) -> &str {
        let key = (from, to, of, lang);
        if !matches!(&self.range, Some((k, _)) if *k == key) {
            let text = i18n::tr(
                lang,
                T::GuideChannelsRange,
                &[&from.to_string(), &to.to_string(), &of.to_string()],
            );
            self.range = Some((key, text));
        }
        self.range.as_ref().map_or("", |(_, s)| s.as_str())
    }

    fn playing(&mut self, lang: Lang, what: &str) -> &str {
        if !matches!(&self.playing, Some((w, l, _)) if w == what && *l == lang) {
            let text = i18n::tr(lang, T::GuidePlaying, &[what]);
            self.playing = Some((what.to_owned(), lang, text));
        }
        self.playing.as_ref().map_or("", |(_, _, s)| s.as_str())
    }
}

/// Where the top bar's parts go in a bar `bar`; it narrows down to the 640 px
/// minimum window.
struct TopBar {
    date: Rect,
    modes: Rect,
    filter: Rect,
    info: Rect,
    prev: Rect,
    next: Rect,
}

impl TopBar {
    fn new(bar: Rect) -> Self {
        let narrow = bar.width() < 900.0;
        let h = 36.0;
        let y = bar.center().y - h / 2.0;
        let at = |x: f32, w: f32| Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        let (date_w, modes_w, filter_w) = if narrow {
            (120.0, 190.0, 150.0)
        } else {
            (160.0, 250.0, 240.0)
        };
        let date = at(bar.left() + MARGIN, date_w);
        let modes = at(date.right() + 10.0, modes_w);
        let filter = at(modes.right() + 10.0, filter_w);
        let next = at(bar.right() - MARGIN - h, h);
        let prev = at(next.left() - 6.0 - h, h);
        let info = Rect::from_min_max(
            Pos2::new(filter.right() + 14.0, y),
            Pos2::new((prev.left() - 12.0).max(filter.right() + 14.0), y + h),
        );
        Self {
            date,
            modes,
            filter,
            info,
            prev,
            next,
        }
    }
}

impl GuideView {
    /// Index into the guide's channel list of the leftmost column, and how many
    /// columns show: where the fetcher starts.
    pub fn visible_window(&self) -> (usize, usize) {
        self.visible
    }

    /// One frame of the guide over the whole window. Per frame it works only on
    /// the visible rows of the visible columns.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        data: &GuideData,
        logos: &mut LogoStore,
    ) -> Option<GuideAction> {
        let screen = ctx.screen_rect();
        let cols = (((screen.width() - 2.0 * MARGIN + COLUMN_GAP) / (COLUMN_W + COLUMN_GAP))
            as usize)
            .clamp(1, 10);
        let area = Rect::from_min_max(
            Pos2::new(screen.left(), screen.top() + TOP_BAR_H),
            Pos2::new(screen.right(), screen.bottom() - FOOTER_H),
        );
        let rows = ((area.height() - HEADER_H) / ROW_H).floor().max(1.0) as usize;
        let bar = TopBar::new(Rect::from_min_size(
            screen.min,
            Vec2::new(screen.width(), TOP_BAR_H),
        ));

        self.follow_today(data.today);
        if self.all_for != Some(data.channels_revision) || self.all.len() != data.channels.len() {
            self.all = (0..data.channels.len()).collect();
            self.all_for = Some(data.channels_revision);
        }
        self.refresh_filter(data.channels, data.store, data.channels_revision);
        // Out of `self` for this frame, so the page can borrow them while `self`
        // changes; moving a Vec allocates nothing.
        let filtered = self.filtered.take();
        let all = std::mem::take(&mut self.all);
        let (shown, matches) = match &filtered {
            Some((_, result)) => (&result.channels[..], Some(&result.matches)),
            None => (&all[..], None),
        };
        let page = Page {
            channels: data.channels,
            store: data.store,
            shown,
            cols,
            rows,
            now: data.now,
            today: data.today,
        };
        self.clamp(&page);
        // Before the keys: `refresh_filter` has just made the day, the mode and
        // the matches agree, and a key steps from rows that match the copy of
        // the EPG the store holds now. A key that changes the day or the mode
        // calls `realign`, and the next frame lines the columns up on it.
        self.align(&page, matches);

        let mut action = None;
        let keys = read_keys(ctx);
        let focused = ctx.memory(|m| m.has_focus(filter_id()));
        if data.keys && !focused && !self.filter_had_focus {
            action = self.keys(ctx, &keys, &page);
        } else if focused && keys.down {
            ctx.memory_mut(|m| m.surrender_focus(filter_id()));
        }
        self.clamp(&page);
        let on_page = page.shown.len().saturating_sub(self.page_start).min(cols);
        self.visible = (
            page.shown.get(self.page_start).copied().unwrap_or(0),
            on_page,
        );

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                self.top_bar(ui, data, &page, &bar);
                if let Some(a) = self.columns(ui, data, &page, matches, logos, area) {
                    action = Some(a);
                }
                // Right-aligned; in a window too narrow for it, from the left
                // margin on and ending in "…".
                let color = Color32::from_white_alpha(95);
                let footer = i18n::t(data.lang, T::GuideKeysFooter);
                let job = single_line(
                    footer,
                    FontId::proportional(11.0),
                    color,
                    screen.width() - 2.0 * MARGIN,
                );
                let p = ui.painter();
                let galley = p.layout_job(job);
                let x = (screen.right() - MARGIN - galley.size().x).max(screen.left() + MARGIN);
                let y = screen.bottom() - FOOTER_H / 2.0 - galley.size().y / 2.0;
                p.galley(Pos2::new(x, y), galley, color);
            });
        if self.date_pick.is_some() {
            // Here, after the keys and the clicks of this frame: a list that
            // opens has its days at once, so its window is made in this frame
            // (egui keeps a new window invisible for its first frame) and shows
            // in the next.
            self.refresh_days(data);
            self.date_list(ctx, data, bar.date);
        }
        self.filter_had_focus = ctx.memory(|m| m.has_focus(filter_id()));
        self.filtered = filtered;
        self.all = all;
        action
    }

    /// The keyboard, while the filter box does not have it.
    fn keys(&mut self, ctx: &egui::Context, k: &Keys, page: &Page) -> Option<GuideAction> {
        if self.date_pick.is_some() {
            let days = std::mem::take(&mut self.days);
            if k.up {
                self.move_date(&days, false);
            }
            if k.down {
                self.move_date(&days, true);
            }
            self.days = days;
            if k.enter {
                if let Some(day) = self.date_pick {
                    self.pick_day(day);
                }
            }
            if k.esc {
                self.date_pick = None;
            }
            return k.close.then_some(GuideAction::Close);
        }
        if k.help {
            return Some(GuideAction::Help);
        }
        if k.esc {
            return self.escape();
        }
        if k.close {
            return Some(GuideAction::Close);
        }
        if k.filter {
            // The f puts the keyboard in the box; it is no letter typed into it.
            ctx.input_mut(|i| {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Text(t) if t.eq_ignore_ascii_case("f")))
            });
            ctx.memory_mut(|m| m.request_focus(filter_id()));
            self.filter_wanted = true;
            return None;
        }
        if k.now_next {
            self.now_and_next(page.today);
        }
        if k.primetime {
            self.primetime();
        }
        if k.dates {
            self.open_dates();
        }
        if k.left {
            self.move_column(page, false);
        }
        if k.right {
            self.move_column(page, true);
        }
        if k.page_up {
            self.move_page(page, false);
        }
        if k.page_down {
            self.move_page(page, true);
        }
        if k.up {
            self.move_row(page, RowStep::Up);
        }
        if k.down {
            self.move_row(page, RowStep::Down);
        }
        if k.home {
            self.move_row(page, RowStep::First);
        }
        if k.end {
            self.move_row(page, RowStep::Last);
        }
        if k.enter {
            return self.play_selected(page);
        }
        None
    }

    /// The date list's days, worked out again when the store, the channels or
    /// the day changed.
    fn refresh_days(&mut self, data: &GuideData) {
        let key = (data.store.revision(), data.channels_revision, data.today);
        if self.days_for == Some(key) {
            return;
        }
        let back = data
            .channels
            .iter()
            .map(|c| c.archive_days)
            .max()
            .unwrap_or(0);
        self.days = time::date_range(data.today, back, data.store.last_day(time::day_of));
        self.days_for = Some(key);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, data: &GuideData, page: &Page, bar: &TopBar) {
        let lang = data.lang;
        // The date.
        let date = ui.interact(bar.date, egui::Id::new("__guide_date__"), CLICK);
        {
            let p = ui.painter();
            let fill = if date.hovered() { PANEL_HOVER } else { PANEL };
            p.rect_filled(bar.date, 6.0, fill);
            p.text(
                bar.date.left_center() + Vec2::new(14.0, 0.0),
                Align2::LEFT_CENTER,
                self.labels.day(lang, self.day, data.today),
                FontId::proportional(15.0),
                Color32::WHITE,
            );
            chevron_down(p, bar.date.right_center() - Vec2::new(16.0, 0.0));
        }
        if date.clicked() {
            if self.date_pick.is_some() {
                self.date_pick = None;
            } else {
                self.open_dates();
            }
        }
        // Nu & straks | Primetime.
        ui.painter().rect_filled(bar.modes, 6.0, PANEL);
        let mid = bar.modes.center().x;
        let now_half = Rect::from_min_max(bar.modes.min, Pos2::new(mid, bar.modes.max.y));
        let prime_half = Rect::from_min_max(Pos2::new(mid, bar.modes.min.y), bar.modes.max);
        let now_text = i18n::t(lang, T::GuideNow);
        let now_active = self.mode == Mode::NowAndNext;
        if segment(ui, now_half, "__guide_now__", now_text, now_active).clicked() {
            self.now_and_next(data.today);
        }
        let prime_text = i18n::t(lang, T::GuidePrime);
        let prime_active = self.mode == Mode::Primetime;
        if segment(ui, prime_half, "__guide_prime__", prime_text, prime_active).clicked() {
            self.primetime();
        }
        // The filter box: the keyboard gets there by `f` or a click only, and
        // not at all while the help is over the guide. It is as big as its slot
        // like the buttons beside it: the margins make up the height the text
        // does not need (which also keeps the hint text, drawn at the top of
        // the text area, level with what is typed).
        let font = FontId::proportional(15.0);
        let row_h = ui.fonts(|f| f.row_height(&font));
        let pad = egui::Margin::symmetric(8.0, ((bar.filter.height() - row_h) / 2.0).max(0.0));
        let edit = egui::TextEdit::singleline(&mut self.filter)
            .id(filter_id())
            .hint_text(i18n::t(lang, T::GuideFilterHint))
            .font(font)
            .margin(pad)
            .desired_width(bar.filter.width() - pad.left - pad.right);
        let response = ui
            .allocate_ui_at_rect(bar.filter, |ui| ui.add_enabled(data.keys, edit))
            .inner;
        if response.clicked() {
            self.filter_wanted = true;
        }
        if response.gained_focus() && !self.filter_wanted {
            response.surrender_focus();
        }
        if response.lost_focus() {
            self.filter_wanted = false;
        }
        if response.changed() {
            self.realign();
        }
        // ‹ › a page of channels.
        if chevron_button(ui, bar.prev, "__guide_prev__", false).clicked() {
            self.move_page(page, false);
        }
        if chevron_button(ui, bar.next, "__guide_next__", true).clicked() {
            self.move_page(page, true);
        }
        // Which channels show, and what plays.
        if bar.info.width() > 90.0 && !page.shown.is_empty() {
            let from = self.page_start + 1;
            let to = (self.page_start + page.cols).min(page.shown.len());
            ui.painter().text(
                bar.info.right_center(),
                Align2::RIGHT_CENTER,
                self.labels.range(lang, from, to, page.shown.len()),
                FontId::proportional(12.0),
                Color32::from_white_alpha(150),
            );
            let room = bar.info.width() - 180.0;
            if let (Some(what), true) = (data.playing, room > 60.0) {
                let text = self.labels.playing(lang, what);
                let p = ui.painter();
                let job = single_line(text, FontId::proportional(12.0), LIVE_GREEN, room);
                let galley = p.layout_job(job);
                let at = Pos2::new(bar.info.left(), bar.info.center().y - galley.size().y / 2.0);
                p.galley(at, galley, LIVE_GREEN);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn columns(
        &mut self,
        ui: &egui::Ui,
        data: &GuideData,
        page: &Page,
        matches: Option<&HashMap<i64, Vec<usize>>>,
        logos: &mut LogoStore,
        area: Rect,
    ) -> Option<GuideAction> {
        if page.shown.is_empty() {
            let at = Pos2::new(area.center().x, area.top() + 80.0);
            let (font, color) = (FontId::proportional(16.0), Color32::from_white_alpha(140));
            let p = ui.painter();
            if data.channels.is_empty() {
                let message = i18n::t(data.lang, T::GuideNoChannels);
                p.text(at, Align2::CENTER_CENTER, message, font, color);
            } else {
                let message = i18n::tr(data.lang, T::GuideNothingFound, &[self.filter.trim()]);
                p.text(at, Align2::CENTER_CENTER, message, font, color);
            }
            return None;
        }
        let (hover, wheel) = ui
            .ctx()
            .input(|i| (i.pointer.hover_pos(), i.raw_scroll_delta.y));
        let live_label = i18n::t(data.lang, T::GuideLiveBadge);
        let mut action = None;
        for col in 0..page.cols {
            let Some(ch) = page.channel(self.page_start, col) else {
                break;
            };
            let left = area.left() + MARGIN + col as f32 * (COLUMN_W + COLUMN_GAP);
            let column = Rect::from_min_max(
                Pos2::new(left, area.top()),
                Pos2::new(left + COLUMN_W, area.bottom()),
            );
            let loading = data.fetching.contains(&ch.archive_id)
                || ch.logo.as_deref().is_some_and(|url| logos.is_loading(url));
            paint_header(ui, logos, ch, column, loading);
            let rows = Rect::from_min_max(Pos2::new(left, area.top() + HEADER_H), column.max);
            if wheel != 0.0 && hover.is_some_and(|p| rows.contains(p)) {
                self.scroll(page, col, -wheel / ROW_H);
            }
            if let Some(a) = self.rows(ui, page, matches, col, ch, rows, live_label) {
                action = Some(a);
            }
        }
        action
    }

    /// The visible rows of one column; a click plays the programme.
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &mut self,
        ui: &egui::Ui,
        page: &Page,
        matches: Option<&HashMap<i64, Vec<usize>>>,
        col: usize,
        ch: &GuideChannel,
        area: Rect,
        live_label: &str,
    ) -> Option<GuideAction> {
        let epg = page.store.get(ch.archive_id)?;
        let entries = epg.entries();
        let range = self.day_range(entries);
        if range.is_empty() {
            return None;
        }
        let top = self.top_of(ch.archive_id, &range, page);
        let first = top.floor();
        let shift = (top - first) * ROW_H;
        let cursor = if col == self.selected {
            self.cursor.get(&ch.archive_id).copied()
        } else {
            None
        };
        let hits = matches.and_then(|m| m.get(&ch.archive_id));
        let painter = ui.painter().with_clip_rect(area);
        let mut clicked = None;
        for r in 0..=page.rows + 1 {
            let i = first as i64 + r as i64;
            if i < range.start as i64 || i >= range.end as i64 {
                continue;
            }
            let i = i as usize;
            let y = area.top() + r as f32 * ROW_H - shift;
            if y >= area.bottom() {
                break;
            }
            let size = Vec2::new(area.width(), ROW_H - 3.0);
            let row = Rect::from_min_size(Pos2::new(area.left(), y), size);
            let entry = &entries[i];
            let trusts_flag = epg.trusts_flag_for(entry);
            let playable = time::playable(entry, ch.archive_days, trusts_flag, page.now);
            let id = egui::Id::new(("__guide_row__", ch.archive_id, i));
            let response = ui.interact(row.intersect(area), id, CLICK);
            let look = RowLook {
                playable,
                cursor: cursor == Some(i),
                hovered: response.hovered(),
                hit: hits.is_some_and(|h| h.binary_search(&i).is_ok()),
            };
            paint_row(&painter, row, entry, look, page.now, live_label);
            if response.clicked() {
                clicked = Some(i);
            }
        }
        let i = clicked?;
        self.selected = col;
        // A click is a move like a key: the column keeps this programme on a
        // newer copy of its EPG.
        self.put_cursor(ch.archive_id, entries, i);
        self.play(page, ch, i)
    }

    /// The date list under the date button; a click picks a day.
    fn date_list(&mut self, ctx: &egui::Context, data: &GuideData, anchor: Rect) {
        let Some(pick) = self.date_pick else {
            return;
        };
        let n = self.days.len();
        if n == 0 {
            return;
        }
        let i = self.days.iter().position(|d| *d == pick).unwrap_or(0);
        let first = i
            .saturating_sub(DATE_ROWS / 2)
            .min(n.saturating_sub(DATE_ROWS));
        let count = DATE_ROWS.min(n);
        let mut clicked = None;
        egui::Area::new(egui::Id::new("__guide_dates__"))
            .order(egui::Order::Foreground)
            .sense(Sense::hover())
            .fixed_pos(anchor.left_bottom() + Vec2::new(0.0, 4.0))
            .show(ctx, |ui| {
                let size = Vec2::new(anchor.width(), count as f32 * DATE_ROW_H);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 6.0, PANEL);
                for (k, day) in self.days.iter().enumerate().skip(first).take(count) {
                    let row = Rect::from_min_size(
                        rect.min + Vec2::new(0.0, (k - first) as f32 * DATE_ROW_H),
                        Vec2::new(rect.width(), DATE_ROW_H),
                    );
                    let response = ui.interact(row, egui::Id::new(("__guide_day__", k)), CLICK);
                    if *day == pick || response.hovered() {
                        p.rect_filled(row.shrink(2.0), 4.0, ROW_SELECTED);
                    }
                    let color = if *day == self.day {
                        ACCENT
                    } else {
                        Color32::WHITE
                    };
                    p.text(
                        row.left_center() + Vec2::new(14.0, 0.0),
                        Align2::LEFT_CENTER,
                        i18n::day_label(data.lang, *day, data.today),
                        FontId::proportional(14.0),
                        color,
                    );
                    if response.clicked() {
                        clicked = Some(*day);
                    }
                }
            });
        if let Some(day) = clicked {
            self.pick_day(day);
        }
    }
}

/// How a programme row looks.
struct RowLook {
    playable: Playable,
    cursor: bool,
    hovered: bool,
    /// The title holds the filter's text.
    hit: bool,
}

fn paint_row(
    p: &egui::Painter,
    row: Rect,
    e: &EpgEntry,
    look: RowLook,
    now: DateTime<Utc>,
    live_label: &str,
) {
    let live = look.playable == Playable::Live;
    let fill = match (look.cursor, look.hovered, live) {
        (true, _, _) => ROW_SELECTED,
        (false, true, true) => ROW_LIVE_HOVER,
        (false, true, false) => ROW_HOVER,
        (false, false, true) => ROW_LIVE,
        (false, false, false) => ROW_BG,
    };
    p.rect_filled(row, 4.0, fill);
    if look.cursor {
        let edge = Rect::from_min_size(row.min, Vec2::new(3.0, row.height()));
        p.rect_filled(edge, 0.0, ACCENT);
    }
    let dim = look.playable == Playable::Unavailable;
    let time_color = Color32::from_white_alpha(if dim { 70 } else { 170 });
    let title_color = if look.hit {
        MATCH_YELLOW
    } else if dim {
        Color32::from_white_alpha(95)
    } else {
        Color32::WHITE
    };
    p.text(
        row.left_center() + Vec2::new(10.0, 0.0),
        Align2::LEFT_CENTER,
        crate::epg::hhmm(e.start),
        FontId::monospace(14.0),
        time_color,
    );
    let badge_w = match look.playable {
        Playable::Live => 48.0,
        Playable::Replay => 22.0,
        _ => 0.0,
    };
    let title_x = row.left() + 62.0;
    let width = (row.right() - 8.0 - badge_w - title_x).max(10.0);
    let job = single_line(&e.title, FontId::proportional(16.0), title_color, width);
    let galley = p.layout_job(job);
    p.galley(
        Pos2::new(title_x, row.center().y - galley.size().y / 2.0),
        galley,
        title_color,
    );
    match look.playable {
        Playable::Live => {
            let pill = Rect::from_center_size(
                Pos2::new(row.right() - 28.0, row.center().y),
                Vec2::new(42.0, 18.0),
            );
            p.rect_filled(pill, 9.0, LIVE_GREEN);
            p.text(
                pill.center(),
                Align2::CENTER_CENTER,
                live_label,
                FontId::proportional(11.0),
                Color32::from_rgb(20, 20, 20),
            );
            let total = (e.end - e.start).num_seconds().max(1) as f32;
            let done = ((now - e.start).num_seconds().max(0) as f32 / total).clamp(0.0, 1.0);
            let bar = Rect::from_min_size(
                Pos2::new(row.left(), row.bottom() - 2.0),
                Vec2::new(row.width(), 2.0),
            );
            p.rect_filled(bar, 1.0, Color32::from_white_alpha(35));
            p.rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar.width() * done, 2.0)),
                1.0,
                LIVE_GREEN,
            );
        }
        Playable::Replay => play_mark(p, Pos2::new(row.right() - 14.0, row.center().y)),
        Playable::Unavailable | Playable::Future => {}
    }
}

/// One line of text, cut off with "…" at `width`.
fn single_line(text: &str, font: FontId, color: Color32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    job
}

/// ▶: a past programme that can be watched back.
fn play_mark(p: &egui::Painter, center: Pos2) {
    let s = 6.0;
    let points = vec![
        center + Vec2::new(-s * 0.6, -s),
        center + Vec2::new(-s * 0.6, s),
        center + Vec2::new(s, 0.0),
    ];
    p.add(egui::Shape::convex_polygon(
        points,
        REPLAY_BLUE,
        Stroke::NONE,
    ));
}

/// ▾ beside the date.
fn chevron_down(p: &egui::Painter, c: Pos2) {
    let points = vec![
        c + Vec2::new(-5.0, -3.0),
        c + Vec2::new(5.0, -3.0),
        c + Vec2::new(0.0, 3.0),
    ];
    p.add(egui::Shape::convex_polygon(
        points,
        Color32::WHITE,
        Stroke::NONE,
    ));
}

/// One half of "Nu & straks | Primetime"; the active one in the accent colour.
fn segment(ui: &egui::Ui, rect: Rect, id: &str, text: &str, active: bool) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(id), CLICK);
    let p = ui.painter();
    if response.hovered() {
        p.rect_filled(rect.shrink(3.0), 5.0, PANEL_HOVER);
    }
    let color = if active { ACCENT } else { Color32::WHITE };
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(15.0),
        color,
    );
    response
}

/// ‹ or ›: a square button with a chevron.
fn chevron_button(ui: &egui::Ui, rect: Rect, id: &str, right: bool) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(id), CLICK);
    let p = ui.painter();
    let fill = if response.hovered() {
        PANEL_HOVER
    } else {
        PANEL
    };
    p.rect_filled(rect, 6.0, fill);
    let c = rect.center();
    let (dx, dy) = if right { (4.0, 7.0) } else { (-4.0, 7.0) };
    let stroke = Stroke::new(2.0, Color32::WHITE);
    p.line_segment([c + Vec2::new(-dx, -dy), c + Vec2::new(dx, 0.0)], stroke);
    p.line_segment([c + Vec2::new(dx, 0.0), c + Vec2::new(-dx, dy)], stroke);
    response
}

/// A column's header: the logo, or the coloured badge and the name; a dot while
/// its EPG or logo is on its way.
fn paint_header(
    ui: &egui::Ui,
    logos: &mut LogoStore,
    ch: &GuideChannel,
    column: Rect,
    loading: bool,
) {
    let rect = Rect::from_min_size(column.min, Vec2::new(column.width(), HEADER_H - 6.0));
    let texture = ch
        .logo
        .as_deref()
        .and_then(|url| logos.texture(ui.ctx(), url))
        .map(|t| (t.id(), t.size_vec2()));
    let p = ui.painter();
    p.rect_filled(rect, 6.0, PANEL);
    match texture {
        Some((id, size)) => {
            let room = rect.shrink2(Vec2::new(10.0, 6.0)).size();
            let fit = (room.x / size.x).min(room.y / size.y).min(1.0);
            let image = Rect::from_center_size(rect.center(), size * fit);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            p.image(id, image, uv, Color32::WHITE);
        }
        None => {
            let center = Pos2::new(rect.left() + 24.0, rect.center().y);
            p.circle_filled(center, 13.0, channel_accent_color(&ch.name));
            p.text(
                center,
                Align2::CENTER_CENTER,
                channel_badge_label(&ch.name),
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            let job = single_line(
                &ch.name,
                FontId::proportional(14.0),
                Color32::WHITE,
                rect.width() - 56.0,
            );
            let galley = p.layout_job(job);
            p.galley(
                Pos2::new(rect.left() + 44.0, rect.center().y - galley.size().y / 2.0),
                galley,
                Color32::WHITE,
            );
        }
    }
    if loading {
        p.circle_filled(
            rect.right_top() + Vec2::new(-10.0, 10.0),
            3.0,
            Color32::from_white_alpha(140),
        );
    }
}

/// Pick a recognisable accent color for a channel based on common Dutch
/// pattern matches. Purely visual - lets the user spot a channel category
/// at a glance without us shipping logos. Falls back to neutral grey.
pub(crate) fn channel_accent_color(name: &str) -> Color32 {
    let n = name.to_uppercase();
    if n.contains("NPO 1") || n.contains("NPO1") {
        Color32::from_rgb(245, 130, 32)
    } else if n.contains("NPO 2") || n.contains("NPO2") {
        Color32::from_rgb(40, 165, 220)
    } else if n.contains("NPO 3") || n.contains("NPO3") {
        Color32::from_rgb(120, 200, 70)
    } else if n.contains("NPO") {
        Color32::from_rgb(220, 110, 30)
    } else if n.contains("RTL 4") || n.contains("RTL4") {
        Color32::from_rgb(220, 30, 30)
    } else if n.contains("RTL 5") || n.contains("RTL5") {
        Color32::from_rgb(180, 50, 50)
    } else if n.contains("RTL Z") || n.contains("RTLZ") {
        Color32::from_rgb(160, 30, 30)
    } else if n.contains("RTL") {
        Color32::from_rgb(200, 40, 40)
    } else if n.contains("SBS") {
        Color32::from_rgb(255, 160, 0)
    } else if n.contains("NET 5") || n.contains("NET5") {
        Color32::from_rgb(180, 90, 180)
    } else if n.contains("VERONICA") {
        Color32::from_rgb(200, 50, 100)
    } else if n.contains("SPORT")
        || n.contains("ESPN")
        || n.contains("ZIGGO SPORT")
        || n.contains("FOX SPORTS")
    {
        Color32::from_rgb(50, 170, 90)
    } else if n.contains("DISCOVERY")
        || n.contains("NATIONAL GEOGRAPHIC")
        || n.contains("HISTORY")
        || n.contains("ANIMAL PLANET")
    {
        Color32::from_rgb(40, 140, 140)
    } else if n.contains("FILM") || n.contains("CINEMA") || n.contains("MOVIE") {
        Color32::from_rgb(140, 70, 200)
    } else if n.contains("KIDS")
        || n.contains("DISNEY")
        || n.contains("NICKELODEON")
        || n.contains("CARTOON")
    {
        Color32::from_rgb(230, 90, 160)
    } else if n.contains("NIEUWS") || n.contains("NEWS") || n.contains("BNR") || n.contains("NOS") {
        Color32::from_rgb(70, 130, 220)
    } else {
        Color32::from_rgb(110, 110, 120)
    }
}

/// Extract a 1-3 character "badge" label from a channel name. For Dutch
/// patterns we prefer the channel number ("NPO 1" -> "1", "RTL 4" -> "4");
/// for others we fall back to the first letter.
pub(crate) fn channel_badge_label(name: &str) -> String {
    let up = name.to_uppercase();
    for prefix in ["NPO ", "NPO", "RTL ", "RTL", "SBS ", "SBS", "NET ", "NET"] {
        if let Some(idx) = up.find(prefix) {
            let rest = &up[idx + prefix.len()..];
            let digits: String = rest
                .chars()
                .skip_while(|c| !c.is_ascii_digit() && *c != 'Z')
                .take_while(|c| c.is_ascii_digit() || *c == 'Z')
                .collect();
            if !digits.is_empty() {
                return digits;
            }
        }
    }
    name.chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epg::Epg;
    use crate::guide::logos::LogoStore;
    use crate::guide::store::ChannelEpg;
    use crate::i18n::Lang;
    use chrono::{Duration, TimeZone};
    use egui::{Key, Pos2, Rect, Vec2};

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

    fn data<'a>(f: &'a Fixture, fetching: &'a HashSet<i64>, keys: bool) -> GuideData<'a> {
        GuideData {
            lang: Lang::En,
            channels: &f.channels,
            channels_revision: 1,
            store: &f.store,
            fetching,
            playing: None,
            now: now(),
            today: time::day_of(now()),
            keys,
        }
    }

    fn key(k: Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn text(s: &str) -> egui::Event {
        egui::Event::Text(s.into())
    }

    /// The guide on a headless egui context, in a window `width` x 720.
    struct Screen {
        ctx: egui::Context,
        logos: LogoStore,
        width: f32,
        _dir: tempfile::TempDir,
    }

    impl Screen {
        fn new(width: f32) -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self {
                ctx: egui::Context::default(),
                logos: LogoStore::new(dir.path().to_path_buf()),
                width,
                _dir: dir,
            }
        }

        /// One frame with these events: the action, and everything drawn.
        fn run(
            &mut self,
            v: &mut GuideView,
            data: &GuideData,
            events: Vec<egui::Event>,
        ) -> (Option<GuideAction>, Vec<egui::epaint::ClippedShape>) {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(self.width, 720.0),
                )),
                events,
                ..Default::default()
            };
            let mut action = None;
            let logos = &mut self.logos;
            let output = self.ctx.run(raw, |ctx| action = v.show(ctx, data, logos));
            // egui draws its complaint about two widgets with one id as text.
            let clash = output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().contains("use of widget ID") => {
                    Some(t.galley.text().to_string())
                }
                _ => None,
            });
            assert_eq!(clash, None, "two widgets share an id");
            (action, output.shapes)
        }

        /// One frame with these events: the action, and every text painted.
        fn frame(
            &mut self,
            v: &mut GuideView,
            data: &GuideData,
            events: Vec<egui::Event>,
        ) -> (Option<GuideAction>, Vec<String>) {
            let (action, shapes) = self.run(v, data, events);
            let painted = shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            (action, painted)
        }

        /// A click at `pos`, one frame per step.
        fn click(&mut self, v: &mut GuideView, data: &GuideData, pos: Pos2) -> Option<GuideAction> {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(v, data, vec![egui::Event::PointerMoved(pos)]);
            self.frame(v, data, vec![button(true)]);
            self.frame(v, data, vec![button(false)]).0
        }

        fn focused(&self) -> bool {
            self.ctx.memory(|m| m.has_focus(filter_id()))
        }
    }

    /// Rows in a column of a 720 high window, and the now-line's row.
    fn rows_and_line() -> (usize, usize) {
        let rows = ((720.0 - TOP_BAR_H - FOOTER_H - HEADER_H) / ROW_H).floor() as usize;
        (rows, (rows as f32 * NOW_LINE) as usize)
    }

    #[test]
    fn enter_plays_the_programme_on_the_now_line() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        s.frame(&mut v, &data(&f, &none, true), vec![]);
        let (action, painted) = s.frame(&mut v, &data(&f, &none, true), vec![key(Key::Enter)]);
        match action {
            Some(GuideAction::Play {
                channel, playable, ..
            }) => assert_eq!((channel.archive_id, playable), (601, Playable::Live)),
            other => panic!("{other:?}"),
        }
        assert!(painted.iter().any(|t| t == "Today"), "the date says Today");
        assert!(painted.iter().any(|t| t == "LIVE"));
    }

    #[test]
    fn f_opens_the_filter_box_and_letters_there_are_no_hotkeys() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        s.frame(&mut v, &d, vec![key(Key::F), text("f")]);
        s.frame(&mut v, &d, vec![]);
        assert!(s.focused(), "f puts the keyboard in the box");
        assert_eq!(v.filter, "", "and is no letter typed into it");
        for (k, c) in [(Key::N, "n"), (Key::P, "p"), (Key::G, "g"), (Key::D, "d")] {
            let (action, _) = s.frame(&mut v, &d, vec![key(k), text(c)]);
            assert_eq!(action, None);
        }
        assert_eq!(v.filter, "npgd");
        assert_eq!(
            (v.mode, v.date_pick),
            (Mode::NowAndNext, None),
            "no hotkey went off"
        );
        let (action, _) = s.frame(&mut v, &d, vec![key(Key::Enter)]);
        assert_eq!(action, None, "the Enter that leaves the box plays nothing");
        s.frame(&mut v, &d, vec![]);
        assert!(!s.focused());
    }

    #[test]
    fn esc_in_the_filter_box_only_leaves_it() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        s.frame(&mut v, &d, vec![key(Key::F), text("f")]);
        s.frame(&mut v, &d, vec![text("z")]);
        let (action, _) = s.frame(&mut v, &d, vec![key(Key::Escape)]);
        assert_eq!(action, None);
        assert_eq!(v.filter, "z", "the first Esc only leaves the box");
        s.frame(&mut v, &d, vec![]);
        assert!(!s.focused());
        assert_eq!(s.frame(&mut v, &d, vec![key(Key::Escape)]).0, None);
        assert!(v.filter.is_empty(), "the second wipes the filter");
        let (action, _) = s.frame(&mut v, &d, vec![key(Key::Escape)]);
        assert_eq!(
            action,
            Some(GuideAction::Close),
            "the third closes the guide"
        );
    }

    #[test]
    fn tab_never_puts_the_keyboard_in_the_filter_box() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        for _ in 0..3 {
            s.frame(&mut v, &d, vec![key(Key::Tab)]);
            s.frame(&mut v, &d, vec![]);
            assert!(!s.focused());
        }
    }

    #[test]
    fn page_down_shows_the_next_page_of_channels() {
        let f = Fixture::new(12, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        assert_eq!(
            v.visible_window(),
            (0, 5),
            "five columns fit in 1280 pixels"
        );
        s.frame(&mut v, &d, vec![key(Key::PageDown)]);
        assert_eq!(v.visible_window(), (5, 5));
        s.frame(&mut v, &d, vec![key(Key::PageDown)]);
        assert_eq!(v.visible_window(), (7, 5), "the last full page");
    }

    #[test]
    fn a_click_on_a_programme_plays_it() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let (_, line) = rows_and_line();
        let y = TOP_BAR_H + HEADER_H + line as f32 * ROW_H + ROW_H / 2.0;
        let x = MARGIN + COLUMN_W + COLUMN_GAP + COLUMN_W / 2.0;
        match s.click(&mut v, &d, Pos2::new(x, y)) {
            Some(GuideAction::Play {
                channel, playable, ..
            }) => assert_eq!((channel.archive_id, playable), (602, Playable::Live)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_wheel_scrolls_only_the_column_under_the_pointer() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let before = (v.top[&601], v.top[&602]);
        let over_second = Pos2::new(MARGIN + COLUMN_W + COLUMN_GAP + 50.0, 300.0);
        s.frame(
            &mut v,
            &d,
            vec![
                egui::Event::PointerMoved(over_second),
                egui::Event::Scroll(Vec2::new(0.0, 2.0 * ROW_H)),
            ],
        );
        assert_eq!(v.top[&602], before.1 - 2.0, "two rows earlier");
        assert_eq!(v.top[&601], before.0, "the other column stays");
    }

    #[test]
    fn a_filter_that_matches_nothing_says_so() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        v.filter = "voetbal".into();
        let (_, painted) = s.frame(&mut v, &data(&f, &none, true), vec![]);
        assert!(
            painted.iter().any(|t| t == "nothing found for 'voetbal'"),
            "{painted:?}"
        );
    }

    #[test]
    fn the_guide_fits_a_640_pixel_window() {
        let f = Fixture::new(5, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(640.0), GuideView::new(time::day_of(now())));
        let (_, painted) = s.frame(&mut v, &data(&f, &none, true), vec![]);
        assert_eq!(v.visible_window(), (0, 2));
        assert!(painted.iter().any(|t| t == "Today"));
    }

    #[test]
    fn the_date_button_opens_the_list_and_the_keys_pick_a_day() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let bar = TopBar::new(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, TOP_BAR_H),
        ));
        s.click(&mut v, &d, bar.date.center());
        assert!(v.date_pick.is_some(), "the list is open");
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "Yesterday"));
        s.frame(&mut v, &d, vec![key(Key::ArrowUp)]);
        s.frame(&mut v, &d, vec![key(Key::Enter)]);
        let yesterday = time::day_of(now()).pred_opt().unwrap();
        assert_eq!(
            (v.day, v.mode, v.date_pick),
            (yesterday, Mode::Primetime, None)
        );
    }

    #[test]
    fn question_mark_asks_for_the_help_and_without_keys_nothing_happens() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        s.frame(&mut v, &data(&f, &none, true), vec![]);
        let (action, _) = s.frame(&mut v, &data(&f, &none, true), vec![text("?")]);
        assert_eq!(action, Some(GuideAction::Help));
        let (action, _) = s.frame(&mut v, &data(&f, &none, false), vec![key(Key::Enter)]);
        assert_eq!(action, None, "under the help the guide takes no keys");
    }

    // --- Rulings R7 and R17 ---

    #[test]
    fn a_day_picked_with_the_keys_lines_the_column_up_on_that_days_match() {
        let mut f = Fixture::new(1, &[601]);
        // "Special" runs now (today) and a day earlier (yesterday).
        let mut entries = hourly().entries().to_vec();
        entries[36].title = "Special".into();
        entries[12].title = "Special".into();
        f.store
            .insert(601, ChannelEpg::new(Epg::new(entries), now()));
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        v.filter = "special".into();
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        assert_eq!(v.cursor[&601], 36, "today's match");
        // d opens the date list, up goes to yesterday, Enter picks it.
        s.frame(&mut v, &d, vec![key(Key::D)]);
        s.frame(&mut v, &d, vec![key(Key::ArrowUp)]);
        s.frame(&mut v, &d, vec![key(Key::Enter)]);
        let yesterday = time::day_of(now()).pred_opt().unwrap();
        assert_eq!(v.day, yesterday);
        s.frame(&mut v, &d, vec![]);
        s.frame(&mut v, &d, vec![]);
        let (_, line) = rows_and_line();
        assert_eq!(
            v.cursor[&601], 12,
            "yesterday's match, not the one of today"
        );
        assert_eq!(v.top[&601], 12.0 - line as f32);
    }

    #[test]
    fn a_key_steps_from_the_rows_of_the_copy_that_landed_in_the_same_frame() {
        let mut f = Fixture::new(1, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        s.frame(&mut v, &data(&f, &none, true), vec![]);
        for _ in 0..2 {
            s.frame(&mut v, &data(&f, &none, true), vec![key(Key::ArrowDown)]);
        }
        let title = |f: &Fixture, v: &GuideView| {
            f.store.get(601).unwrap().entries()[v.cursor[&601]]
                .title
                .clone()
        };
        assert_eq!(title(&f, &v), "P2");
        // A newer copy lands: the first six programmes are gone, every index
        // moves. The key of the same frame steps from "P2", not from the
        // programme that now sits on P2's old index.
        f.store.insert(601, later_copy());
        s.frame(&mut v, &data(&f, &none, true), vec![key(Key::ArrowDown)]);
        assert_eq!(title(&f, &v), "P3");
    }

    #[test]
    fn a_click_on_a_programme_keeps_it_when_a_newer_copy_of_the_epg_lands() {
        let mut f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        s.frame(&mut v, &data(&f, &none, true), vec![]);
        // Two rows under the now-line of the second column: "P2".
        let (_, line) = rows_and_line();
        let y = TOP_BAR_H + HEADER_H + (line + 2) as f32 * ROW_H + ROW_H / 2.0;
        let x = MARGIN + COLUMN_W + COLUMN_GAP + COLUMN_W / 2.0;
        match s.click(&mut v, &data(&f, &none, true), Pos2::new(x, y)) {
            Some(GuideAction::Play {
                channel,
                entry,
                playable,
            }) => assert_eq!(
                (channel.archive_id, entry.title.as_str(), playable),
                (602, "P2", Playable::Future)
            ),
            other => panic!("{other:?}"),
        }
        // A newer copy of that column's EPG lands without its first programmes.
        f.store.insert(602, later_copy());
        s.frame(&mut v, &data(&f, &none, true), vec![]);
        let entries = f.store.get(602).unwrap().entries();
        assert_eq!(
            entries[v.cursor[&602]].title, "P2",
            "the cursor stays on the clicked programme"
        );
        assert_ne!(v.cursor[&602], f.live(602), "and is not lined up on now");
    }

    #[test]
    fn a_zero_for_has_archive_on_a_programme_that_ran_at_the_fetch_is_not_dimmed() {
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
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.run(&mut v, &d, vec![]);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let marks = shapes
            .iter()
            .filter(|s| matches!(&s.shape, egui::Shape::Path(p) if p.fill == REPLAY_BLUE))
            .count();
        assert_eq!(
            marks, 2,
            "a play mark on the kept programme and on the one that ran at the fetch"
        );
    }

    // --- The mouse, beyond the programmes ---

    #[test]
    fn a_click_in_the_filter_box_puts_the_keyboard_there() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let bar = TopBar::new(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, TOP_BAR_H),
        ));
        s.click(&mut v, &d, bar.filter.center());
        s.frame(&mut v, &d, vec![]);
        assert!(s.focused(), "a click puts the keyboard in the box");
        s.frame(&mut v, &d, vec![text("z")]);
        assert_eq!(v.filter, "z", "and what is typed goes into it");
    }

    #[test]
    fn a_click_on_a_day_in_the_list_picks_it() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let bar = TopBar::new(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, TOP_BAR_H),
        ));
        s.click(&mut v, &d, bar.date.center());
        s.frame(&mut v, &d, vec![]);
        // The first row of the list, under the date button: a week back.
        let first = Pos2::new(
            bar.date.left() + 20.0,
            bar.date.bottom() + 4.0 + DATE_ROW_H / 2.0,
        );
        s.click(&mut v, &d, first);
        let week_ago = time::day_of(now()) - Duration::days(7);
        assert_eq!(
            (v.day, v.mode, v.date_pick),
            (week_ago, Mode::Primetime, None)
        );
    }

    // --- Layout ---

    #[test]
    fn the_filter_box_fills_its_slot_in_the_top_bar() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.run(&mut v, &d, vec![]);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let slot = TopBar::new(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, TOP_BAR_H),
        ))
        .filter;
        let fill = s.ctx.style().visuals.extreme_bg_color;
        let boxes: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == fill => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(boxes.len(), 1, "one filter box");
        let (min, max) = (boxes[0].min - slot.min, boxes[0].max - slot.max);
        assert!(
            min.abs().max_elem() < 0.6 && max.abs().max_elem() < 0.6,
            "the box {:?} is as big as its slot {slot:?}, like the buttons beside it",
            boxes[0]
        );
        // And the hint sits in the middle of it, level with the date's text.
        let level = |what: &str| {
            shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == what => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()).center().y)
                }
                _ => None,
            })
        };
        let (hint, date) = (
            level("channel or programme").unwrap(),
            level("Today").unwrap(),
        );
        assert!((hint - date).abs() < 1.0, "hint at {hint}, date at {date}");
    }

    #[test]
    fn nothing_leaves_a_640_pixel_window_in_any_language() {
        let f = Fixture::new(5, &[]);
        let none = HashSet::new();
        for lang in Lang::ALL {
            let (mut s, mut v) = (Screen::new(640.0), GuideView::new(time::day_of(now())));
            let d = GuideData {
                lang,
                ..data(&f, &none, true)
            };
            s.run(&mut v, &d, vec![]);
            let (_, shapes) = s.run(&mut v, &d, vec![]);
            for shape in &shapes {
                if let egui::Shape::Text(t) = &shape.shape {
                    let r = t.galley.rect.translate(t.pos.to_vec2());
                    assert!(
                        r.left() >= -0.5 && r.right() <= 640.5,
                        "{lang:?}: {:?} is drawn at {r:?}",
                        t.galley.text()
                    );
                }
            }
        }
    }

    // --- Keys and buttons ---

    #[test]
    fn the_arrow_keys_move_between_columns_and_programmes() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        // What Enter plays after a key: the channel and the programme's title.
        let played = |s: &mut Screen, v: &mut GuideView, k: Key| {
            s.frame(v, &d, vec![key(k)]);
            match s.frame(v, &d, vec![key(Key::Enter)]).0 {
                Some(GuideAction::Play { channel, entry, .. }) => (channel.archive_id, entry.title),
                other => panic!("{other:?}"),
            }
        };
        // The channel and the title of programme `i` in its EPG.
        let at = |id: i64, i: usize| (id, f.store.get(id).unwrap().entries()[i].title.clone());
        let range = time::day_slice(f.store.get(601).unwrap().entries(), time::day_of(now()));
        s.frame(&mut v, &d, vec![]);
        let live = f.live(601);
        assert_eq!(played(&mut s, &mut v, Key::ArrowRight), at(602, live));
        assert_eq!(played(&mut s, &mut v, Key::ArrowDown), at(602, live + 1));
        assert_eq!(played(&mut s, &mut v, Key::ArrowLeft), at(601, live));
        assert_eq!(played(&mut s, &mut v, Key::ArrowUp), at(601, live - 1));
        assert_eq!(played(&mut s, &mut v, Key::Home), at(601, range.start));
        assert_eq!(played(&mut s, &mut v, Key::End), at(601, range.end - 1));
    }

    #[test]
    fn the_mode_buttons_and_the_page_arrows_work_with_the_mouse() {
        let f = Fixture::new(12, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![]);
        let bar = TopBar::new(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(1280.0, TOP_BAR_H),
        ));
        let half = |dx: f32| Pos2::new(bar.modes.center().x + dx, bar.modes.center().y);
        // Primetime, the right half of the button: the columns line up on 19:00.
        s.click(&mut v, &d, half(20.0));
        assert_eq!(v.mode, Mode::Primetime);
        s.frame(&mut v, &d, vec![]);
        let seven = time::at_hour(time::day_of(now()), time::PRIMETIME_HOUR);
        let e = &f.store.get(601).unwrap().entries()[v.cursor[&601]];
        assert!(
            e.start <= seven && seven < e.end,
            "{} runs at 19:00",
            e.title
        );
        // Now & next, the left half: back on what runs now.
        s.click(&mut v, &d, half(-20.0));
        assert_eq!(v.mode, Mode::NowAndNext);
        s.frame(&mut v, &d, vec![]);
        assert_eq!(v.cursor[&601], f.live(601));
        // › and ‹ turn the page.
        s.click(&mut v, &d, bar.next.center());
        s.frame(&mut v, &d, vec![]);
        assert_eq!(v.visible_window(), (5, 5));
        s.click(&mut v, &d, bar.prev.center());
        s.frame(&mut v, &d, vec![]);
        assert_eq!(v.visible_window(), (0, 5));
    }

    #[test]
    fn esc_closes_the_date_list_before_anything_else_and_g_closes_the_guide() {
        let f = Fixture::new(3, &[]);
        let none = HashSet::new();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        let d = data(&f, &none, true);
        s.frame(&mut v, &d, vec![key(Key::D)]);
        assert!(v.date_pick.is_some());
        assert_eq!(s.frame(&mut v, &d, vec![key(Key::Escape)]).0, None);
        assert_eq!(v.date_pick, None, "Esc closes the list");
        s.frame(&mut v, &d, vec![key(Key::D)]);
        let (action, _) = s.frame(&mut v, &d, vec![key(Key::G)]);
        assert_eq!(action, Some(GuideAction::Close), "g closes the guide");
    }

    #[test]
    fn opening_the_guide_does_not_leave_the_filter_box_wanting_the_keyboard() {
        let mut v = GuideView::new(time::day_of(now()));
        (v.filter_wanted, v.filter_had_focus) = (true, true);
        v.opened(time::day_of(now()));
        assert!(!v.filter_wanted && !v.filter_had_focus);
    }

    #[test]
    fn the_headers_the_top_bar_and_an_empty_guide_say_what_is_going_on() {
        let f = Fixture::new(3, &[602]);
        let none = HashSet::new();
        let fetching: HashSet<i64> = [602].into();
        let (mut s, mut v) = (Screen::new(1280.0), GuideView::new(time::day_of(now())));
        // A dot on the column being fetched; nothing under it yet.
        let d = GuideData {
            playing: Some("NPO 1"),
            ..data(&f, &fetching, true)
        };
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let count =
            |what: &dyn Fn(&egui::Shape) -> bool| shapes.iter().filter(|s| what(&s.shape)).count();
        let dot = |s: &egui::Shape| matches!(s, egui::Shape::Circle(c) if c.fill == Color32::from_white_alpha(140));
        let live = |s: &egui::Shape| matches!(s, egui::Shape::Text(t) if t.galley.text() == "LIVE");
        assert_eq!(count(&dot), 1, "one column is being fetched");
        assert_eq!(count(&live), 2, "the two others have a programme on now");
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "playing: NPO 1"), "{painted:?}");
        assert!(painted.iter().any(|t| t == "channels 1-3 of 3"));
        // No fetch: no dot.
        let (_, shapes) = s.run(&mut v, &data(&f, &none, true), vec![]);
        assert_eq!(shapes.iter().filter(|s| dot(&s.shape)).count(), 0);
        // No channels at all.
        let empty = Fixture::new(0, &[]);
        let (_, painted) = s.frame(&mut v, &data(&empty, &none, true), vec![]);
        assert!(painted
            .iter()
            .any(|t| t == "no channels with programme information"));
    }
}
