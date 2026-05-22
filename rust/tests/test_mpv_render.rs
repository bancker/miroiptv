use std::sync::Arc;
use std::time::Duration;
use tvplayer::player::{FrameBus, Mpv, RenderCtx};

#[test]
#[ignore]
fn render_frames_from_test_pattern() {
    let mpv = Mpv::new().expect("mpv init");
    let frames = Arc::new(FrameBus::new());
    let mut rctx = RenderCtx::new(&mpv, frames.clone(), 1280, 720).expect("render ctx");

    mpv.command(&[
        "loadfile",
        "av://lavfi:smptebars=size=1280x720:rate=25:duration=2",
    ])
    .expect("loadfile");

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut frames_rendered = 0;
    let mut saw_end = false;
    while std::time::Instant::now() < deadline && frames_rendered < 5 && !saw_end {
        let evt = mpv.wait_event(0.05);
        let id = unsafe { (*evt).event_id };
        if id == tvplayer::player::mpv_sys::MPV_EVENT_END_FILE {
            saw_end = true;
        }
        if rctx.has_new_frame() {
            rctx.render_into_frames().expect("render");
            frames_rendered += 1;
        }
    }

    assert!(
        frames_rendered >= 1,
        "expected at least 1 frame rendered, got {}",
        frames_rendered
    );
    let f = frames.read();
    assert_eq!(f.w, 1280);
    assert_eq!(f.h, 720);
    assert_eq!(f.data.len(), 1280 * 720 * 4);
    assert!(
        f.data.iter().any(|&b| b != 0),
        "frame should not be all-zero"
    );
}
