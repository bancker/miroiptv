#ifndef HLS_PREFETCH_H
#define HLS_PREFETCH_H

#include <stddef.h>
#include <pthread.h>

/* Forward. Internals are opaque to callers. */
typedef struct hls_prefetch hls_prefetch_t;

/* Forward declare AVFormatContext to avoid libav dependencies in unit tests. */
typedef struct AVFormatContext AVFormatContext;

/* Open a prefetch session for a live HLS manifest URL. Spawns a
 * background thread that polls the manifest, downloads new segments,
 * and feeds the bytes into an internal ring buffer.
 *
 * Returns a prefetch handle on success, NULL on failure (invalid URL,
 * thread spawn failure, memory). Caller owns the handle and must close
 * it with hls_prefetch_close() when done.
 *
 * The prefetcher keeps running until close — even if the network
 * fails, it keeps retrying with backoff. */
hls_prefetch_t *hls_prefetch_open(const char *manifest_url);

/* Attach this prefetch session to an AVFormatContext as its pb
 * (AVIOContext). libav then reads from us instead of from a URL.
 *
 * MUST be called before avformat_open_input. After this, call
 * avformat_open_input with fmt->pb set and url=NULL (or empty).
 *
 * Returns 0 on success, -1 if attach fails (fmt is NULL / already
 * has pb). */
int  hls_prefetch_attach(hls_prefetch_t *pf, AVFormatContext *fmt);

/* Stop the prefetcher thread, free the ring buffer, close all HTTP
 * handles. Safe to call after avformat_close_input has torn down
 * the AVFormatContext. Idempotent. */
void hls_prefetch_close(hls_prefetch_t *pf);

/* Observability for the debug HUD. Returns current buffer depth in
 * bytes (0 if empty), capacity, and a monotonic counter of bytes
 * ever written (useful for tail-head deltas). Lock-free point reads
 * of volatile fields. */
typedef struct {
    size_t bytes_buffered;     /* what decoder will read next */
    size_t bytes_capacity;     /* ring size */
    size_t segments_fetched;   /* total segments ever pulled */
    size_t manifest_refreshes; /* total manifest GETs */
    size_t manifest_errors;    /* GETs that returned non-200 */
    unsigned int last_refresh_ms;  /* SDL_GetTicks of last 200 */
} hls_prefetch_stats_t;

void hls_prefetch_get_stats(const hls_prefetch_t *pf,
                            hls_prefetch_stats_t *out);

#endif
