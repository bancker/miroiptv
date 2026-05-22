pub mod events;
pub mod mpv;
pub mod mpv_sys;
pub mod render;

pub use mpv::{Mpv, MpvError};
pub use render::{FrameBus, RenderCtx, RgbaFrame};

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};

#[derive(Debug)]
pub enum Cmd {
    LoadUrl(String),
    Pause(bool),
    SeekRelative(f64),
    CycleAudio,
    CycleSubtitle,
    SetWindowSize(u32, u32),
    Quit,
}

#[derive(Debug, Clone)]
pub enum Event {
    FileLoaded,
    PlaybackStarted,
    EndOfFile { reason: String },
    Error { msg: String },
    PropertyChanged { name: String, value: String },
}

pub struct PlayerHandle {
    pub cmd_tx: UnboundedSender<Cmd>,
    pub evt_rx: parking_lot::Mutex<UnboundedReceiver<Event>>,
    pub frames: Arc<FrameBus>,
}

pub fn spawn(initial_w: u32, initial_h: u32) -> anyhow::Result<PlayerHandle> {
    let (cmd_tx, mut cmd_rx) = unbounded_channel::<Cmd>();
    let (evt_tx, evt_rx) = unbounded_channel::<Event>();
    let frames = Arc::new(FrameBus::new());
    let frames_for_thread = frames.clone();

    thread::Builder::new()
        .name("mpv-player".into())
        .spawn(move || {
            let mpv = match Mpv::new() {
                Ok(m) => m,
                Err(e) => {
                    let _ = evt_tx.send(Event::Error {
                        msg: format!("mpv init: {}", e),
                    });
                    return;
                }
            };
            let mut rctx =
                match RenderCtx::new(&mpv, frames_for_thread, initial_w.max(1), initial_h.max(1)) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = evt_tx.send(Event::Error {
                            msg: format!("render ctx: {}", e),
                        });
                        return;
                    }
                };

            info!("mpv player thread started");

            loop {
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::LoadUrl(url)) => {
                            info!("loadfile {}", url);
                            if let Err(e) = mpv.command(&["loadfile", &url]) {
                                warn!("loadfile failed: {}", e);
                                let _ = evt_tx.send(Event::Error { msg: e.to_string() });
                            }
                        }
                        Ok(Cmd::Pause(p)) => {
                            let _ = mpv.set_property("pause", if p { "yes" } else { "no" });
                        }
                        Ok(Cmd::SeekRelative(s)) => {
                            let _ = mpv.command(&["seek", &s.to_string(), "relative"]);
                        }
                        Ok(Cmd::CycleAudio) => {
                            let _ = mpv.command(&["cycle", "audio"]);
                        }
                        Ok(Cmd::CycleSubtitle) => {
                            let _ = mpv.command(&["cycle", "sub"]);
                        }
                        Ok(Cmd::SetWindowSize(w, h)) => {
                            rctx.resize(w, h);
                        }
                        Ok(Cmd::Quit) => {
                            info!("player thread quit");
                            return;
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return,
                    }
                }

                let evt = mpv.wait_event(0.005);
                let id = unsafe { (*evt).event_id };
                if id != mpv_sys::MPV_EVENT_NONE {
                    debug!("mpv event id={}", id);
                    if let Some(e) = unsafe { events::from_mpv(evt) } {
                        let _ = evt_tx.send(e);
                    }
                    if id == mpv_sys::MPV_EVENT_PROPERTY_CHANGE {
                        if let Some((n, v)) = unsafe { events::property_change(evt) } {
                            let _ = evt_tx.send(Event::PropertyChanged { name: n, value: v });
                        }
                    }
                    if id == mpv_sys::MPV_EVENT_SHUTDOWN {
                        return;
                    }
                }

                if rctx.has_new_frame() {
                    if let Err(e) = rctx.render_into_frames() {
                        error!("render failed: {}", e);
                        let _ = evt_tx.send(Event::Error {
                            msg: format!("render: {}", e),
                        });
                        thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    thread::sleep(Duration::from_micros(500));
                }
            }
        })?;

    Ok(PlayerHandle {
        cmd_tx,
        evt_rx: parking_lot::Mutex::new(evt_rx),
        frames,
    })
}
