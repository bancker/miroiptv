#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    player_t     player;
    audio_out_t  audio;
    video_tex_t  tex;
    av_clock_t   clk;
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
    if (player_start(&pb->player) != 0)         { player_close(&pb->player); free(pb->url); pb->url = NULL; return -1; }
    if (audio_open(&pb->audio, &pb->player.audio_q, pb->player.audio_sample_rate_out) != 0) {
        player_stop(&pb->player); player_close(&pb->player); free(pb->url); pb->url = NULL; return -1;
    }

    av_clock_init(&pb->clk, &pb->audio.samples_played, pb->audio.sample_rate);
    video_tex_init(&pb->tex);

    char title[128];
    snprintf(title, sizeof(title), "tv - %s", ch->display);
    SDL_SetWindowTitle(r->window, title);
    return 0;
}

static void playback_close(playback_t *pb) {
    if (pb->url) {  /* only tear down if we successfully opened */
        player_stop(&pb->player);
        audio_close(&pb->audio);
        video_tex_destroy(&pb->tex);
        player_close(&pb->player);
        free(pb->url);
    }
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

    int running = 1;
    int paused  = 0;

    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) running = 0;
            else if (ev.type == SDL_KEYDOWN) {
                SDL_Keycode k = ev.key.keysym.sym;
                if (k == SDLK_q || k == SDLK_ESCAPE) running = 0;
                else if (k == SDLK_f) render_toggle_fullscreen(&r);
                else if (k == SDLK_SPACE) {
                    paused = !paused;
                    SDL_PauseAudioDevice(pb.audio.device, paused);
                }
                else {
                    const npo_channel_t *nch = key_to_channel(k);
                    if (nch && nch != pb.channel) {
                        playback_t new_pb;
                        /* Try to open the new channel WITHOUT tearing down current.
                         * If it fails (R1 resolver broken), keep current playback alive. */
                        if (playback_open(&new_pb, &r, nch, NULL) == 0) {
                            playback_close(&pb);
                            pb = new_pb;
                            paused = 0;
                        } else {
                            fprintf(stderr, "channel switch to %s failed - staying on %s\n",
                                    nch->display, pb.channel->display);
                            /* restore title (playback_open may have changed it briefly - nope,
                             * it only sets title on success, so this is a no-op) */
                        }
                    }
                }
            }
        }

        video_frame_t *vf = queue_pop(&pb.player.video_q);
        if (!vf) {
            SDL_Delay(100);
            continue;
        }
        if (!pb.clk.have_first) av_clock_mark_first_pts(&pb.clk, vf->pts);

        double diff = vf->pts - av_clock_now(&pb.clk);
        if (diff > 0) SDL_Delay((Uint32)((diff > 0.1 ? 0.1 : diff) * 1000));
        if (av_clock_now(&pb.clk) - vf->pts > 0.040) {
            video_frame_free(vf);
            continue;
        }

        video_tex_upload(&pb.tex, r.renderer, vf);
        SDL_RenderClear(r.renderer);
        SDL_RenderCopy(r.renderer, pb.tex.texture, NULL, NULL);
        SDL_RenderPresent(r.renderer);
        video_frame_free(vf);
    }

    playback_close(&pb);
    render_shutdown(&r);
    curl_global_cleanup();
    return 0;
}
