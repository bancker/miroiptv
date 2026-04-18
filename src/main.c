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
} playback_t;

static int playback_open(playback_t *pb, render_t *r, const npo_channel_t *ch,
                         const char *override_url, const xtream_t *portal) {
    memset(pb, 0, sizeof(*pb));
    pb->channel = ch;

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
    if (portal && portal->host) {
        int ch_idx = (int)(ch - &NPO_CHANNELS[0]);
        epg_rc = xtream_fetch_epg(portal, XTREAM_NPO_STREAM_IDS[ch_idx], &pb->epg);
        if (epg_rc == 0) {
            fprintf(stderr, "EPG: loaded %zu entries from portal for %s\n",
                    pb->epg.count, ch->display);
        }
    } else {
        epg_rc = npo_fetch_epg(ch, &pb->epg);
    }
    if (epg_rc != 0) {
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

    av_clock_init(&pb->clk, &pb->audio.samples_played, pb->audio.sample_rate);
    video_tex_init(&pb->tex);

    char title[128];
    snprintf(title, sizeof(title), "tv - %s", ch->display);
    SDL_SetWindowTitle(r->window, title);
    return 0;
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
    if (!pb || playback_open(pb, &r, &NPO_CHANNELS[0], initial_url, &portal) != 0) {
        fputs("initial playback_open failed\n"
              "  try: ./tv.exe --xtream user:pass@host:port\n"
              "  or:  ./tv.exe https://some/stream.m3u8\n", stderr);
        free(pb);
        free(initial_url);
        xtream_free(&portal);
        render_shutdown(&r);
        return 2;
    }
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
            if (ev.type == SDL_MOUSEBUTTONUP && ev.button.button == SDL_BUTTON_RIGHT) {
                drag_active = 0;
                SDL_CaptureMouse(SDL_FALSE);
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
            else {
                const npo_channel_t *nch = key_to_channel(k);
                if (nch && nch != pb->channel) {
                    int ch_idx = (int)(nch - &NPO_CHANNELS[0]);
                    fprintf(stderr, "[switch] %s -> %s (idx=%d)\n",
                            pb->channel->display, nch->display, ch_idx);
                    char *switch_url = build_channel_url(ch_idx, &portal, direct_url);
                    /* Heap-allocate new_pb so its address is stable beyond this scope;
                     * pointer swap avoids the dangling-pointer bug that bit us on the
                     * first channel switch. */
                    playback_t *new_pb = calloc(1, sizeof(*new_pb));
                    if (new_pb && playback_open(new_pb, &r, nch, switch_url, &portal) == 0) {
                        fprintf(stderr, "[switch] new playback opened, tearing down old\n");
                        playback_close(pb);
                        free(pb);
                        pb = new_pb;
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

        /* 1b) Apply window drag if right-button is held. */
        if (drag_active) {
            int mx, my;
            SDL_GetGlobalMouseState(&mx, &my);
            SDL_SetWindowPosition(r.window,
                drag_anchor_wx + (mx - drag_anchor_mx),
                drag_anchor_wy + (my - drag_anchor_my));
        }

        /* 2) Try to grab a new frame if we don't have one pending. */
        if (!pending_vf) pending_vf = queue_try_pop(&pb->player.video_q);
        if (pending_vf) last_frame_ts = SDL_GetTicks();

        /* 3) If we have a pending frame, decide what to do with it. */
        if (pending_vf) {
            if (!pb->clk.have_first) av_clock_mark_first_pts(&pb->clk, pending_vf->pts);
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
         * doesn't trigger it. */
        if (have_texture && SDL_GetTicks() - last_frame_ts > STALL_MS) {
            fprintf(stderr, "[stall] no frames for %ums on %s - restarting\n",
                    STALL_MS, pb->channel->display);
            int ch_idx = (int)(pb->channel - &NPO_CHANNELS[0]);
            char *restart_url = build_channel_url(ch_idx, &portal, direct_url);
            playback_t *new_pb = calloc(1, sizeof(*new_pb));
            if (new_pb && playback_open(new_pb, &r, pb->channel, restart_url, &portal) == 0) {
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
        if (have_texture) {
            SDL_RenderCopy(r.renderer, pb->tex.texture, NULL, NULL);
            if (show_overlay) {
                int ww, wh;
                SDL_GetRendererOutputSize(r.renderer, &ww, &wh);
                overlay_render(&ov, r.renderer, &pb->epg, ww, wh);
            }
        }
        SDL_RenderPresent(r.renderer);  /* blocks ~16ms @ 60Hz with VSYNC */
    }

    if (pending_vf) video_frame_free(pending_vf);
    playback_close(pb);
    free(pb);
    overlay_shutdown(&ov);
    render_shutdown(&r);
    xtream_free(&portal);
    curl_global_cleanup();
    return 0;
}
