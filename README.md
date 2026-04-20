# miroiptv

A small, fast IPTV player for Windows. Plain C on SDL2 + libav + libcurl;
one executable plus its DLLs in a portable zip, no installer needed.
Tuned for Xtream Codes portals — Dutch NPO/RTL shortcuts baked in, but
any portal works.

## Features

- **Instant zap** — mouse wheel, up/down arrows, or `1`/`2`/`3` for
  NPO 1/2/3. ~300 ms from keypress to new channel on a working stream;
  three-phase UX shows the channel name before audio/video arrive.
- **Cross-catalog search** — press `f`, type; matches live channels,
  VOD movies, and series (the portal catalog at m.hnlol.com has
  15 k+9 k+11 k of each). `Enter` on a series opens an episode picker.
  Title-tag language (`(NL)`, `(FR)`, …) auto-selects the matching
  audio track when the file has multiple.
- **EPG + catch-up** — `e` for a compact bottom-strip overlay, `Shift+e`
  for a scrollable multi-day guide. Enter on a past entry replays it via
  the portal's `/timeshift/` endpoint.
- **Favorites** — `*` toggles the current live channel (also works on any
  `[LIVE]` row in `f`-search); `Shift+F` opens the favorites list, Enter
  zaps, Del removes. Persisted to `%APPDATA%\miroiptv\favorites.json` on
  Windows / `$XDG_CONFIG_HOME/miroiptv/favorites.json` on Linux.
- **News shortcuts** — `n` finds the latest NOS Journaal across NPO
  1/2/3 archives and starts it from the beginning; `r` does the same for
  RTL Nieuws across RTL 4/5/7/8/Z.
- **Audio/subtitle tracks** — `a` cycles audio tracks with a language
  toast; `s` cycles subtitles (text formats: subrip / mov_text / ass /
  webvtt) and renders them at the bottom of the video.
- **VOD seek** — `←/→` skip ±30 s in movies, episodes, and catch-up.
  Seek runs under SDL audio-lock, drains queues both sides, and lets
  the callback re-seed `first_pts` from the actual post-seek position.
- **Borderless window** — right-drag to move, left-double-click cycles
  size 1×→1.5×→2×→3×→4×→1×, `t` toggles always-on-top, `F11` fullscreen.
- **Auto update-check** — on launch, pings GitHub Releases and toasts
  when a newer version is available.

Press `?` in the app at any time for the full keymap.

## Download

Grab the latest `miroiptv-<version>.zip` from
[Releases](../../releases/latest). Unzip anywhere, edit the
`XTREAM_CREDS=` line in `run.bat` with your portal
(`user:pass@host:port`), double-click `run.bat`.

Or pass them on the command line:

    miroiptv.exe --xtream user:pass@host:port

or a bare stream URL for ad-hoc playback:

    miroiptv.exe https://example.com/something.m3u8

## Build from source

Windows + [MSYS2](https://www.msys2.org) + MinGW64:

    pacman -S mingw-w64-x86_64-{gcc,ffmpeg,SDL2,SDL2_ttf,curl,cjson,pkgconf,make}
    ./mk               # incremental build -> build/miroiptv.exe
    ./mk clean && ./mk # fresh rebuild
    ./build-dist.sh    # build + package the portable zip

`./mk` is a thin wrapper that runs the command inside MSYS2's MinGW64
environment so the codec DLLs resolve correctly.

## Releasing

Uses the tag to derive the version; push a tag, then:

    ./build-dist.sh --release    # build + gh release create

Requires the [GitHub CLI](https://cli.github.com) (`gh auth login` once).

## How it works

- **Demux + decode:** libavformat reads the HTTP stream, libavcodec
  decodes H.264 / AAC. Decoder runs on its own pthread, pushing frames
  into bounded queues (16 video / 32 audio) that the main thread
  consumes at VSYNC pace.
- **A/V clock:** audio-master. The SDL audio callback seeds `first_pts`
  from the first chunk it pulls; `av_clock_now() = first_pts +
  samples_played/sample_rate` drives the video-frame pacing decisions
  in main. SDL audio starts paused and only unpauses when the decoder
  has produced its first video frame, so every zap starts with A/V in
  sync.
- **Async zap:** wheel commit spawns a detached `zap_prep_worker` that
  fetches EPG and opens playback off-main. The main thread shows an
  instant "channel name" toast, enriches with the current programme
  once EPG lands, and swaps the playback struct when the stream is
  ready — never blocking the UI on HTTP or libav probe.
- **Search:** live + VOD + series catalogs fetched once at startup via
  `player_api.php` (Xtream Codes). Case-insensitive substring match,
  capped at 12 visible hits, tagged `[LIVE]`/`[MOVIE]`/`[SERIES]` so
  the user sees what Enter will do.

## Project layout

    src/main.c            event loop, zap, search, overlay wiring
    src/player.{c,h}      libav demux + decoder thread, track switching
    src/render.{c,h}      SDL window / texture upload / overlays / audio device
    src/queue.{c,h}       bounded SPSC-ish blocking queue (mutex + conds)
    src/sync.{c,h}        av_clock seeded from audio samples_played
    src/npo.{c,h}         HTTP GET, EPG JSON parse, channel resolve
    src/xtream.{c,h}      Xtream Codes portal API (live/VOD/series/EPG)
    src/update_check.{c,h}  one-shot GitHub-Releases version check
    assets/DejaVuSans.ttf font used for every overlay
    build-dist.sh         bundle into portable zip + optional gh release
    mk, mk.cmd            MSYS2 env wrappers for the build/run commands

## Licence

Personal project, no licence declared. If you want to reuse any of the
code, open an issue and we'll figure something out.
