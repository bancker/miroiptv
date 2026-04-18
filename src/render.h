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

#endif
