use bongo_player::player::Mpv;

#[test]
#[ignore]
fn create_and_destroy() {
    let mpv = Mpv::new().expect("mpv init");
    let v = mpv.get_property_string("mpv-version");
    assert!(v.is_some(), "mpv-version should be readable");
    println!("mpv-version: {}", v.unwrap());
}

#[test]
#[ignore]
fn runs_none_of_mpvs_builtin_scripts() {
    // Each is a Lua VM on its own thread (~2.5 MB together) that the app never
    // uses; ytdl_hook even launches yt-dlp on the portal URL when a stream
    // fails to open - another portal session, seconds later.
    let mpv = Mpv::new().expect("mpv init");
    for opt in [
        "ytdl",
        "load-stats-overlay",
        "load-console",
        "load-auto-profiles",
        "load-select",
        "load-positioning",
        "load-commands",
    ] {
        assert_eq!(
            mpv.get_property_string(opt).as_deref(),
            Some("no"),
            "{opt} is on"
        );
    }
}

#[test]
#[ignore]
fn loadfile_dummy_succeeds() {
    let mpv = Mpv::new().expect("mpv init");
    let r = mpv.command(&[
        "loadfile",
        "av://lavfi:smptebars=size=640x360:rate=25:duration=1",
    ]);
    assert!(r.is_ok(), "loadfile should succeed: {:?}", r);
}
