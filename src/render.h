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

#endif
