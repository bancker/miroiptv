# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Two codebases in one repo

This repo holds **two implementations of the same IPTV player**, both living on `main`, each in its own self-contained project folder (own sources, build scripts, `.gitignore` and build output — nothing builds into the repo root):

- **`rust/` — Bongo Player (ACTIVE; formerly `tvplayer`).** A clean rebuild on **libmpv + egui/eframe (wgpu) + tokio**. This is where new work happens.
- **`c/` — `miroiptv` (FALLBACK).** The original plain-C player on **SDL2 + libav + libcurl**. Kept working as a fallback while the Rust rewrite is validated. Touch it only when explicitly working on the C app.

Shared design docs stay at the repo root in `docs/`.

When a request is ambiguous, assume it targets the **Rust Bongo Player** app.

**Performance is the top constraint.** The whole point of this player is "small, fast, featherlight" — instant zap (~300 ms), one portable exe + DLLs, no installer. The C app was abandoned partly because it grew slow. No change should regress startup time, zap latency, or memory footprint.

## Commands

### Rust Bongo Player (primary — run from `rust/`)

```powershell
.\vendor\fetch-mpv.ps1               # one-time: vendor libmpv-2.dll + headers
cargo test                           # unit + integration tests (see rust/tests/)
cargo test --test test_epg           # a single integration-test file
cargo test <substring>               # a single test by name substring
cargo build --release                # -> rust\target\release\BongoPlayer.exe
.\target\release\BongoPlayer.exe --selftest   # headless self-check, no portal needed
cargo clippy --all-targets           # lint
cargo fmt                            # format
```

Package a portable zip (also from `rust/`; output stays in `rust\dist\`):

```powershell
.\build-zip.ps1                      # -> rust\dist\BongoPlayer-vX.Y.Z.zip
.\verify.ps1                         # smoke-test the built zip
```

### C `miroiptv` (fallback — Windows + MSYS2/MinGW64, run from `c/`)

```bash
./mk                 # incremental build -> c/build/miroiptv.exe (wraps make in the MinGW64 env)
./mk test            # build + run the C unit tests
./mk clean && ./mk   # fresh rebuild
./build-dist.sh      # build + package the portable zip (-> c/miroiptv-<ver>.zip)
./build-dist.sh --release   # build + `gh release create` (version derived from git tag)
```

`./mk` / `mk.cmd` exist because the codec DLLs only resolve inside MSYS2's MinGW64 shell. The C exe loads `assets/` **relative to the working directory**, so it must run from `c/`; `mk.cmd` and `build-dist.cmd` cd to their own folder, so they work from any directory.

### Running

Both apps take a portal on the command line: `--xtream user:pass@host:port`, or a bare stream URL for ad-hoc playback. `tv.bat` (repo root) is a personal launcher for the C app (via `c\mk.cmd`) holding real credentials and is **git-ignored** — never commit it or hardcode credentials.

## Rust architecture (the big picture)

The module tree is declared in `rust/src/lib.rs`. The non-obvious structure:

- **`player/` — the libmpv boundary.** `mpv_sys.rs` is the raw `unsafe` FFI (constants, structs, extern fns); `mpv.rs` is the safe wrapper around it. `player::spawn()` starts a **dedicated `mpv-player` thread** that owns *all* mpv interaction — never call mpv from elsewhere. The rest of the app talks to it through channels:
  - `Cmd` (LoadUrl, Pause, SeekRelative, CycleAudio/Subtitle, SetWindowSize, Quit) flows in.
  - `Event` (FileLoaded, PlaybackStarted, EndOfFile, Error, PropertyChanged) flows out.
  - `render.rs` renders mpv frames into a shared `FrameBus` of `RgbaFrame`s, which the egui layer uploads as a texture. Video is drawn *by mpv's render API into RGBA*, not by a windowed mpv.
- **`app.rs` — the egui app + all UX.** Owns the event loop, zap logic, search overlay, EPG guide (now/next vs prime-time views, channel modes All/Live/Catchup), favorites, quality switching, and the `ProgrammeStatus` (Live / Catchup / Future / PastUnavailable) that decides whether an EPG entry is playable. This is the largest file; most feature work lands here. It is a shared bottleneck — do features **sequentially**, not in parallel, to avoid collisions.
- **`relay/` — local HLS relay for live streams.** Live channels never go to mpv directly: mpv plays `http://127.0.0.1:<port>/live/<id>.ts` and the relay fetches the portal's HLS segments strictly in media-sequence order into that one never-ending TS response. A refused playlist refresh (HTTP 509) is retried — same edge URL, then a fresh token via the portal — while mpv plays from the ~3-segment cushion, so it never sees an error. Also opens the portal once per zap (mpv opened it twice) and keeps credentials out of mpv's URLs/logs. `Relay::local_url()` already starts fetching (prefetch), so the portal round trips overlap mpv tearing down the previous stream; a newer load cancels an unclaimed prefetch, and a session ends the moment mpv hangs up. Starting a session waits out "busy" answers (407/503, up to `busy_wait`) but fails fast on real errors. Catch-up/HLS VOD go through `Relay::vod_url()`: the finished playlist is fetched once and served from memory to both of mpv's opens (each portal open is a playlist generation of ~2.5 s); segments still come straight from the portal, so seeking is mpv's own. `load_url()` in `app.rs` does the routing.
- **`portal/` — Xtream Codes API.** `xtream.rs` implements the `Portal` trait; `types.rs` holds the wire types (`LiveChannel`, etc.). HTTP is async via `reqwest` on a tokio runtime.
- **`catalog.rs`** builds the channel catalog, ranks stream quality (`quality_rank`/`quality_label`) and dedups to one best-quality entry per channel.
- **`epg.rs`** parses EPG listings (handles entries carrying either `start` or `start_timestamp`).
- **`search.rs`** cross-catalog search over live / VOD / series (`ItemKind`).
- **`playback_health.rs`** is the **freeze/stall watchdog**: it keys off mpv's `paused-for-cache` property (observed as a string "yes"/"no") to detect a frozen picture and drive recovery / quality switching. Armed for live channels, not for VOD/catch-up.
- **`favorites.rs` / `storage.rs` / `presets.rs`** handle persistence (favorites, last-watched-for-resume) under the platform config dir.
- **`shortcuts.rs`** maps keys to actions; Dutch NPO/RTL channel shortcuts are baked in.
- **`zap.rs`** paces up/down zapping (`ZapPacer`): a lone press zaps at once; quick presses or a held key only flip the names and load where the user stops.

### Gotchas worth knowing

- **Names come from `lib.rs`.** `APP_NAME` ("Bongo Player") is what users see; `APP_ID` ("BongoPlayer") names the exe, `BongoPlayer.ini`, `%APPDATA%\BongoPlayer\config\`, the log file and the portal User-Agent. Crate names differ again (`bongo_player` lib, `BongoPlayer` bin), so the default log filter is built from `module_path!()` — never hardcode crate names there, or the app's logs go silent. `legacy.rs` migrates the pre-rename `tvplayer` data (`%APPDATA%\tvplayer\config`, `tvplayer.ini`) once on first start; keep it until no tvplayer installs remain.
- **Never route live HLS straight to mpv.** libav gives up on the first refused playlist refresh (`hls: Failed to reload playlist` → eof → frozen picture); this portal refuses ~1 in 4 directly-played live streams around the first reload. The relay rides that out invisibly. As a last line, an unexpected eof on a live stream auto-reloads (`ReloadBudget`, 3/min) before giving up with a toast.
- **The portal allows one session per account, newest wins, and throttles bursts.** Every load or stray request (a leftover relay session, a prefetch, mpv's ytdl_hook retrying a URL) takes the session from the stream being watched. A held arrow key used to open ~30 sessions a second (each with an EPG fetch): the portal answered 503 and stayed slow for minutes (catch-up playlists 15–17 s, 407 on live). Hence the zap pacer, relay sessions that stop with their player, and mpv's built-in Lua scripts (ytdl_hook among them) switched off in `player/mpv.rs`.
- **`video-latency-hacks=yes` in `player/mpv.rs` is deliberate.** Measured zap → first frame 1.10 → 0.82 s. Keeping the audio device open across zaps (`gapless-audio=yes`) and software decoding were measured too and were slower — benchmark before changing mpv options; mpv's `playback-restart` event can fire before the picture, so time to its "playback restart complete" log line instead.
- **Benign mpv log noise is downgraded on purpose.** `player/mod.rs::effective_log_level()` demotes specific ffmpeg "error" lines to debug: HLS HTTP-EOF (Xtream portals close the playlist response without Content-Length) and the H.264 PPS/SPS/"no frame!" flood on every zap (B/P frames before the first keyframe). These are *not* real errors — don't "fix" them by removing the filter, or the log becomes unreadable on every channel switch.
- **Release profile is `panic = "abort"`** (plus thin LTO, `strip`, `codegen-units = 1`). No unwinding — a panic kills the process. Keep that in mind for anything relying on catching panics.

## C architecture (fallback, brief)

Decoder runs on its own pthread pushing into bounded queues (16 video / 32 audio); the main thread consumes at VSYNC. A/V clock is **audio-master** — the SDL audio callback seeds `first_pts` and drives video pacing. Zap is async (`zap_prep_worker`) so the UI never blocks on HTTP/probe. See `c/src/main.c` (event loop), `c/src/player.c` (demux/decode), `c/src/render.c` (SDL/overlays/audio), and `c/README.md` "How it works" for the full picture.

## Design docs & validation

Feature specs and plans live under `docs/superpowers/specs/` and `docs/superpowers/plans/` (e.g. the rust-tvplayer, favorites, prebuffer, freeze-detection, and quality-switching designs). Manual QA steps are in `docs/VALIDATION.md`. Consult the matching design doc before extending a feature.
