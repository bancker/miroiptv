#ifndef TV_SYNC_H
#define TV_SYNC_H

#include <stdint.h>

/* Audio-master clock. Returns "what is the currently-playing audio's
 * stream time in seconds?". Used by the main thread to decide when to
 * upload each decoded video frame to the SDL texture.
 *
 * Implementation: baseline is the first audio pts we actually heard (set
 * once by the audio callback when it pops the first chunk), offset by
 * samples_played / sample_rate as time advances.
 *
 * All three pointed-to fields live in audio_out_t and are updated by the
 * audio callback on SDL's thread; volatility is enough — the clock is
 * read on main but the writes are simple word-sized and monotone. */
typedef struct {
    const volatile int64_t *samples_played;
    const volatile double  *first_pts;
    const volatile int     *has_first_pts;
    int                     sample_rate;
} av_clock_t;

void   av_clock_init_from_audio(av_clock_t *c,
                                const volatile int64_t *samples_played,
                                const volatile double  *first_pts,
                                const volatile int     *has_first_pts,
                                int sample_rate);
int    av_clock_ready(const av_clock_t *c);     /* 1 once audio has started */
double av_clock_now(const av_clock_t *c);

#endif
