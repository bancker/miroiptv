#include "render.h"
#include <stdio.h>
#include <string.h>

int render_init(render_t *r, int w, int h, const char *title) {
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | SDL_INIT_EVENTS) != 0) {
        fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
        return -1;
    }
    r->width = w; r->height = h; r->fullscreen = false;
    r->window = SDL_CreateWindow(title,
        SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED,
        w, h, SDL_WINDOW_SHOWN | SDL_WINDOW_RESIZABLE);
    if (!r->window) { fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError()); return -1; }
    r->renderer = SDL_CreateRenderer(r->window, -1,
        SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
    if (!r->renderer) { fprintf(stderr, "SDL_CreateRenderer: %s\n", SDL_GetError()); return -1; }
    return 0;
}

void render_shutdown(render_t *r) {
    if (r->renderer) SDL_DestroyRenderer(r->renderer);
    if (r->window)   SDL_DestroyWindow(r->window);
    SDL_Quit();
}

void render_toggle_fullscreen(render_t *r) {
    r->fullscreen = !r->fullscreen;
    SDL_SetWindowFullscreen(r->window,
        r->fullscreen ? SDL_WINDOW_FULLSCREEN_DESKTOP : 0);
}

void video_tex_init(video_tex_t *t) {
    t->texture = NULL; t->width = t->height = 0;
}

static int ensure_texture(video_tex_t *t, SDL_Renderer *r, int w, int h) {
    if (t->texture && t->width == w && t->height == h) return 0;
    if (t->texture) SDL_DestroyTexture(t->texture);
    t->texture = SDL_CreateTexture(r, SDL_PIXELFORMAT_IYUV,
                                   SDL_TEXTUREACCESS_STREAMING, w, h);
    if (!t->texture) { fprintf(stderr, "SDL_CreateTexture: %s\n", SDL_GetError()); return -1; }
    t->width = w; t->height = h;
    return 0;
}

int video_tex_upload(video_tex_t *t, SDL_Renderer *r, const video_frame_t *f) {
    if (ensure_texture(t, r, f->width, f->height) != 0) return -1;
    return SDL_UpdateYUVTexture(t->texture, NULL,
        f->y, f->stride_y, f->u, f->stride_u, f->v, f->stride_v);
}

void video_tex_destroy(video_tex_t *t) {
    if (t->texture) SDL_DestroyTexture(t->texture);
    t->texture = NULL;
}

static void audio_callback(void *ud, uint8_t *stream, int len) {
    audio_out_t *ao = ud;
    int16_t *out = (int16_t *)stream;
    int need_samples = len / (int)sizeof(int16_t) / 2; /* per channel */

    while (need_samples > 0) {
        if (ao->cur_remaining == 0) {
            if (ao->cur) { audio_chunk_free(ao->cur); ao->cur = NULL; }
            audio_chunk_t *c = queue_pop(ao->q);
            if (!c) {
                memset(out, 0, (size_t)need_samples * 2 * sizeof(int16_t));
                return;
            }
            ao->cur = c;
            ao->cur_samples   = c->samples;
            ao->cur_remaining = c->n_samples;
        }
        int take = need_samples < (int)ao->cur_remaining ? need_samples : (int)ao->cur_remaining;
        memcpy(out, ao->cur_samples, (size_t)take * 2 * sizeof(int16_t));
        out               += take * 2;
        ao->cur_samples   += take * 2;
        ao->cur_remaining -= take;
        need_samples      -= take;
        ao->samples_played += take;
    }
}

int audio_open(audio_out_t *ao, queue_t *q, int sample_rate) {
    memset(ao, 0, sizeof(*ao));
    ao->q = q; ao->sample_rate = sample_rate;

    SDL_AudioSpec want = {0}, have = {0};
    want.freq     = sample_rate;
    want.format   = AUDIO_S16SYS;
    want.channels = 2;
    want.samples  = 1024;
    want.callback = audio_callback;
    want.userdata = ao;

    ao->device = SDL_OpenAudioDevice(NULL, 0, &want, &have, 0);
    if (ao->device == 0) {
        fprintf(stderr, "SDL_OpenAudioDevice: %s\n", SDL_GetError());
        return -1;
    }
    SDL_PauseAudioDevice(ao->device, 0);
    return 0;
}

void audio_close(audio_out_t *ao) {
    if (ao->device) SDL_CloseAudioDevice(ao->device);
    if (ao->cur)    audio_chunk_free(ao->cur);
    memset(ao, 0, sizeof(*ao));
}
