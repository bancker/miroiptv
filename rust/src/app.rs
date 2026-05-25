use crate::catalog::{CatalogStatus, CatalogStore};
use crate::epg::{Epg, EpgEntry};
use crate::favorites::Favorites;
use crate::player::{Cmd, Event, PlayerHandle, RgbaFrame};
use crate::portal::LiveChannel;
use crate::search::ItemKind;
use crate::shortcuts;
use crate::storage::Storage;
use egui::{Color32, ColorImage, Key, TextureHandle, TextureOptions};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgrammeStatus {
    /// Currently airing.
    Live,
    /// Already aired and the channel exposes catch-up (`tv_archive=1`).
    Catchup,
    /// Not yet aired.
    Future,
    /// Past programme on a channel with no catch-up - shown dim, not playable.
    PastUnavailable,
}

fn programme_status(
    entry: &EpgEntry,
    channel: &LiveChannel,
    now: chrono::DateTime<chrono::Utc>,
) -> ProgrammeStatus {
    if entry.start <= now && now < entry.end {
        ProgrammeStatus::Live
    } else if entry.end <= now {
        if channel.tv_archive == 1 {
            ProgrammeStatus::Catchup
        } else {
            ProgrammeStatus::PastUnavailable
        }
    } else {
        ProgrammeStatus::Future
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeMode {
    /// Each column scrolls to its currently-airing programme on open / mode-switch.
    NowAndNext,
    /// Each column scrolls to ~20:00 (start of prime-time block).
    Primetime,
}

/// NLZIET-style column EPG guide opened with `g`.
///
/// Layout: top filter/time bar, then N vertical channel columns side-by-side.
/// Each column has its own virtualized programme list. Cursor lives at
/// (selected_col, selected_row_per_channel[chan_id]) so jumping between
/// columns preserves where the user was in each channel.
struct GuideState {
    open: bool,
    /// Free-text filter (typed in the top bar; matches channel name substring).
    filter: String,
    /// Indices into `catalog.live_channels()` for entries matching `filter`.
    /// Rebuilt when filter or catalog changes.
    visible: Vec<usize>,
    visible_filter_snapshot: String,
    /// Index INTO `visible` of the LEFTMOST channel currently rendered.
    column_offset: usize,
    /// Which visible column has cursor focus (0..num_visible_columns).
    selected_col: usize,
    /// Programme-row cursor per channel, keyed by stream_id so it survives
    /// horizontal scrolling and time-mode changes.
    row_per_channel: HashMap<i64, usize>,

    /// EPG cache: stream_id -> Epg. Filled by background tokio tasks; never
    /// evicted during session (~50 KB per channel, bounded by channels visited).
    epg_cache: Arc<Mutex<HashMap<i64, Epg>>>,
    /// Set of stream_ids with an in-flight fetch (prevents duplicate requests
    /// when the user pans back and forth across the same column).
    epg_pending: Arc<Mutex<HashSet<i64>>>,
    /// When did the visible-column SET last change? Debounce so rapid
    /// horizontal panning doesn't fire dozens of HTTP calls.
    visible_set_settled_since: Instant,
    visible_set_settled_snapshot: Vec<i64>,

    time_mode: TimeMode,
    /// When set, force-scroll each visible column to the position that matches
    /// `time_mode` on the next paint. Consumed by paint_guide.
    pending_time_scroll: bool,
}

impl Default for GuideState {
    fn default() -> Self {
        Self {
            open: false,
            filter: String::new(),
            visible: Vec::new(),
            visible_filter_snapshot: String::new(),
            column_offset: 0,
            selected_col: 0,
            row_per_channel: HashMap::new(),
            epg_cache: Arc::new(Mutex::new(HashMap::new())),
            epg_pending: Arc::new(Mutex::new(HashSet::new())),
            visible_set_settled_since: Instant::now(),
            visible_set_settled_snapshot: Vec::new(),
            time_mode: TimeMode::NowAndNext,
            pending_time_scroll: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    /// `--xtream user:pass@host:port` was given with non-placeholder values.
    Configured,
    /// No `--xtream` argument given at all.
    Missing,
    /// `--xtream` was given but with the placeholder template values.
    Placeholder,
}

pub struct TvApp {
    player: PlayerHandle,
    catalog: Arc<CatalogStore>,
    favorites: Favorites,
    storage: Storage,
    portal_state: PortalState,

    video_tex: Option<TextureHandle>,
    last_frame_version: u64,

    current_idx: Option<usize>,
    current_name: Option<String>,
    current_stream_id: Option<i64>,

    current_epg: Option<Epg>,
    epg_slot: Arc<Mutex<Option<(i64, Epg)>>>,
    epg_fetch_pending_for: Option<i64>,

    toast: Option<(String, Instant)>,

    show_favs: bool,
    show_search: bool,
    show_epg_strip: bool,
    show_epg_grid: bool,
    show_debug: bool,

    search_query: String,

    guide: GuideState,
    /// Tracks borderless+always-on-top toggle (`b` key).
    borderless: bool,
}

impl TvApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        player: PlayerHandle,
        catalog: Arc<CatalogStore>,
        storage: Storage,
        portal_state: PortalState,
    ) -> Self {
        // Whenever a new mpv frame is ready, ask egui to redraw.
        let ctx = cc.egui_ctx.clone();
        player
            .frames
            .set_new_frame_callback(move || ctx.request_repaint());

        let favorites = Favorites::load(&storage.favorites_path()).unwrap_or_default();
        // Only kick off a fetch if we actually have a real portal — otherwise the
        // request would hang on a placeholder/missing host until the network timeout
        // and the user would see "loading..." for ~45s before any error appears.
        if portal_state == PortalState::Configured {
            catalog.spawn_fetch();
        }

        Self {
            player,
            catalog,
            favorites,
            storage,
            portal_state,
            video_tex: None,
            last_frame_version: 0,
            current_idx: None,
            current_name: None,
            current_stream_id: None,
            current_epg: None,
            epg_slot: Arc::new(Mutex::new(None)),
            epg_fetch_pending_for: None,
            toast: None,
            show_favs: false,
            show_search: false,
            show_epg_strip: false,
            show_epg_grid: false,
            show_debug: false,
            search_query: String::new(),
            guide: GuideState::default(),
            borderless: false,
        }
    }

    fn set_toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn drain_events(&mut self) {
        let mut buf: Vec<Event> = Vec::new();
        {
            let mut rx = self.player.evt_rx.lock();
            while let Ok(evt) = rx.try_recv() {
                buf.push(evt);
            }
        }
        for evt in buf {
            match evt {
                Event::FileLoaded => {}
                Event::PlaybackStarted => {}
                Event::EndOfFile { reason } => self.set_toast(format!("ended: {}", reason)),
                Event::Error { msg } => {
                    warn!("player error: {}", msg);
                    self.set_toast(format!("error: {}", msg));
                }
                Event::PropertyChanged { .. } => {}
            }
        }
    }

    fn drain_epg(&mut self) {
        let arrived = self.epg_slot.lock().take();
        if let Some((sid, epg)) = arrived {
            if Some(sid) == self.epg_fetch_pending_for {
                self.epg_fetch_pending_for = None;
                // Enrich the zap toast with current programme if this EPG
                // is for the channel we're now playing. We still call
                // set_toast (4s lifetime) so the user sees the title pop
                // in shortly after the channel name.
                if Some(sid) == self.current_stream_id {
                    if let Some(now_prog) = epg.current_at(chrono::Utc::now()) {
                        let name = self
                            .current_name
                            .clone()
                            .unwrap_or_else(|| "?".into());
                        let start = now_prog.start.with_timezone(&chrono::Local);
                        let end = now_prog.end.with_timezone(&chrono::Local);
                        self.set_toast(format!(
                            "[TV] {}  |  {}  ({}-{})",
                            name,
                            now_prog.title,
                            start.format("%H:%M"),
                            end.format("%H:%M")
                        ));
                    }
                }
                self.current_epg = Some(epg);
            }
        }
    }

    fn kick_epg_fetch(&mut self, stream_id: i64) {
        let portal = self.catalog.portal().clone();
        let slot = self.epg_slot.clone();
        self.epg_fetch_pending_for = Some(stream_id);
        tokio::spawn(async move {
            match portal.fetch_epg(stream_id).await {
                Ok(epg) => {
                    *slot.lock() = Some((stream_id, epg));
                }
                Err(e) => tracing::warn!("epg fetch failed for {}: {}", stream_id, e),
            }
        });
    }

    fn zap_to(&mut self, sid: i64, name: &str, idx: Option<usize>) {
        let url = self.catalog.portal().live_stream_url(sid);
        tracing::info!(
            "zap: -> {} (stream_id={}, idx={:?}, prev={:?})",
            name,
            sid,
            idx,
            self.current_name
        );
        let _ = self.player.cmd_tx.send(Cmd::LoadUrl(url));
        self.current_idx = idx;
        self.current_name = Some(name.to_owned());
        self.current_stream_id = Some(sid);
        self.current_epg = None;
        self.set_toast(format!("[TV] {}", name));
        self.kick_epg_fetch(sid);
    }

    fn zap_delta(&mut self, delta: i32) {
        let live = self.catalog.live_channels();
        if live.is_empty() {
            if !self.catalog.is_loaded() {
                self.set_toast("catalog loading...");
            } else {
                self.set_toast("no live channels in catalog");
            }
            return;
        }
        if let Some(i) = shortcuts::next_live_idx(self.current_idx, live.len(), delta) {
            let ch = &live[i];
            let sid = ch.stream_id;
            let name = ch.name.clone();
            self.zap_to(sid, &name, Some(i));
        }
    }

    fn zap_by_id(&mut self, sid: i64) {
        let live = self.catalog.live_channels();
        if let Some((i, ch)) = live.iter().enumerate().find(|(_, c)| c.stream_id == sid) {
            let name = ch.name.clone();
            self.zap_to(sid, &name, Some(i));
        }
    }

    fn zap_npo(&mut self, n: u8) {
        if let Some(sid) = shortcuts::npo_shortcut_id(&self.catalog, n) {
            self.zap_by_id(sid);
        } else {
            self.set_toast(format!("NPO {} not in catalog", n));
        }
    }

    fn play_movie(&mut self, sid: i64, name: &str) {
        let ext = self.catalog.movie_extension(sid);
        let url = self.catalog.portal().movie_stream_url(sid, &ext);
        let _ = self.player.cmd_tx.send(Cmd::LoadUrl(url));
        self.current_name = Some(name.to_owned());
        self.current_idx = None;
        self.current_stream_id = None;
        self.current_epg = None;
        self.set_toast(format!("[FILM] {}", name));
    }

    fn toggle_favorite_current(&mut self) {
        let Some(sid) = self.current_stream_id else {
            self.set_toast("no current channel to favorite");
            return;
        };
        let name = self.current_name.clone().unwrap_or_default();
        let was = self.favorites.contains(sid);
        self.favorites.toggle(sid, &name);
        let _ = self.favorites.save(&self.storage.favorites_path());
        if was {
            self.set_toast(format!("[-] removed: {}", name));
        } else {
            self.set_toast(format!("[+] added: {}", name));
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Search box owns input when open; only handle escape/enter externally.
        if self.show_search {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.show_search = false;
                self.search_query.clear();
            }
            return;
        }

        let (
            down,
            up,
            left,
            right,
            scroll,
            f_key,
            e_key,
            shift,
            d,
            n1,
            n2,
            n3,
            f11,
            esc,
            n_key,
            r_key,
            a_key,
            s_key,
            star,
            f5,
            g_key,
            b_key,
        ) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.raw_scroll_delta.y,
                i.key_pressed(Key::F),
                i.key_pressed(Key::E),
                i.modifiers.shift,
                i.key_pressed(Key::D),
                i.key_pressed(Key::Num1),
                i.key_pressed(Key::Num2),
                i.key_pressed(Key::Num3),
                i.key_pressed(Key::F11),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::N),
                i.key_pressed(Key::R),
                i.key_pressed(Key::A),
                i.key_pressed(Key::S),
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "*")),
                i.key_pressed(Key::F5),
                i.key_pressed(Key::G),
                i.key_pressed(Key::B),
            )
        });

        if down {
            self.zap_delta(1);
        }
        if up {
            self.zap_delta(-1);
        }
        if scroll > 0.5 {
            self.zap_delta(-1);
        }
        if scroll < -0.5 {
            self.zap_delta(1);
        }

        if left {
            let _ = self.player.cmd_tx.send(Cmd::SeekRelative(-30.0));
            self.set_toast("<< -30s");
        }
        if right {
            let _ = self.player.cmd_tx.send(Cmd::SeekRelative(30.0));
            self.set_toast(">> +30s");
        }

        if f_key && !shift {
            self.show_search = !self.show_search;
            if self.show_search {
                self.search_query.clear();
            }
        }
        if f_key && shift {
            self.show_favs = !self.show_favs;
        }

        if e_key {
            if shift {
                self.show_epg_grid = !self.show_epg_grid;
            } else {
                self.show_epg_strip = !self.show_epg_strip;
            }
        }

        if d {
            self.show_debug = !self.show_debug;
        }

        if n1 {
            self.zap_npo(1);
        }
        if n2 {
            self.zap_npo(2);
        }
        if n3 {
            self.zap_npo(3);
        }

        if n_key {
            if let Some(sid) = shortcuts::news_npo(&self.catalog) {
                self.zap_by_id(sid);
            } else {
                self.set_toast("no NPO channel in catalog");
            }
        }
        if r_key {
            if let Some(sid) = shortcuts::news_rtl(&self.catalog) {
                self.zap_by_id(sid);
            } else {
                self.set_toast("no RTL channel in catalog");
            }
        }

        if a_key {
            let _ = self.player.cmd_tx.send(Cmd::CycleAudio);
            self.set_toast("cycle audio");
        }
        if s_key {
            let _ = self.player.cmd_tx.send(Cmd::CycleSubtitle);
            self.set_toast("cycle subtitle");
        }

        if f11 {
            let want = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want));
        }

        if esc {
            self.show_favs = false;
            self.show_epg_strip = false;
            self.show_epg_grid = false;
            self.show_debug = false;
            self.search_query.clear();
        }

        if star {
            self.toggle_favorite_current();
        }

        if f5 {
            match self.portal_state {
                PortalState::Configured => {
                    self.catalog.spawn_fetch();
                    self.set_toast("retrying portal fetch...");
                }
                PortalState::Missing | PortalState::Placeholder => {
                    self.set_toast("no portal configured - edit run.bat");
                }
            }
        }

        if g_key {
            self.toggle_guide();
        }

        if b_key {
            self.toggle_borderless(ctx);
        }
    }

    fn toggle_borderless(&mut self, ctx: &egui::Context) {
        self.borderless = !self.borderless;
        if self.borderless {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            self.set_toast("borderless + always-on-top  (press b again to restore)");
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::Normal,
            ));
            self.set_toast("normal window");
        }
    }

    fn update_video_texture(&mut self, ctx: &egui::Context) {
        let frame: RgbaFrame = self.player.frames.read();
        if frame.w == 0 || frame.h == 0 {
            return;
        }
        if frame.version == self.last_frame_version && self.video_tex.is_some() {
            return;
        }
        self.last_frame_version = frame.version;

        // mpv emits rgb0: bytes are R, G, B, 0. egui ColorImage wants RGBA.
        // Just force the 4th byte to 255 (opaque).
        let mut rgba = frame.data.clone();
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let img = ColorImage::from_rgba_unmultiplied([frame.w as usize, frame.h as usize], &rgba);
        match self.video_tex.as_mut() {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.video_tex = Some(ctx.load_texture("video", img, TextureOptions::LINEAR)),
        }
    }

    fn forward_window_size(&self, ctx: &egui::Context) {
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            let w = rect.width().max(1.0) as u32;
            let h = rect.height().max(1.0) as u32;
            let _ = self.player.cmd_tx.send(Cmd::SetWindowSize(w, h));
        }
    }

    fn paint_toast(&self, ctx: &egui::Context) {
        let Some((text, t0)) = &self.toast else {
            return;
        };
        if t0.elapsed() > Duration::from_secs(4) {
            return;
        }
        egui::Area::new(egui::Id::new("__toast__"))
            .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(Color32::WHITE).heading());
                    });
            });
    }

    fn paint_search(&mut self, ctx: &egui::Context) {
        if !self.show_search {
            return;
        }
        let mut zap_pick: Option<(i64, String, ItemKind)> = None;

        egui::Area::new(egui::Id::new("__search__"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(220))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("search live / movie / series...")
                                .desired_width(540.0),
                        );
                        resp.request_focus();

                        let hits = self.catalog.search(&self.search_query);
                        ui.separator();
                        for it in hits.iter().take(12) {
                            let tag = match it.kind {
                                ItemKind::Live => "[LIVE]",
                                ItemKind::Movie => "[FILM]",
                                ItemKind::Series => "[SERIE]",
                            };
                            if ui
                                .selectable_label(false, format!("{} {}", tag, it.name))
                                .clicked()
                            {
                                zap_pick = Some((it.id, it.name.clone(), it.kind));
                            }
                        }
                        if ctx.input(|i| i.key_pressed(Key::Enter)) {
                            if let Some(first) = hits.first() {
                                zap_pick = Some((first.id, first.name.clone(), first.kind));
                            }
                        }
                    });
            });

        if let Some((id, name, kind)) = zap_pick {
            self.handle_search_pick(id, &name, kind);
        }
    }

    fn handle_search_pick(&mut self, id: i64, name: &str, kind: ItemKind) {
        match kind {
            ItemKind::Live => {
                self.zap_by_id(id);
            }
            ItemKind::Movie => {
                self.play_movie(id, name);
            }
            ItemKind::Series => {
                self.set_toast(format!("series picker not in v1: {}", name));
            }
        }
        self.show_search = false;
        self.search_query.clear();
    }

    fn paint_favs(&mut self, ctx: &egui::Context) {
        if !self.show_favs {
            return;
        }
        let mut zap_id: Option<i64> = None;
        let mut remove_id: Option<i64> = None;
        let entries: Vec<_> = self.favorites.iter().cloned().collect();

        egui::Area::new(egui::Id::new("__favs__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(230))
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        ui.heading("Favorites");
                        ui.separator();
                        if entries.is_empty() {
                            ui.label(egui::RichText::new("press * on a channel to add").italics());
                        }
                        for entry in entries {
                            ui.horizontal(|ui| {
                                if ui.selectable_label(false, &entry.name).clicked() {
                                    zap_id = Some(entry.stream_id);
                                }
                                if ui.small_button("x").clicked() {
                                    remove_id = Some(entry.stream_id);
                                }
                            });
                        }
                    });
            });

        if let Some(id) = zap_id {
            self.zap_by_id(id);
            self.show_favs = false;
        }
        if let Some(id) = remove_id {
            self.favorites.remove(id);
            let _ = self.favorites.save(&self.storage.favorites_path());
        }
    }

    fn paint_epg_strip(&self, ctx: &egui::Context) {
        if !self.show_epg_strip {
            return;
        }
        let Some(epg) = &self.current_epg else {
            return;
        };
        let now = chrono::Utc::now();
        let mut text = String::new();
        if let Some(cur) = epg.current_at(now) {
            text.push_str(&format!(
                "> {}  ({}-{})",
                cur.title,
                cur.start.format("%H:%M"),
                cur.end.format("%H:%M")
            ));
        }
        if let Some(nxt) = epg.next_at(now) {
            text.push_str(&format!(
                "    ->  {}  {}",
                nxt.start.format("%H:%M"),
                nxt.title
            ));
        }
        if text.is_empty() {
            return;
        }

        egui::Area::new(egui::Id::new("__epg_strip__"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -60.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(Color32::WHITE));
                    });
            });
    }

    fn paint_epg_grid(&self, ctx: &egui::Context) {
        if !self.show_epg_grid {
            return;
        }
        let Some(epg) = &self.current_epg else {
            return;
        };
        egui::Area::new(egui::Id::new("__epg_grid__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(230))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        ui.heading("EPG");
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| {
                                for e in epg.entries() {
                                    ui.label(format!(
                                        "{}-{}  {}",
                                        e.start.format("%H:%M"),
                                        e.end.format("%H:%M"),
                                        e.title
                                    ));
                                }
                            });
                    });
            });
    }

    fn paint_debug_hud(&self, ctx: &egui::Context) {
        if !self.show_debug {
            return;
        }
        egui::Area::new(egui::Id::new("__debug__"))
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        let frame = self.player.frames.read();
                        ui.label(format!("frame v{} {}x{}", frame.version, frame.w, frame.h));
                        if let Some(n) = &self.current_name {
                            ui.label(format!("channel: {}", n));
                        }
                        ui.label(format!("favs: {}", self.favorites.iter().count()));
                        ui.label(format!("catalog: {:?}", self.catalog.status()));
                        ui.label(format!("portal: {:?}", self.portal_state));
                    });
            });
    }

    fn paint_empty_state(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(
                egui::RichText::new("tvplayer")
                    .color(Color32::from_white_alpha(180))
                    .size(48.0),
            );
            ui.add_space(20.0);
            match self.portal_state {
                PortalState::Missing => {
                    ui.label(
                        egui::RichText::new("no portal configured")
                            .color(Color32::from_white_alpha(140))
                            .size(20.0),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(
                            "Edit run.bat and set XTREAM_CREDS=user:pass@host:port,\n\
                             or pass --xtream user:pass@host:port on the command line.",
                        )
                        .color(Color32::from_white_alpha(110)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "(A bare URL also works: tvplayer.exe https://example.com/some.m3u8)",
                        )
                        .italics()
                        .color(Color32::from_white_alpha(80)),
                    );
                }
                PortalState::Placeholder => {
                    ui.label(
                        egui::RichText::new("placeholder credentials in run.bat")
                            .color(Color32::from_rgb(255, 200, 80))
                            .size(20.0),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(
                            "The XTREAM_CREDS line in run.bat still has the example values\n\
                             (user:pass@host.example.com:...). Edit it with your real portal\n\
                             credentials and start the player again.",
                        )
                        .color(Color32::from_white_alpha(140)),
                    );
                }
                PortalState::Configured => match self.catalog.status() {
                    CatalogStatus::Idle | CatalogStatus::Fetching(_) => {
                        let msg = if let CatalogStatus::Fetching(s) = self.catalog.status() {
                            s
                        } else {
                            "preparing...".into()
                        };
                        ui.label(
                            egui::RichText::new(msg)
                                .color(Color32::from_white_alpha(140))
                                .size(20.0),
                        );
                        ui.add_space(8.0);
                        ui.spinner();
                    }
                    CatalogStatus::Loaded => {
                        ui.label(
                            egui::RichText::new("catalog loaded - press 1/2/3 for NPO or f to search")
                                .color(Color32::from_white_alpha(140))
                                .size(18.0),
                        );
                    }
                    CatalogStatus::Failed(e) => {
                        ui.label(
                            egui::RichText::new("portal fetch failed")
                                .color(Color32::from_rgb(255, 120, 120))
                                .size(20.0),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(e).color(Color32::from_white_alpha(140)),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("press F5 to retry").italics()
                                .color(Color32::from_white_alpha(100)),
                        );
                    }
                },
            }
        });
    }

    // ------------------------------------------------------------------
    // Guide (`g` key): NLZIET-style vertical-column EPG browser
    // ------------------------------------------------------------------

    fn toggle_guide(&mut self) {
        if !self.catalog.is_loaded() {
            self.set_toast("catalog still loading - press g again in a moment");
            return;
        }
        self.guide.open = !self.guide.open;
        if self.guide.open {
            self.guide.filter.clear();
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
            self.guide.pending_time_scroll = true;
            // visible list (re)builds on next paint via refresh_visible_if_stale
        }
    }

    /// Re-derive `visible` (channel indices matching the filter) when the
    /// filter changes or the catalog reloads. Cheap to call per-frame.
    fn refresh_visible_if_stale(&mut self, channels: &[LiveChannel]) {
        let f = self.guide.filter.trim().to_lowercase();
        if self.guide.visible_filter_snapshot == f && !self.guide.visible.is_empty() {
            return;
        }
        self.guide.visible = if f.is_empty() {
            (0..channels.len()).collect()
        } else {
            channels
                .iter()
                .enumerate()
                .filter(|(_, c)| c.name.to_lowercase().contains(&f))
                .map(|(i, _)| i)
                .collect()
        };
        self.guide.visible_filter_snapshot = f;
        if self.guide.column_offset >= self.guide.visible.len() {
            self.guide.column_offset = 0;
        }
        self.guide.selected_col = 0;
    }

    /// How many columns fit at the current window width. Adaptive.
    fn guide_num_visible_cols(&self, ctx: &egui::Context) -> usize {
        const COL_PX: f32 = 200.0;
        let w = ctx
            .input(|i| i.viewport().inner_rect.map(|r| r.width()))
            .unwrap_or(1280.0);
        let usable = (w - 60.0).max(COL_PX);
        ((usable / COL_PX) as usize).clamp(1, 10)
    }

    fn guide_visible_channel_at<'a>(
        &self,
        visible_col: usize,
        channels: &'a [LiveChannel],
    ) -> Option<&'a LiveChannel> {
        let abs = self.guide.column_offset.checked_add(visible_col)?;
        let ch_idx = *self.guide.visible.get(abs)?;
        channels.get(ch_idx)
    }

    /// Kick EPG fetches for the visible-column set when it's been stable for
    /// 200 ms and we don't already have / aren't already fetching the data.
    /// Naively fetching on every column-offset change would fire 1 HTTP call
    /// per arrow-press while the user is panning - the debounce makes the
    /// guide feel snappy regardless of catalog size.
    fn guide_kick_visible_fetches(&mut self, channels: &[LiveChannel], num_cols: usize) {
        let visible_ids: Vec<i64> = (0..num_cols)
            .filter_map(|col| self.guide_visible_channel_at(col, channels).map(|c| c.stream_id))
            .collect();
        if visible_ids != self.guide.visible_set_settled_snapshot {
            self.guide.visible_set_settled_snapshot = visible_ids;
            self.guide.visible_set_settled_since = Instant::now();
            return;
        }
        if self.guide.visible_set_settled_since.elapsed() < Duration::from_millis(200) {
            return;
        }
        let cache_g = self.guide.epg_cache.lock();
        let pend_g = self.guide.epg_pending.lock();
        let to_fetch: Vec<i64> = self
            .guide
            .visible_set_settled_snapshot
            .iter()
            .copied()
            .filter(|sid| !cache_g.contains_key(sid) && !pend_g.contains(sid))
            .collect();
        drop(pend_g);
        drop(cache_g);
        for sid in to_fetch {
            let is_archive = channels
                .iter()
                .any(|c| c.stream_id == sid && c.tv_archive == 1);
            self.guide.epg_pending.lock().insert(sid);
            let cache = self.guide.epg_cache.clone();
            let pending = self.guide.epg_pending.clone();
            let portal = self.catalog.portal().clone();
            tokio::spawn(async move {
                let r = if is_archive {
                    portal.fetch_day_epg(sid).await
                } else {
                    portal.fetch_epg(sid).await
                };
                match r {
                    Ok(epg) => { cache.lock().insert(sid, epg); }
                    Err(e) => tracing::warn!("guide: EPG fetch failed for {}: {}", sid, e),
                }
                pending.lock().remove(&sid);
            });
        }
    }

    /// Row to scroll-to for the current `time_mode` given a channel's EPG.
    fn guide_target_row(epg: &Epg, mode: TimeMode, now: chrono::DateTime<chrono::Utc>) -> usize {
        let entries = epg.entries();
        if entries.is_empty() {
            return 0;
        }
        match mode {
            TimeMode::NowAndNext => entries
                .iter()
                .position(|e| e.end > now)
                .unwrap_or(entries.len() - 1),
            TimeMode::Primetime => {
                use chrono::TimeZone;
                let target = chrono::Local::now()
                    .date_naive()
                    .and_hms_opt(20, 0, 0)
                    .and_then(|n| chrono::Local.from_local_datetime(&n).single())
                    .map(|l| l.with_timezone(&chrono::Utc))
                    .unwrap_or(now);
                entries
                    .iter()
                    .position(|e| e.start >= target)
                    .unwrap_or(entries.len() - 1)
            }
        }
    }

    /// Apply the time-mode scroll for every visible column whose EPG has
    /// landed in cache. Called once when the guide opens, when the user
    /// switches mode (n/p), and after each new EPG fetch completes.
    fn guide_apply_time_scroll(&mut self, channels: &[LiveChannel], num_cols: usize) {
        let now = chrono::Utc::now();
        let cache = self.guide.epg_cache.lock();
        let mut any = false;
        for col in 0..num_cols {
            let Some(ch) = self.guide_visible_channel_at(col, channels) else { continue; };
            if let Some(epg) = cache.get(&ch.stream_id) {
                let row = Self::guide_target_row(epg, self.guide.time_mode, now);
                self.guide.row_per_channel.insert(ch.stream_id, row);
                any = true;
            }
        }
        if any {
            // Drop the request once we've applied to at least one column;
            // remaining columns get re-applied on the next paint as their
            // EPGs land.
            self.guide.pending_time_scroll = false;
        }
    }

    fn guide_play(&mut self, channels: &[LiveChannel]) {
        let Some(channel) = self
            .guide_visible_channel_at(self.guide.selected_col, channels)
            .cloned()
        else { return; };
        let row = *self.guide.row_per_channel.get(&channel.stream_id).unwrap_or(&0);
        let maybe_epg = self.guide.epg_cache.lock().get(&channel.stream_id).cloned();
        let Some(epg) = maybe_epg else {
            self.set_toast("EPG not loaded yet for this channel");
            return;
        };
        let Some(entry) = epg.entries().get(row).cloned() else { return; };
        let now = chrono::Utc::now();
        let status = programme_status(&entry, &channel, now);
        let (url, tag) = match status {
            ProgrammeStatus::Future => {
                self.set_toast(format!(
                    "not yet aired: {} ({})",
                    entry.title,
                    entry.start.with_timezone(&chrono::Local).format("%H:%M")
                ));
                return;
            }
            ProgrammeStatus::PastUnavailable => {
                self.set_toast(format!(
                    "no catch-up for '{}': not an archive channel",
                    channel.name
                ));
                return;
            }
            ProgrammeStatus::Live => {
                let live_sid = if channel.tv_archive == 1 {
                    self.catalog
                        .live_id_by_name(&channel.name)
                        .unwrap_or(channel.stream_id)
                } else {
                    channel.stream_id
                };
                (self.catalog.portal().live_stream_url(live_sid), "LIVE")
            }
            ProgrammeStatus::Catchup => {
                let dur_min = ((entry.end - entry.start).num_minutes() as u32).max(1);
                (
                    self.catalog
                        .portal()
                        .catchup_url(channel.stream_id, entry.start, dur_min),
                    "CATCHUP",
                )
            }
        };
        tracing::info!(
            "guide: [{}] {} | {} ({} -> {})",
            tag,
            channel.name,
            entry.title,
            entry.start.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
            entry.end.with_timezone(&chrono::Local).format("%H:%M")
        );
        let _ = self.player.cmd_tx.send(Cmd::LoadUrl(url));
        self.current_name = Some(format!("{} - {}", channel.name, entry.title));
        self.current_stream_id = Some(channel.stream_id);
        self.current_epg = None;
        self.set_toast(format!(
            "[{}] {} | {} @ {}",
            tag,
            channel.name,
            entry.title,
            entry.start.with_timezone(&chrono::Local).format("%H:%M")
        ));
        self.guide.open = false;
    }

    fn handle_guide_keys(&mut self, ctx: &egui::Context) {
        let (esc, g_key, up, down, left, right, pgup, pgdn, home, end, enter, backspace, n_key, p_key) =
            ctx.input(|i| (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::G),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::PageDown),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Backspace),
                i.key_pressed(Key::N),
                i.key_pressed(Key::P),
            ));
        if esc || g_key {
            self.guide.open = false;
            return;
        }
        // Typed chars feed the filter (except 'n'/'p' which are time-mode
        // hotkeys - we suppress those from the filter input).
        let typed: String = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) if t != "n" && t != "p" => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        if !typed.is_empty() {
            self.guide.filter.push_str(&typed);
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
        }
        if backspace && !self.guide.filter.is_empty() {
            self.guide.filter.pop();
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
        }
        if n_key && self.guide.time_mode != TimeMode::NowAndNext {
            self.guide.time_mode = TimeMode::NowAndNext;
            self.guide.pending_time_scroll = true;
        }
        if p_key && self.guide.time_mode != TimeMode::Primetime {
            self.guide.time_mode = TimeMode::Primetime;
            self.guide.pending_time_scroll = true;
        }

        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);
        let num_cols = self.guide_num_visible_cols(ctx);
        let n_visible = self.guide.visible.len();

        // Programme cursor (up/down/pgup/pgdn/home/end) acts on the
        // selected column.
        if (up || down || pgup || pgdn || home || end) && n_visible > 0 {
            if let Some(ch) = self.guide_visible_channel_at(self.guide.selected_col, &channels) {
                let sid = ch.stream_id;
                let total = self
                    .guide
                    .epg_cache
                    .lock()
                    .get(&sid)
                    .map(|e| e.entries().len())
                    .unwrap_or(0);
                if total > 0 {
                    let cur = *self.guide.row_per_channel.get(&sid).unwrap_or(&0);
                    let new = if down { (cur + 1).min(total - 1) }
                        else if up { cur.saturating_sub(1) }
                        else if pgdn { (cur + 10).min(total - 1) }
                        else if pgup { cur.saturating_sub(10) }
                        else if home { 0 }
                        else { total - 1 };
                    self.guide.row_per_channel.insert(sid, new);
                }
            }
        }

        // Column selection (left/right) - shifts selected_col within
        // the visible window; when bumping into the edge, scrolls the
        // window via column_offset.
        if left {
            if self.guide.selected_col > 0 {
                self.guide.selected_col -= 1;
            } else if self.guide.column_offset > 0 {
                self.guide.column_offset -= 1;
            }
        }
        if right {
            let last_visible_abs = self.guide.column_offset + self.guide.selected_col + 1;
            if self.guide.selected_col + 1 < num_cols && last_visible_abs < n_visible {
                self.guide.selected_col += 1;
            } else if self.guide.column_offset + num_cols < n_visible {
                self.guide.column_offset += 1;
            }
        }

        if enter {
            self.guide_play(&channels);
        }
    }

    fn paint_guide(&mut self, ctx: &egui::Context) {
        if !self.guide.open { return; }
        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);
        let num_cols = self.guide_num_visible_cols(ctx);
        self.guide_kick_visible_fetches(&channels, num_cols);
        if self.guide.pending_time_scroll {
            self.guide_apply_time_scroll(&channels, num_cols);
        }

        let n_visible = self.guide.visible.len();
        let max_offset = n_visible.saturating_sub(num_cols);
        if self.guide.column_offset > max_offset {
            self.guide.column_offset = max_offset;
        }
        if self.guide.selected_col >= num_cols {
            self.guide.selected_col = num_cols.saturating_sub(1);
        }

        let now = chrono::Utc::now();
        let time_mode = self.guide.time_mode;
        let filter_visible = self.guide.filter.clone();
        let selected_col = self.guide.selected_col;
        let column_offset = self.guide.column_offset;

        // Snapshot EPG data + row cursors for the visible columns into
        // owned data BEFORE rendering. This keeps the egui draw closure
        // free of borrows on `self` (which would prevent us from updating
        // row_per_channel on click).
        struct ColData {
            channel: LiveChannel,
            programmes: Vec<EpgEntry>,
            row: usize,
            loading: bool,
        }
        let col_data: Vec<ColData> = {
            let cache = self.guide.epg_cache.lock();
            let pending = self.guide.epg_pending.lock();
            (0..num_cols)
                .filter_map(|col| {
                    let ch = self.guide_visible_channel_at(col, &channels)?.clone();
                    let sid = ch.stream_id;
                    let programmes = cache
                        .get(&sid)
                        .map(|e| e.entries().to_vec())
                        .unwrap_or_default();
                    let row = *self.guide.row_per_channel.get(&sid).unwrap_or(&0);
                    let loading = pending.contains(&sid);
                    Some(ColData { channel: ch, programmes, row, loading })
                })
                .collect()
        };

        // Click intent collected during draw, applied after the borrow ends.
        let mut clicked: Option<(usize, usize)> = None; // (col, row)

        egui::Window::new("__guide__")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(238))
                    .inner_margin(12.0),
            )
            .show(ctx, |ui| {
                let avail = ctx.input(|i| i.viewport().inner_rect.map(|r| (r.width(), r.height())))
                    .unwrap_or((1280.0, 720.0));
                ui.set_width(avail.0 - 40.0);
                ui.set_height(avail.1 - 40.0);

                // ---- Top bar ----
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("TV-gids").heading().color(Color32::WHITE));
                    ui.add_space(16.0);

                    let nn_active = time_mode == TimeMode::NowAndNext;
                    let pt_active = time_mode == TimeMode::Primetime;
                    let mode_pill = |ui: &mut egui::Ui, label: &str, active: bool| {
                        let bg = if active {
                            Color32::from_rgb(60, 95, 140)
                        } else {
                            Color32::from_rgb(40, 40, 45)
                        };
                        let fg = if active { Color32::WHITE } else { Color32::from_white_alpha(180) };
                        egui::Frame::none()
                            .fill(bg)
                            .rounding(4.0)
                            .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(label).color(fg));
                            });
                    };
                    mode_pill(ui, "Nu & straks (n)", nn_active);
                    ui.add_space(6.0);
                    mode_pill(ui, "Primetime (p)", pt_active);

                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new("type to filter   /   arrows = nav   /   Enter = play   /   Esc/g = close")
                            .color(Color32::from_white_alpha(110))
                            .italics(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "channels {}-{} of {}",
                                column_offset + 1,
                                (column_offset + col_data.len()).min(n_visible),
                                n_visible
                            ))
                            .color(Color32::from_white_alpha(140)),
                        );
                    });
                });
                if !filter_visible.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("filter: {}_", filter_visible))
                            .color(Color32::from_rgb(220, 220, 80))
                            .monospace(),
                    );
                }
                ui.separator();

                // ---- Column grid ----
                if col_data.is_empty() {
                    ui.label(
                        egui::RichText::new("no channels match filter")
                            .color(Color32::from_white_alpha(140))
                            .italics(),
                    );
                    return;
                }
                ui.columns(col_data.len(), |cols| {
                    for (vi, cd) in col_data.iter().enumerate() {
                        let column_ui = &mut cols[vi];
                        let is_selected_col = vi == selected_col;
                        let is_archive = cd.channel.tv_archive == 1;

                        // Header
                        let header_bg = if is_selected_col {
                            Color32::from_rgb(50, 80, 120)
                        } else {
                            Color32::from_rgb(35, 35, 40)
                        };
                        egui::Frame::none()
                            .fill(header_bg)
                            .rounding(4.0)
                            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                            .show(column_ui, |ui| {
                                let badge = if is_archive { "[TK]  " } else { "" };
                                ui.label(
                                    egui::RichText::new(format!("{}{}", badge, cd.channel.name))
                                        .color(Color32::WHITE)
                                        .strong(),
                                );
                            });

                        if cd.programmes.is_empty() {
                            let msg = if cd.loading {
                                "loading EPG..."
                            } else {
                                "no EPG data"
                            };
                            column_ui.add_space(8.0);
                            column_ui.label(
                                egui::RichText::new(msg)
                                    .color(Color32::from_white_alpha(120))
                                    .italics(),
                            );
                            continue;
                        }

                        let cursor = cd.row;
                        let row_h = 38.0;
                        let mut clicked_in_col: Option<usize> = None;
                        egui::ScrollArea::vertical()
                            .id_source(format!("guide_col_{}", vi))
                            .auto_shrink([false, false])
                            .show_rows(column_ui, row_h, cd.programmes.len(), |ui, range| {
                                for i in range {
                                    let e = &cd.programmes[i];
                                    let status = programme_status(e, &cd.channel, now);
                                    let local_start = e.start.with_timezone(&chrono::Local);
                                    let time_label = local_start.format("%H:%M").to_string();
                                    let (bg, fg, badge) = match status {
                                        ProgrammeStatus::Live => (
                                            Color32::from_rgb(45, 70, 35),
                                            Color32::WHITE,
                                            Some(("LIVE", Color32::from_rgb(120, 220, 100))),
                                        ),
                                        ProgrammeStatus::Catchup => (
                                            Color32::from_rgb(28, 28, 32),
                                            Color32::from_white_alpha(200),
                                            Some(("Terugkijken", Color32::from_rgb(140, 180, 220))),
                                        ),
                                        ProgrammeStatus::Future => (
                                            Color32::from_rgb(22, 22, 26),
                                            Color32::WHITE,
                                            None,
                                        ),
                                        ProgrammeStatus::PastUnavailable => (
                                            Color32::from_rgb(22, 22, 26),
                                            Color32::from_white_alpha(90),
                                            None,
                                        ),
                                    };
                                    let row_bg = if i == cursor && is_selected_col {
                                        Color32::from_rgb(70, 100, 150)
                                    } else {
                                        bg
                                    };
                                    let frame = egui::Frame::none()
                                        .fill(row_bg)
                                        .rounding(3.0)
                                        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(&time_label)
                                                        .color(Color32::from_white_alpha(180))
                                                        .monospace()
                                                        .size(11.0),
                                                );
                                                ui.label(
                                                    egui::RichText::new(&e.title)
                                                        .color(fg)
                                                        .size(12.0),
                                                );
                                                if let Some((b_txt, b_col)) = badge {
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(egui::Align::Center),
                                                        |ui| {
                                                            ui.label(
                                                                egui::RichText::new(b_txt)
                                                                    .color(b_col)
                                                                    .size(10.0)
                                                                    .strong(),
                                                            );
                                                        },
                                                    );
                                                }
                                            });
                                            // Live progress bar at the bottom of the row.
                                            if matches!(status, ProgrammeStatus::Live) {
                                                let total = (e.end - e.start).num_seconds().max(1) as f32;
                                                let elapsed = (now - e.start).num_seconds().max(0) as f32;
                                                let progress = (elapsed / total).clamp(0.0, 1.0);
                                                let bar_rect = ui.available_rect_before_wrap();
                                                let (full, _) = ui.allocate_exact_size(
                                                    egui::vec2(bar_rect.width(), 3.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    full,
                                                    1.0,
                                                    Color32::from_white_alpha(30),
                                                );
                                                let mut done = full;
                                                done.set_width(full.width() * progress);
                                                ui.painter().rect_filled(
                                                    done,
                                                    1.0,
                                                    Color32::from_rgb(120, 220, 100),
                                                );
                                            }
                                        });
                                    if frame.response.interact(egui::Sense::click()).clicked() {
                                        clicked_in_col = Some(i);
                                    }
                                    if i == cursor && is_selected_col {
                                        frame.response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                        if let Some(row) = clicked_in_col {
                            clicked = Some((vi, row));
                        }
                    }
                });
            });

        // Apply clicks now that the borrow on `self` from .show() is gone.
        if let Some((col, row)) = clicked {
            if let Some(ch) = self.guide_visible_channel_at(col, &channels).cloned() {
                self.guide.selected_col = col;
                self.guide.row_per_channel.insert(ch.stream_id, row);
                self.guide_play(&channels);
            }
        }
    }
}

impl Drop for TvApp {
    fn drop(&mut self) {
        let _ = self.favorites.save(&self.storage.favorites_path());
        let _ = self.player.cmd_tx.send(Cmd::Quit);
    }
}

impl eframe::App for TvApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.drain_epg();
        self.forward_window_size(ctx);
        self.update_video_texture(ctx);
        // Guide owns input while it's open - we don't want `1`/`2`/`3` etc.
        // firing zaps when the user is typing into the channel filter.
        if self.guide.open {
            self.handle_guide_keys(ctx);
        } else {
            self.handle_keys(ctx);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                if let Some(tex) = &self.video_tex {
                    let avail = ui.available_size();
                    ui.add(egui::Image::new(tex).fit_to_exact_size(avail));
                } else {
                    self.paint_empty_state(ui);
                }
            });

        self.paint_toast(ctx);
        self.paint_search(ctx);
        self.paint_favs(ctx);
        self.paint_epg_strip(ctx);
        self.paint_epg_grid(ctx);
        self.paint_debug_hud(ctx);
        self.paint_guide(ctx);

        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
