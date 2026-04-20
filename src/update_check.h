#ifndef TV_UPDATE_CHECK_H
#define TV_UPDATE_CHECK_H

#include <stddef.h>

/* Start a background thread that hits
 *   https://api.github.com/repos/<TV_UPDATE_REPO>/releases/latest
 * once and stashes the result. Safe to call exactly once at startup;
 * does nothing if TV_UPDATE_REPO / TV_VERSION aren't compile-time
 * defined. Fire-and-forget: no cancellation, no blocking, failures
 * are silent. */
void update_check_start(void);

/* Non-blocking poll from the main thread. Returns 1 exactly once if
 * a newer tag than TV_VERSION was found, copying a short user-facing
 * toast into `out` (truncated to `cap` bytes). Returns 0 otherwise —
 * either still in flight, no newer tag, request failed, or the hit
 * has already been reported. Main's toast pipeline owns the result
 * string from that point on; we don't cache anywhere else. */
int  update_check_take_notice(char *out, size_t cap);

#endif
