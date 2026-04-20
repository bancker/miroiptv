#include "hls_prefetch.h"
#include "hls_prefetch_internal.h"
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <pthread.h>

/* ---------------------------------------------------------------------------
 * Ring buffer — bounded byte FIFO with blocking producer/consumer discipline.
 * ring_write  blocks on not_full   (returns n bytes written, or -1 if closed)
 * ring_read   blocks on not_empty  (returns bytes read, 0 on timeout/closed-empty)
 * ring_close  broadcasts both conds so blocked callers wake immediately.
 * --------------------------------------------------------------------------- */

struct ring_buf {
    unsigned char  *data;
    size_t          cap;
    size_t          head;    /* read position  (index into data[]) */
    size_t          tail;    /* write position (index into data[]) */
    size_t          count;
    int             closed;
    pthread_mutex_t mu;
    pthread_cond_t  not_full;
    pthread_cond_t  not_empty;
};

ring_buf_t *ring_new(size_t cap) {
    if (cap == 0) return NULL;
    ring_buf_t *r = calloc(1, sizeof(*r));
    if (!r) return NULL;
    r->data = malloc(cap);
    if (!r->data) { free(r); return NULL; }
    r->cap = cap;
    pthread_mutex_init(&r->mu, NULL);
    pthread_cond_init(&r->not_full,  NULL);
    pthread_cond_init(&r->not_empty, NULL);
    return r;
}

void ring_free(ring_buf_t *r) {
    if (!r) return;
    free(r->data);
    pthread_mutex_destroy(&r->mu);
    pthread_cond_destroy(&r->not_full);
    pthread_cond_destroy(&r->not_empty);
    free(r);
}

/* Write exactly n bytes. Blocks until all bytes are placed.
 * Returns n on success, -1 if closed before all bytes could be written. */
int ring_write(ring_buf_t *r, const unsigned char *buf, size_t n) {
    size_t written = 0;
    pthread_mutex_lock(&r->mu);
    while (written < n) {
        /* If closed, abort */
        if (r->closed) {
            pthread_mutex_unlock(&r->mu);
            return -1;
        }
        /* Wait while ring is full */
        while (r->count == r->cap && !r->closed) {
            pthread_cond_wait(&r->not_full, &r->mu);
        }
        if (r->closed) {
            pthread_mutex_unlock(&r->mu);
            return -1;
        }
        /* Write as many bytes as space allows */
        while (written < n && r->count < r->cap) {
            r->data[r->tail] = buf[written++];
            r->tail = (r->tail + 1) % r->cap;
            r->count++;
        }
        pthread_cond_signal(&r->not_empty);
    }
    pthread_mutex_unlock(&r->mu);
    return (int)n;
}

/* Read up to `want` bytes with a timeout.
 * Returns bytes read (>0), 0 on timeout with no data, 0 on closed-and-empty. */
int ring_read(ring_buf_t *r, unsigned char *buf, int want, int timeout_ms) {
    if (!r || want <= 0) return 0;

    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    long ms  = deadline.tv_nsec / 1000000L + timeout_ms;
    deadline.tv_sec  += ms / 1000;
    deadline.tv_nsec  = (ms % 1000) * 1000000L;

    pthread_mutex_lock(&r->mu);

    /* Wait until there's data, the ring is closed, or we time out */
    while (r->count == 0 && !r->closed) {
        int rc = pthread_cond_timedwait(&r->not_empty, &r->mu, &deadline);
        if (rc != 0) {
            /* ETIMEDOUT or other error — return 0 */
            pthread_mutex_unlock(&r->mu);
            return 0;
        }
    }

    /* If still empty (closed with nothing left), return 0 */
    if (r->count == 0) {
        pthread_mutex_unlock(&r->mu);
        return 0;
    }

    /* Drain up to `want` bytes */
    int got = 0;
    while (got < want && r->count > 0) {
        buf[got++] = r->data[r->head];
        r->head = (r->head + 1) % r->cap;
        r->count--;
    }
    pthread_cond_signal(&r->not_full);
    pthread_mutex_unlock(&r->mu);
    return got;
}

/* Close the ring — wake all blocked writers and readers. */
void ring_close(ring_buf_t *r) {
    if (!r) return;
    pthread_mutex_lock(&r->mu);
    r->closed = 1;
    pthread_cond_broadcast(&r->not_full);
    pthread_cond_broadcast(&r->not_empty);
    pthread_mutex_unlock(&r->mu);
}

size_t ring_count(const ring_buf_t *r) {
    if (!r) return 0;
    /* Safe read — the caller is responsible for synchronization if needed,
     * but count is size_t so the load is atomic on x86. */
    return r->count;
}

size_t ring_capacity(const ring_buf_t *r) {
    if (!r) return 0;
    return r->cap;
}

/* ---------------------------------------------------------------------------
 * Manifest parser — stub (Task 3)
 * --------------------------------------------------------------------------- */

int manifest_parse(const char *text, size_t len, const char *base_url,
                   manifest_t *out) {
    (void)text; (void)len; (void)base_url; (void)out;
    return -1;
}

void manifest_free(manifest_t *m) {
    (void)m;
}

/* ---------------------------------------------------------------------------
 * Public hls_prefetch interface — stubs (Tasks 6-7)
 * --------------------------------------------------------------------------- */

struct hls_prefetch {
    char *manifest_url;
};

hls_prefetch_t *hls_prefetch_open(const char *manifest_url) {
    (void)manifest_url;
    return NULL;
}

int hls_prefetch_attach(hls_prefetch_t *pf, AVFormatContext *fmt) {
    (void)pf; (void)fmt;
    return -1;
}

void hls_prefetch_close(hls_prefetch_t *pf) {
    (void)pf;
}

void hls_prefetch_get_stats(const hls_prefetch_t *pf,
                            hls_prefetch_stats_t *out) {
    (void)pf;
    if (out) memset(out, 0, sizeof(*out));
}
