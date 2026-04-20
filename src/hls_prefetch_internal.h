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

#endif
