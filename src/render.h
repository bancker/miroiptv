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

    /* PTS of the first audio chunk ever popped. Written once in the audio
     * callback, then read by the main thread via av_clock. Using the first
     * AUDIO pts (instead of first VIDEO pts) as the clock baseline matters
     * for mid-stream opens like timeshift, where audio starts seconds
     * before the first IDR video frame. */
    volatile double   first_pts;
    volatile int      has_first_pts;

    /* Points at player_t.decoder_done. When non-NULL and *halt_on_miss is
     * set, the audio callback stops advancing samples_played on queue-miss
     * so the stall detector / watchdog in main can fire. NULL-safe — if
     * unset, keeps the previous silence-with-tick behaviour (needed during
     * normal network hiccups where the decoder is still alive). */
    const volatile int *halt_on_miss;

    /* Audio-pipeline latency (seconds). Samples are written to SDL's
     * buffer, then drained through SDL's callback queue, the OS audio
     * subsystem, and the hardware — typical ~30-50 ms on Windows.
     * samples_played counts writes, so the clock leads actually-audible
     * time by this offset. Seeded at audio_open from (2 * have.samples)
     * / sample_rate; overridable via env TV_AUDIO_LATENCY_MS.
     *
     * Applied by subtracting it from first_pts at clock-seed time —
     * so the clock returns time-of-currently-audible-sample, not
     * time-of-currently-written-sample. Video display decisions now
     * match audio output, eliminating the ~40ms audio lag. */
    double            pipeline_latency_s;
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
    /* Quantized ref_time of last render — rebucket every 15 s so the
     * overlay refreshes ~4x per minute without re-rendering at 60 Hz. */
    int          cached_time_bucket;
} overlay_t;

int  overlay_init(overlay_t *o, const char *font_path);
void overlay_shutdown(overlay_t *o);
void overlay_mark_dirty(overlay_t *o);
/* ref_time is the "now" we compare EPG entries against — pass time(NULL) for
 * live streams, or (timeshift_start + samples_played/sample_rate) for a
 * timeshift replay so the overlay shows the programme the user is actually
 * watching instead of what's airing on real-world NPO. */
int  overlay_render(overlay_t *o, SDL_Renderer *r, const epg_t *epg,
                    time_t ref_time, int window_w, int window_h);

/* Full keybinding cheat sheet, centered on screen with a translucent box. */
int  overlay_render_help(overlay_t *o, SDL_Renderer *r, int ww, int wh);
/* One-line transient hint ("Press ? for help"), bottom-right. Used at
 * startup only, disappears after N seconds. */
int  overlay_render_hint(overlay_t *o, SDL_Renderer *r, const char *text,
                         int ww, int wh);

/* Search overlay: translucent panel top-centered with the query box and up
 * to `visible_n` match rows beneath. `sel` is an index into `names[0..n-1]`
 * and gets a highlighted background. Caller is responsible for keeping
 * `sel` within [0, n) and for capping `n` to what it wants visible. */
int  overlay_render_search(overlay_t *o, SDL_Renderer *r, const char *query,
                           const char *const *names, int n, int sel,
                           int ww, int wh);

/* Subtitle line: center-bottom, black box behind white text. Handles
 * word-wrap at the box edge. Call once per frame with the current text;
 * caller decides whether to call (timing is tracked externally). */
int  overlay_render_subtitle(overlay_t *o, SDL_Renderer *r, const char *text,
                             int ww, int wh);

/* Debug HUD: small top-left status text on a translucent strip. Used by
 * 'd'-toggled debug mode to surface live queue depths / decoder age /
 * clk so the user can see, in the window, why playback is (or isn't)
 * healthy. Caller throttles calls (suggested: re-render when the text
 * would change, not every frame). */
int  overlay_render_debug(overlay_t *o, SDL_Renderer *r, const char *text,
                          int ww, int wh);

#endif
