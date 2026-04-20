#include "hls_prefetch.h"
#include "hls_prefetch_internal.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
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
 * Manifest parser — line-oriented HLS M3U8 parser (Task 3)
 * --------------------------------------------------------------------------- */

/* Resolve a segment URL against base_url.
 * - Absolute (http://, https://) → use as-is.
 * - Absolute path (/...) → scheme+host from base_url + path.
 * - Relative → replace last component of base_url's path.
 */
static char *resolve_url(const char *seg_url, const char *base_url) {
    if (!seg_url) return NULL;

    /* If seg_url is absolute, use it */
    if (strncmp(seg_url, "http://", 7) == 0 ||
        strncmp(seg_url, "https://", 8) == 0) {
        char *result = malloc(strlen(seg_url) + 1);
        if (result) strcpy(result, seg_url);
        return result;
    }

    if (!base_url) {
        /* No base to resolve against, return segment as-is */
        char *result = malloc(strlen(seg_url) + 1);
        if (result) strcpy(result, seg_url);
        return result;
    }

    /* If seg_url starts with /, extract scheme+host from base_url */
    if (seg_url[0] == '/') {
        /* Find end of scheme+host in base_url */
        const char *p = base_url;
        if (strncmp(p, "https://", 8) == 0) p += 8;
        else if (strncmp(p, "http://", 7) == 0) p += 7;
        else {
            /* No scheme in base? Just use seg as-is */
            char *result = malloc(strlen(seg_url) + 1);
            if (result) strcpy(result, seg_url);
            return result;
        }

        /* Find the first / after scheme+host */
        const char *slash = strchr(p, '/');
        if (!slash) slash = p + strlen(p);

        /* Reconstruct: scheme+host+seg_url */
        size_t prefix_len = slash - base_url;
        size_t total_len = prefix_len + strlen(seg_url) + 1;
        char *result = malloc(total_len);
        if (result) {
            strncpy(result, base_url, prefix_len);
            strcpy(result + prefix_len, seg_url);
        }
        return result;
    }

    /* Relative: replace last component of base_url's path */
    const char *last_slash = strrchr(base_url, '/');
    if (!last_slash) {
        /* No path separator? just append */
        char *result = malloc(strlen(base_url) + strlen(seg_url) + 2);
        if (result) sprintf(result, "%s/%s", base_url, seg_url);
        return result;
    }

    size_t prefix_len = last_slash - base_url + 1;  /* include the / */
    size_t total_len = prefix_len + strlen(seg_url) + 1;
    char *result = malloc(total_len);
    if (result) {
        strncpy(result, base_url, prefix_len);
        strcpy(result + prefix_len, seg_url);
    }
    return result;
}

int manifest_parse(const char *text, size_t len, const char *base_url,
                   manifest_t *out) {
    if (!text || !out) return -1;
    memset(out, 0, sizeof(*out));
    out->target_duration_ms = 10000;   /* HLS spec default: 10 seconds */

    /* Make a working copy (null-terminated for safety) */
    char *buf = malloc(len + 1);
    if (!buf) return -1;
    memcpy(buf, text, len);
    buf[len] = '\0';

    /* Track if we've seen #EXTM3U and current parsing state */
    int seen_m3u = 0;
    int expect_url = 0;

    /* Line-by-line parsing */
    size_t i = 0;
    while (i < len) {
        /* Find the end of this line */
        size_t line_start = i;
        while (i < len && buf[i] != '\n' && buf[i] != '\r') i++;
        size_t line_end = i;

        /* Skip the newline(s) */
        while (i < len && (buf[i] == '\n' || buf[i] == '\r')) i++;

        /* Extract and process the line */
        size_t line_len = line_end - line_start;
        if (line_len > 0) {
            char *line = buf + line_start;

            /* Trim trailing spaces/tabs/CR from the line */
            while (line_len > 0 && (line[line_len - 1] == ' ' ||
                                     line[line_len - 1] == '\t' ||
                                     line[line_len - 1] == '\r')) {
                line_len--;
            }

            if (line_len > 0) {
                /* Skip leading whitespace */
                size_t skip = 0;
                while (skip < line_len && (line[skip] == ' ' || line[skip] == '\t')) skip++;

                char *actual_line = line + skip;
                size_t actual_len = line_len - skip;

                if (!seen_m3u) {
                    /* First non-blank line must be #EXTM3U */
                    if (actual_len >= 7 && strncmp(actual_line, "#EXTM3U", 7) == 0) {
                        seen_m3u = 1;
                    } else {
                        /* Not an M3U file */
                        free(buf);
                        return -1;
                    }
                } else if (actual_len > 22 && strncmp(actual_line, "#EXT-X-MEDIA-SEQUENCE:", 22) == 0) {
                    out->media_sequence = atoi(actual_line + 22);
                } else if (actual_len > 22 && strncmp(actual_line, "#EXT-X-TARGETDURATION:", 22) == 0) {
                    int duration = atoi(actual_line + 22);
                    out->target_duration_ms = duration * 1000;
                } else if (actual_len > 8 && strncmp(actual_line, "#EXTINF:", 8) == 0) {
                    expect_url = 1;
                } else if (actual_line[0] != '#' && expect_url && actual_len > 0) {
                    /* This is a segment URL — resolve and add */
                    /* Make a null-terminated copy of the URL */
                    char url_buf[256];  /* reasonable limit for segment URLs */
                    if (actual_len >= sizeof(url_buf)) {
                        /* URL too long */
                        free(buf);
                        return -1;
                    }
                    memcpy(url_buf, actual_line, actual_len);
                    url_buf[actual_len] = '\0';

                    char *resolved_url = resolve_url(url_buf, base_url);
                    if (resolved_url) {
                        /* Grow segments array */
                        hls_segment_t *new_segs = realloc(out->segments,
                                                          (out->n_segments + 1) * sizeof(hls_segment_t));
                        if (new_segs) {
                            out->segments = new_segs;
                            out->segments[out->n_segments].url = resolved_url;
                            out->segments[out->n_segments].sequence = out->media_sequence + (int)out->n_segments;
                            out->segments[out->n_segments].fetched = 0;
                            out->n_segments++;
                            expect_url = 0;
                        } else {
                            free(resolved_url);
                            free(buf);
                            return -1;
                        }
                    }
                }
            }
        }
    }

    free(buf);
    return seen_m3u ? 0 : -1;
}

void manifest_free(manifest_t *m) {
    if (!m) return;
    if (m->segments) {
        for (size_t i = 0; i < m->n_segments; i++) {
            free(m->segments[i].url);
        }
        free(m->segments);
    }
    memset(m, 0, sizeof(*m));
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
