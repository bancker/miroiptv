#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include "xtream.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Build the URL for channel index 0..2 using these precedence rules:
 *   1. if xtream portal configured -> xtream stream URL for that channel
 *   2. if argv[1] direct URL given -> that URL (same URL for all channels; restart on switch)
 *   3. otherwise NULL -> playback_open falls through to the (broken) NPO resolver
 * Returned string is malloc'd and caller frees, OR NULL if no source. */
static char *build_channel_url(int ch_idx, const xtream_t *portal, const char *direct_url) {
    if (portal && portal->host) return xtream_stream_url(portal, XTREAM_NPO_STREAM_IDS[ch_idx]);
    if (direct_url)             return strdup(direct_url);
    return NULL;
}

typedef struct {
    player_t     player;
    audio_out_t  audio;
    video_tex_t  tex;
    av_clock_t   clk;
    epg_t        epg;
    char        *url;
    const npo_channel_t *channel;
    /* stream_id: operational identity of the currently playing stream.
     *   0  = NPO mode. Restart URL derived from pb->channel via XTREAM_NPO_STREAM_IDS.
     *   >0 = Xtream portal mode (NPO or zap). Restart uses this id directly;
     *        switch comparisons distinguish this playback from pb->channel alone.
     * Without this field, pb->channel stays "sticky" after a wheel-zap so
     * 1/2/3, 'n', and stall-restart all treated the prior NPO channel as the
     * "current" one even while the user was watching TROS Radar. */
    int          stream_id;

    /* Timeshift state. timeshift_start != 0 means we're playing a
     * /timeshift/.../START/STREAM.ts URL; left/right arrow shift the start
     * by TIMESHIFT_STEP_SEC and reopen. stream_id is the archive id and
     * timeshift_dur_min is the URL's duration segment. */
    time_t       timeshift_start;
    int          timeshift_dur_min;
} playback_t;

/* epg_stream_id:
 *   0  -> derive from ch (existing NPO 1/2/3 behavior via XTREAM_NPO_STREAM_IDS)
 *   -1 -> skip EPG fetch (used when zapping to a non-NPO portal channel)
 *   >0 -> use this exact stream_id for the portal's get_short_epg call. */
static int playback_open(playback_t *pb, render_t *r, const npo_channel_t *ch,
                         const char *override_url, const xtream_t *portal,
                         int epg_stream_id) {
    memset(pb, 0, sizeof(*pb));
    pb->channel   = ch;
    /* Persist the stream identity (used later by stall-restart and the
     * switch comparisons). 0 means "NPO mode, derive from ch". */
    pb->stream_id = epg_stream_id > 0 ? epg_stream_id : 0;

    if (override_url) {
        pb->url = strdup(override_url);
    } else if (npo_resolve_stream(ch, &pb->url) != 0) {
        fprintf(stderr, "playback_open: resolve failed for %s (NPO API broken, see R1)\n",
                ch->display);
        return -1;
    }

    if (player_open(&pb->player, pb->url) != 0) { free(pb->url); pb->url = NULL; return -1; }

    /* EPG: prefer portal's get_short_epg when running in xtream mode (it works
     * today); fall back to NPO's start-api when no portal (currently broken,
     * R1). Either failure is non-fatal — overlay shows "(geen programma)". */
    int epg_rc = -1;
    if (epg_stream_id == -1) {
        /* caller said: skip EPG entirely. */
    } else if (portal && portal->host) {
        int use_id = epg_stream_id > 0
                   ? epg_stream_id
                   : XTREAM_NPO_STREAM_IDS[(int)(ch - &NPO_CHANNELS[0])];
        epg_rc = xtream_fetch_epg(portal, use_id, &pb->epg);
        if (epg_rc == 0) {
            fprintf(stderr, "EPG: loaded %zu entries from portal for %s\n",
                    pb->epg.count, ch->display);
        }
    } else {
        epg_rc = npo_fetch_epg(ch, &pb->epg);
    }
    if (epg_rc != 0 && epg_stream_id != -1) {
        fprintf(stderr, "warn: EPG fetch failed for %s\n", ch->display);
        memset(&pb->epg, 0, sizeof(pb->epg));
    }

    if (player_start(&pb->player) != 0) {
        npo_epg_free(&pb->epg); player_close(&pb->player);
        free(pb->url); pb->url = NULL; return -1;
    }
    if (audio_open(&pb->audio, &pb->player.audio_q, pb->player.audio_sample_rate_out) != 0) {
        npo_epg_free(&pb->epg); player_stop(&pb->player); player_close(&pb->player);
        free(pb->url); pb->url = NULL; return -1;
    }

    av_clock_init_from_audio(&pb->clk,
                             &pb->audio.samples_played,
                             &pb->audio.first_pts,
                             &pb->audio.has_first_pts,
                             pb->audio.sample_rate);
    video_tex_init(&pb->tex);
    /* NB: window title is set by the caller on the main thread. We used to
     * SDL_SetWindowTitle here, but when playback_open is called from the zap
     * worker that SDL call happens off-main, which SDL docs forbid and which
     * could crash on some platforms. (void)r; kept to retain the signature. */
    (void)r;
    return 0;
}

/* Small helper so callers don't repeat the "tv - <display>" format. Must be
 * called from the main thread. */
static void set_window_title(render_t *r, const char *display) {
    char title[256];
    snprintf(title, sizeof(title), "tv - %s", display ? display : "");
    SDL_SetWindowTitle(r->window, title);
}

static void playback_close(playback_t *pb) {
    if (pb->url) {
        player_stop(&pb->player);
        audio_close(&pb->audio);
        video_tex_destroy(&pb->tex);
        player_close(&pb->player);
        free(pb->url);
    }
    npo_epg_free(&pb->epg);
    memset(pb, 0, sizeof(*pb));
}

static const npo_channel_t *key_to_channel(SDL_Keycode k) {
    switch (k) {
        case SDLK_1: return &NPO_CHANNELS[0];
        case SDLK_2: return &NPO_CHANNELS[1];
        case SDLK_3: return &NPO_CHANNELS[2];
        default: return NULL;
    }
}

typedef struct {
    int    channel_idx;   /* 0..2, or -1 if nothing found */
    int    is_airing;     /* 1 if currently on air, 0 if already ended or future */
    int    is_past;       /* 1 if the winning entry already ended */
    time_t start;         /* start time (unix) of the winning entry */
    time_t end;           /* end time (unix) of the winning entry */
    long long seconds_ago;/* how long ago it started (positive = past) */
    long long seconds_til;/* how long until start (positive = future) */
} journaal_hit_t;

/* Find the LATEST NOS Journaal across NPO 1/2/3 — i.e. the most-recently-started
 * one. Preference:
 *   1. currently airing  (start <= now < end) — pick the latest such entry
 *   2. most recent past  (end <= now)         — pick the latest ended one
 *   3. soonest upcoming  (start > now)        — only as last resort
 *
 * Fetches EPG for each NPO channel via the portal (~3 HTTP calls, ~1s total). */
static journaal_hit_t find_latest_journaal(const xtream_t *portal) {
    journaal_hit_t hit = { .channel_idx = -1 };
    if (!portal || !portal->host) return hit;
    time_t now = time(NULL);

    /* Track three candidates separately so we can pick in preference order. */
    int    best_airing_ch = -1; time_t best_airing_start = 0; time_t best_airing_end = 0;
    int    best_past_ch   = -1; time_t best_past_start   = 0; time_t best_past_end   = 0;
    int    best_next_ch   = -1; time_t best_next_start   = 0; time_t best_next_end   = 0;

    /* Use the FULL EPG (get_simple_data_table) on the archive IDs. The
     * live IDs' get_short_epg only returns ~4 upcoming entries and never
     * contains past journaals — that's why the previous version always
     * reported "NOS Journaal not found in EPG". The archive stream has
     * the same programme guide for its channel but the portal exposes
     * 800+ entries spanning the full 2-day catch-up window. */
    for (int i = 0; i < 3; ++i) {
        epg_t e = {0};
        if (xtream_fetch_epg_full(portal, XTREAM_NPO_ARCHIVE_STREAM_IDS[i], &e) != 0) {
            fprintf(stderr, "[journaal] EPG fetch failed for archive id %d\n",
                    XTREAM_NPO_ARCHIVE_STREAM_IDS[i]);
            continue;
        }
        int journaals_seen = 0;
        for (size_t j = 0; j < e.count; ++j) {
            if (!e.entries[j].is_news) continue;
            journaals_seen++;
            time_t s = e.entries[j].start, en = e.entries[j].end;
            if (s <= now && now < en) {
                if (s > best_airing_start) {
                    best_airing_ch = i; best_airing_start = s; best_airing_end = en;
                }
            } else if (en <= now) {
                if (s > best_past_start) {
                    best_past_ch = i; best_past_start = s; best_past_end = en;
                }
            } else { /* s > now */
                if (best_next_ch < 0 || s < best_next_start) {
                    best_next_ch = i; best_next_start = s; best_next_end = en;
                }
            }
        }
        fprintf(stderr, "[journaal] scanned NPO %d archive: %zu entries, %d journaals\n",
                i + 1, e.count, journaals_seen);
        npo_epg_free(&e);
    }

    if (best_airing_ch >= 0) {
        hit.channel_idx = best_airing_ch;
        hit.is_airing   = 1;
        hit.start       = best_airing_start;
        hit.end         = best_airing_end;
        hit.seconds_ago = (long long)(now - best_airing_start);
    } else if (best_past_ch >= 0) {
        hit.channel_idx = best_past_ch;
        hit.is_past     = 1;
        hit.start       = best_past_start;
        hit.end         = best_past_end;
        hit.seconds_ago = (long long)(now - best_past_end);
    } else if (best_next_ch >= 0) {
        hit.channel_idx = best_next_ch;
        hit.start       = best_next_start;
        hit.end         = best_next_end;
        hit.seconds_til = (long long)(best_next_start - now);
    }
    return hit;
}

/* --- Background zap preparation, 3-phase, per-request allocation ---
 *
 * UX goal: channel name shows INSTANTLY on wheel commit, then programme
 * title is filled in as soon as the EPG comes back (~200-400ms), then the
 * stream swaps when libav finishes probing (~1-2s). Main never blocks.
 *
 * Critical design choice: each zap request gets its OWN heap-allocated
 * zap_prep_t. When the user cancels (fast wheel scrolling), we set
 * state=5 on the old prep, detach its thread, and null out our pointer —
 * the detached worker sees state=5, frees everything it holds (including
 * the struct itself because self_owned=1), and vanishes. Main immediately
 * allocates a fresh struct for the new request. Two workers running
 * simultaneously is fine because they touch disjoint structs.
 *
 * This fixes a fast-scroll crash where the old code reused a single
 * shared struct and the new request would stomp the old worker's state
 * mid-flight.
 *
 * State machine:
 *   0 running, EPG fetch in progress     (initial after pthread_create)
 *   2 EPG ready, stream open in progress (worker -> main signal)
 *   3 stream ready, pb valid             (worker -> main signal)
 *   4 failed (worker owns cleanup)
 *   5 cancelled by main (worker owns cleanup, frees struct too)
 *
 * Ownership:
 *   - main allocates zp, fills inputs, spawns worker, assigns active_prep.
 *   - Worker frees zp->url and owns lifetime of preload_epg + pb until
 *     state >= 2 or 3 (handoff) OR state == 4/5 (worker deallocates).
 *   - If self_owned, worker frees zp itself at exit. */
typedef struct {
    pthread_t             thread;
    volatile int          state;          /* see comment above */
    volatile int          self_owned;     /* 1 = worker frees zp on exit */
    xtream_t             *portal;
    render_t             *render;
    char                 *url;
    int                   epg_stream_id;
    const npo_channel_t  *channel;
    char                  label[192];
    int                   list_idx;

    epg_t                 preload_epg;
    int                   epg_toast_shown; /* main: "I already baked this EPG into the toast" */
    playback_t           *pb;
} zap_prep_t;

static void *zap_prep_worker(void *arg) {
    zap_prep_t *zp = arg;

    /* Phase 1: EPG fetch. Usually 200-400ms. */
    epg_t ep = {0};
    (void)xtream_fetch_epg(zp->portal, zp->epg_stream_id, &ep);
    if (zp->state == 5) {
        npo_epg_free(&ep);
        free(zp->url); zp->url = NULL;
        if (zp->self_owned) free(zp);
        return NULL;
    }
    zp->preload_epg = ep;
    zp->state = 2;

    /* Phase 2: open playback. epg_stream_id=-1 skips the internal EPG
     * fetch because we already have the EPG ready to splice in. */
    playback_t *pb = calloc(1, sizeof(*pb));
    if (!pb) goto fail;
    int rc = playback_open(pb, zp->render, zp->channel, zp->url, zp->portal, -1);
    free(zp->url); zp->url = NULL;
    if (rc != 0) { free(pb); goto fail; }

    /* Splice the pre-fetched EPG in — move, don't copy. */
    npo_epg_free(&pb->epg);
    pb->epg = zp->preload_epg;
    memset(&zp->preload_epg, 0, sizeof(zp->preload_epg));

    if (zp->state == 5) {
        playback_close(pb); free(pb);
        if (zp->self_owned) free(zp);
        return NULL;
    }
    zp->pb    = pb;
    zp->state = 3;
    /* Main will consume and free zp on state==3; we don't. */
    return NULL;

fail:
    npo_epg_free(&zp->preload_epg);
    zp->state = 4;
    if (zp->self_owned) free(zp);
    return NULL;
}

int main(int argc, char **argv) {
    /* Line-buffer stdout so diagnostic prints appear in real time even when
     * redirected to a file (default fully-buffered would only flush at exit,
     * hiding what happened right before a crash). */
    setvbuf(stdout, NULL, _IOLBF, 0);

    /* Silence libav's info-level chatter ("Opening '...ts'" on every segment).
     * We keep warnings/errors, which is where real diagnostics live. Our own
     * [decoder]/[stall] fprintf(stderr, ...) lines are untouched. */
    av_log_set_level(AV_LOG_WARNING);

    curl_global_init(CURL_GLOBAL_DEFAULT);

    /* Parse CLI: supports `--xtream user:pass@host[:port]` OR a bare URL as argv[1]. */
    xtream_t portal = {0};
    const char *direct_url = NULL;
    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--xtream") == 0 && i + 1 < argc) {
            if (xtream_parse(argv[++i], &portal) != 0) {
                fprintf(stderr, "bad --xtream spec (expected user:pass@host[:port])\n");
                return 2;
            }
        } else if (!direct_url) {
            direct_url = argv[i];
        }
    }
    if (portal.host) {
        printf("xtream portal: %s:%d user=%s\n", portal.host, portal.port, portal.user);
    } else if (direct_url) {
        printf("direct URL: %s\n", direct_url);
    }

    render_t r;
    if (render_init(&r, 960, 540, "tv - booting") != 0) return 1;

    /* IMPORTANT: playback_t is HEAP-allocated, not stack. The audio callback
     * userdata and av_clock point INTO this struct (at &pb->audio, etc.); if
     * we ever assign `pb = new_pb` by value, those pointers would dangle to
     * a scope-local new_pb. Heap + pointer-swap avoids the UAF. */
    char *initial_url = build_channel_url(0, &portal, direct_url);
    playback_t *pb = calloc(1, sizeof(*pb));
    if (!pb || playback_open(pb, &r, &NPO_CHANNELS[0], initial_url, &portal, 0) != 0) {
        fputs("initial playback_open failed\n"
              "  try: ./tv.exe --xtream user:pass@host:port\n"
              "  or:  ./tv.exe https://some/stream.m3u8\n", stderr);
        free(pb);
        free(initial_url);
        xtream_free(&portal);
        render_shutdown(&r);
        return 2;
    }
    set_window_title(&r, NPO_CHANNELS[0].display);
    free(initial_url);

    overlay_t ov;
    if (overlay_init(&ov, "assets/DejaVuSans.ttf") != 0) {
        playback_close(pb);
        free(pb);
        render_shutdown(&r);
        return 5;
    }
    int show_overlay   = 1;
    int running        = 1;
    int paused         = 0;
    int have_texture   = 0;
    video_frame_t *pending_vf = NULL;  /* held across iterations when not yet due */
    Uint32 last_frame_ts = SDL_GetTicks();

    /* Right-click drag state — since the window is borderless there's no title
     * bar to grab, so we implement window-move by: on right-button-down we
     * snapshot the global mouse and window positions; on right-button-up we
     * stop; every iteration while dragging we move the window by the current
     * global mouse delta relative to the snapshot. SDL_CaptureMouse lets the
     * drag continue even when the cursor leaves the window bounds. */
    int drag_active = 0;
    int drag_anchor_mx = 0, drag_anchor_my = 0;
    int drag_anchor_wx = 0, drag_anchor_wy = 0;

    /* Left double-click cycles the window through a set of zoom factors.
     * Base size is the window's initial dimensions (960x540). */
    static const float SIZE_FACTORS[] = { 1.0f, 1.5f, 2.0f, 3.0f, 4.0f };
    const int  SIZE_COUNT = (int)(sizeof(SIZE_FACTORS) / sizeof(SIZE_FACTORS[0]));
    int        size_idx   = 0;
    const int  base_w     = 960;
    const int  base_h     = 540;

    /* Help overlay state.
     *  help_visible   — toggled by '?' key
     *  hint_until_ms  — while SDL_GetTicks() < this, show a transient hint
     *                   at bottom-right suggesting "Press ? for help" */
    int    help_visible  = 0;
    Uint32 hint_until_ms = SDL_GetTicks() + 8000;  /* 8 seconds after start */

    /* Toast state — short message shown in the bottom-right corner for a few
     * seconds. Written by actions like 'n' (Journaal search) so the user gets
     * visible feedback regardless of stderr visibility. */
    char    toast_text[512] = {0};  /* room for channel + " | " + programme title */
    Uint32  toast_until_ms  = 0;

    /* Mouse-wheel zapping through ALL portal live channels. We fetch the full
     * catalog once, find our current stream_id in it, and the wheel moves the
     * index ±1 per notch. Actual switch is debounced — rapid scrolling only
     * triggers one playback_open after the wheel stops for WHEEL_DEBOUNCE_MS,
     * which matters because the portal caps concurrent connections at 1. */
    xtream_live_list_t live_list = {0};
    int    current_live_idx = -1;
    int    pending_wheel_delta = 0;
    Uint32 last_wheel_ts = 0;
    const Uint32 WHEEL_DEBOUNCE_MS = 350;

    /* Background zap prep: see zap_prep_t docs above. Pointer, not inline —
     * each wheel commit that cancels a prior prep leaves the old detached
     * worker to clean its OWN struct, and main immediately allocates a
     * fresh one for the next prep. Two workers can coexist safely because
     * their structs are disjoint. */
    zap_prep_t *zap_prep = NULL;

    /* Diagnostic: if TV_TEST_JOURNAAL env var is set, call the Journaal
     * search once at startup and log the result. Lets automated smoke
     * tests verify the lookup works without synthesizing key events. */
    if (portal.host && getenv("TV_TEST_JOURNAAL")) {
        journaal_hit_t h = find_latest_journaal(&portal);
        fprintf(stderr, "[test-journaal] ch=%d airing=%d past=%d future=%d start=%ld end=%ld\n",
                h.channel_idx, h.is_airing, h.is_past, (h.channel_idx >= 0 && !h.is_airing && !h.is_past) ? 1 : 0,
                (long)h.start, (long)h.end);
    }

    if (portal.host) {
        if (xtream_fetch_live_list(&portal, NULL, &live_list) == 0) {
            fprintf(stderr, "[zap] loaded %zu channels from portal\n", live_list.count);
            for (size_t i = 0; i < live_list.count; ++i) {
                if (live_list.entries[i].stream_id == XTREAM_NPO_STREAM_IDS[0]) {
                    current_live_idx = (int)i;
                    break;
                }
            }
        } else {
            fprintf(stderr, "[zap] failed to fetch live channel list — wheel disabled\n");
        }
    }

    /* Sync tolerances (seconds):
     *  DUE_WINDOW  — how "early" a frame can be to count as due-now (one vsync @ 60Hz ≈ 16ms)
     *  DROP_LATE   — how late a frame can be before we drop it (80ms ≈ 2 frames at 25fps) */
    const double DUE_WINDOW    = 0.010;
    const double DROP_LATE     = 0.080;
    /* Stall detection: if we've had a texture up and no new frame arrives for
     * STALL_MS, assume the decoder thread died (network drop, HLS sequence
     * reset, portal timeout) and restart the same channel. */
    const Uint32 STALL_MS      = 3000;

    while (running) {
        /* 1) Events first, every iteration — keeps UI responsive regardless of decode state. */
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) { running = 0; break; }
            if (ev.type == SDL_MOUSEBUTTONDOWN && ev.button.button == SDL_BUTTON_RIGHT) {
                SDL_GetGlobalMouseState(&drag_anchor_mx, &drag_anchor_my);
                SDL_GetWindowPosition(r.window, &drag_anchor_wx, &drag_anchor_wy);
                drag_active = 1;
                SDL_CaptureMouse(SDL_TRUE);
                continue;
            }
            if (ev.type == SDL_MOUSEBUTTONDOWN && ev.button.button == SDL_BUTTON_LEFT &&
                ev.button.clicks == 2 && !r.fullscreen) {
                size_idx = (size_idx + 1) % SIZE_COUNT;
                int new_w = (int)(base_w * SIZE_FACTORS[size_idx]);
                int new_h = (int)(base_h * SIZE_FACTORS[size_idx]);
                SDL_SetWindowSize(r.window, new_w, new_h);
                SDL_SetWindowPosition(r.window, SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED);
                fprintf(stderr, "[size] %.1fx (%dx%d)\n", SIZE_FACTORS[size_idx], new_w, new_h);
                continue;
            }
            if (ev.type == SDL_MOUSEBUTTONUP && ev.button.button == SDL_BUTTON_RIGHT) {
                drag_active = 0;
                SDL_CaptureMouse(SDL_FALSE);
                continue;
            }
            if (ev.type == SDL_MOUSEWHEEL && current_live_idx >= 0) {
                /* wheel up (positive y) = previous channel; down = next channel */
                pending_wheel_delta -= ev.wheel.y;
                last_wheel_ts = SDL_GetTicks();
                continue;
            }
            if (ev.type != SDL_KEYDOWN) continue;
            SDL_Keycode k = ev.key.keysym.sym;
            if (k == SDLK_q || k == SDLK_ESCAPE) { running = 0; break; }
            else if (k == SDLK_f) render_toggle_fullscreen(&r);
            else if (k == SDLK_SPACE) {
                paused = !paused;
                SDL_PauseAudioDevice(pb->audio.device, paused);
            }
            else if (k == SDLK_e) show_overlay = !show_overlay;
            else if (k == SDLK_t) {
                static int always_on_top = 0;
                always_on_top = !always_on_top;
                SDL_SetWindowAlwaysOnTop(r.window, always_on_top ? SDL_TRUE : SDL_FALSE);
                snprintf(toast_text, sizeof(toast_text),
                         "Always on top: %s", always_on_top ? "ON" : "OFF");
                toast_until_ms = SDL_GetTicks() + 2500;
            }
            else if ((k == SDLK_UP || k == SDLK_DOWN) && current_live_idx >= 0) {
                /* Same pipeline as mouse wheel: accumulate into pending_wheel_delta
                 * and let the WHEEL_DEBOUNCE_MS commit it through the async zap
                 * worker. Up arrow = previous channel (lower index), down = next. */
                pending_wheel_delta += (k == SDLK_UP) ? -1 : +1;
                last_wheel_ts = SDL_GetTicks();
            }
            else if ((k == SDLK_LEFT || k == SDLK_RIGHT) && pb->timeshift_start != 0) {
                int delta = (k == SDLK_RIGHT) ? +30 : -30;
                time_t new_start = pb->timeshift_start + delta;
                char *u = xtream_timeshift_url(&portal, pb->stream_id, new_start,
                                               pb->timeshift_dur_min);
                playback_t *new_pb = calloc(1, sizeof(*new_pb));
                if (u && new_pb && playback_open(new_pb, &r, pb->channel, u,
                                                 &portal, pb->stream_id) == 0) {
                    new_pb->timeshift_start   = new_start;
                    new_pb->timeshift_dur_min = pb->timeshift_dur_min;
                    playback_close(pb); free(pb); pb = new_pb;
                    set_window_title(&r, pb->channel->display);
                    paused = 0; have_texture = 0;
                    if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                    overlay_mark_dirty(&ov);
                    struct tm lt = *localtime(&new_start);
                    snprintf(toast_text, sizeof(toast_text),
                             "%s 30s -> replay starts at %02d:%02d",
                             delta > 0 ? "Skipped" : "Back", lt.tm_hour, lt.tm_min);
                } else {
                    free(new_pb);
                    snprintf(toast_text, sizeof(toast_text),
                             "%s 30s failed", delta > 0 ? "Skip" : "Back");
                }
                free(u);
                toast_until_ms = SDL_GetTicks() + 4000;
            }
            else if (k == SDLK_QUESTION ||
                     (k == SDLK_SLASH && (ev.key.keysym.mod & KMOD_SHIFT))) {
                help_visible = !help_visible;
                hint_until_ms = 0;  /* startup hint has served its purpose */
            }
            else if (k == SDLK_n) {
                journaal_hit_t hit = find_latest_journaal(&portal);
                if (hit.channel_idx < 0) {
                    snprintf(toast_text, sizeof(toast_text),
                             "NOS Journaal not found in EPG");
                    fprintf(stderr, "[journaal] no NOS Journaal in NPO 1/2/3 EPG\n");
                } else {
                    struct tm lt = *localtime(&hit.start);
                    const char *chname = NPO_CHANNELS[hit.channel_idx].display;

                    /* Decide URL + whether we'll need to open a different playback:
                     *   - airing now  -> live NPO stream (we see the in-progress live feed)
                     *   - past        -> timeshift URL on TERUGKIJKEN stream (replay from start)
                     *   - future      -> switch to live NPO (user sees upcoming start) */
                    char *target_url = NULL;
                    int   target_stream_id = 0;  /* for stall-restart tracking */

                    if (hit.is_airing) {
                        long long mins_in = hit.seconds_ago / 60;
                        snprintf(toast_text, sizeof(toast_text),
                                 "NOS Journaal airing on %s (started %02d:%02d, %lld min ago)",
                                 chname, lt.tm_hour, lt.tm_min, mins_in);
                        target_url = build_channel_url(hit.channel_idx, &portal, direct_url);
                        target_stream_id = 0;  /* treat as standard live NPO channel */
                    } else if (hit.is_past) {
                        long long ago_m = hit.seconds_ago / 60;
                        int dur_min = (int)((hit.end - hit.start) / 60);
                        snprintf(toast_text, sizeof(toast_text),
                                 "Replaying NOS Journaal from %02d:%02d on %s (ended %lld min ago)",
                                 lt.tm_hour, lt.tm_min, chname, ago_m);
                        target_stream_id = XTREAM_NPO_ARCHIVE_STREAM_IDS[hit.channel_idx];
                        target_url = xtream_timeshift_url(&portal, target_stream_id,
                                                          hit.start, dur_min);
                    } else {
                        long long til_m = hit.seconds_til / 60;
                        snprintf(toast_text, sizeof(toast_text),
                                 "Next NOS Journaal on %s at %02d:%02d (in %lld min)",
                                 chname, lt.tm_hour, lt.tm_min, til_m);
                        target_url = build_channel_url(hit.channel_idx, &portal, direct_url);
                        target_stream_id = 0;
                    }
                    fprintf(stderr, "[journaal] %s\n", toast_text);

                    if (target_url) {
                        const npo_channel_t *target = &NPO_CHANNELS[hit.channel_idx];
                        playback_t *new_pb = calloc(1, sizeof(*new_pb));
                        /* For timeshift we use epg_stream_id=target_stream_id so the
                         * archive EPG loads; for live, 0 keeps NPO-mode defaults. */
                        int epg_id = hit.is_past ? target_stream_id : 0;
                        if (new_pb && playback_open(new_pb, &r, target, target_url,
                                                    &portal, epg_id) == 0) {
                            /* Stamp timeshift state so LEFT/RIGHT arrows know
                             * the current start time and can shift it. */
                            if (hit.is_past) {
                                new_pb->timeshift_start    = hit.start;
                                new_pb->timeshift_dur_min  = (int)((hit.end - hit.start) / 60);
                            }
                            playback_close(pb); free(pb); pb = new_pb;
                            set_window_title(&r, target->display);
                            paused = 0; have_texture = 0;
                            if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                            overlay_mark_dirty(&ov);
                        } else {
                            free(new_pb);
                            snprintf(toast_text, sizeof(toast_text),
                                     "Journaal %s to %s failed",
                                     hit.is_past ? "replay" : "switch", target->display);
                        }
                        free(target_url);
                    }
                }
                toast_until_ms = SDL_GetTicks() + 6000;  /* 6 sec visible */
            }
            else {
                const npo_channel_t *nch = key_to_channel(k);
                /* Same "force-on-zap" rule as the 'n' handler: if the user
                 * pressed 1/2/3 while zapped, pb->channel is a stale NPO stamp;
                 * use stream_id to detect that we're actually elsewhere. */
                if (nch && (nch != pb->channel || pb->stream_id != 0)) {
                    int ch_idx = (int)(nch - &NPO_CHANNELS[0]);
                    fprintf(stderr, "[switch] %s -> %s (idx=%d)\n",
                            pb->channel->display, nch->display, ch_idx);
                    char *switch_url = build_channel_url(ch_idx, &portal, direct_url);
                    /* Heap-allocate new_pb so its address is stable beyond this scope;
                     * pointer swap avoids the dangling-pointer bug that bit us on the
                     * first channel switch. */
                    playback_t *new_pb = calloc(1, sizeof(*new_pb));
                    if (new_pb && playback_open(new_pb, &r, nch, switch_url, &portal, 0) == 0) {
                        fprintf(stderr, "[switch] new playback opened, tearing down old\n");
                        playback_close(pb);
                        free(pb);
                        pb = new_pb;
                        set_window_title(&r, nch->display);
                        paused       = 0;
                        have_texture = 0;
                        if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                        overlay_mark_dirty(&ov);
                        fprintf(stderr, "[switch] swap complete, now on %s\n", pb->channel->display);
                    } else {
                        free(new_pb);
                        fprintf(stderr, "channel switch to %s failed - staying on %s\n",
                                nch->display, pb->channel->display);
                    }
                    free(switch_url);
                }
            }
        }

        /* 1b) Apply window drag if right-button is held. Skip when fullscreen —
         * moving a fullscreen window is meaningless and SDL_SetWindowPosition
         * on a fullscreen surface is undefined on some platforms. */
        if (drag_active && !r.fullscreen) {
            int mx, my;
            SDL_GetGlobalMouseState(&mx, &my);
            SDL_SetWindowPosition(r.window,
                drag_anchor_wx + (mx - drag_anchor_mx),
                drag_anchor_wy + (my - drag_anchor_my));
        }

        /* 1c) Debounced wheel-zap: flash channel name INSTANTLY, then spawn
         * a worker with its OWN heap-allocated zap_prep_t. Rapid scrolling
         * just detaches the old prep (which self-destructs) and allocates a
         * new one — no shared state to race on. */
        if (pending_wheel_delta != 0 && current_live_idx >= 0 &&
            SDL_GetTicks() - last_wheel_ts > WHEEL_DEBOUNCE_MS) {
            int new_idx = current_live_idx + pending_wheel_delta;
            if (new_idx < 0) new_idx = 0;
            if (new_idx >= (int)live_list.count) new_idx = (int)live_list.count - 1;
            pending_wheel_delta = 0;

            if (new_idx != current_live_idx) {
                xtream_live_entry_t *e = &live_list.entries[new_idx];
                fprintf(stderr, "[zap] [%d/%zu] %s (id=%d)\n",
                        new_idx + 1, live_list.count, e->name, e->stream_id);

                /* Abandon any prior prep — per-state handling avoids leaking
                 * a finished worker's result.
                 *   running(0) / EPG-ready(2) -> worker still alive; detach,
                 *     set self_owned+state=5, worker self-destructs.
                 *   stream-ready(3)           -> worker already exited; we
                 *     must join and destroy the pb we'll never show.
                 *   failed(4)                 -> worker already exited; join
                 *     and free the struct.
                 * Before this branch was a blanket detach, which leaked the
                 * prepared playback_t under fast-scroll. */
                if (zap_prep) {
                    int st = zap_prep->state;
                    if (st == 3) {
                        pthread_join(zap_prep->thread, NULL);
                        if (zap_prep->pb) { playback_close(zap_prep->pb); free(zap_prep->pb); }
                        free(zap_prep);
                    } else if (st == 4) {
                        pthread_join(zap_prep->thread, NULL);
                        free(zap_prep);
                    } else {
                        zap_prep->self_owned = 1;
                        zap_prep->state      = 5;
                        pthread_detach(zap_prep->thread);
                    }
                    zap_prep = NULL;
                }

                /* Fresh per-request struct. */
                zap_prep_t *zp = calloc(1, sizeof(*zp));
                if (zp) {
                    zp->portal        = &portal;
                    zp->render        = &r;
                    zp->url           = xtream_stream_url(&portal, e->stream_id);
                    zp->epg_stream_id = e->stream_id;
                    zp->channel       = pb->channel;
                    zp->list_idx      = new_idx;
                    snprintf(zp->label, sizeof(zp->label), "%s", e->name);

                    if (zp->url && pthread_create(&zp->thread, NULL,
                                                  zap_prep_worker, zp) == 0) {
                        zap_prep = zp;  /* track it */
                    } else {
                        free(zp->url); free(zp);
                    }
                }

                /* Phase-0 toast: channel name INSTANTLY — before the worker
                 * has even done anything. */
                snprintf(toast_text, sizeof(toast_text), "%s", e->name);
                toast_until_ms = SDL_GetTicks() + 8000;
            }
        }

        /* 1d) Phase 2: EPG arrived — enrich the toast. */
        if (zap_prep && zap_prep->state == 2 && !zap_prep->epg_toast_shown) {
            time_t tnow = time(NULL);
            const char *now_title = NULL;
            for (size_t i = 0; i < zap_prep->preload_epg.count; ++i) {
                if (zap_prep->preload_epg.entries[i].start <= tnow
                    && tnow < zap_prep->preload_epg.entries[i].end) {
                    now_title = zap_prep->preload_epg.entries[i].title;
                    break;
                }
            }
            if (now_title && *now_title) {
                snprintf(toast_text, sizeof(toast_text), "%s  |  %s",
                         zap_prep->label, now_title);
                toast_until_ms = SDL_GetTicks() + 8000;
            }
            zap_prep->epg_toast_shown = 1;
        }

        /* 1e) Phase 3: stream opened — swap playback. */
        if (zap_prep && zap_prep->state == 3) {
            if (!zap_prep->epg_toast_shown && zap_prep->pb) {
                time_t tnow = time(NULL);
                const char *nt = NULL;
                for (size_t i = 0; i < zap_prep->pb->epg.count; ++i) {
                    if (zap_prep->pb->epg.entries[i].start <= tnow
                        && tnow < zap_prep->pb->epg.entries[i].end) {
                        nt = zap_prep->pb->epg.entries[i].title; break;
                    }
                }
                if (nt && *nt)
                    snprintf(toast_text, sizeof(toast_text), "%s  |  %s", zap_prep->label, nt);
                zap_prep->epg_toast_shown = 1;
            }

            playback_t *new_pb = zap_prep->pb;
            pthread_join(zap_prep->thread, NULL);
            playback_close(pb); free(pb); pb = new_pb;
            current_live_idx = zap_prep->list_idx;
            paused = 0; have_texture = 0;
            if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
            overlay_mark_dirty(&ov);

            set_window_title(&r, zap_prep->label);
            toast_until_ms = SDL_GetTicks() + 3000;

            free(zap_prep); zap_prep = NULL;
        } else if (zap_prep && zap_prep->state == 4) {
            pthread_join(zap_prep->thread, NULL);
            snprintf(toast_text, sizeof(toast_text), "Tuning to %s failed", zap_prep->label);
            toast_until_ms = SDL_GetTicks() + 3000;
            free(zap_prep); zap_prep = NULL;
        }

        /* 2) Try to grab a new frame if we don't have one pending. */
        if (!pending_vf) pending_vf = queue_try_pop(&pb->player.video_q);
        if (pending_vf) last_frame_ts = SDL_GetTicks();

        /* 3) If we have a pending frame, decide what to do with it.
         * Skip the decide-when block until audio has actually started
         * (first chunk consumed by SDL) — without that baseline av_clock
         * returns 0 and every frame with a real pts looks "late". */
        if (pending_vf && av_clock_ready(&pb->clk)) {
            double now  = av_clock_now(&pb->clk);
            double diff = pending_vf->pts - now;

            if (diff < DUE_WINDOW) {
                /* Due now (or past due). Either upload-for-display, or drop if way late. */
                if (-diff > DROP_LATE) {
                    video_frame_free(pending_vf);
                } else {
                    video_tex_upload(&pb->tex, r.renderer, pending_vf);
                    video_frame_free(pending_vf);
                    have_texture = 1;
                }
                pending_vf = NULL;
            }
            /* else: hold for next iteration — its time hasn't come yet. */
        }

        /* 3b) Stall detection + auto-restart. Only kicks in after we've seen
         * at least one frame (have_texture) so the initial connect phase
         * doesn't trigger it. Branches on pb->stream_id: if >0 we were zapped
         * to a portal channel and must reopen THAT stream, not fall back to
         * an NPO URL derived from the (stale) pb->channel pointer. */
        if (have_texture && SDL_GetTicks() - last_frame_ts > STALL_MS) {
            char *restart_url;
            int   restart_epg_id;
            if (pb->timeshift_start != 0 && portal.host) {
                /* Timeshift replay: resume at the same start/duration so we
                 * don't randomly switch to the archive stream's live feed. */
                restart_url = xtream_timeshift_url(&portal, pb->stream_id,
                                                   pb->timeshift_start,
                                                   pb->timeshift_dur_min);
                restart_epg_id = pb->stream_id;
                fprintf(stderr, "[stall] timeshift id=%d start=%ld restarting\n",
                        pb->stream_id, (long)pb->timeshift_start);
            } else if (pb->stream_id != 0 && portal.host) {
                restart_url    = xtream_stream_url(&portal, pb->stream_id);
                restart_epg_id = pb->stream_id;
                fprintf(stderr, "[stall] no frames for %ums on portal stream_id=%d - restarting\n",
                        STALL_MS, pb->stream_id);
            } else {
                int ch_idx = (int)(pb->channel - &NPO_CHANNELS[0]);
                restart_url    = build_channel_url(ch_idx, &portal, direct_url);
                restart_epg_id = 0;
                fprintf(stderr, "[stall] no frames for %ums on %s - restarting\n",
                        STALL_MS, pb->channel->display);
            }
            playback_t *new_pb = calloc(1, sizeof(*new_pb));
            if (new_pb && playback_open(new_pb, &r, pb->channel, restart_url, &portal, restart_epg_id) == 0) {
                /* Preserve timeshift state across restart. */
                if (pb->timeshift_start != 0) {
                    new_pb->timeshift_start   = pb->timeshift_start;
                    new_pb->timeshift_dur_min = pb->timeshift_dur_min;
                }
                playback_close(pb);
                free(pb);
                pb = new_pb;
                paused       = 0;
                have_texture = 0;
                if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                overlay_mark_dirty(&ov);
                fprintf(stderr, "[stall] restart OK\n");
            } else {
                free(new_pb);
                fprintf(stderr, "[stall] restart failed - will retry in %ums\n", STALL_MS);
            }
            free(restart_url);
            last_frame_ts = SDL_GetTicks();  /* reset either way */
        }

        /* 4) Always render. VSYNC in SDL_RenderPresent paces the loop to ~60Hz,
         *    so holding the last frame costs nothing and keeps the window alive
         *    even when new frames haven't arrived. */
        SDL_RenderClear(r.renderer);
        int ww = 0, wh = 0;
        SDL_GetRendererOutputSize(r.renderer, &ww, &wh);
        if (have_texture) {
            SDL_RenderCopy(r.renderer, pb->tex.texture, NULL, NULL);
            if (show_overlay)
                overlay_render(&ov, r.renderer, &pb->epg, ww, wh);
        }
        /* Help + toast + startup hint on top of everything so they stay legible.
         * Priority: help > toast > startup hint. */
        if (help_visible) {
            overlay_render_help(&ov, r.renderer, ww, wh);
        } else if (SDL_GetTicks() < toast_until_ms && toast_text[0]) {
            overlay_render_hint(&ov, r.renderer, toast_text, ww, wh);
        } else if (SDL_GetTicks() < hint_until_ms) {
            overlay_render_hint(&ov, r.renderer, "Press ? for help", ww, wh);
        }
        SDL_RenderPresent(r.renderer);  /* blocks ~16ms @ 60Hz with VSYNC */
    }

    /* On quit, hand the in-flight prep to the worker to clean itself up.
     * Detach rather than join — worker could be mid-network and joining
     * would stall the exit by up to ~10s. */
    if (zap_prep) {
        zap_prep->self_owned = 1;
        zap_prep->state      = 5;
        pthread_detach(zap_prep->thread);
        zap_prep = NULL;
    }

    if (pending_vf) video_frame_free(pending_vf);
    playback_close(pb);
    free(pb);
    overlay_shutdown(&ov);
    render_shutdown(&r);
    xtream_live_list_free(&live_list);
    xtream_free(&portal);
    curl_global_cleanup();
    return 0;
}
