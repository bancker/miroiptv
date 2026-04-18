#ifndef TV_RENDER_H
#define TV_RENDER_H

#include <SDL2/SDL.h>
#include <stdbool.h>

typedef struct {
    SDL_Window   *window;
    SDL_Renderer *renderer;
    int           width;
    int           height;
    bool          fullscreen;
} render_t;

int  render_init(render_t *r, int w, int h, const char *title);
void render_shutdown(render_t *r);
void render_toggle_fullscreen(render_t *r);

#include "common.h"

typedef struct {
    SDL_Texture *texture;
    int          width;
    int          height;
} video_tex_t;

void video_tex_init(video_tex_t *t);
int  video_tex_upload(video_tex_t *t, SDL_Renderer *r, const video_frame_t *f);
void video_tex_destroy(video_tex_t *t);

typedef struct {
    SDL_AudioDeviceID device;
    int               sample_rate;
    queue_t          *q;              /* audio_chunk_t* queue we pull from */

    /* partial-chunk carry-over */
    const int16_t    *cur_samples;    /* points into cur->samples */
    size_t            cur_remaining;  /* per channel */
    audio_chunk_t    *cur;            /* currently consumed chunk */
    volatile int64_t  samples_played; /* monotonically increasing, guarded by device lock */
} audio_out_t;

int  audio_open(audio_out_t *ao, queue_t *q, int sample_rate);
void audio_close(audio_out_t *ao);

#include <SDL2/SDL_ttf.h>
#include "npo.h"

typedef struct {
    TTF_Font    *font_regular;
    TTF_Font    *font_bold;
    SDL_Texture *cached;     /* last-rendered overlay texture */
    int          cached_w;
    int          cached_h;
    int          dirty;
} overlay_t;

int  overlay_init(overlay_t *o, const char *font_path);
void overlay_shutdown(overlay_t *o);
void overlay_mark_dirty(overlay_t *o);
int  overlay_render(overlay_t *o, SDL_Renderer *r,
                    const epg_t *epg, int window_w, int window_h);

#endif
