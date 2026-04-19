#include "render.h"
#include <stdio.h>
#include <string.h>
#include <time.h>

int render_init(render_t *r, int w, int h, const char *title) {
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | SDL_INIT_EVENTS) != 0) {
        fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
        return -1;
    }
    r->width = w; r->height = h; r->fullscreen = false;
    r->window = SDL_CreateWindow(title,
        SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED,
        w, h, SDL_WINDOW_SHOWN | SDL_WINDOW_BORDERLESS);
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
            /* try_pop, NOT pop — audio callback runs on SDL's thread and must
             * return quickly. Blocking on an empty queue (decoder briefly
             * stalled between HLS segments) would freeze SDL audio entirely.
             * On miss, fill the remaining frame with silence and return. */
            audio_chunk_t *c = queue_try_pop(ao->q);
            if (!c) {
                /* No audio data. Write silence AND advance samples_played so
                 * the av_clock keeps moving — otherwise a decoder hiccup
                 * freezes the clock, main holds the pending video frame
                 * forever, the video queue fills to cap, the decoder blocks
                 * on queue_push, and everything deadlocks. */
                memset(out, 0, (size_t)need_samples * 2 * sizeof(int16_t));
                ao->samples_played += need_samples;
                return;
            }
            /* Seed the A/V clock baseline from the FIRST audio pts we play,
             * not the first video frame's pts — in mid-stream opens audio
             * starts well before the first IDR and using video's pts made
             * the clock race ahead, dropping every subsequent video frame. */
            if (!ao->has_first_pts) {
                ao->first_pts     = c->pts;
                ao->has_first_pts = 1;
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

int overlay_init(overlay_t *o, const char *font_path) {
    memset(o, 0, sizeof(*o));
    if (TTF_Init() != 0) { fprintf(stderr, "TTF_Init: %s\n", TTF_GetError()); return -1; }
    o->font_regular = TTF_OpenFont(font_path, 18);
    o->font_bold    = TTF_OpenFont(font_path, 20);
    if (!o->font_regular || !o->font_bold) {
        fprintf(stderr, "TTF_OpenFont(%s): %s\n", font_path, TTF_GetError());
        return -1;
    }
    TTF_SetFontStyle(o->font_bold, TTF_STYLE_BOLD);
    o->dirty = 1;
    return 0;
}

void overlay_shutdown(overlay_t *o) {
    if (o->cached) SDL_DestroyTexture(o->cached);
    if (o->font_regular) TTF_CloseFont(o->font_regular);
    if (o->font_bold)    TTF_CloseFont(o->font_bold);
    TTF_Quit();
    memset(o, 0, sizeof(*o));
}

void overlay_mark_dirty(overlay_t *o) { o->dirty = 1; }

static SDL_Surface *render_line(TTF_Font *font, const char *text, SDL_Color col) {
    return TTF_RenderUTF8_Blended(font, text, col);
}

static const epg_entry_t *find_current(const epg_t *epg, time_t now) {
    for (size_t i = 0; i < epg->count; ++i)
        if (epg->entries[i].start <= now && now < epg->entries[i].end)
            return &epg->entries[i];
    return NULL;
}

int overlay_render(overlay_t *o, SDL_Renderer *r, const epg_t *epg,
                   time_t ref_time, int ww, int wh) {
    const int pad = 12, line_h = 26, max_lines = 4, overlay_h = pad * 2 + line_h * max_lines;
    const int overlay_w = ww;

    /* Quantize to 15-second buckets so the overlay picks up time progress
     * (new "Nu:" as programme boundaries cross) without rebuilding the
     * SDL texture 60 times a second. */
    time_t nowref = ref_time ? ref_time : time(NULL);
    int time_bucket = (int)(nowref / 15);

    if (o->dirty || !o->cached || o->cached_w != overlay_w || o->cached_h != overlay_h
        || time_bucket != o->cached_time_bucket) {
        if (o->cached) SDL_DestroyTexture(o->cached);
        o->cached = SDL_CreateTexture(r, SDL_PIXELFORMAT_RGBA8888,
                                      SDL_TEXTUREACCESS_TARGET, overlay_w, overlay_h);
        if (!o->cached) return -1;
        SDL_SetTextureBlendMode(o->cached, SDL_BLENDMODE_BLEND);

        SDL_SetRenderTarget(r, o->cached);
        SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
        SDL_SetRenderDrawColor(r, 0, 0, 0, 180);
        SDL_RenderClear(r);

        time_t now = nowref;
        const epg_entry_t *cur = find_current(epg, now);

        SDL_Color white = { 235, 235, 235, 255 };
        SDL_Color red   = { 255,  90,  90, 255 };

        int y = pad;
        char buf[256];
        if (cur) {
            struct tm lt_s = *localtime(&cur->start);
            struct tm lt_e = *localtime(&cur->end);
            snprintf(buf, sizeof(buf), "Nu: %s  (%02d:%02d - %02d:%02d)",
                     cur->title, lt_s.tm_hour, lt_s.tm_min, lt_e.tm_hour, lt_e.tm_min);
        } else {
            snprintf(buf, sizeof(buf), "Nu: (geen programma gevonden in EPG)");
        }
        SDL_Surface *surf = render_line(o->font_bold, buf,
                                        (cur && cur->is_news) ? red : white);
        if (surf) {
            SDL_Texture *tx = SDL_CreateTextureFromSurface(r, surf);
            SDL_Rect dst = { pad, y, surf->w, surf->h };
            SDL_RenderCopy(r, tx, NULL, &dst);
            SDL_DestroyTexture(tx);
            SDL_FreeSurface(surf);
        }

        int shown = 0;
        size_t start_idx = 0;
        int found_future = 0;
        for (size_t i = 0; i < epg->count; ++i) {
            if (epg->entries[i].start > now) { start_idx = i; found_future = 1; break; }
        }
        if (found_future) {
            for (size_t i = start_idx; i < epg->count && shown < max_lines - 1; ++i) {
                y += line_h;
                const epg_entry_t *e = &epg->entries[i];
                struct tm lt_s = *localtime(&e->start);
                snprintf(buf, sizeof(buf), "   %02d:%02d  %s",
                         lt_s.tm_hour, lt_s.tm_min, e->title);
                SDL_Surface *s2 = render_line(o->font_regular, buf, e->is_news ? red : white);
                if (s2) {
                    SDL_Texture *tx2 = SDL_CreateTextureFromSurface(r, s2);
                    SDL_Rect dst = { pad, y, s2->w, s2->h };
                    SDL_RenderCopy(r, tx2, NULL, &dst);
                    SDL_DestroyTexture(tx2);
                    SDL_FreeSurface(s2);
                }
                shown++;
            }
        }

        SDL_SetRenderTarget(r, NULL);
        o->cached_w           = overlay_w;
        o->cached_h           = overlay_h;
        o->cached_time_bucket = time_bucket;
        o->dirty              = 0;
    }

    SDL_Rect dst = { 0, wh - o->cached_h, o->cached_w, o->cached_h };
    SDL_RenderCopy(r, o->cached, NULL, &dst);
    return 0;
}

int overlay_render_help(overlay_t *o, SDL_Renderer *r, int ww, int wh) {
    /* Help sheet lines. Keep in sync with the actual bindings in main.c. */
    static const char *lines[] = {
        "Keyboard",
        "",
        "  1 / 2 / 3     Switch NPO 1 / 2 / 3",
        "  n             Latest NOS Journaal (most recent, any NPO channel)",
        "  r             Latest RTL Nieuws (across RTL 4/5/7/8/Z archives)",
        "  up / down     Zap to previous / next channel (same as wheel)",
        "  left / right  Skip -30s / +30s (in VOD, episodes, or timeshift)",
        "  e             Toggle EPG overlay (bottom strip)",
        "  Shift+e       Full multi-day EPG, Enter replays past programme",
        "  f             Search channels + movies + series (type, up/down, Enter)",
        "  F11           Toggle fullscreen",
        "  t             Toggle always-on-top",
        "  a             Cycle audio track (NL / FR / EN / …)",
        "  s             Cycle subtitles: off / each language track",
        "  space         Pause / resume audio",
        "  ?             Toggle this help",
        "  q / Esc       Quit",
        "",
        "Mouse",
        "",
        "  Wheel         Zap through all portal channels",
        "  Right-drag    Move the window",
        "  Double-click  Cycle window size (1x -> 4x -> back)",
    };
    const int N = (int)(sizeof(lines) / sizeof(lines[0]));
    const int line_h = 24;
    const int pad_x  = 28;
    const int pad_y  = 20;
    const int box_w  = 680;
    const int box_h  = N * line_h + pad_y * 2;
    SDL_Rect box = { (ww - box_w) / 2, (wh - box_h) / 2, box_w, box_h };

    SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(r, 0, 0, 0, 220);
    SDL_RenderFillRect(r, &box);

    const SDL_Color white = { 240, 240, 240, 255 };
    int y = box.y + pad_y;
    for (int i = 0; i < N; ++i) {
        if (!*lines[i]) { y += line_h; continue; }
        TTF_Font *font = (i == 0 || strcmp(lines[i], "Mouse") == 0) ? o->font_bold : o->font_regular;
        SDL_Surface *s = TTF_RenderUTF8_Blended(font, lines[i], white);
        if (s) {
            SDL_Texture *t = SDL_CreateTextureFromSurface(r, s);
            SDL_Rect dst = { box.x + pad_x, y, s->w, s->h };
            SDL_RenderCopy(r, t, NULL, &dst);
            SDL_DestroyTexture(t);
            SDL_FreeSurface(s);
        }
        y += line_h;
    }
    return 0;
}

int overlay_render_hint(overlay_t *o, SDL_Renderer *r, const char *text, int ww, int wh) {
    if (!text || !*text) return 0;
    const SDL_Color white = { 255, 255, 255, 230 };
    SDL_Surface *s = TTF_RenderUTF8_Blended(o->font_regular, text, white);
    if (!s) return -1;
    const int pad = 8;
    SDL_Rect box = { ww - s->w - pad * 3, wh - s->h - pad * 3, s->w + pad * 2, s->h + pad * 2 };
    SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(r, 0, 0, 0, 180);
    SDL_RenderFillRect(r, &box);
    SDL_Texture *t = SDL_CreateTextureFromSurface(r, s);
    SDL_Rect dst = { box.x + pad, box.y + pad, s->w, s->h };
    SDL_RenderCopy(r, t, NULL, &dst);
    SDL_DestroyTexture(t);
    SDL_FreeSurface(s);
    return 0;
}

int overlay_render_subtitle(overlay_t *o, SDL_Renderer *r, const char *text,
                            int ww, int wh) {
    if (!text || !*text) return 0;
    const SDL_Color white = { 245, 245, 245, 255 };

    /* Render as one blended line. TTF_RenderUTF8_Blended doesn't wrap, but
     * most subtitle lines are short and fit a 960-1920px window. If it
     * overflows we let SDL clip — better than juggling multi-line layout
     * for a cosmetic feature. */
    SDL_Surface *s = TTF_RenderUTF8_Blended(o->font_bold, text, white);
    if (!s) return -1;

    const int pad_x = 14;
    const int pad_y = 6;
    /* Max width: 80% of window. Scale down via destination rect if needed. */
    int max_w = (int)(ww * 0.8);
    int w = s->w, h = s->h;
    if (w > max_w) { h = h * max_w / w; w = max_w; }

    SDL_Rect box = { (ww - w) / 2 - pad_x, wh - h - pad_y * 2 - 36,
                     w + pad_x * 2, h + pad_y * 2 };
    SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(r, 0, 0, 0, 170);
    SDL_RenderFillRect(r, &box);

    SDL_Texture *t = SDL_CreateTextureFromSurface(r, s);
    SDL_Rect dst = { box.x + pad_x, box.y + pad_y, w, h };
    SDL_RenderCopy(r, t, NULL, &dst);
    SDL_DestroyTexture(t);
    SDL_FreeSurface(s);
    return 0;
}

int overlay_render_search(overlay_t *o, SDL_Renderer *r, const char *query,
                          const char *const *names, int n, int sel,
                          int ww, int wh) {
    (void)wh;  /* box is anchored to the top; we don't need the window height */
    const int pad      = 14;
    const int line_h   = 26;
    /* Widen so longer EPG labels ("NU Vandaag 18:00  NOS Journaal extra …")
     * fit without eliding. Capped at 760 so the box doesn't span the whole
     * screen at 3x/4x zoom; 40px side gutters keep it off small windows'
     * edges. */
    int box_w = ww - 40;
    if (box_w > 760) box_w = 760;
    if (box_w < 200) box_w = 200;
    const int box_h    = pad * 3 + line_h + (n > 0 ? line_h * n + pad : 0);
    SDL_Rect box = { (ww - box_w) / 2, 40, box_w, box_h };

    SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
    SDL_SetRenderDrawColor(r, 0, 0, 0, 220);
    SDL_RenderFillRect(r, &box);

    const SDL_Color white  = { 240, 240, 240, 255 };
    const SDL_Color yellow = { 255, 220,  80, 255 };
    const SDL_Color dim    = { 180, 180, 180, 255 };

    /* The caller supplies the header verbatim — so the main search overlay
     * sends "Search:  <query>_" (with trailing cursor) and the episode-picker
     * reuse sends "SeriesName — Enter to play, Esc to go back". */
    SDL_Surface *qs = TTF_RenderUTF8_Blended(o->font_bold, query ? query : "", yellow);
    if (qs) {
        SDL_Texture *qt = SDL_CreateTextureFromSurface(r, qs);
        SDL_Rect dst = { box.x + pad, box.y + pad, qs->w, qs->h };
        SDL_RenderCopy(r, qt, NULL, &dst);
        SDL_DestroyTexture(qt);
        SDL_FreeSurface(qs);
    }

    if (n == 0) {
        /* Two distinct empty states: no query yet (just opened the prompt) vs.
         * query typed but no channel matches. Different messages so the user
         * knows whether to type more or try different text. */
        const char *hint = (query && *query)
            ? "(no matches — try different text, or Esc to cancel)"
            : "(type to search channels — up/down + Enter to pick, Esc to cancel)";
        SDL_Surface *ns = TTF_RenderUTF8_Blended(o->font_regular, hint, dim);
        if (ns) {
            SDL_Texture *nt = SDL_CreateTextureFromSurface(r, ns);
            SDL_Rect dst = { box.x + pad, box.y + pad + line_h + 4, ns->w, ns->h };
            SDL_RenderCopy(r, nt, NULL, &dst);
            SDL_DestroyTexture(nt);
            SDL_FreeSurface(ns);
        }
        return 0;
    }

    int y = box.y + pad + line_h + pad;
    for (int i = 0; i < n; ++i, y += line_h) {
        if (i == sel) {
            SDL_Rect hl = { box.x + 4, y - 2, box_w - 8, line_h };
            SDL_SetRenderDrawColor(r, 60, 90, 140, 220);
            SDL_RenderFillRect(r, &hl);
        }
        SDL_Surface *s = TTF_RenderUTF8_Blended(o->font_regular, names[i] ? names[i] : "", white);
        if (!s) continue;
        SDL_Texture *t = SDL_CreateTextureFromSurface(r, s);
        SDL_Rect dst = { box.x + pad, y, s->w, s->h };
        SDL_RenderCopy(r, t, NULL, &dst);
        SDL_DestroyTexture(t);
        SDL_FreeSurface(s);
    }
    return 0;
}
