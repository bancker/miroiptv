#include "sync.h"

void av_clock_init_from_audio(av_clock_t *c,
                              const volatile int64_t *samples_played,
                              const volatile double  *first_pts,
                              const volatile int     *has_first_pts,
                              int sample_rate) {
    c->samples_played = samples_played;
    c->first_pts      = first_pts;
    c->has_first_pts  = has_first_pts;
    c->sample_rate    = sample_rate;
}

int av_clock_ready(const av_clock_t *c) {
    return c && c->has_first_pts && *c->has_first_pts;
}

double av_clock_now(const av_clock_t *c) {
    if (!av_clock_ready(c)) return 0.0;
    return *c->first_pts + (double)(*c->samples_played) / (double)c->sample_rate;
}
