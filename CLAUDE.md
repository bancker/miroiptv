# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Two codebases in one repo

This repo holds **two implementations of the same IPTV player**, both living on `main`:

- **`rust/` — `tvplayer` (ACTIVE).** A clean rebuild on **libmpv + egui/eframe (wgpu) + tokio**. This is where new work happens.
- **`src/` — `miroiptv` (FALLBACK).** The original plain-C player on **SDL2 + libav + libcurl**. Kept working as a fallback while the Rust rewrite is validated. Touch it only when explicitly working on the C app.

When a request is ambiguous, assume it targets the **Rust `tvplayer`** app.

**Performance is the top constraint.** The whole point of this player is "small, fast, featherlight" — instant zap (~300 ms), one portable exe + DLLs, no installer. The C app was abandoned partly because it grew slow. No change should regress startup time, zap latency, or memory footprint.

## Commands

### Rust `tvplayer` (primary — run from `rust/` unless noted)

```powershell
.\rust\vendor\fetch-mpv.ps1          # one-time: vendor libmpv-2.dll + headers (run from repo root)
cd rust
cargo test                           # unit + integration tests (see rust/tests/)
cargo test --test test_epg           # a single integration-test file
cargo test <substring>               # a single test by name substring
cargo build --release                # -> rust\target\release\tvplayer.exe
.\target\release\tvplayer.exe --selftest   # headless self-check, no portal needed
cargo clippy --all-targets           # lint
cargo fmt                            # format
```

Package a portable zip (from repo root):

```powershell
.\build-zip.ps1                      # -> dist\tvplayer-vX.Y.Z.zip
.\verify.ps1                         # smoke-test the built zip
```

### C `miroiptv` (fallback — Windows + MSYS2/MinGW64)

```bash
./mk                 # incremental build -> build/miroiptv.exe (wraps make in the MinGW64 env)
./mk clean && ./mk   # fresh rebuild
./build-dist.sh      # build + package the portable zip
./build-dist.sh --release   # build + `gh release create` (version derived from git tag)
```

`./mk` / `mk.cmd` exist because the codec DLLs only resolve inside MSYS2's MinGW64 shell.

### Running

Both apps take a portal on the command line: `--xtream user:pass@host:port`, or a bare stream URL for ad-hoc playback. `tv.bat` is a personal launcher holding real credentials and is **git-ignored** — never commit it or hardcode credentials.

## Rust architecture (the big picture)

The module tree is declared in `rust/src/lib.rs`. The non-obvious structure:

- **`player/` — the libmpv boundary.** `mpv_sys.rs` is the raw `unsafe` FFI (constants, structs, extern fns); `mpv.rs` is the safe wrapper around it. `player::spawn()` starts a **dedicated `mpv-player` thread** that owns *all* mpv interaction — never call mpv from elsewhere. The rest of the app talks to it through channels:
  - `Cmd` (LoadUrl, Pause, SeekRelative, CycleAudio/Subtitle, SetWindowSize, Quit) flows in.
  - `Event` (FileLoaded, PlaybackStarted, EndOfFile, Error, PropertyChanged) flows out.
  - `render.rs` renders mpv frames into a shared `FrameBus` of `RgbaFrame`s, which the egui layer uploads as a texture. Video is drawn *by mpv's render API into RGBA*, not by a windowed mpv.
- **`app.rs` — the egui app + all UX.** Owns the event loop, zap logic, search overlay, EPG guide (now/next vs prime-time views, channel modes All/Live/Catchup), favorites, quality switching, and the `ProgrammeStatus` (Live / Catchup / Future / PastUnavailable) that decides whether an EPG entry is playable. This is the largest file; most feature work lands here. It is a shared bottleneck — do features **sequentially**, not in parallel, to avoid collisions.
- **`portal/` — Xtream Codes API.** `xtream.rs` implements the `Portal` trait; `types.rs` holds the wire types (`LiveChannel`, etc.). HTTP is async via `reqwest` on a tokio runtime.
- **`catalog.rs`** builds the channel catalog, ranks stream quality (`quality_rank`/`quality_label`) and dedups to one best-quality entry per channel.
- **`epg.rs`** parses EPG listings (handles entries carrying either `start` or `start_timestamp`).
- **`search.rs`** cross-catalog search over live / VOD / series (`ItemKind`).
- **`playback_health.rs`** is the **freeze/stall watchdog**: it keys off mpv's `paused-for-cache` property (observed as a string "yes"/"no") to detect a frozen picture and drive recovery / quality switching. Armed for live channels, not for VOD/catch-up.
- **`favorites.rs` / `storage.rs` / `presets.rs`** handle persistence (favorites, last-watched-for-resume) under the platform config dir.
- **`shortcuts.rs`** maps keys to actions; Dutch NPO/RTL channel shortcuts are baked in.

### Two gotchas worth knowing

- **Benign mpv log noise is downgraded on purpose.** `player/mod.rs::effective_log_level()` demotes specific ffmpeg "error" lines to debug: HLS HTTP-EOF (Xtream portals close the playlist response without Content-Length) and the H.264 PPS/SPS/"no frame!" flood on every zap (B/P frames before the first keyframe). These are *not* real errors — don't "fix" them by removing the filter, or the log becomes unreadable on every channel switch.
- **Release profile is `panic = "abort"`** (plus thin LTO, `strip`, `codegen-units = 1`). No unwinding — a panic kills the process. Keep that in mind for anything relying on catching panics.

## C architecture (fallback, brief)

Decoder runs on its own pthread pushing into bounded queues (16 video / 32 audio); the main thread consumes at VSYNC. A/V clock is **audio-master** — the SDL audio callback seeds `first_pts` and drives video pacing. Zap is async (`zap_prep_worker`) so the UI never blocks on HTTP/probe. See `src/main.c` (event loop), `src/player.c` (demux/decode), `src/render.c` (SDL/overlays/audio), and README.md "How it works" for the full picture.

## Design docs & validation

Feature specs and plans live under `docs/superpowers/specs/` and `docs/superpowers/plans/` (e.g. the rust-tvplayer, favorites, prebuffer, freeze-detection, and quality-switching designs). Manual QA steps are in `docs/VALIDATION.md`. Consult the matching design doc before extending a feature.
