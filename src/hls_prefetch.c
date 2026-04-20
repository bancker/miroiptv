#include "hls_prefetch.h"
#include "hls_prefetch_internal.h"
#include <stdlib.h>
#include <string.h>

/* Ring buffer implementation */
struct ring_buf {
    unsigned char  *data;
    size_t          cap;
    size_t          head;   /* read position */
    size_t          tail;   /* write position */
    size_t          count;
    int             closed;
};

ring_buf_t *ring_new(size_t cap) {
    /* Stub: return NULL */
    (void)cap;
    return NULL;
}

void ring_free(ring_buf_t *r) {
    /* Stub: no-op */
    (void)r;
}

int ring_write(ring_buf_t *r, const unsigned char *buf, size_t n) {
    /* Stub: return -1 */
    (void)r;
    (void)buf;
    (void)n;
    return -1;
}

int ring_read(ring_buf_t *r, unsigned char *buf, int want, int timeout_ms) {
    /* Stub: return 0 */
    (void)r;
    (void)buf;
    (void)want;
    (void)timeout_ms;
    return 0;
}

void ring_close(ring_buf_t *r) {
    /* Stub: no-op */
    (void)r;
}

size_t ring_count(const ring_buf_t *r) {
    /* Stub: return 0 */
    (void)r;
    return 0;
}

size_t ring_capacity(const ring_buf_t *r) {
    /* Stub: return 0 */
    (void)r;
    return 0;
}

/* Manifest parser */
int manifest_parse(const char *text, size_t len, const char *base_url,
                   manifest_t *out) {
    /* Stub: return -1 */
    (void)text;
    (void)len;
    (void)base_url;
    (void)out;
    return -1;
}

void manifest_free(manifest_t *m) {
    /* Stub: no-op */
    (void)m;
}

/* Public hls_prefetch interface */
struct hls_prefetch {
    /* Opaque to callers */
    char *manifest_url;
};

hls_prefetch_t *hls_prefetch_open(const char *manifest_url) {
    /* Stub: return NULL */
    (void)manifest_url;
    return NULL;
}

int hls_prefetch_attach(hls_prefetch_t *pf, AVFormatContext *fmt) {
    /* Stub: return -1 */
    (void)pf;
    (void)fmt;
    return -1;
}

void hls_prefetch_close(hls_prefetch_t *pf) {
    /* Stub: no-op */
    (void)pf;
}

void hls_prefetch_get_stats(const hls_prefetch_t *pf,
                            hls_prefetch_stats_t *out) {
    /* Stub: zero the stats struct */
    (void)pf;
    if (out) {
        memset(out, 0, sizeof(*out));
    }
}
