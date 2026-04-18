#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    curl_global_init(CURL_GLOBAL_DEFAULT);

    char *url = NULL;
    if (argc > 1) {
        url = strdup(argv[1]);
        printf("stream (override): %s\n", url);
    } else {
        if (npo_resolve_stream(&NPO_CHANNELS[0], &url) != 0) {
            fputs("usage: ./tv.exe <hls-url>  (NPO resolver broken, see R1)\n", stderr);
            return 2;
        }
        printf("stream: %s\n", url);
    }

    player_t pl;
    if (player_open(&pl, url) != 0) { free(url); return 3; }
    if (player_start(&pl) != 0)     { free(url); player_close(&pl); return 3; }

    render_t r;
    if (render_init(&r, pl.vctx->width, pl.vctx->height, "tv - NPO") != 0) return 4;
    video_tex_t tex; video_tex_init(&tex);

    audio_out_t ao;
    if (audio_open(&ao, &pl.audio_q, pl.audio_sample_rate_out) != 0) return 4;

    av_clock_t clk;
    av_clock_init(&clk, &ao.samples_played, ao.sample_rate);

    bool running = true;
    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) running = false;
            else if (ev.type == SDL_KEYDOWN) {
                if (ev.key.keysym.sym == SDLK_q || ev.key.keysym.sym == SDLK_ESCAPE) running = false;
                else if (ev.key.keysym.sym == SDLK_f) render_toggle_fullscreen(&r);
            }
        }

        video_frame_t *vf = queue_pop(&pl.video_q);
        if (!vf) { running = false; break; }

        if (!clk.have_first) av_clock_mark_first_pts(&clk, vf->pts);

        /* wait until audio clock reaches video pts, or drop if too late */
        const double DROP_THRESHOLD = 0.040;    /* 40 ms */
        const double MAX_SLEEP      = 0.100;    /* cap sleep to stay responsive to events */

        double now = av_clock_now(&clk);
        double diff = vf->pts - now;
        if (diff > 0) {
            if (diff > MAX_SLEEP) diff = MAX_SLEEP;
            SDL_Delay((Uint32)(diff * 1000));
        }
        now = av_clock_now(&clk);
        if (now - vf->pts > DROP_THRESHOLD) {
            /* too late, drop */
            video_frame_free(vf);
            continue;
        }

        video_tex_upload(&tex, r.renderer, vf);
        SDL_RenderClear(r.renderer);
        SDL_RenderCopy(r.renderer, tex.texture, NULL, NULL);
        SDL_RenderPresent(r.renderer);
        video_frame_free(vf);
    }

    player_stop(&pl);
    video_tex_destroy(&tex);
    audio_close(&ao);
    render_shutdown(&r);
    player_close(&pl);
    free(url);
    curl_global_cleanup();
    return 0;
}
