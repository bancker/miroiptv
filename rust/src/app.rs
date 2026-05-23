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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidePane {
    Channels,
    Programmes,
}

/// Two-pane EPG guide opened with `g`.
/// Left pane = filterable channel list (all live + archive channels).
/// Right pane = programme list of the highlighted channel.
/// EPG fetched lazily per channel with 200 ms debounce + per-session cache.
struct GuideState {
    open: bool,
    pane: GuidePane,
    filter: String,
    /// Indices into `catalog.live_channels()` for entries that match `filter`.
    /// Rebuilt only when filter changes or catalog reloads.
    visible: Vec<usize>,
    visible_filter_snapshot: String,
    channel_cursor: usize,
    programme_cursor: usize,
    /// stream_id -> EPG. Filled by background tokio tasks; UI reads each frame.
    epg_cache: Arc<Mutex<HashMap<i64, Epg>>>,
    /// Currently-pending fetch (None means idle). Prevents duplicate requests.
    epg_pending: Arc<Mutex<Option<i64>>>,
    /// When did the highlighted channel last change? Used for 200 ms debounce
    /// so rapid arrow-down scrolling doesn't fire one HTTP fetch per row.
    cursor_settled_since: Instant,
    cursor_settled_for: Option<i64>,
}

impl Default for GuideState {
    fn default() -> Self {
        Self {
            open: false,
            pane: GuidePane::Channels,
            filter: String::new(),
            visible: Vec::new(),
            visible_filter_snapshot: String::new(),
            channel_cursor: 0,
            programme_cursor: 0,
            epg_cache: Arc::new(Mutex::new(HashMap::new())),
            epg_pending: Arc::new(Mutex::new(None)),
            cursor_settled_since: Instant::now(),
            cursor_settled_for: None,
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
    // Guide (`g` key): two-pane channel + programme browser with catch-up
    // ------------------------------------------------------------------

    fn toggle_guide(&mut self) {
        if !self.catalog.is_loaded() {
            self.set_toast("catalog still loading - press g again in a moment");
            return;
        }
        self.guide.open = !self.guide.open;
        if self.guide.open {
            self.guide.pane = GuidePane::Channels;
            self.guide.filter.clear();
            self.guide.channel_cursor = 0;
            self.guide.programme_cursor = 0;
            self.guide.cursor_settled_since = Instant::now();
            self.guide.cursor_settled_for = None;
            // visible list will (re)build on next paint via refresh_visible()
        }
    }

    /// Rebuild `guide.visible` when the filter text has changed since the
    /// last frame. Cheap to call every frame because the snapshot check
    /// short-circuits when nothing has changed.
    fn refresh_visible_if_stale(&mut self, channels: &[LiveChannel]) {
        if self.guide.visible_filter_snapshot == self.guide.filter
            && self.guide.visible.len() <= channels.len()
            && !self.guide.visible.is_empty()
        {
            return;
        }
        let f = self.guide.filter.trim().to_lowercase();
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
        self.guide.visible_filter_snapshot = self.guide.filter.clone();
        if self.guide.channel_cursor >= self.guide.visible.len() {
            self.guide.channel_cursor = 0;
        }
    }

    fn current_guide_channel<'a>(&self, channels: &'a [LiveChannel]) -> Option<&'a LiveChannel> {
        let idx = *self.guide.visible.get(self.guide.channel_cursor)?;
        channels.get(idx)
    }

    /// If cursor has been stable on a channel for >=200ms and we haven't
    /// fetched its EPG yet (and nothing is currently being fetched), spawn
    /// the background fetch. Uses fetch_day_epg for archive channels (full
    /// catch-up history) and fetch_epg for live ones (now + next).
    fn guide_maybe_fetch_epg(&mut self, channels: &[LiveChannel]) {
        let Some(channel) = self.current_guide_channel(channels) else { return; };
        let sid = channel.stream_id;
        // Track when the cursor moved to this channel.
        if self.guide.cursor_settled_for != Some(sid) {
            self.guide.cursor_settled_for = Some(sid);
            self.guide.cursor_settled_since = Instant::now();
            self.guide.programme_cursor = 0;
            return;
        }
        if self.guide.cursor_settled_since.elapsed() < Duration::from_millis(200) {
            return;
        }
        if self.guide.epg_cache.lock().contains_key(&sid) {
            return;
        }
        if *self.guide.epg_pending.lock() == Some(sid) {
            return;
        }
        *self.guide.epg_pending.lock() = Some(sid);
        let cache = self.guide.epg_cache.clone();
        let pending = self.guide.epg_pending.clone();
        let portal = self.catalog.portal().clone();
        let is_archive = channel.tv_archive == 1;
        tokio::spawn(async move {
            let r = if is_archive {
                portal.fetch_day_epg(sid).await
            } else {
                portal.fetch_epg(sid).await
            };
            match r {
                Ok(epg) => {
                    cache.lock().insert(sid, epg);
                }
                Err(e) => tracing::warn!("guide: EPG fetch failed for {}: {}", sid, e),
            }
            *pending.lock() = None;
        });
    }

    fn guide_play_selected_programme(&mut self, channels: &[LiveChannel]) {
        let Some(channel) = self.current_guide_channel(channels).cloned() else { return; };
        let maybe_epg = self.guide.epg_cache.lock().get(&channel.stream_id).cloned();
        let Some(epg) = maybe_epg else {
            self.set_toast("EPG not loaded yet for this channel");
            return;
        };
        let Some(entry) = epg.entries().get(self.guide.programme_cursor).cloned() else {
            return;
        };
        let now = chrono::Utc::now();
        if entry.start > now {
            self.set_toast(format!(
                "not yet aired: {} ({})",
                entry.title,
                entry.start.format("%H:%M")
            ));
            return;
        }
        // Past or currently-airing. Pick the right URL + stream_id.
        let (url, tag) = if entry.end <= now {
            // Past - needs catch-up. Requires archive channel.
            if channel.tv_archive != 1 {
                self.set_toast(format!(
                    "no catch-up for '{}': not an archive channel",
                    channel.name
                ));
                return;
            }
            let dur_min =
                ((entry.end - entry.start).num_minutes() as u32).max(1);
            (
                self.catalog
                    .portal()
                    .catchup_url(channel.stream_id, entry.start, dur_min),
                "CATCHUP",
            )
        } else {
            // Currently airing: live URL. If we're on the archive variant,
            // swap to its live twin by name; otherwise use the channel as-is.
            let live_sid = if channel.tv_archive == 1 {
                self.catalog
                    .live_id_by_name(&channel.name)
                    .unwrap_or(channel.stream_id)
            } else {
                channel.stream_id
            };
            (
                self.catalog.portal().live_stream_url(live_sid),
                "LIVE",
            )
        };
        tracing::info!(
            "guide: [{}] {} | {} ({} -> {})",
            tag,
            channel.name,
            entry.title,
            entry.start.format("%Y-%m-%d %H:%M"),
            entry.end.format("%H:%M")
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
            entry.start.format("%H:%M")
        ));
        self.guide.open = false;
    }

    /// Tune the highlighted channel live (Enter from Channels pane).
    fn guide_tune_selected_channel(&mut self, channels: &[LiveChannel]) {
        let Some(channel) = self.current_guide_channel(channels).cloned() else { return; };
        // Archive channels don't serve /live/; swap to the live twin if it exists.
        let (live_sid, name) = if channel.tv_archive == 1 {
            match self.catalog.live_id_by_name(&channel.name) {
                Some(id) => (id, channel.name.clone()),
                None => {
                    self.set_toast(format!(
                        "no live variant for archive '{}' - pick a programme on the right",
                        channel.name
                    ));
                    return;
                }
            }
        } else {
            (channel.stream_id, channel.name.clone())
        };
        let live = self.catalog.live_channels();
        let idx = live.iter().position(|c| c.stream_id == live_sid);
        self.zap_to(live_sid, &name, idx);
        self.guide.open = false;
    }

    fn handle_guide_keys(&mut self, ctx: &egui::Context) {
        let (esc, up, down, pgup, pgdn, home, end, left, right, tab, enter, backspace, g_key) =
            ctx.input(|i| {
                (
                    i.key_pressed(Key::Escape),
                    i.key_pressed(Key::ArrowUp),
                    i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::PageUp),
                    i.key_pressed(Key::PageDown),
                    i.key_pressed(Key::Home),
                    i.key_pressed(Key::End),
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::Tab),
                    i.key_pressed(Key::Enter),
                    i.key_pressed(Key::Backspace),
                    i.key_pressed(Key::G),
                )
            });
        if esc || g_key {
            self.guide.open = false;
            return;
        }
        // Type chars -> filter (in Channels pane). Doesn't reach the
        // Programmes pane to avoid wiping filter while inspecting EPG.
        if self.guide.pane == GuidePane::Channels {
            let typed: String = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| {
                        if let egui::Event::Text(t) = e {
                            Some(t.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            });
            if !typed.is_empty() {
                self.guide.filter.push_str(&typed);
                self.guide.channel_cursor = 0;
            }
            if backspace && !self.guide.filter.is_empty() {
                self.guide.filter.pop();
                self.guide.channel_cursor = 0;
            }
        }
        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);

        match self.guide.pane {
            GuidePane::Channels => {
                let n = self.guide.visible.len();
                if n > 0 {
                    if down { self.guide.channel_cursor = (self.guide.channel_cursor + 1) % n; }
                    if up   { self.guide.channel_cursor = (self.guide.channel_cursor + n - 1) % n; }
                    if pgdn { self.guide.channel_cursor = (self.guide.channel_cursor + 10).min(n - 1); }
                    if pgup { self.guide.channel_cursor = self.guide.channel_cursor.saturating_sub(10); }
                    if home { self.guide.channel_cursor = 0; }
                    if end  { self.guide.channel_cursor = n - 1; }
                }
                if right || tab {
                    self.guide.pane = GuidePane::Programmes;
                }
                if enter {
                    self.guide_tune_selected_channel(&channels);
                }
            }
            GuidePane::Programmes => {
                let n = self
                    .current_guide_channel(&channels)
                    .and_then(|c| self.guide.epg_cache.lock().get(&c.stream_id).map(|e| e.entries().len()))
                    .unwrap_or(0);
                if n > 0 {
                    if down { self.guide.programme_cursor = (self.guide.programme_cursor + 1) % n; }
                    if up   { self.guide.programme_cursor = (self.guide.programme_cursor + n - 1) % n; }
                    if pgdn { self.guide.programme_cursor = (self.guide.programme_cursor + 10).min(n - 1); }
                    if pgup { self.guide.programme_cursor = self.guide.programme_cursor.saturating_sub(10); }
                    if home { self.guide.programme_cursor = 0; }
                    if end  { self.guide.programme_cursor = n - 1; }
                }
                if left {
                    self.guide.pane = GuidePane::Channels;
                }
                if enter {
                    self.guide_play_selected_programme(&channels);
                }
            }
        }
    }

    fn paint_guide(&mut self, ctx: &egui::Context) {
        if !self.guide.open { return; }
        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);
        self.guide_maybe_fetch_epg(&channels);

        let current_channel = self.current_guide_channel(&channels).cloned();
        let cache = self.guide.epg_cache.lock();
        let programmes: Vec<EpgEntry> = current_channel
            .as_ref()
            .and_then(|c| cache.get(&c.stream_id).map(|e| e.entries().to_vec()))
            .unwrap_or_default();
        drop(cache);
        let now = chrono::Utc::now();
        let visible = self.guide.visible.clone();
        let pane = self.guide.pane;

        egui::Window::new("__guide__")
            .title_bar(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(235))
                    .inner_margin(10.0),
            )
            .show(ctx, |ui| {
                ui.set_width(1100.0);
                ui.set_height(620.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("guide").heading().color(Color32::WHITE),
                    );
                    ui.label(
                        egui::RichText::new("  type to filter   |   arrows nav   |   Tab/right -> programmes   |   Enter play   |   Esc/g close")
                            .color(Color32::from_white_alpha(120))
                            .italics(),
                    );
                });
                if !self.guide.filter.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("filter: {}_", self.guide.filter))
                            .color(Color32::from_rgb(220, 220, 80))
                            .monospace(),
                    );
                }
                ui.separator();

                ui.columns(2, |cols| {
                    // ---- Channels pane ----
                    let active = pane == GuidePane::Channels;
                    cols[0].label(
                        egui::RichText::new(format!(
                            "Channels ({} of {}){}",
                            visible.len(),
                            channels.len(),
                            if active { "  <-" } else { "" }
                        ))
                        .color(if active {
                            Color32::WHITE
                        } else {
                            Color32::from_white_alpha(140)
                        }),
                    );
                    let cursor = self.guide.channel_cursor;
                    let row_h = 18.0;
                    egui::ScrollArea::vertical()
                        .id_source("guide_channels")
                        .auto_shrink([false, false])
                        .show_rows(&mut cols[0], row_h, visible.len(), |ui, range| {
                            for i in range {
                                let ch_idx = visible[i];
                                let ch = &channels[ch_idx];
                                let is_archive = ch.tv_archive == 1;
                                let prefix = if is_archive { "[TK] " } else { "     " };
                                let lbl = format!("{}{}", prefix, ch.name);
                                let resp = ui.selectable_label(i == cursor, lbl);
                                if i == cursor {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                            }
                        });

                    // ---- Programmes pane ----
                    let active = pane == GuidePane::Programmes;
                    let header = match &current_channel {
                        Some(c) => format!("Programmes - {}", c.name),
                        None => "Programmes".into(),
                    };
                    cols[1].label(
                        egui::RichText::new(format!(
                            "{}{}",
                            header,
                            if active { "  <-" } else { "" }
                        ))
                        .color(if active {
                            Color32::WHITE
                        } else {
                            Color32::from_white_alpha(140)
                        }),
                    );
                    if programmes.is_empty() {
                        let pending = *self.guide.epg_pending.lock();
                        let msg = if pending.is_some() {
                            "loading EPG..."
                        } else if current_channel.is_none() {
                            "no channel selected"
                        } else {
                            "no EPG entries"
                        };
                        cols[1].label(
                            egui::RichText::new(msg)
                                .color(Color32::from_white_alpha(120))
                                .italics(),
                        );
                    } else {
                        let cursor = self.guide.programme_cursor;
                        let row_h = 20.0;
                        egui::ScrollArea::vertical()
                            .id_source("guide_programmes")
                            .auto_shrink([false, false])
                            .show_rows(&mut cols[1], row_h, programmes.len(), |ui, range| {
                                for i in range {
                                    let e = &programmes[i];
                                    let marker = if e.start <= now && now < e.end {
                                        "NU "
                                    } else if e.end <= now {
                                        "    "
                                    } else {
                                        "-> "
                                    };
                                    let local = e.start.with_timezone(&chrono::Local);
                                    let lbl = format!(
                                        "{}{}  {}",
                                        marker,
                                        local.format("%a %H:%M"),
                                        e.title
                                    );
                                    let color = if e.end <= now {
                                        Color32::from_white_alpha(120)
                                    } else if e.start <= now {
                                        Color32::from_rgb(220, 220, 80)
                                    } else {
                                        Color32::WHITE
                                    };
                                    let text = egui::RichText::new(lbl).color(color);
                                    let resp = ui.selectable_label(i == cursor, text);
                                    if i == cursor && pane == GuidePane::Programmes {
                                        resp.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                    }
                });
            });
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
