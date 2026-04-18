#include "sync.h"

void av_clock_init(av_clock_t *c, const volatile int64_t *samples_played, int sample_rate) {
    c->samples_played = samples_played;
    c->sample_rate    = sample_rate;
    c->first_pts      = 0.0;
    c->have_first     = 0;
}

void av_clock_mark_first_pts(av_clock_t *c, double pts) {
    if (c->have_first) return;
    c->first_pts  = pts;
    c->have_first = 1;
}

double av_clock_now(const av_clock_t *c) {
    return c->first_pts + (double)(*c->samples_played) / (double)c->sample_rate;
}
