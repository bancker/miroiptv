#ifndef HLS_PREFETCH_INTERNAL_H
#define HLS_PREFETCH_INTERNAL_H

#include "hls_prefetch.h"
#include <stddef.h>

typedef struct {
    char *url;
    int   sequence;
    int   fetched;
} hls_segment_t;

typedef struct {
    int            target_duration_ms;
    int            media_sequence;
    hls_segment_t *segments;
    size_t         n_segments;
} manifest_t;

int  manifest_parse(const char *text, size_t len, const char *base_url,
                    manifest_t *out);
void manifest_free(manifest_t *m);

/* Ring buffer helpers — exposed for tests. Production code accesses
 * the ring through the hls_prefetch_t handle. */
typedef struct ring_buf ring_buf_t;

ring_buf_t *ring_new(size_t cap);
void        ring_free(ring_buf_t *r);
int         ring_write(ring_buf_t *r, const unsigned char *buf, size_t n);
int         ring_read(ring_buf_t *r, unsigned char *buf, int want,
                      int timeout_ms);
void        ring_close(ring_buf_t *r);
size_t      ring_count(const ring_buf_t *r);
size_t      ring_capacity(const ring_buf_t *r);

/* ---------------------------------------------------------------------------
 * Test helpers for segment queue (Task 4) — expose hls_prefetch_t internals
 * for unit test access. These are NOT part of the public API.
 * --------------------------------------------------------------------------- */

typedef struct hls_prefetch hls_prefetch_t;

/* Create a new prefetch session for testing (no background thread yet) */
hls_prefetch_t *_pf_new_for_test(void);

/* Enqueue segments from a manifest, respecting dedup and capacity limits.
 * Returns the count of new segments added. */
int _pf_enqueue_new_segments_for_test(hls_prefetch_t *pf,
                                      const manifest_t *m);

/* Get the current number of segments in the queue */
size_t _pf_segment_count_for_test(const hls_prefetch_t *pf);

/* Retrieve a queued segment URL by index. Returns 0 on success, -1 on
 * index out of bounds. */
int _pf_get_segment_url_for_test(const hls_prefetch_t *pf,
                                 size_t idx, char *buf,
                                 size_t buflen);

/* Free a test-created prefetch session */
void _pf_free_for_test(hls_prefetch_t *pf);

/* Fetch one segment by URL and push its bytes into ring r.
 * Uses curl; streaming write callback calls ring_write.
 * Returns 0 on success (HTTP 200, no curl error),
 * -1 on any failure (non-200 status, curl error, write error). */
int _pf_fetch_segment_for_test(const char *url, ring_buf_t *r);

/* ---------------------------------------------------------------------------
 * Test helper for the AVIO read callback (Task 7).
 * Calls the internal avio_read_packet() function directly against pf's ring,
 * without going through AVFormatContext.  Use this to unit-test the read
 * path without needing a live libav demux session.
 *
 * Returns the number of bytes read (>= 0), or 0 if the ring is empty/closed
 * (libav maps 0 to AVERROR_EOF internally).
 * --------------------------------------------------------------------------- */
int _pf_avio_read_for_test(hls_prefetch_t *pf, unsigned char *buf, int buf_size);

/* Ring accessor — tests that need to pre-load the ring or close it use this.
 * Returns the ring_buf_t* owned by pf, or NULL if pf has none. */
ring_buf_t *_pf_get_ring_for_test(hls_prefetch_t *pf);

/* Set the ring on a test-created prefetch handle (from _pf_new_for_test).
 * The ring becomes owned by pf and will be freed by _pf_free_for_test. */
void _pf_set_ring_for_test(hls_prefetch_t *pf, ring_buf_t *r);

/* Forward-declare AVIOContext so callers don't need to include libav headers
 * just to retrieve the pointer.  Tests that compare fmt->pb need
 * libavformat anyway, so this is just a lightweight forward. */
struct AVIOContext;

/* Returns pf->avio (may be NULL if hls_prefetch_attach has not been called). */
struct AVIOContext *_pf_get_avio_for_test(const hls_prefetch_t *pf);

#endif
