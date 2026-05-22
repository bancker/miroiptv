use crate::catalog::CatalogStore;
use crate::epg::Epg;
use crate::favorites::Favorites;
use crate::player::{Cmd, Event, PlayerHandle, RgbaFrame};
use crate::search::ItemKind;
use crate::shortcuts;
use crate::storage::Storage;
use egui::{Color32, ColorImage, Key, TextureHandle, TextureOptions};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

pub struct TvApp {
    player: PlayerHandle,
    catalog: Arc<CatalogStore>,
    favorites: Favorites,
    storage: Storage,

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
}

impl TvApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        player: PlayerHandle,
        catalog: Arc<CatalogStore>,
        storage: Storage,
    ) -> Self {
        // Whenever a new mpv frame is ready, ask egui to redraw.
        let ctx = cc.egui_ctx.clone();
        player.frames.set_new_frame_callback(move || ctx.request_repaint());

        let favorites = Favorites::load(&storage.favorites_path()).unwrap_or_default();
        catalog.spawn_fetch();

        Self {
            player,
            catalog,
            favorites,
            storage,
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
        let mut s = self.epg_slot.lock();
        if let Some((sid, epg)) = s.take() {
            if Some(sid) == self.epg_fetch_pending_for {
                self.current_epg = Some(epg);
                self.epg_fetch_pending_for = None;
            }
        }
    }

    fn kick_epg_fetch(&mut self, stream_id: i64) {
        let portal = self.catalog.portal().clone();
        let slot = self.epg_slot.clone();
        self.epg_fetch_pending_for = Some(stream_id);
        tokio::spawn(async move {
            match portal.fetch_epg(stream_id).await {
                Ok(epg) => { *slot.lock() = Some((stream_id, epg)); }
                Err(e) => tracing::warn!("epg fetch failed for {}: {}", stream_id, e),
            }
        });
    }

    fn zap_to(&mut self, sid: i64, name: &str, idx: Option<usize>) {
        let url = self.catalog.portal().live_stream_url(sid);
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
            down, up, left, right, scroll, f_key, e_key, shift, d, n1, n2, n3,
            f11, esc, n_key, r_key, a_key, s_key, star,
        ) = ctx.input(|i| (
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
            i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "*")),
        ));

        if down { self.zap_delta(1); }
        if up { self.zap_delta(-1); }
        if scroll > 0.5 { self.zap_delta(-1); }
        if scroll < -0.5 { self.zap_delta(1); }

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
            if self.show_search { self.search_query.clear(); }
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

        if d { self.show_debug = !self.show_debug; }

        if n1 { self.zap_npo(1); }
        if n2 { self.zap_npo(2); }
        if n3 { self.zap_npo(3); }

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

        if star { self.toggle_favorite_current(); }
    }

    fn update_video_texture(&mut self, ctx: &egui::Context) {
        let frame: RgbaFrame = self.player.frames.read();
        if frame.w == 0 || frame.h == 0 { return; }
        if frame.version == self.last_frame_version && self.video_tex.is_some() { return; }
        self.last_frame_version = frame.version;

        // mpv emits rgb0: bytes are R, G, B, 0. egui ColorImage wants RGBA.
        // Just force the 4th byte to 255 (opaque).
        let mut rgba = frame.data.clone();
        for px in rgba.chunks_exact_mut(4) { px[3] = 255; }
        let img = ColorImage::from_rgba_unmultiplied(
            [frame.w as usize, frame.h as usize],
            &rgba,
        );
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
        let Some((text, t0)) = &self.toast else { return; };
        if t0.elapsed() > Duration::from_secs(4) { return; }
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
        if !self.show_search { return; }
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
                            if ui.selectable_label(false, format!("{} {}", tag, it.name)).clicked() {
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
            ItemKind::Live => { self.zap_by_id(id); }
            ItemKind::Movie => { self.play_movie(id, name); }
            ItemKind::Series => {
                self.set_toast(format!("series picker not in v1: {}", name));
            }
        }
        self.show_search = false;
        self.search_query.clear();
    }

    fn paint_favs(&mut self, ctx: &egui::Context) {
        if !self.show_favs { return; }
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
        if !self.show_epg_strip { return; }
        let Some(epg) = &self.current_epg else { return; };
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
            text.push_str(&format!("    ->  {}  {}", nxt.start.format("%H:%M"), nxt.title));
        }
        if text.is_empty() { return; }

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
        if !self.show_epg_grid { return; }
        let Some(epg) = &self.current_epg else { return; };
        egui::Area::new(egui::Id::new("__epg_grid__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(230))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        ui.heading("EPG");
                        ui.separator();
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
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
        if !self.show_debug { return; }
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
                        ui.label(format!("catalog: {}", if self.catalog.is_loaded() { "loaded" } else { "loading..." }));
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
        self.handle_keys(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                if let Some(tex) = &self.video_tex {
                    let avail = ui.available_size();
                    ui.add(egui::Image::new(tex).fit_to_exact_size(avail));
                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.4);
                        ui.label(
                            egui::RichText::new("tvplayer")
                                .color(Color32::from_white_alpha(80))
                                .heading()
                        );
                        if !self.catalog.is_loaded() {
                            ui.label(
                                egui::RichText::new("loading catalog...")
                                    .color(Color32::from_white_alpha(60))
                            );
                        }
                    });
                }
            });

        self.paint_toast(ctx);
        self.paint_search(ctx);
        self.paint_favs(ctx);
        self.paint_epg_strip(ctx);
        self.paint_epg_grid(ctx);
        self.paint_debug_hud(ctx);

        ctx.request_repaint_after(Duration::from_millis(33));
    }
}
