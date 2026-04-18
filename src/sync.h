#ifndef TV_SYNC_H
#define TV_SYNC_H

#include <stdint.h>

/* The audio-master clock: "what is the audio's current playback time in seconds?".
 * Implemented as samples_played / sample_rate, taken from the audio_out_t state. */
typedef struct {
    const volatile int64_t *samples_played;
    int                     sample_rate;
    double                  first_pts;   /* first audio pts we saw; baseline offset */
    int                     have_first;
} av_clock_t;

void   av_clock_init(av_clock_t *c, const volatile int64_t *samples_played, int sample_rate);
void   av_clock_mark_first_pts(av_clock_t *c, double pts);
double av_clock_now(const av_clock_t *c);

#endif
