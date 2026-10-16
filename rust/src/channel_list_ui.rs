//! The channel list over the running picture (`g` on an M3U source; spec
//! 2026-10-15, §3): the groups on the left, the chosen group's channels on the
//! right. `ChannelListView` keeps the list's state while it is open; `show`
//! draws one frame, the rows on screen only, and hands what the user did to
//! app.rs as [`ListAction`]s, as guide_ui.rs does.

use crate::channel_list::{ChannelGroups, Cursor, GroupKey};
use crate::guide_ui::{play_mark, single_line, CLICK};
use crate::i18n::{self, Lang, T};
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::HashMap;

/// What the user did in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    /// The selection moved by a key, or a channel was clicked: it starts at the
    /// zap tempo (`repeat`: a held key's repeat, which only moves on).
    Preview {
        stream_id: i64,
        name: String,
        repeat: bool,
    },
    /// Enter, or a double click: the list closes and the channel plays now,
    /// unless it plays already.
    Play { stream_id: i64, name: String },
    /// Another group: nothing starts, and a start that waits for the tempo is
    /// dropped.
    GroupChanged,
    /// Esc or g: the list closes; what plays goes on, a waiting start is dropped.
    Close,
}

/// What the list shows but app.rs owns.
pub struct ListData<'a> {
    pub lang: Lang,
    /// The channel that plays: its row has a ▶.
    pub playing: Option<i64>,
    /// When each defect channel failed (unix seconds), for "how long ago".
    pub marks: &'a HashMap<i64, i64>,
    pub now_unix: i64,
}

const ROW_H: f32 = 30.0;
const MARGIN: f32 = 16.0;
const TITLE_H: f32 = 44.0;
const FOOTER_H: f32 = 28.0;
const GROUPS_W: f32 = 280.0;
const MIN_W: f32 = 320.0;
const MAX_W: f32 = 940.0;
/// Room left under the list: the toast ("streamfout op …") shows there.
const TOAST_ROOM: f32 = 64.0;

// The list's colours: tokens of its own, as the guide has its own; some have
// the guide's values (the selected and the hovered row, the accent, the green).
const PANEL: Color32 = Color32::from_rgb(18, 18, 24);
const ROW_SELECTED: Color32 = Color32::from_rgb(40, 55, 80);
const ROW_HOVER: Color32 = Color32::from_rgb(30, 30, 38);
const GROUP_OPEN: Color32 = Color32::from_rgb(34, 34, 46);
const ACCENT: Color32 = Color32::from_rgb(255, 80, 140);
const PLAYING: Color32 = Color32::from_rgb(120, 230, 100);
const BROKEN: Color32 = Color32::from_rgb(230, 120, 110);

/// Where the parts of the list go in a window `screen`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    pub(crate) panel: Rect,
    pub(crate) groups: Rect,
    pub(crate) rows: Rect,
    /// How many rows a column has room for: a page.
    pub(crate) visible: usize,
}

impl Layout {
    pub(crate) fn of(screen: Rect) -> Self {
        let width = (screen.width() - 2.0 * MARGIN).clamp(MIN_W, MAX_W);
        let height = (screen.height() - MARGIN - TOAST_ROOM).max(TITLE_H + FOOTER_H + ROW_H);
        let panel = Rect::from_min_size(
            Pos2::new(screen.center().x - width / 2.0, screen.top() + MARGIN),
            Vec2::new(width, height),
        );
        let (top, bottom) = (panel.top() + TITLE_H, panel.bottom() - FOOTER_H);
        let split = panel.left() + GROUPS_W.min(width * 0.4);
        let groups = Rect::from_min_max(Pos2::new(panel.left(), top), Pos2::new(split, bottom));
        let rows = Rect::from_min_max(Pos2::new(split, top), Pos2::new(panel.right(), bottom));
        let visible = ((rows.height() / ROW_H).floor() as usize).max(1);
        Self {
            panel,
            groups,
            rows,
            visible,
        }
    }

    /// The `k`th row on screen of a column.
    pub(crate) fn row(column: Rect, k: usize) -> Rect {
        Rect::from_min_size(
            Pos2::new(column.left() + 6.0, column.top() + k as f32 * ROW_H),
            Vec2::new(column.width() - 12.0, ROW_H - 2.0),
        )
    }
}

/// A click in the list, taken in after the frame is drawn.
enum Click {
    Group(usize),
    Row { row: usize, double: bool },
}

/// The list's state while it is open.
pub struct ChannelListView {
    groups: ChannelGroups,
    cursor: Cursor,
    /// The first group row and the first channel row on screen.
    group_top: usize,
    row_top: usize,
    /// What the wheel has turned that is not a whole row yet, in rows: at most
    /// half a row, either way. One for both columns.
    wheel: f32,
    /// The selection moved by a key or a click: it is brought on screen.
    follow: bool,
    /// The list opened, or the group changed: the selection goes to the middle
    /// of the channels. The groups column follows only when the group is off
    /// screen: a click on a group that shows must not scroll it from under the
    /// pointer.
    center: bool,
}

impl ChannelListView {
    /// The list as it opens (spec §3.3): see [`Cursor::open`]. Starts nothing.
    pub fn new(groups: ChannelGroups, last: Option<&GroupKey>, playing: Option<i64>) -> Self {
        let cursor = Cursor::open(&groups, last, playing);
        Self {
            groups,
            cursor,
            group_top: 0,
            row_top: 0,
            wheel: 0.0,
            follow: true,
            center: true,
        }
    }

    pub fn groups(&self) -> &ChannelGroups {
        &self.groups
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The group the selection is in: the list opens there next time.
    pub fn group_key(&self) -> Option<&GroupKey> {
        self.groups.key(self.cursor.group)
    }

    /// Groups made anew while the list is open (another copy of the list, a
    /// channel marked or cleared): the selection keeps its place
    /// ([`Cursor::follow`]).
    pub fn replace_groups(&mut self, groups: ChannelGroups) {
        self.cursor.follow(&self.groups, &groups);
        self.groups = groups;
        self.follow = true;
    }

    /// One frame of the list over the window: its keys, the wheel, the rows on
    /// screen, the clicks. What the user did, in the order they did it.
    pub fn show(&mut self, ctx: &egui::Context, data: &ListData) -> Vec<ListAction> {
        let layout = Layout::of(ctx.screen_rect());
        let mut actions = Vec::new();
        let closing = self.keys(ctx, layout.visible, data.playing, &mut actions);
        self.wheel(ctx, &layout);
        self.bring_on_screen(layout.visible);
        let mut clicked = None;
        egui::Area::new(egui::Id::new("__channel_list__"))
            // Its own click sense would make the frame a Tab stop that shows nothing.
            .sense(Sense::hover())
            .fixed_pos(layout.panel.min)
            .show(ctx, |ui| {
                let _ = ui.allocate_exact_size(layout.panel.size(), Sense::hover());
                clicked = self.paint(ui, &layout, data);
            });
        if let Some(click) = clicked.filter(|_| !closing) {
            self.click(click, data.playing, &mut actions);
        }
        actions
    }

    /// The keys of this frame, in the order they came; `page` is how many rows
    /// the screen holds. True when one closed the list: what comes after it is
    /// not the list's.
    fn keys(
        &mut self,
        ctx: &egui::Context,
        page: usize,
        playing: Option<i64>,
        actions: &mut Vec<ListAction>,
    ) -> bool {
        let presses: Vec<(Key, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        ..
                    } => Some((*key, *repeat)),
                    _ => None,
                })
                .collect()
        });
        let page = page as isize;
        for (key, repeat) in presses {
            let moved = match key {
                Key::ArrowUp => self.cursor.step(&self.groups, -1),
                Key::ArrowDown => self.cursor.step(&self.groups, 1),
                Key::PageUp => self.cursor.step(&self.groups, -page),
                Key::PageDown => self.cursor.step(&self.groups, page),
                Key::Home => self.cursor.to_end(&self.groups, false),
                Key::End => self.cursor.to_end(&self.groups, true),
                Key::ArrowLeft | Key::ArrowRight => {
                    let delta = if key == Key::ArrowLeft { -1 } else { 1 };
                    if self.cursor.step_group(&self.groups, delta, playing) {
                        self.center = true;
                        actions.push(ListAction::GroupChanged);
                    }
                    false
                }
                Key::Enter => {
                    actions.push(match self.cursor.channel(&self.groups) {
                        Some(c) => ListAction::Play {
                            stream_id: c.stream_id,
                            name: c.name.clone(),
                        },
                        None => ListAction::Close,
                    });
                    return true;
                }
                Key::Escape | Key::G => {
                    actions.push(ListAction::Close);
                    return true;
                }
                _ => false,
            };
            if moved {
                self.follow = true;
                if let Some(c) = self.cursor.channel(&self.groups) {
                    actions.push(ListAction::Preview {
                        stream_id: c.stream_id,
                        name: c.name.clone(),
                        repeat,
                    });
                }
            }
        }
        false
    }

    /// The wheel scrolls the column under the pointer; the selection stays and
    /// nothing starts. A turn is worked out in rows, and what is not a whole row
    /// yet is kept for the next turn (at most half a row), so small turns add up
    /// and the list follows a touchpad. A mouse notch (50 points) is two rows.
    fn wheel(&mut self, ctx: &egui::Context, layout: &Layout) {
        let (pointer, delta) = ctx.input(|i| (i.pointer.hover_pos(), i.raw_scroll_delta.y));
        // What is not a number would stay in the remainder and stop the wheel.
        let Some(at) = pointer.filter(|_| delta != 0.0 && delta.is_finite()) else {
            return;
        };
        let (top, most) = if layout.rows.contains(at) {
            let most = self
                .groups
                .rows(self.cursor.group)
                .saturating_sub(layout.visible);
            (&mut self.row_top, most)
        } else if layout.groups.contains(at) {
            let most = self.groups.group_count().saturating_sub(layout.visible);
            (&mut self.group_top, most)
        } else {
            return;
        };
        let total = self.wheel - delta / ROW_H;
        let rows = total.round();
        self.wheel = total - rows;
        *top = top.saturating_add_signed(rows as isize).min(most);
    }

    /// The selection on screen after a key or a click moved it (in the middle
    /// when the list opened or the group changed), the selected group too: its
    /// column moves for a group that is off screen only. A turn of the wheel
    /// alone leaves the rows where it put them.
    fn bring_on_screen(&mut self, visible: usize) {
        let rows_most = self.groups.rows(self.cursor.group).saturating_sub(visible);
        let groups_most = self.groups.group_count().saturating_sub(visible);
        let (row, group) = (self.cursor.row, self.cursor.group);
        if self.center {
            self.row_top = row.saturating_sub(visible / 2);
            // The groups column moves only when the group is off screen: a click
            // on a group that shows must not scroll it from under the pointer.
            if group < self.group_top || group >= self.group_top + visible {
                self.group_top = group.saturating_sub(visible / 2);
            }
        } else if self.follow {
            if row < self.row_top {
                self.row_top = row;
            } else if row >= self.row_top + visible {
                self.row_top = row + 1 - visible;
            }
            if group < self.group_top {
                self.group_top = group;
            } else if group >= self.group_top + visible {
                self.group_top = group + 1 - visible;
            }
        }
        self.row_top = self.row_top.min(rows_most);
        self.group_top = self.group_top.min(groups_most);
        self.center = false;
        self.follow = false;
    }

    /// Draws the frame: the rows on screen only. A click on a group or a row.
    fn paint(&self, ui: &egui::Ui, layout: &Layout, data: &ListData) -> Option<Click> {
        let lang = data.lang;
        let p = ui.painter();
        p.rect_filled(layout.panel, 8.0, PANEL);
        p.text(
            Pos2::new(
                layout.panel.left() + MARGIN,
                layout.panel.top() + TITLE_H / 2.0,
            ),
            Align2::LEFT_CENTER,
            i18n::t(lang, T::ChannelListTitle),
            FontId::proportional(20.0),
            Color32::WHITE,
        );
        p.line_segment(
            [layout.groups.right_top(), layout.groups.right_bottom()],
            Stroke::new(1.0, Color32::from_white_alpha(30)),
        );
        let mut clicked = None;
        // The groups, with their counts.
        let dim = Color32::from_white_alpha(130);
        let end = (self.group_top + layout.visible).min(self.groups.group_count());
        for (k, g) in (self.group_top..end).enumerate() {
            let Some(key) = self.groups.key(g) else {
                break;
            };
            let row = Layout::row(layout.groups, k);
            let response = ui.interact(row, egui::Id::new(("__list_group__", g)), CLICK);
            // The open group has a fill of its own; the selected row's
            // background, with the accent bar, is the channel column's alone.
            if g == self.cursor.group {
                p.rect_filled(row, 4.0, GROUP_OPEN);
            } else if response.hovered() {
                p.rect_filled(row, 4.0, ROW_HOVER);
            }
            let count = p.layout_no_wrap(
                self.groups.rows(g).to_string(),
                FontId::proportional(13.0),
                dim,
            );
            let count_w = count.size().x;
            let count_at = Pos2::new(
                row.right() - 8.0 - count_w,
                row.center().y - count.size().y / 2.0,
            );
            p.galley(count_at, count, dim);
            let color = if *key == GroupKey::Defect {
                BROKEN
            } else {
                Color32::WHITE
            };
            let width = (row.width() - 26.0 - count_w).max(10.0);
            let galley = p.layout_job(single_line(
                group_label(lang, key),
                FontId::proportional(15.0),
                color,
                width,
            ));
            p.galley(
                Pos2::new(row.left() + 10.0, row.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            if response.clicked() {
                clicked = Some(Click::Group(g));
            }
        }
        // The channels of the open group.
        let g = self.cursor.group;
        let rows = self.groups.rows(g);
        if rows == 0 {
            p.text(
                layout.rows.center_top() + Vec2::new(0.0, 40.0),
                Align2::CENTER_CENTER,
                i18n::t(lang, T::ListEmptyGroup),
                FontId::proportional(15.0),
                Color32::from_white_alpha(140),
            );
        }
        let defect_group = self.groups.key(g) == Some(&GroupKey::Defect);
        let end = (self.row_top + layout.visible).min(rows);
        for (k, r) in (self.row_top..end).enumerate() {
            let Some(channel) = self.groups.channel(g, r) else {
                break;
            };
            let row = Layout::row(layout.rows, k);
            let response = ui.interact(row, egui::Id::new(("__list_row__", g, r)), CLICK);
            // The selected row's background: painted here, once.
            if r == self.cursor.row {
                p.rect_filled(row, 4.0, ROW_SELECTED);
                p.rect_filled(
                    Rect::from_min_size(row.min, Vec2::new(3.0, row.height())),
                    0.0,
                    ACCENT,
                );
            } else if response.hovered() {
                p.rect_filled(row, 4.0, ROW_HOVER);
            }
            let playing = data.playing == Some(channel.stream_id);
            if playing {
                play_mark(p, Pos2::new(row.left() + 16.0, row.center().y), PLAYING);
            }
            let mut right = row.right() - 8.0;
            if defect_group {
                if let Some(&failed) = data.marks.get(&channel.stream_id) {
                    let ago = failed_ago(lang, data.now_unix.saturating_sub(failed));
                    let galley = p.layout_no_wrap(ago, FontId::proportional(12.0), BROKEN);
                    right -= galley.size().x;
                    p.galley(
                        Pos2::new(right, row.center().y - galley.size().y / 2.0),
                        galley,
                        BROKEN,
                    );
                    right -= 12.0;
                }
            }
            let color = if playing { PLAYING } else { Color32::WHITE };
            let x = row.left() + 32.0;
            let galley = p.layout_job(single_line(
                &channel.name,
                FontId::proportional(16.0),
                color,
                (right - x).max(10.0),
            ));
            p.galley(
                Pos2::new(x, row.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            if response.double_clicked() {
                clicked = Some(Click::Row {
                    row: r,
                    double: true,
                });
            } else if response.clicked() {
                clicked = Some(Click::Row {
                    row: r,
                    double: false,
                });
            }
        }
        // The keys, one line.
        let color = Color32::from_white_alpha(110);
        let footer = single_line(
            i18n::t(lang, T::ChannelListFooter),
            FontId::proportional(11.0),
            color,
            layout.panel.width() - 2.0 * MARGIN,
        );
        let galley = p.layout_job(footer);
        let y = layout.panel.bottom() - FOOTER_H / 2.0 - galley.size().y / 2.0;
        p.galley(Pos2::new(layout.panel.left() + MARGIN, y), galley, color);
        clicked
    }

    /// A click, taken in after the frame was drawn: a group opens; a channel is
    /// selected and starts, like one press; a double click's second click
    /// closes the list as Enter does (its first started the channel already).
    fn click(&mut self, click: Click, playing: Option<i64>, actions: &mut Vec<ListAction>) {
        match click {
            Click::Group(g) => {
                if self.cursor.select_group(&self.groups, g, playing) {
                    self.center = true;
                    actions.push(ListAction::GroupChanged);
                }
            }
            Click::Row { row, double } => {
                self.cursor.select_row(&self.groups, row);
                let Some(c) = self.cursor.channel(&self.groups) else {
                    return;
                };
                let (stream_id, name) = (c.stream_id, c.name.clone());
                actions.push(if double {
                    ListAction::Play { stream_id, name }
                } else {
                    ListAction::Preview {
                        stream_id,
                        name,
                        repeat: false,
                    }
                });
            }
        }
    }
}

/// What a group is called: the list's own name, or one of the fixed ones.
pub fn group_label(lang: Lang, key: &GroupKey) -> &str {
    match key {
        GroupKey::All => i18n::t(lang, T::ListAllChannels),
        GroupKey::Favorites => i18n::t(lang, T::FavoritesTitle),
        GroupKey::Named(name) => name,
        GroupKey::NoGroup => i18n::t(lang, T::ListNoGroup),
        GroupKey::Defect => i18n::t(lang, T::ListDefect),
    }
}

/// How long ago a channel failed, `secs` seconds back: just now, minutes,
/// hours, days. An age below a minute is just now, a negative one too: a mark
/// can be later than now (the clock was set back) until it expires.
pub fn failed_ago(lang: Lang, secs: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match secs {
        s if s < MINUTE => i18n::t(lang, T::AgoJustNow).to_owned(),
        s if s < HOUR => i18n::tr(lang, T::AgoMinutes, &[&(s / MINUTE).to_string()]),
        s if s < 2 * HOUR => i18n::t(lang, T::AgoHour).to_owned(),
        s if s < DAY => i18n::tr(lang, T::AgoHours, &[&(s / HOUR).to_string()]),
        s if s < 2 * DAY => i18n::t(lang, T::AgoDay).to_owned(),
        s => i18n::tr(lang, T::AgoDays, &[&(s / DAY).to_string()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_list::{ChannelGroups, Cursor, GroupKey};
    use crate::portal::LiveChannel;
    use egui::{Event, Key, Pos2, Rect, Vec2};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    /// 15 October 2026, 00:00 UTC.
    const NOW: i64 = 1_792_022_400;

    fn ch(id: i64, name: &str, group: &str) -> LiveChannel {
        LiveChannel {
            stream_id: id,
            name: name.into(),
            category_id: Some(group.into()),
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    /// Channels "Kanaal 001" to `n`, in "Groep", every third also in "Ander";
    /// `marked` of them defect. The groups: All, Ander, Groep (and Defect).
    fn groups(n: i64, marked: &[(i64, i64)]) -> ChannelGroups {
        let live: Vec<LiveChannel> = (1..=n)
            .map(|i| {
                let group = if i % 3 == 0 { "Groep;Ander" } else { "Groep" };
                ch(i, &format!("Kanaal {i:03}"), group)
            })
            .collect();
        let marks: HashMap<i64, i64> = marked.iter().copied().collect();
        ChannelGroups::build(Arc::new(live), |_| true, &marks, &HashSet::new())
    }

    fn data(playing: Option<i64>, marks: &HashMap<i64, i64>) -> ListData<'_> {
        ListData {
            lang: Lang::En,
            playing,
            marks,
            now_unix: NOW,
        }
    }

    fn press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn preview(id: i64, repeat: bool) -> ListAction {
        ListAction::Preview {
            stream_id: id,
            name: format!("Kanaal {id:03}"),
            repeat,
        }
    }

    /// The list on a headless egui context, in a window of 1280 x 720, with a
    /// clock of its own: 50 ms a frame.
    struct Screen {
        ctx: egui::Context,
        time: f64,
    }

    impl Screen {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 0.0,
            }
        }

        fn screen() -> Rect {
            Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))
        }

        fn layout() -> Layout {
            Layout::of(Self::screen())
        }

        /// A second goes by: the next click is no double click.
        fn pause(&mut self) {
            self.time += 1.0;
        }

        /// One frame with these events: the actions, and everything drawn.
        fn run(
            &mut self,
            v: &mut ChannelListView,
            d: &ListData,
            events: Vec<Event>,
        ) -> (Vec<ListAction>, Vec<egui::epaint::ClippedShape>) {
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Self::screen()),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let output = self.ctx.run(raw, |ctx| actions = v.show(ctx, d));
            // egui draws its complaint about two widgets with one id as text.
            let clash = output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text().contains("use of widget ID") => {
                    Some(t.galley.text().to_string())
                }
                _ => None,
            });
            assert_eq!(clash, None, "two widgets share an id");
            (actions, output.shapes)
        }

        /// One frame with these events: the actions, and every text painted.
        fn frame(
            &mut self,
            v: &mut ChannelListView,
            d: &ListData,
            events: Vec<Event>,
        ) -> (Vec<ListAction>, Vec<String>) {
            let (actions, shapes) = self.run(v, d, events);
            let painted = shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            (actions, painted)
        }

        /// One frame in which these keys are pressed: its actions.
        fn keys(&mut self, v: &mut ChannelListView, d: &ListData, keys: &[Key]) -> Vec<ListAction> {
            self.frame(v, d, keys.iter().map(|&k| press(k)).collect()).0
        }

        /// A click at `pos`, one frame per step: the actions of the release.
        fn click(&mut self, v: &mut ChannelListView, d: &ListData, pos: Pos2) -> Vec<ListAction> {
            let button = |pressed| Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(v, d, vec![Event::PointerMoved(pos)]);
            self.frame(v, d, vec![button(true)]);
            self.frame(v, d, vec![button(false)]).0
        }
    }

    /// A new view, and its first frame drawn: egui keeps a new area invisible
    /// in its first frame.
    fn open(
        n: i64,
        marked: &[(i64, i64)],
        last: Option<&GroupKey>,
        playing: Option<i64>,
        s: &mut Screen,
        d: &ListData,
    ) -> ChannelListView {
        let mut v = ChannelListView::new(groups(n, marked), last, playing);
        s.frame(&mut v, d, vec![]);
        v
    }

    #[test]
    fn down_previews_the_next_channel_and_up_stops_at_the_first() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(2, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowUp]), [preview(1, false)]);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowUp]),
            [],
            "the first row: it stops"
        );
        assert_eq!(v.cursor().row, 0);
    }

    #[test]
    fn a_held_key_says_it_repeats_and_two_presses_in_a_frame_move_twice() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(2, false)]);
        // The key was never let go: egui calls the next press a repeat.
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowDown]), [preview(3, true)]);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowDown, Key::ArrowDown]),
            [preview(4, true), preview(5, true)]
        );
    }

    #[test]
    fn page_keys_jump_a_screen_and_home_and_end_go_to_the_ends() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        let page = Screen::layout().visible as i64;
        assert_eq!(
            s.keys(&mut v, &d, &[Key::PageDown]),
            [preview(1 + page, false)]
        );
        assert_eq!(s.keys(&mut v, &d, &[Key::End]), [preview(100, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::End]), []);
        assert_eq!(s.keys(&mut v, &d, &[Key::Home]), [preview(1, false)]);
        assert_eq!(s.keys(&mut v, &d, &[Key::PageUp]), []);
    }

    #[test]
    fn left_and_right_change_the_group_and_stop_at_the_ends() {
        let marks = HashMap::new();
        let d = data(Some(4), &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, Some(4), &mut s, &d);
        assert_eq!(v.cursor(), Cursor { group: 0, row: 3 }, "on Kanaal 004");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowLeft]), [], "the first group");
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowRight]),
            [ListAction::GroupChanged]
        );
        assert_eq!(v.group_key(), Some(&GroupKey::Named("Ander".into())));
        assert_eq!(v.cursor().row, 0, "004 is not in Ander");
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowRight]),
            [ListAction::GroupChanged]
        );
        assert_eq!(v.cursor(), Cursor { group: 2, row: 3 }, "Groep, on 004");
        assert_eq!(s.keys(&mut v, &d, &[Key::ArrowRight]), [], "the last group");
    }

    #[test]
    fn enter_plays_the_selection_and_esc_and_g_close() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::Enter]),
            [ListAction::Play {
                stream_id: 1,
                name: "Kanaal 001".into()
            }]
        );
        assert_eq!(s.keys(&mut v, &d, &[Key::Escape]), [ListAction::Close]);
        assert_eq!(s.keys(&mut v, &d, &[Key::G]), [ListAction::Close]);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::Escape, Key::ArrowDown]),
            [ListAction::Close],
            "what comes after a close is not the list's"
        );
    }

    #[test]
    fn an_empty_group_says_so_and_enter_only_closes() {
        let marks: HashMap<i64, i64> = [(1, NOW), (2, NOW)].into();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(2, &[(1, NOW), (2, NOW)], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(
            painted.iter().any(|t| t == "no channels in this group"),
            "{painted:?}"
        );
        assert_eq!(s.keys(&mut v, &d, &[Key::Enter]), [ListAction::Close]);
    }

    #[test]
    fn the_list_paints_its_title_its_groups_with_their_counts_its_rows_and_its_keys() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        // The close hint leads: a window too narrow for the line loses its end.
        let footer = "Esc or g = close  ·  Enter = play  ·  Up / Down = choose  ·  Left / Right = group  ·  PgUp / PgDn, Home / End = jump";
        for text in [
            "Channel list",
            "All channels",
            "Ander",
            "Groep",
            "5",
            "1",
            "Kanaal 001",
            "Kanaal 005",
            footer,
        ] {
            assert!(painted.iter().any(|t| t == text), "{text}: {painted:?}");
        }
    }

    #[test]
    fn only_the_rows_on_screen_are_painted() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(200, &[], None, None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "Kanaal 001"));
        assert!(!painted.iter().any(|t| t == "Kanaal 150"));
        let (_, painted) = s.frame(&mut v, &d, vec![press(Key::End)]);
        assert!(painted.iter().any(|t| t == "Kanaal 200"));
        assert!(!painted.iter().any(|t| t == "Kanaal 001"));
    }

    #[test]
    fn the_channel_that_plays_has_a_play_mark_on_its_row() {
        let marks = HashMap::new();
        let d = data(Some(3), &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, Some(3), &mut s, &d);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let play_marks: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Path(path) if path.closed && path.fill == PLAYING => {
                    Some(Rect::from_points(&path.points))
                }
                _ => None,
            })
            .collect();
        assert_eq!(play_marks.len(), 1, "{play_marks:?}");
        assert!(Layout::row(Screen::layout().rows, 2).contains_rect(play_marks[0]));
    }

    #[test]
    fn broken_channels_say_how_long_ago_they_failed() {
        let failed = [
            (1, NOW - 200),
            (2, NOW - 2 * 3600 - 10),
            (3, NOW - 3 * 86_400),
        ];
        let marks: HashMap<i64, i64> = failed.into();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &failed, Some(&GroupKey::Defect), None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        for text in ["Broken channels", "3 min ago", "2 hours ago", "3 days ago"] {
            assert!(painted.iter().any(|t| t == text), "{text}: {painted:?}");
        }
    }

    #[test]
    fn failed_ago_reads_as_people_say_it() {
        for (secs, text) in [
            (-5, "just now"),
            (59, "just now"),
            (60, "1 min ago"),
            (3_599, "59 min ago"),
            (3_600, "1 hour ago"),
            (7_199, "1 hour ago"),
            (7_200, "2 hours ago"),
            (86_399, "23 hours ago"),
            (86_400, "1 day ago"),
            (172_799, "1 day ago"),
            (172_800, "2 days ago"),
            (604_800, "7 days ago"),
        ] {
            assert_eq!(failed_ago(Lang::En, secs), text, "{secs}");
        }
        assert_eq!(failed_ago(Lang::Nl, 120), "2 min geleden");
        assert_eq!(failed_ago(Lang::De, 7_200), "vor 2 Stunden");
    }

    #[test]
    fn a_long_name_is_cut_off() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let long = format!("Kanaal {}", "x".repeat(300));
        let live = vec![ch(1, &long, "Groep")];
        let groups =
            ChannelGroups::build(Arc::new(live), |_| true, &HashMap::new(), &HashSet::new());
        let mut v = ChannelListView::new(groups, None, None);
        let mut s = Screen::new();
        s.frame(&mut v, &d, vec![]);
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        let elided = shapes.iter().any(|s| {
            matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == long && t.galley.elided)
        });
        assert!(elided, "the name ends in …");
    }

    #[test]
    fn a_click_on_a_group_opens_it_and_a_click_on_a_channel_starts_it() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let layout = Screen::layout();
        let ander = Layout::row(layout.groups, 1).center();
        assert_eq!(s.click(&mut v, &d, ander), [ListAction::GroupChanged]);
        assert_eq!(v.group_key(), Some(&GroupKey::Named("Ander".into())));
        s.pause();
        let all = Layout::row(layout.groups, 0).center();
        assert_eq!(s.click(&mut v, &d, all), [ListAction::GroupChanged]);
        s.pause();
        let third = Layout::row(layout.rows, 2).center();
        assert_eq!(s.click(&mut v, &d, third), [preview(3, false)]);
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn a_double_click_starts_the_channel_and_then_closes_the_list() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let second = Layout::row(Screen::layout().rows, 1).center();
        assert_eq!(s.click(&mut v, &d, second), [preview(2, false)]);
        assert_eq!(
            s.click(&mut v, &d, second),
            [ListAction::Play {
                stream_id: 2,
                name: "Kanaal 002".into()
            }]
        );
    }

    #[test]
    fn the_wheel_scrolls_without_moving_the_selection_or_starting_anything() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        let over = Layout::row(Screen::layout().rows, 3).center();
        s.frame(&mut v, &d, vec![Event::PointerMoved(over)]);
        let (actions, painted) = s.frame(&mut v, &d, vec![Event::Scroll(Vec2::new(0.0, -90.0))]);
        assert!(actions.is_empty(), "{actions:?}");
        assert_eq!(v.cursor().row, 0, "the selection stays");
        assert!(
            !painted.iter().any(|t| t == "Kanaal 001"),
            "three rows down"
        );
        assert!(painted.iter().any(|t| t == "Kanaal 004"));
        // The next key brings the selection back on screen.
        let (_, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowDown)]);
        assert!(painted.iter().any(|t| t == "Kanaal 002"));
    }

    #[test]
    fn groups_made_anew_keep_the_selection_on_its_channel() {
        let mut v = ChannelListView::new(groups(5, &[]), None, Some(3));
        assert_eq!(v.cursor(), Cursor { group: 0, row: 2 });
        v.replace_groups(groups(5, &[(1, NOW)]));
        assert_eq!(
            v.cursor(),
            Cursor { group: 0, row: 1 },
            "Kanaal 003, one up"
        );
        let name = v.cursor().channel(v.groups()).map(|c| c.name.clone());
        assert_eq!(name.as_deref(), Some("Kanaal 003"));
    }

    // ---- beyond the brief's tests ----

    /// `n` channels, each in a group of its own: "Groep 01", "Groep 02", ...
    fn many_groups(n: i64) -> ChannelGroups {
        let live: Vec<LiveChannel> = (1..=n)
            .map(|i| ch(i, &format!("Kanaal {i:03}"), &format!("Groep {i:02}")))
            .collect();
        ChannelGroups::build(Arc::new(live), |_| true, &HashMap::new(), &HashSet::new())
    }

    /// How many rectangles of this fill a frame holds.
    fn filled(shapes: &[egui::epaint::ClippedShape], fill: egui::Color32) -> usize {
        shapes
            .iter()
            .filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == fill))
            .count()
    }

    #[test]
    fn a_mark_later_than_now_says_just_now() {
        // The clock was set back after the channel failed: its mark is an hour
        // ahead of now until it expires, and its age is not minus minutes.
        let failed = [(1, NOW + 3_600), (2, NOW - 90)];
        let marks: HashMap<i64, i64> = failed.into();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(2, &failed, Some(&GroupKey::Defect), None, &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        let ages: Vec<&str> = painted
            .iter()
            .map(String::as_str)
            .filter(|t| *t == "just now" || t.ends_with(" ago"))
            .collect();
        assert_eq!(ages, ["just now", "1 min ago"], "{painted:?}");
        // Whatever a marks file says, an age is never a panic.
        assert_eq!(failed_ago(Lang::En, i64::MIN), "just now");
        assert!(failed_ago(Lang::En, i64::MAX).ends_with(" days ago"));
    }

    #[test]
    fn the_fixed_groups_are_named_in_every_language() {
        let keys = [
            GroupKey::All,
            GroupKey::Favorites,
            GroupKey::NoGroup,
            GroupKey::Defect,
        ];
        for (lang, names) in [
            (
                Lang::Nl,
                [
                    "Alle zenders",
                    "Favorieten",
                    "Zonder groep",
                    "Defecte zenders",
                ],
            ),
            (
                Lang::En,
                ["All channels", "Favorites", "No group", "Broken channels"],
            ),
            (
                Lang::De,
                ["Alle Sender", "Favoriten", "Ohne Gruppe", "Defekte Sender"],
            ),
        ] {
            for (key, name) in keys.iter().zip(names) {
                assert_eq!(group_label(lang, key), name, "{lang:?} {key:?}");
            }
        }
        // The list's own groups are spelled as the file has them.
        let own = GroupKey::Named("nieuws & Sport".into());
        assert_eq!(group_label(Lang::De, &own), "nieuws & Sport");
    }

    #[test]
    fn the_wheel_scrolls_the_groups_when_the_pointer_is_over_them() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ChannelListView::new(many_groups(40), None, None);
        s.frame(&mut v, &d, vec![]);
        let over = Layout::row(Screen::layout().groups, 3).center();
        s.frame(&mut v, &d, vec![Event::PointerMoved(over)]);
        let (actions, painted) = s.frame(&mut v, &d, vec![Event::Scroll(Vec2::new(0.0, -90.0))]);
        assert!(actions.is_empty(), "{actions:?}");
        assert_eq!(
            v.cursor(),
            Cursor { group: 0, row: 0 },
            "the selection stays"
        );
        assert!(
            !painted.iter().any(|t| t == "All channels"),
            "three groups down"
        );
        assert!(!painted.iter().any(|t| t == "Groep 02"));
        assert!(painted.iter().any(|t| t == "Groep 03"));
        assert!(
            painted.iter().any(|t| t == "Kanaal 001"),
            "the channels stay"
        );
        // The next key brings the selected group back on screen.
        let (_, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowDown)]);
        assert!(painted.iter().any(|t| t == "All channels"));
    }

    /// A list of 100 channels with the pointer over its rows, ready for the wheel.
    fn ready_for_the_wheel(s: &mut Screen, d: &ListData) -> ChannelListView {
        let mut v = open(100, &[], None, None, s, d);
        let over = Layout::row(Screen::layout().rows, 3).center();
        s.frame(&mut v, d, vec![Event::PointerMoved(over)]);
        v
    }

    /// One frame in which the wheel turns by `points` (negative: down the list).
    fn turn(s: &mut Screen, v: &mut ChannelListView, d: &ListData, points: f32) -> Vec<String> {
        s.frame(v, d, vec![Event::Scroll(Vec2::new(0.0, points))]).1
    }

    #[test]
    fn small_turns_of_the_wheel_add_up_to_whole_rows() {
        // A touchpad turns by a few points a frame, and the list follows it:
        // what is not a whole row yet is kept for the next turn.
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ready_for_the_wheel(&mut s, &d);
        // 3 points are a tenth of a row.
        let painted = turn(&mut s, &mut v, &d, -3.0);
        assert!(painted.iter().any(|t| t == "Kanaal 001"), "no row yet");
        // 20 turns of 3 points are 60 points: two rows in all, not twenty.
        for _ in 0..19 {
            turn(&mut s, &mut v, &d, -3.0);
        }
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(!painted.iter().any(|t| t == "Kanaal 002"), "two rows down");
        assert!(painted.iter().any(|t| t == "Kanaal 003"));
        // The same turns the other way bring the list back.
        for _ in 0..20 {
            turn(&mut s, &mut v, &d, 3.0);
        }
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "Kanaal 001"), "and back");
        assert_eq!(v.cursor().row, 0, "the selection never moved");
    }

    #[test]
    fn a_notch_of_the_wheel_is_two_rows_and_three_notches_are_five() {
        // A mouse turns by 50 points a notch: a row and two thirds, two on the
        // screen; the third of a row that is over is paid back by the next notch.
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ready_for_the_wheel(&mut s, &d);
        let painted = turn(&mut s, &mut v, &d, -50.0);
        assert!(!painted.iter().any(|t| t == "Kanaal 002"), "{painted:?}");
        assert!(painted.iter().any(|t| t == "Kanaal 003"), "two rows");
        turn(&mut s, &mut v, &d, -50.0);
        let painted = turn(&mut s, &mut v, &d, -50.0);
        assert!(!painted.iter().any(|t| t == "Kanaal 005"), "{painted:?}");
        assert!(
            painted.iter().any(|t| t == "Kanaal 006"),
            "150 points: five rows"
        );
    }

    #[test]
    fn a_turn_that_is_not_a_number_does_not_stop_the_wheel() {
        // The part of a turn that is not a whole row is kept: a NaN kept would
        // keep every turn after it from scrolling.
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ready_for_the_wheel(&mut s, &d);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            turn(&mut s, &mut v, &d, bad);
        }
        let painted = turn(&mut s, &mut v, &d, -90.0);
        assert!(
            !painted.iter().any(|t| t == "Kanaal 003"),
            "three rows down"
        );
        assert!(painted.iter().any(|t| t == "Kanaal 004"), "{painted:?}");
    }

    #[test]
    fn the_list_opens_with_the_playing_channel_in_the_middle() {
        let marks = HashMap::new();
        let d = data(Some(60), &marks);
        let mut s = Screen::new();
        let mut v = open(100, &[], None, Some(60), &mut s, &d);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        let page = Screen::layout().visible;
        // Row 59 has half a page of rows above it and half a page below.
        for id in [60, 59 - page / 2 + 1, 59 + page / 2 - 1] {
            let name = format!("Kanaal {id:03}");
            assert!(painted.contains(&name), "{name}: {painted:?}");
        }
        assert!(!painted.iter().any(|t| t == "Kanaal 001"));
        assert!(!painted.iter().any(|t| t == "Kanaal 100"));
    }

    #[test]
    fn a_new_group_shows_its_selected_channel() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let layout = Screen::layout();
        // By key, from the end of a long group into a short one.
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        s.keys(&mut v, &d, &[Key::End]);
        let (actions, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowRight)]);
        assert_eq!(actions, [ListAction::GroupChanged]);
        assert_eq!(v.cursor(), Cursor { group: 1, row: 0 });
        assert!(painted.iter().any(|t| t == "Kanaal 003"), "{painted:?}");
        // By click.
        let mut s = Screen::new();
        let mut v = open(100, &[], None, None, &mut s, &d);
        s.keys(&mut v, &d, &[Key::End]);
        let ander = Layout::row(layout.groups, 1).center();
        assert_eq!(s.click(&mut v, &d, ander), [ListAction::GroupChanged]);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(painted.iter().any(|t| t == "Kanaal 003"), "{painted:?}");
    }

    #[test]
    fn a_click_on_a_group_on_screen_leaves_the_groups_column_where_it_is() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ChannelListView::new(many_groups(33), None, None);
        s.frame(&mut v, &d, vec![]);
        let at = Layout::row(Screen::layout().groups, 15).center();
        let (_, before) = s.frame(&mut v, &d, vec![]);
        assert_eq!(s.click(&mut v, &d, at), [ListAction::GroupChanged]);
        let (_, after) = s.frame(&mut v, &d, vec![]);
        let names = |painted: &[String]| -> Vec<String> {
            painted
                .iter()
                .filter(|t| t.starts_with("Groep ") || *t == "All channels")
                .cloned()
                .collect()
        };
        assert_eq!(names(&before), names(&after), "the column did not move");
        // A second click at the same place (a double click) opens nothing else.
        assert_eq!(s.click(&mut v, &d, at), []);
        assert_eq!(v.group_key(), Some(&GroupKey::Named("Groep 15".into())));
    }

    #[test]
    fn a_group_that_is_off_screen_is_brought_into_view() {
        // 34 groups, 18 of them on screen. The column moves for the group that
        // leaves the screen, and only then.
        let marks = HashMap::new();
        let d = data(None, &marks);
        let named = |painted: &[String], name: &str| painted.iter().any(|t| t == name);
        // The list opens on a group far down.
        let mut s = Screen::new();
        let far = GroupKey::Named("Groep 30".into());
        let mut v = ChannelListView::new(many_groups(33), Some(&far), None);
        s.frame(&mut v, &d, vec![]);
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(named(&painted, "Groep 30"), "{painted:?}");
        assert_eq!(v.group_key(), Some(&far));
        // Right, group by group: the column stays while the group shows ...
        let mut s = Screen::new();
        let mut v = ChannelListView::new(many_groups(33), None, None);
        s.frame(&mut v, &d, vec![]);
        for _ in 0..17 {
            s.keys(&mut v, &d, &[Key::ArrowRight]);
        }
        assert_eq!(v.cursor().group, 17, "the last group on screen");
        let (_, painted) = s.frame(&mut v, &d, vec![]);
        assert!(named(&painted, "All channels"), "{painted:?}");
        // ... and goes for the group that does not.
        let (actions, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowRight)]);
        assert_eq!(actions, [ListAction::GroupChanged]);
        assert_eq!(v.cursor().group, 18);
        assert!(named(&painted, "Groep 18"), "{painted:?}");
        assert!(!named(&painted, "All channels"));
    }

    #[test]
    fn a_group_the_wheel_scrolled_away_is_brought_back_by_a_key() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = ChannelListView::new(many_groups(33), None, None);
        s.frame(&mut v, &d, vec![]);
        for _ in 0..5 {
            s.keys(&mut v, &d, &[Key::ArrowRight]);
        }
        assert_eq!(v.cursor().group, 5);
        // Ten rows down the groups column: the selected group is above it.
        let over = Layout::row(Screen::layout().groups, 3).center();
        s.frame(&mut v, &d, vec![Event::PointerMoved(over)]);
        let (_, painted) = s.frame(&mut v, &d, vec![Event::Scroll(Vec2::new(0.0, -300.0))]);
        assert!(painted.iter().any(|t| t == "Groep 10"), "{painted:?}");
        assert!(!painted.iter().any(|t| t == "Groep 05"));
        // Left is the group above the selected one, and it is off screen too.
        let (actions, painted) = s.frame(&mut v, &d, vec![press(Key::ArrowLeft)]);
        assert_eq!(actions, [ListAction::GroupChanged]);
        assert_eq!(v.cursor().group, 4);
        assert!(painted.iter().any(|t| t == "Groep 04"), "{painted:?}");
    }

    #[test]
    fn the_list_takes_no_tab_stop() {
        // Its keys are its own: a widget with focus would take Enter or Space
        // for a click, and Tab would hand the keyboard to a frame that shows
        // nothing.
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        for shift in [false, true] {
            let tab = Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    shift,
                    ..Default::default()
                },
            };
            s.frame(&mut v, &d, vec![tab]);
            assert_eq!(s.ctx.memory(|m| m.focused()), None, "shift: {shift}");
        }
    }

    #[test]
    fn a_degenerate_window_does_not_break_the_list() {
        // A panic ends the player: a minimised window is 0 x 0.
        let marks: HashMap<i64, i64> = [(1, NOW)].into();
        let d = data(Some(3), &marks);
        let sizes = [
            (0.0, 0.0),
            (1.0, 1.0),
            (10.0, 700.0),
            (640.0, 10.0),
            (30.0, 30.0),
        ];
        for (w, h) in sizes {
            let ctx = egui::Context::default();
            let mut v = ChannelListView::new(groups(100, &[(1, NOW)]), None, Some(3));
            let at = Pos2::new(20.0, 50.0);
            let frames = [
                vec![],
                vec![press(Key::ArrowDown)],
                vec![press(Key::PageDown)],
                vec![press(Key::End)],
                vec![press(Key::ArrowRight)],
                vec![
                    Event::PointerMoved(at),
                    Event::Scroll(Vec2::new(0.0, -99.0)),
                ],
                vec![press(Key::Home)],
            ];
            for (n, events) in frames.into_iter().enumerate() {
                let raw = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))),
                    time: Some(n as f64 * 0.05),
                    events,
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    v.show(ctx, &d);
                });
            }
            // This holds by construction (`Layout::of` ends in `.max(1)`): the
            // point of the test is that none of the frames above panics.
            let page = Layout::of(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))).visible;
            assert!(page >= 1, "{w} x {h}");
        }
    }

    #[test]
    fn the_selected_row_and_the_open_group_are_each_filled_once() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let layout = Screen::layout();
        let (_, shapes) = s.run(&mut v, &d, vec![]);
        assert_eq!(filled(&shapes, ROW_SELECTED), 1);
        assert_eq!(filled(&shapes, ACCENT), 1, "the bar of the selected row");
        assert_eq!(filled(&shapes, GROUP_OPEN), 1);
        assert_eq!(filled(&shapes, ROW_HOVER), 0);
        // The pointer, one frame to settle, then a frame to look at.
        let mut hover = |column: Rect, k: usize| {
            let at = Layout::row(column, k).center();
            s.frame(&mut v, &d, vec![Event::PointerMoved(at)]);
            s.run(&mut v, &d, vec![]).1
        };
        // On another channel: it is hovered, and the selected row is no more
        // painted for it.
        let shapes = hover(layout.rows, 3);
        assert_eq!(filled(&shapes, ROW_HOVER), 1);
        assert_eq!(filled(&shapes, ROW_SELECTED), 1);
        // On the selected row: its own fill, and no hover fill under it.
        let shapes = hover(layout.rows, 0);
        assert_eq!(filled(&shapes, ROW_HOVER), 0);
        assert_eq!(filled(&shapes, ROW_SELECTED), 1);
        // On another group: hovered. On the open group: its own fill only.
        let shapes = hover(layout.groups, 1);
        assert_eq!(filled(&shapes, ROW_HOVER), 1);
        assert_eq!(filled(&shapes, GROUP_OPEN), 1);
        let shapes = hover(layout.groups, 0);
        assert_eq!(filled(&shapes, ROW_HOVER), 0);
        assert_eq!(filled(&shapes, GROUP_OPEN), 1);
    }

    #[test]
    fn keys_that_are_not_the_lists_do_nothing() {
        // The player's other keys are not the list's while it is open.
        let marks = HashMap::new();
        let d = data(Some(2), &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, Some(2), &mut s, &d);
        let before = v.cursor();
        for key in [
            Key::Space,
            Key::F,
            Key::N,
            Key::O,
            Key::D,
            Key::E,
            Key::A,
            Key::S,
            Key::T,
            Key::Num0,
            Key::Num5,
            Key::F5,
            Key::F11,
            Key::Tab,
            Key::Backspace,
        ] {
            assert_eq!(s.keys(&mut v, &d, &[key]), [], "{key:?}");
        }
        assert_eq!(v.cursor(), before);
    }

    #[test]
    fn keys_act_on_the_selection_the_keys_before_them_left() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        assert_eq!(
            s.keys(&mut v, &d, &[Key::ArrowDown, Key::Enter]),
            [
                preview(2, false),
                ListAction::Play {
                    stream_id: 2,
                    name: "Kanaal 002".into()
                }
            ]
        );
    }

    #[test]
    fn a_click_in_the_frame_of_a_close_key_is_not_the_lists() {
        let marks = HashMap::new();
        let d = data(None, &marks);
        let mut s = Screen::new();
        let mut v = open(5, &[], None, None, &mut s, &d);
        let pos = Layout::row(Screen::layout().rows, 2).center();
        let button = |pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        s.frame(&mut v, &d, vec![Event::PointerMoved(pos)]);
        s.frame(&mut v, &d, vec![button(true)]);
        let (actions, _) = s.frame(&mut v, &d, vec![button(false), press(Key::Escape)]);
        assert_eq!(actions, [ListAction::Close], "Kanaal 003 does not start");
    }
}
