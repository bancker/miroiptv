# Design: Rust TV Player (`tvplayer`) — clean rebuild

Status: approved 2026-05-22
Supersedes: `src/*.c` (kept on `main` as fallback)

## Why rebuild

The C app grew from "play NPO 1/2/3" to "Xtream Codes + EPG + search + favorites + HLS prebuffer + ASS subs + AV-sync nudge keys". Each feature solved its specific bug but added layers — recent commits (`fix(subs): correct comma count for ffmpeg-4.0+ ASS format`, `av-sync: '[' and ']' keys to nudge A/V offset live`, `hls_prefetch: buffer segments atomically — never stream partial bytes`) are all symptoms of re-implementing things that a mature player engine already does correctly. Build process (MSYS2 + MinGW64 + many DLLs) is painful. Startup is sluggish. The codebase is Frankenstein-shaped.

Goal: a lightweight, fast Rust TV player. As capable as VLC for IPTV use but simpler, faster cold start, single portable zip. Commercially sellable.

## Non-goals (v1)

- Catch-up / timeshift (excluded by scope decision — defer to v1.1).
- Auto update-check.
- State import from old `%APPDATA%\miroiptv\`.
- MSI installer.
- NPO direct resolver (it's broken in the C app anyway). Live channels come exclusively via the Xtream portal.
- Linux / macOS builds (v1 is Windows-only — cross-platform later if commercially relevant).

## Architecture

```
┌─────────────────────────────────────────────┐
│  Main thread (winit event loop)             │
│  ┌───────────────────────────────────────┐  │
│  │ egui frame:                          │  │
│  │  ├─ sample mpv RGBA texture          │  │
│  │  ├─ draw overlays (toasts/EPG/search)│  │
│  │  └─ poll player_events.try_recv()    │  │
│  └───────────────────────────────────────┘  │
└──────────────┬──────────────────────────────┘
               │ player_cmd_tx (tokio mpsc)
               ▼
┌─────────────────────────────────────────────┐
│  Tokio runtime (multi-thread)              │
│  ┌──────────────┐  ┌─────────────────────┐ │
│  │ PlayerActor  │  │ Portal HTTP tasks   │ │
│  │  cmd loop    │──▶  reqwest + serde    │ │
│  └──────┬───────┘  └─────────────────────┘ │
│         │ libmpv FFI                        │
└─────────┼───────────────────────────────────┘
          ▼
┌─────────────────────────────────────────────┐
│  libmpv (own worker threads)                │
│  Demux + decode + AV sync + libass subs     │
│  Render: SW path → RGBA pixel buffer        │
└─────────────────────────────────────────────┘
```

### Threading model

- **Main thread**: winit + egui event loop, ~60 fps. Never blocks on I/O. Polls `player_events.try_recv()` once per frame.
- **Tokio runtime** (multi-thread, 2 worker threads sufficient): the `PlayerActor` task receives `Cmd` messages, drives libmpv, fetches portal data via `reqwest`, emits `Event` messages.
- **libmpv internal threads**: managed by mpv itself (demuxer, video, audio, render). Opaque to us.

Communication is exclusively via `tokio::sync::mpsc` channels between threads. No shared mutable state — no `volatile`, no locks beyond what mpsc gives us. Compile-time `Send` guarantees prevent the race-y patterns the C app accumulated.

### Video integration: mpv software-render path

mpv exposes `mpv_render_context` with `MPV_RENDER_API_TYPE_SW`. Each frame:

1. `PlayerActor` (or a dedicated render task) calls `mpv_render_context_render` with our pre-allocated RGBA buffer.
2. The buffer is shared with the UI thread via a triple-buffered `Arc<Mutex<RgbaFrame>>` (UI takes the latest ready buffer; render thread writes to the next).
3. UI thread uploads to a wgpu texture, displays via `egui::Image`. Overlays composite on top in the same egui frame.

Why SW path:
- No GL / D3D / Vulkan context sharing between mpv and wgpu — historically the worst integration pain on Windows.
- mpv still does **hardware decoding** internally (D3D11VA when supported); only the final compositing step is in software.
- 1080p@60fps RGBA = ~475 MB/s memcpy. Negligible on any CPU built since 2018.
- libass-rendered subtitles are baked into the output buffer — we don't render text subtitles ourselves. Whole subtitle module from C app: gone.

If profiling later shows the SW copy is a bottleneck (unlikely for 1080p), we can switch to the GL path without changing module boundaries.

### libmpv FFI: hand-written, no bindgen

Rather than depend on `libmpv2` crate (which uses bindgen → needs libclang at build time → fragile CI), we ship `~30` hand-written FFI declarations in `src/player/mpv_sys.rs` covering the functions we actually call:

- `mpv_create`, `mpv_initialize`, `mpv_destroy`
- `mpv_set_option_string`, `mpv_set_property_string`, `mpv_get_property`, `mpv_observe_property`
- `mpv_command`, `mpv_command_async`
- `mpv_wait_event`, `mpv_request_log_messages`
- `mpv_render_context_create`, `mpv_render_context_render`, `mpv_render_context_free`

The mpv C API is stable since 2017; rolling our own bindings is a one-time ~150-line file that never breaks.

## Components

```
src/
  main.rs            entry: args, logging, build app, run egui
  app.rs             TvApp: top-level state, frame handler
  args.rs            clap-derived CLI: --xtream user:pass@host:port, optional bare URL
  storage.rs         %APPDATA%\tvplayer\ paths, load/save JSON
  config.rs          Config { xtream_creds, last_channel_idx, audio_lang_pref, ... }

  player/
    mod.rs           PlayerActor, Cmd, Event, spawn()
    mpv_sys.rs       FFI declarations
    mpv.rs           safe wrapper: Mpv handle, RAII drop, property helpers
    render.rs        RgbaFrame triple-buffer + render-context glue
    events.rs        mpv event loop → Event mapping

  portal/
    mod.rs           trait Portal { fetch_catalog, stream_url, fetch_epg }
    xtream.rs        Xtream Codes impl using reqwest
    types.rs         LiveChannel, Movie, Series, Episode, EpgEntry

  catalog.rs         Catalog { live, movies, series } with .search(query)
  search.rs          ranked substring matcher: tagged [LIVE]/[MOVIE]/[SERIES]
  epg.rs             EpgEntry, current-program lookup, formatting
  favorites.rs       Favorites list + load/save JSON, toggle/move/remove
  shortcuts.rs       keymap, news handlers (n → NOS Journaal, r → RTL Nieuws)

  ui/
    mod.rs           composition: layer overlays over video
    overlay.rs       channel-name + program toast
    search_box.rs    f-key popup with ranked results
    epg_strip.rs     e-key compact bar
    epg_grid.rs      Shift+E multi-day grid
    favorites_panel.rs   Shift+F list
    debug_hud.rs     d-key diagnostic overlay (fps, mpv stats, A/V offset)
    toast.rs         transient text overlay primitive

tests/
  test_search.rs            ranking, edge cases
  test_favorites.rs         load/save/toggle round-trip
  test_epg.rs               current-program at boundaries, DST edge
  test_xtream_parse.rs      portal JSON → typed structs (golden-file)
  test_news_shortcut.rs     mock portal → find newest NOS Journaal
  test_args.rs              CLI parsing + cred parsing

fixtures/
  xtream_live.json
  xtream_vod.json
  xtream_series.json
  xtream_epg.json

vendor/
  libmpv/
    include/mpv/client.h            (vendored header — version stamp)
    include/mpv/render.h
    libmpv-2.dll                    (LGPL build, ~8 MB)
    libmpv.lib                      (import lib for MSVC linking)
    LICENSE.txt                     (mpv LGPL)
  fetch-mpv.ps1                     (idempotent download script — for CI / fresh clone)

assets/
  DejaVuSans.ttf                    (UI font, copied from current project)
  tvplayer.ico                      (Windows icon resource)

build-zip.ps1                       (cargo build --release + zip up artifacts)
verify.ps1                          (smoke test: launch app headlessly, assert exit codes)
README.md                           (user-facing — keymap, install, usage)
Cargo.toml
Cargo.lock
```

## Dependencies

| Crate | Use | Pin |
|-------|-----|-----|
| `egui` + `eframe` (wgpu backend) | UI + windowing | latest stable |
| `tokio` (rt-multi-thread, sync, macros) | async runtime | latest stable |
| `reqwest` (rustls-tls, json) | HTTP for portal | latest stable |
| `serde` + `serde_json` | config + portal JSON | latest stable |
| `directories` | %APPDATA% paths | latest |
| `tracing` + `tracing-subscriber` | structured logging | latest |
| `anyhow` + `thiserror` | error types | latest |
| `clap` (derive) | CLI args | latest |
| `chrono` | EPG time math + DST handling | latest |
| `parking_lot` | faster Mutex for the render triple-buffer | latest |

Dev-deps only:
| `wiremock` | mock HTTP server for portal tests |
| `insta` | snapshot tests for parsers |
| `proptest` | property tests for parsers |
| `tempfile` | favorites/config round-trip tests |

Total binary size estimate: `tvplayer.exe` ~6 MB + `libmpv-2.dll` ~8 MB + assets ~1 MB = ~15 MB portable zip.

## Data flow: zap (representative path)

```
User keypress (Down arrow)
   │
   ▼
egui detects KeyDown → app.rs handle_key()
   │ generates Cmd::Zap{idx: catalog_idx}
   ▼
player_cmd_tx.try_send(Cmd::Zap)             (non-blocking)
   │
   ▼ (tokio)
PlayerActor::handle_zap()
   ├─ stream_url = catalog.live[idx].xtream_url(portal_creds)
   ├─ mpv.command("loadfile", &[stream_url])    (async — mpv handles in own thread)
   ├─ epg_task = tokio::spawn(portal.fetch_epg(stream_id))
   └─ send Event::Zapping{name: catalog.live[idx].name}
       │
       ▼
UI receives Event::Zapping → show toast immediately
   │
   ▼ (mpv async)
"file-loaded" event from mpv → PlayerActor maps to Event::ChannelReady
   │
   ▼
UI updates toast: now showing channel name + (loading EPG…)
   │
   ▼ (epg_task completes)
Event::ProgrammeNow{title, end_time} → UI enriches toast with program info
   │
   ▼ (continuous)
render task: mpv_render → RgbaFrame triple buffer
   │
   ▼ (each egui frame)
UI samples latest RgbaFrame → wgpu texture → displays + overlays
```

## Error handling

- Library boundaries: `thiserror` enums (`PortalError`, `MpvError`, `StorageError`, `CatalogError`).
- App level: `anyhow::Result` with `.context("zap to channel {n}")`.
- mpv events: subscribe to `error`, `end-file`, `idle`. On stream-error → `Event::StreamEnded{reason}` → UI toast + auto-retry once after 1s, then surface for user.
- Never silent: every `Result::Err` path either logs via `tracing::warn!` (recoverable) or `tracing::error!` (fatal). No empty `let _ = …` discards.
- Panic policy: `panic = "abort"` in release for size + simplicity. `tracing::error!` panic hook prints last frames before abort.

## Testing

| Layer | Tool | Scope |
|-------|------|-------|
| Unit | `cargo test` | Pure functions: search ranking, EPG lookup, favorites toggle, M3U/Xtream parse, args parsing |
| Property | `proptest` | M3U + Xtream JSON parsers — fuzz unicode, quoting, edge sizes |
| Snapshot | `insta` | Catalog dumps, EPG formatted strings — review diffs on parser changes |
| Integration | `wiremock` + tokio | Portal traits against simulated Xtream server with golden fixtures |
| Smoke / acceptance | `verify.ps1` | Launches `tvplayer.exe --selftest`, exits 0 on success. Validates: mpv loads, dummy stream plays for 3s, frame counter advances |
| Manual | `docs/VALIDATION.md` | Checklist run by human pre-release: zap 5 channels, NOS shortcut, favorite, ondertitels, fullscreen, F-search → series → episode |

### "Validated perfect" definition

Before claiming v1.0 ready:
- All unit + integration + property tests green (`cargo test`).
- `cargo clippy -- -D warnings` clean.
- `verify.ps1 --smoke` exits 0.
- `verify.ps1 --acceptance` runs through the manual checklist (script prompts human to press keys, asserts overlay states from log).
- 30 minutes continuous playback shows no AV drift (mpv `audio-pts` vs `video-pts` < 50 ms throughout — read via `mpv_get_property`).
- Zip extracted on a clean Windows VM: double-click `run.bat`, plays within 5 seconds.

## Build & distribution

```powershell
# Developer build:
cargo build --release       # → target\release\tvplayer.exe

# Package portable zip:
.\build-zip.ps1             # → dist\tvplayer-vX.Y.Z.zip with:
                            #     tvplayer.exe
                            #     libmpv-2.dll
                            #     assets\DejaVuSans.ttf
                            #     run.bat (sample with --xtream placeholder)
                            #     README.txt

# Bootstrap fresh clone:
.\vendor\fetch-mpv.ps1      # downloads + verifies libmpv-2.dll + headers
cargo build --release
```

No MSYS2. No MinGW. Stable Rust + MSVC toolchain. CI on GitHub Actions: `cargo test` on each push, `build-zip.ps1` + `gh release create` on tag.

## Performance budget

| Metric | Budget | How measured |
|--------|--------|--------------|
| Cold start → window visible | < 300 ms | `tracing` span from `main` to first egui frame |
| Zap-perceived (toast visible) | < 30 ms | input event → next frame's toast draw |
| Zap-real (first video frame) | < 500 ms typical for portal | mpv `file-loaded` event timestamp |
| Frame budget at 60 fps | 16.6 ms | egui repaint metric |
| RSS during 1080p playback | < 150 MB | Windows Task Manager |
| CPU during 1080p HEVC | < 5% with HW-decode | mpv stats overlay |
| Total install size | < 20 MB unzipped | `dir` after extract |

Targets are budgets, not promises. Spec passes only if v1.0 measured numbers are within budget.

## Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Hand-written mpv FFI drifts from upstream | Vendor mpv header with version stamp. CI fails if local header diff against latest tagged mpv release exceeds whitelist. |
| Hardware decode failure on some GPUs | Fallback chain: `hwdec=auto` → `hwdec=d3d11va` → `hwdec=no` (CPU). Configurable via Config. |
| Portal API changes break catalog parse | Golden-file tests + `insta` snapshots catch shape drift early. App degrades to toast "portal returned unexpected JSON" instead of crashing. |
| egui repaint cadence vs mpv frame production mismatch (60Hz UI but 24/25/50fps video → visible judder) | egui repaint-on-event mode; mpv-render signals new frame via a `wake_up` callback that calls `ctx.request_repaint()`. UI repaints only when video has a new frame OR overlay changes. |
| LGPL implications of bundling libmpv | Ship official LGPL build (no GPL-only codecs). Document in `README.md` that the DLL can be swapped. Source link to mpv project. Distribute mpv LICENSE.txt in the zip. |
| Anti-virus false positives on unsigned exe | Document workaround; code-signing certificate is a separate purchase decision when going commercial. |

## Out-of-scope deliberate omissions for v1

(Listed so we don't accumulate scope creep mid-execution.)

- No catch-up / timeshift.
- No watchdog auto-restart loop — mpv's own end-file→retry handles transient drops cleanly.
- No HLS prefetch ring buffer — mpv has internal demuxer prebuffer (`cache-secs=10` setting).
- No custom subtitle renderer — libass is invoked by mpv automatically.
- No multi-monitor handling beyond default Windows behavior.
- No theme/skin system — single visual style in v1.
- No CEC / remote control / IR / IPTV-set-top integration.
