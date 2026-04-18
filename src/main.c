#include "render.h"
#include <SDL2/SDL.h>
#include <stdio.h>

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    render_t r = {0};
    if (render_init(&r, 960, 540, "tv — NPO") != 0) return 1;

    bool running = true;
    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            switch (ev.type) {
            case SDL_QUIT: running = false; break;
            case SDL_KEYDOWN:
                if (ev.key.keysym.sym == SDLK_q || ev.key.keysym.sym == SDLK_ESCAPE)
                    running = false;
                else if (ev.key.keysym.sym == SDLK_f)
                    render_toggle_fullscreen(&r);
                break;
            }
        }
        SDL_SetRenderDrawColor(r.renderer, 20, 20, 30, 255);
        SDL_RenderClear(r.renderer);
        SDL_RenderPresent(r.renderer);
        SDL_Delay(16);
    }

    render_shutdown(&r);
    return 0;
}
