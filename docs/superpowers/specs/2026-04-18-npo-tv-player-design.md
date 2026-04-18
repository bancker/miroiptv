# Design: NPO TV Player (C POC)

**Date:** 2026-04-18
**Status:** Approved (brainstorming), pending user review of spec
**Target platform:** Windows 11, MSYS2 / MinGW64 toolchain

## 1. Context and goals

### Problem
We want a small C program that plays the Dutch public broadcaster NPO 1 live stream,
with a focus on the EPG (programme guide) and in particular the news broadcasts
(NOS Journaal). Secondary channels NPO 2 and NPO 3 should also be playable.

### Goals
1. Decode and display NPO 1 live video + audio using the FFmpeg libraries (`libav*`)
   and SDL2, all in C.
2. Fetch EPG data from NPO's public API. Render a toggleable on-screen overlay that
   highlights NOS Journaal entries.
3. Allow switching between NPO 1 / 2 / 3 via keyboard (`1` / `2` / `3`).
4. Compile cleanly on a fresh MSYS2 MinGW64 install with a single `make` command.
5. Be small enough to remain a POC (~500–800 LoC C across ~6 modules).

### Non-goals
- No IPTV portal access (deliberately rejected during brainstorming — NPO is free).
- No DVR / rewind / recording / PVR features.
- No hardware-accelerated decode.
- No subtitles / teletext / second-screen / cast.
- No channel coverage beyond NPO 1 / 2 / 3.

## 2. Architecture

### 2.1 Process and threads

The player is a single process with three active threads:

| Thread | Owns | Responsibility |
|---|---|---|
| Main | SDL | Event loop, video rendering, EPG-overlay rendering, channel switching |
| Decoder | libav | Opens HLS, demuxes, decodes video + audio, pushes raw frames to queues |
| SDL audio callback | (SDL-internal) | Pulls from audio queue, writes PCM to audio device |

Communication between threads happens through bounded single-producer/single-consumer
queues protected by a mutex and a condition variable. Bounded so that a fast decoder
cannot OOM the process — backpressure is built-in.

### 2.2 Data flow

**Video:**
HLS URL → `libavformat` demux → `libavcodec` decode (H.264) → `AVFrame` (YUV420)
→ (optional `libswscale` if pixel format differs) → SDL `YV12` texture
→ `SDL_RenderCopy` → screen.

**Audio:**
HLS URL → `libavformat` demux → `libavcodec` decode (AAC-LC) → `libswresample`
to `s16le stereo 48 kHz` → bounded ring buffer → SDL audio callback → device.

**EPG:**
`libcurl` GET on `https://start-api.npo.nl/v2/schedule/channel/<code>?...`
→ JSON blob → `cJSON` parse → array of `epg_entry` → overlay renderer flags
entries whose `title` begins with `"NOS Journaal"` for highlighting.

### 2.3 A/V sync

- **Audio is the master clock.** We track how many samples have been submitted to
  the SDL audio device; that position divided by the sample rate is "now".
- For each ready video frame, main thread compares `video_pts` to `audio_now`:
  - `video_pts - audio_now > 0` → sleep until due (up to a cap, to stay responsive
    to events).
  - `audio_now - video_pts > drop_threshold` → drop the frame to catch up.
  - otherwise → present immediately.
- Drop threshold ≈ 40 ms (roughly one frame at 25 fps). This is a deliberately
  simplified version of `ffplay.c`'s synchronization; adequate for stable live
  content, not bulletproof for stream stalls (see risk R2).

### 2.4 Thread-safe bounded queue

- Circular buffer with capacity (video: 16 frames; audio: 32 frames).
- Single mutex + one condition variable for "not full" / "not empty".
- Enqueue blocks when full; dequeue blocks when empty.
- Poison sentinel for shutdown: pushing `NULL` signals "no more frames".

## 3. Modules

Six C source files. Everything else is headers.

| File | Responsibility |
|---|---|
| `src/main.c` | CLI-arg parsing, SDL/player init, event loop, channel switching, cleanup |
| `src/npo.c` + `npo.h` | `libcurl` + `cJSON`. EPG fetch; stream-URL resolve per channel |
| `src/player.c` + `player.h` | Wraps libav: open HLS, demux, decode video and audio, push to queues |
| `src/render.c` + `render.h` | SDL2 video texture upload + present; SDL audio callback; EPG overlay via SDL_ttf |
| `src/sync.c` + `sync.h` | Audio-master clock; pacing helpers for video presentation |
| `src/common.h` | `frame_t`, `audio_chunk_t`, `queue_t` with its API |

## 4. Dependencies

Installed via a single `pacman -S` in MSYS2 MinGW64:

```
mingw-w64-x86_64-gcc
mingw-w64-x86_64-ffmpeg      # libavformat, libavcodec, libavutil, libswscale, libswresample
mingw-w64-x86_64-SDL2
mingw-w64-x86_64-SDL2_ttf
mingw-w64-x86_64-curl
mingw-w64-x86_64-cjson
mingw-w64-x86_64-pkgconf
make
```

## 5. Build

Plain Makefile driven by `pkg-config`:

```
CFLAGS  = -Wall -Wextra -std=c11 $(shell pkg-config --cflags \
            libavformat libavcodec libavutil libswscale libswresample \
            sdl2 SDL2_ttf libcurl libcjson)
LDLIBS  = $(shell pkg-config --libs \
            libavformat libavcodec libavutil libswscale libswresample \
            sdl2 SDL2_ttf libcurl libcjson) -pthread
```

- `make` → `build/tv.exe`.
- `make DEBUG=1` → adds `-g -O0 -fsanitize=address -fno-omit-frame-pointer`.
- `make clean`, `make run`.

## 6. Project layout

```
tv/
├── Makefile
├── README.md
├── docs/superpowers/specs/2026-04-18-npo-tv-player-design.md
├── src/
│   ├── main.c
│   ├── npo.c         npo.h
│   ├── player.c      player.h
│   ├── render.c      render.h
│   ├── sync.c        sync.h
│   └── common.h
├── tests/
│   └── test_npo_parse.c    (EPG JSON fixture test)
├── assets/
│   └── DejaVuSans.ttf      (bundled font, free licence)
└── build/                  (gitignored)
    └── tv.exe
```

## 7. Runtime interface

### CLI flags
- `--channel NED1|NED2|NED3` — start on a specific channel (default `NED1`).
- `--stream-url <url>` — bypass NPO's URL-resolve and open this HLS directly. Break-glass.
- `--font <path>` — override the SDL_ttf font (default `assets/DejaVuSans.ttf`).
- `--help`.

### Keyboard
| Key | Action |
|---|---|
| `1` / `2` / `3` | Switch to NED1 / NED2 / NED3 |
| `e` | Toggle EPG overlay |
| `f` | Toggle fullscreen |
| `space` | Pause / resume |
| `q` / `Esc` | Quit |

## 8. Channels (hardcoded)

```c
static const Channel channels[] = {
    { "NPO 1", "NED1", "LI_NL1_4188102" },
    { "NPO 2", "NED2", "LI_NL1_4188103" },
    { "NPO 3", "NED3", "LI_NL1_4188105" },
};
```

The third field is NPO's productId, used by the player-token endpoint to resolve
to a signed HLS manifest URL.

## 9. Risks

### R1 — NPO stream-URL API is undocumented
The `player-token` flow used by the NPO web player is not an officially public API.
If NPO changes it, our URL resolver breaks.

**Mitigation:** `--stream-url <url>` flag as break-glass; explicit per-step logging
so failures point at the exact step (token request, URL request, libav open).

### R2 — A/V sync edge cases
Our "audio is master" strategy is robust for stable live content but can drift
during long stream stalls. POC-acceptable: if it happens, quit + relaunch.

### R3 — HLS discontinuities
Ad breaks or codec switches occasionally cause libavformat to stall briefly.
POC-acceptable.

### R4 — Font licensing
SDL_ttf requires a `.ttf`. We bundle `DejaVuSans.ttf` (derivative of Bitstream Vera,
free redistribution).

### R5 — Codec coverage
NPO 1 live is currently H.264 + AAC-LC, both in MSYS2's FFmpeg build. If NPO
switches to HEVC or AC-3, the fix is `pacman -Syu` — no code change needed.

## 10. Testing

Deliberately minimal for POC scope.

**Manual (primary):**
- Run `./build/tv.exe`: NPO 1 plays with audio within a few seconds of launch.
- Press `e`: overlay appears, shows current + next 3 entries, NOS Journaal entries in red.
- Press `1` / `2` / `3` rapidly: channels switch without crash; no lingering audio from old channel.
- Press `f`, `space`, `q`: fullscreen toggle, pause, quit all behave sensibly.

**Automated (single test):**
- `tests/test_npo_parse.c` feeds a captured EPG JSON response into the parser
  and asserts: total entry count, that NOS Journaal entries are flagged,
  that start/end times parse to expected `time_t` values.
- Reason for one test only: streaming/SDL/FFmpeg code is 80 % integration with
  external systems; the unit test covers the one place where silent data corruption
  would actually be plausible (JSON parsing).

## 11. Out of scope (explicit)

- Any IPTV portal access (`m.hnlol.com` or similar) — rejected in brainstorming.
- DVR / timeshift / recording.
- Subtitles or teletext.
- Multiple simultaneous streams.
- Installer or packaging — `tv.exe` is run from its build folder.
- Localization of the overlay text beyond Dutch.

## 12. Open questions for implementation

These are flagged to surface during the implementation-plan phase rather than resolved now:

- **Exact NPO token-endpoint URL and response shape** need to be verified at implementation
  time. If the current endpoint is dead or has moved, implementation switches to the
  `--stream-url` fallback and documents the issue.
- **Font path on Windows**: do we ship `DejaVuSans.ttf` in `assets/` or rely on a
  system font? Default plan: ship it, to keep the POC self-contained.
- **AddressSanitizer on MinGW**: occasionally finicky on Windows. If it misbehaves,
  debug builds drop ASan and keep `-g -O0`.
