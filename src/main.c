#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

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
                         const char *override_url) {
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

    if (npo_fetch_epg(ch, &pb->epg) != 0) {
        fprintf(stderr, "warn: EPG fetch failed for %s (expected if start-api.npo.nl broken)\n",
                ch->display);
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
    curl_global_init(CURL_GLOBAL_DEFAULT);

    const char *override_url = (argc > 1) ? argv[1] : NULL;
    if (override_url) printf("stream (override): %s\n", override_url);

    render_t r;
    if (render_init(&r, 960, 540, "tv - booting") != 0) return 1;

    playback_t pb;
    if (playback_open(&pb, &r, &NPO_CHANNELS[0], override_url) != 0) {
        fputs("initial playback_open failed - pass a working HLS URL as argv[1]\n", stderr);
        render_shutdown(&r);
        return 2;
    }

    overlay_t ov;
    if (overlay_init(&ov, "assets/DejaVuSans.ttf") != 0) {
        playback_close(&pb);
        render_shutdown(&r);
        return 5;
    }
    int show_overlay   = 1;
    int running        = 1;
    int paused         = 0;
    int have_texture   = 0;
    video_frame_t *pending_vf = NULL;  /* held across iterations when not yet due */

    /* Sync tolerances (seconds):
     *  DUE_WINDOW  — how "early" a frame can be to count as due-now (one vsync @ 60Hz ≈ 16ms)
     *  DROP_LATE   — how late a frame can be before we drop it (80ms ≈ 2 frames at 25fps) */
    const double DUE_WINDOW = 0.010;
    const double DROP_LATE  = 0.080;

    while (running) {
        /* 1) Events first, every iteration — keeps UI responsive regardless of decode state. */
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) { running = 0; break; }
            if (ev.type != SDL_KEYDOWN) continue;
            SDL_Keycode k = ev.key.keysym.sym;
            if (k == SDLK_q || k == SDLK_ESCAPE) { running = 0; break; }
            else if (k == SDLK_f) render_toggle_fullscreen(&r);
            else if (k == SDLK_SPACE) {
                paused = !paused;
                SDL_PauseAudioDevice(pb.audio.device, paused);
            }
            else if (k == SDLK_e) show_overlay = !show_overlay;
            else {
                const npo_channel_t *nch = key_to_channel(k);
                if (nch && nch != pb.channel) {
                    playback_t new_pb;
                    if (playback_open(&new_pb, &r, nch, override_url) == 0) {
                        playback_close(&pb);
                        pb = new_pb;
                        paused       = 0;
                        have_texture = 0;
                        if (pending_vf) { video_frame_free(pending_vf); pending_vf = NULL; }
                        overlay_mark_dirty(&ov);
                    } else {
                        fprintf(stderr, "channel switch to %s failed - staying on %s\n",
                                nch->display, pb.channel->display);
                    }
                }
            }
        }

        /* 2) Try to grab a new frame if we don't have one pending. */
        if (!pending_vf) pending_vf = queue_try_pop(&pb.player.video_q);

        /* 3) If we have a pending frame, decide what to do with it. */
        if (pending_vf) {
            if (!pb.clk.have_first) av_clock_mark_first_pts(&pb.clk, pending_vf->pts);
            double now  = av_clock_now(&pb.clk);
            double diff = pending_vf->pts - now;

            if (diff < DUE_WINDOW) {
                /* Due now (or past due). Either upload-for-display, or drop if way late. */
                if (-diff > DROP_LATE) {
                    video_frame_free(pending_vf);
                } else {
                    video_tex_upload(&pb.tex, r.renderer, pending_vf);
                    video_frame_free(pending_vf);
                    have_texture = 1;
                }
                pending_vf = NULL;
            }
            /* else: hold for next iteration — its time hasn't come yet. */
        }

        /* 4) Always render. VSYNC in SDL_RenderPresent paces the loop to ~60Hz,
         *    so holding the last frame costs nothing and keeps the window alive
         *    even when new frames haven't arrived. */
        SDL_RenderClear(r.renderer);
        if (have_texture) {
            SDL_RenderCopy(r.renderer, pb.tex.texture, NULL, NULL);
            if (show_overlay) {
                int ww, wh;
                SDL_GetRendererOutputSize(r.renderer, &ww, &wh);
                overlay_render(&ov, r.renderer, &pb.epg, ww, wh);
            }
        }
        SDL_RenderPresent(r.renderer);  /* blocks ~16ms @ 60Hz with VSYNC */
    }

    if (pending_vf) video_frame_free(pending_vf);
    playback_close(&pb);
    overlay_shutdown(&ov);
    render_shutdown(&r);
    curl_global_cleanup();
    return 0;
}
