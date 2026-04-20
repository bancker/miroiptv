#include "hls_prefetch.h"
#include "hls_prefetch_internal.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <time.h>
#include <pthread.h>
#include <curl/curl.h>
#include <libavformat/avformat.h>
#include <libavformat/avio.h>
#include <libavutil/mem.h>

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
                            manifest_free(out);   /* frees any partial segments */
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
 * Segment queue — bounded FIFO of hls_segment_t. Capacity = 16.
 * Tracks highest-sequence-ever-seen to skip stale manifests.
 * --------------------------------------------------------------------------- */

#define HLS_SEGMENT_QUEUE_CAP 16

typedef struct {
    hls_segment_t segments[HLS_SEGMENT_QUEUE_CAP];
    size_t        count;
    int           highest_sequence_seen;
    pthread_mutex_t mu;
} segment_queue_t;

static segment_queue_t *seg_queue_new(void) {
    segment_queue_t *q = malloc(sizeof(*q));
    if (!q) return NULL;
    memset(q, 0, sizeof(*q));
    q->highest_sequence_seen = -1;  /* None seen yet */
    pthread_mutex_init(&q->mu, NULL);
    return q;
}

static void seg_queue_free(segment_queue_t *q) {
    if (!q) return;
    for (size_t i = 0; i < q->count; i++) {
        free(q->segments[i].url);
    }
    pthread_mutex_destroy(&q->mu);
    free(q);
}

/* Enqueue segments from a manifest, respecting dedup and capacity.
 * Returns the count of new segments added. */
static int seg_queue_enqueue(segment_queue_t *q, const manifest_t *m) {
    if (!q || !m) return 0;

    pthread_mutex_lock(&q->mu);

    int new_count = 0;
    for (size_t i = 0; i < m->n_segments; i++) {
        int seq = m->segments[i].sequence;

        /* Skip if this sequence has already been seen */
        if (seq <= q->highest_sequence_seen) {
            continue;
        }

        /* Update highest-sequence-seen */
        if (seq > q->highest_sequence_seen) {
            q->highest_sequence_seen = seq;
        }

        /* If at capacity, drop the oldest (oldest is at index 0) */
        if (q->count >= HLS_SEGMENT_QUEUE_CAP) {
            free(q->segments[0].url);
            memmove(&q->segments[0], &q->segments[1],
                    (HLS_SEGMENT_QUEUE_CAP - 1) * sizeof(hls_segment_t));
            q->count--;
        }

        /* Enqueue the new segment */
        hls_segment_t *dst = &q->segments[q->count];
        dst->url = malloc(strlen(m->segments[i].url) + 1);
        if (!dst->url) {
            pthread_mutex_unlock(&q->mu);
            return new_count;  /* Partial enqueue on malloc failure */
        }
        strcpy(dst->url, m->segments[i].url);
        dst->sequence = seq;
        dst->fetched = 0;
        q->count++;
        new_count++;
    }

    pthread_mutex_unlock(&q->mu);
    return new_count;
}

static size_t seg_queue_count(const segment_queue_t *q) {
    if (!q) return 0;
    return q->count;
}

static const char *seg_queue_get_url(const segment_queue_t *q, size_t idx) {
    if (!q || idx >= q->count) return NULL;
    return q->segments[idx].url;
}

/* ---------------------------------------------------------------------------
 * msleep — sleep for ms milliseconds (no SDL dep, no extra libs)
 * --------------------------------------------------------------------------- */

static void msleep(int ms) {
    if (ms <= 0) return;
    struct timespec ts;
    ts.tv_sec  = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

/* Return monotonic wall-clock milliseconds (used for last_refresh_ms stats).
 * Uses clock_gettime(CLOCK_MONOTONIC) which is available in MinGW. */
static unsigned int get_ticks_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (unsigned int)(ts.tv_sec * 1000UL + ts.tv_nsec / 1000000UL);
}

/* ---------------------------------------------------------------------------
 * fetch_manifest — curl GET of manifest_url into a manifest_t
 * Uses CURLOPT_FOLLOWLOCATION + CURLOPT_TIMEOUT 15.
 * Returns 0 on success (HTTP 200, valid M3U8), -1 on any failure.
 * --------------------------------------------------------------------------- */

typedef struct {
    char  *buf;
    size_t len;
    size_t cap;
} grow_buf_t;

static size_t grow_buf_write_cb(char *ptr, size_t size, size_t nmemb,
                                void *userdata) {
    grow_buf_t *gb = (grow_buf_t *)userdata;
    size_t n = size * nmemb;
    if (n == 0) return 0;
    if (gb->len + n + 1 > gb->cap) {
        size_t new_cap = gb->cap == 0 ? 4096 : gb->cap * 2;
        while (new_cap < gb->len + n + 1) new_cap *= 2;
        char *newbuf = realloc(gb->buf, new_cap);
        if (!newbuf) return 0;  /* abort curl */
        gb->buf = newbuf;
        gb->cap = new_cap;
    }
    memcpy(gb->buf + gb->len, ptr, n);
    gb->len += n;
    gb->buf[gb->len] = '\0';
    return n;
}

static int fetch_manifest(const char *url, manifest_t *out) {
    if (!url || !out) return -1;

    grow_buf_t gb = {0};

    CURL *c = curl_easy_init();
    if (!c) return -1;

    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, 5L);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, grow_buf_write_cb);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, &gb);
    curl_easy_setopt(c, CURLOPT_USERAGENT, "Lavf/58.76.100");
    curl_easy_setopt(c, CURLOPT_FAILONERROR, 1L);

    CURLcode rc = curl_easy_perform(c);
    long status = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(c);

    if (rc != CURLE_OK || status != 200) {
        free(gb.buf);
        return -1;
    }

    int parse_rc = manifest_parse(gb.buf, gb.len, url, out);
    free(gb.buf);
    return parse_rc;
}

/* ---------------------------------------------------------------------------
 * Public hls_prefetch interface (Task 6)
 *
 * The struct is defined here first so that the callbacks below can access
 * its fields by name.
 * --------------------------------------------------------------------------- */

struct hls_prefetch {
    /* Immutable after open */
    char           *manifest_url;

    /* Ring buffer — owned; shared with the ring_write_cb */
    ring_buf_t     *ring;

    /* Segment queue */
    segment_queue_t *queue;

    /* Thread control */
    pthread_t       thread;
    volatile int    stop;

    /* Stats — single-writer (thread), point-read by get_stats */
    volatile size_t  segments_fetched;
    volatile size_t  manifest_refreshes;
    volatile size_t  manifest_errors;
    volatile unsigned int last_refresh_ms;

    /* AVIOContext bridge — set by hls_prefetch_attach(); freed in
     * hls_prefetch_close() IFF the caller has not already freed it
     * by calling avformat_close_input().  See lifecycle note below. */
    AVIOContext    *avio;
};

/* Production curl write callback — pushes bytes into pf->ring. */
static size_t pf_ring_write_cb(char *ptr, size_t size, size_t nmemb,
                                void *userdata) {
    hls_prefetch_t *pf = (hls_prefetch_t *)userdata;
    size_t total = size * nmemb;
    if (total == 0) return 0;
    if (pf->stop) return 0;   /* abort if closing */
    int written = ring_write(pf->ring, (const unsigned char *)ptr, total);
    if (written < 0) return 0;
    return (size_t)written;
}

/* Fetch one segment URL into pf->ring. Returns 0 on success, -1 on failure. */
static int fetch_segment_into_ring(hls_prefetch_t *pf, const char *url) {
    if (!url || !pf) return -1;

    CURL *c = curl_easy_init();
    if (!c) return -1;

    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, 5L);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, pf_ring_write_cb);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, pf);
    curl_easy_setopt(c, CURLOPT_USERAGENT, "Lavf/58.76.100");
    curl_easy_setopt(c, CURLOPT_FAILONERROR, 1L);

    CURLcode rc = curl_easy_perform(c);
    long status = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(c);

    if (rc != CURLE_OK || status != 200) {
        fprintf(stderr, "[prefetch] segment fetch failed: url=%s curl=%d status=%ld\n",
                url, (int)rc, status);
        return -1;
    }
    return 0;
}

/* The prefetcher worker thread. */
static void *prefetch_thread(void *arg) {
    hls_prefetch_t *pf = (hls_prefetch_t *)arg;
    int backoff_ms = 500;

    while (!pf->stop) {
        manifest_t m;
        memset(&m, 0, sizeof(m));

        int rc = fetch_manifest(pf->manifest_url, &m);
        pf->manifest_refreshes++;

        if (rc != 0) {
            pf->manifest_errors++;
            fprintf(stderr, "[prefetch] manifest fetch failed — retry in %d ms\n",
                    backoff_ms);
            msleep(backoff_ms);
            backoff_ms = (backoff_ms * 2 > 5000) ? 5000 : backoff_ms * 2;
            continue;
        }

        /* Successful manifest refresh */
        backoff_ms = 500;
        pf->last_refresh_ms = get_ticks_ms();

        /* Capture target_duration before manifest_free zeros the struct */
        int target_duration_ms = m.target_duration_ms;

        /* Enqueue any segments not yet seen */
        seg_queue_enqueue(pf->queue, &m);

        /* Fetch all pending segments into the ring */
        while (!pf->stop) {
            /* Pop the next pending segment from the queue */
            pthread_mutex_lock(&pf->queue->mu);
            if (pf->queue->count == 0) {
                pthread_mutex_unlock(&pf->queue->mu);
                break;  /* nothing left to fetch */
            }
            /* Copy URL out so we can release the lock before fetching */
            char *seg_url = malloc(strlen(pf->queue->segments[0].url) + 1);
            if (!seg_url) {
                pthread_mutex_unlock(&pf->queue->mu);
                break;
            }
            strcpy(seg_url, pf->queue->segments[0].url);
            /* Remove from queue (shift remaining entries left) */
            free(pf->queue->segments[0].url);
            memmove(&pf->queue->segments[0], &pf->queue->segments[1],
                    (pf->queue->count - 1) * sizeof(hls_segment_t));
            pf->queue->count--;
            pthread_mutex_unlock(&pf->queue->mu);

            int fetch_rc = fetch_segment_into_ring(pf, seg_url);
            free(seg_url);
            if (fetch_rc == 0) {
                pf->segments_fetched++;
            }
            /* On failure, log already printed inside; just continue */
        }

        manifest_free(&m);

        /* Sleep target_duration / 2, clamped 2000-6000 ms */
        if (!pf->stop) {
            int sleep_ms = target_duration_ms / 2;
            if (sleep_ms < 2000) sleep_ms = 2000;
            if (sleep_ms > 6000) sleep_ms = 6000;
            msleep(sleep_ms);
        }
    }
    return NULL;
}

/* Default ring size: 20 MiB; overridable via TV_PREBUFFER_BYTES env. */
#define DEFAULT_PREBUFFER_BYTES (20 * 1024 * 1024)

hls_prefetch_t *hls_prefetch_open(const char *manifest_url) {
    if (!manifest_url) return NULL;

    hls_prefetch_t *pf = calloc(1, sizeof(*pf));
    if (!pf) return NULL;

    pf->manifest_url = malloc(strlen(manifest_url) + 1);
    if (!pf->manifest_url) { free(pf); return NULL; }
    strcpy(pf->manifest_url, manifest_url);

    /* Ring buffer — size from env or default */
    size_t ring_cap = DEFAULT_PREBUFFER_BYTES;
    const char *env = getenv("TV_PREBUFFER_BYTES");
    if (env) {
        long v = atol(env);
        if (v > 0) ring_cap = (size_t)v;
    }
    pf->ring = ring_new(ring_cap);
    if (!pf->ring) { free(pf->manifest_url); free(pf); return NULL; }

    /* Segment queue */
    pf->queue = seg_queue_new();
    if (!pf->queue) {
        ring_free(pf->ring);
        free(pf->manifest_url);
        free(pf);
        return NULL;
    }

    /* Pre-flight: synchronous single manifest fetch with short timeout.
     * A permanently-unreachable URL (wrong hostname, bad port, 404) is
     * detected here and propagated as open-failure rather than silently
     * retrying forever in the background. 1-second timeout keeps the
     * zap-latency cost invisible on healthy channels. */
    manifest_t m_preflight = {0};
    CURL *preflight = curl_easy_init();
    if (!preflight) {
        /* existing cleanup path: free ring, free queue, free manifest_url, free pf */
        goto preflight_fail;
    }
    grow_buf_t gb = {0};
    curl_easy_setopt(preflight, CURLOPT_URL, manifest_url);
    curl_easy_setopt(preflight, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(preflight, CURLOPT_TIMEOUT, 1L);
    curl_easy_setopt(preflight, CURLOPT_CONNECTTIMEOUT, 1L);
    curl_easy_setopt(preflight, CURLOPT_FAILONERROR, 1L);
    curl_easy_setopt(preflight, CURLOPT_WRITEFUNCTION, grow_buf_write_cb);
    curl_easy_setopt(preflight, CURLOPT_WRITEDATA, &gb);
    curl_easy_setopt(preflight, CURLOPT_USERAGENT, "Lavf/58.76.100");
    CURLcode crc = curl_easy_perform(preflight);
    curl_easy_cleanup(preflight);
    if (crc != CURLE_OK) {
        fprintf(stderr, "hls_prefetch: pre-flight fetch failed: %s\n",
                curl_easy_strerror(crc));
        free(gb.buf);
        goto preflight_fail;
    }
    int mrc = manifest_parse(gb.buf, gb.len, manifest_url, &m_preflight);
    free(gb.buf);
    if (mrc != 0) {
        fprintf(stderr, "hls_prefetch: pre-flight manifest parse failed\n");
        goto preflight_fail;
    }
    /* We have a valid first manifest. Seed the segment queue so the
     * first segment fetches start immediately after pthread_create. */
    seg_queue_enqueue(pf->queue, &m_preflight);
    manifest_free(&m_preflight);
    pf->manifest_refreshes++;
    pf->last_refresh_ms = (unsigned int)get_ticks_ms();

    /* Spawn the worker thread */
    if (pthread_create(&pf->thread, NULL, prefetch_thread, pf) != 0) {
        seg_queue_free(pf->queue);
        ring_free(pf->ring);
        free(pf->manifest_url);
        free(pf);
        return NULL;
    }

    return pf;

preflight_fail:
    manifest_free(&m_preflight);
    seg_queue_free(pf->queue);
    ring_free(pf->ring);
    free(pf->manifest_url);
    free(pf);
    return NULL;
}

/* ---------------------------------------------------------------------------
 * AVIO bridge — §4.7
 *
 * Lifecycle note on avio_read_buf:
 *   avio_alloc_context() takes OWNERSHIP of the buffer passed to it.
 *   When avio_context_free() is called, it calls av_free(avio->buffer)
 *   internally.  Therefore we must NOT separately free avio_read_buf
 *   once we have handed it to avio_alloc_context — doing so would be a
 *   double-free.  hls_prefetch_close() frees the AVIOContext via
 *   avio_context_free(), which transitively frees the buffer.
 *
 * Teardown contract between hls_prefetch_close() and player.c:
 *   hls_prefetch_close() frees pf->avio unconditionally.
 *   player.c MUST set fmt->pb = NULL BEFORE calling
 *   avformat_close_input(); otherwise libav would call
 *   avio_context_free() on an already-freed pointer (double-free).
 * --------------------------------------------------------------------------- */

static int avio_read_packet(void *opaque, uint8_t *buf, int buf_size) {
    hls_prefetch_t *pf = (hls_prefetch_t *)opaque;
    /* 200 ms timeout — keeps libav from deadlocking on a temporarily
     * empty ring while the prefetch thread is fetching the next segment. */
    int got = ring_read(pf->ring, buf, buf_size, 200);
    /* ring_read returns 0 on timeout or closed-empty. libav's fill_buffer
     * retries when read_packet returns 0 (ffmpeg >= 7.x), so 0 is
     * effectively "no data yet, try again". A closed+empty ring will
     * spin at the ring_read timeout (200ms) per call until
     * io_interrupt_cb fires (p->stop=1). Returning AVERROR_EOF here
     * would stop the decoder immediately — we deliberately do NOT, so
     * the decoder survives normal segment gaps of 2-6s. */
    return got;
}

int hls_prefetch_attach(hls_prefetch_t *pf, AVFormatContext *fmt) {
    if (!pf || !fmt || fmt->pb) return -1;

    /* Allocate the 32 KB I/O scratch buffer libav requires.
     * Ownership is transferred to avio_alloc_context — do NOT free
     * avio_read_buf separately (see lifecycle note above). */
    unsigned char *avio_read_buf = av_malloc(32768);
    if (!avio_read_buf) return -1;

    pf->avio = avio_alloc_context(
        avio_read_buf,       /* buffer — ownership transferred to avio */
        32768,               /* buffer size */
        0,                   /* write_flag: 0 = read-only */
        pf,                  /* opaque passed to read_packet */
        avio_read_packet,    /* read callback */
        NULL,                /* write callback: none */
        NULL);               /* seek callback: none */

    if (!pf->avio) {
        /* avio_alloc_context failed; it doesn't free the buffer on failure,
         * so we must free it ourselves to avoid a leak. */
        av_free(avio_read_buf);
        return -1;
    }

    pf->avio->seekable = 0;          /* live stream — not seekable */
    fmt->pb    = pf->avio;
    fmt->flags |= AVFMT_FLAG_CUSTOM_IO;
    return 0;
}

void hls_prefetch_close(hls_prefetch_t *pf) {
    if (!pf) return;

    /* Signal the thread to stop */
    pf->stop = 1;

    /* Wake the ring so the thread's ring_write / avio_read_packet don't
     * block forever on a full or empty ring. */
    if (pf->ring) ring_close(pf->ring);

    /* Wait for the thread to exit */
    pthread_join(pf->thread, NULL);

    /* Free the AVIOContext.  avio_context_free() calls av_free(avio->buffer)
     * internally, which frees the avio_read_buf we passed to
     * avio_alloc_context — do NOT free avio_read_buf separately.
     *
     * Caller contract: player.c MUST set fmt->pb = NULL before calling
     * avformat_close_input() so libav does not double-free this avio. */
    if (pf->avio) {
        avio_context_free(&pf->avio);
        pf->avio = NULL;
    }

    /* Free everything else */
    if (pf->queue) { seg_queue_free(pf->queue); pf->queue = NULL; }
    if (pf->ring)  { ring_free(pf->ring);        pf->ring  = NULL; }
    free(pf->manifest_url);
    free(pf);
}

void hls_prefetch_get_stats(const hls_prefetch_t *pf,
                            hls_prefetch_stats_t *out) {
    if (!out) return;
    memset(out, 0, sizeof(*out));
    if (!pf) return;
    out->bytes_buffered    = pf->ring ? ring_count(pf->ring) : 0;
    out->bytes_capacity    = pf->ring ? ring_capacity(pf->ring) : 0;
    out->segments_fetched  = pf->segments_fetched;
    out->manifest_refreshes= pf->manifest_refreshes;
    out->manifest_errors   = pf->manifest_errors;
    out->last_refresh_ms   = pf->last_refresh_ms;
}

/* ---------------------------------------------------------------------------
 * Test helpers (Task 4) — expose segment queue internals for unit tests
 * --------------------------------------------------------------------------- */

hls_prefetch_t *_pf_new_for_test(void) {
    hls_prefetch_t *pf = malloc(sizeof(*pf));
    if (!pf) return NULL;
    memset(pf, 0, sizeof(*pf));
    pf->queue = seg_queue_new();
    if (!pf->queue) {
        free(pf);
        return NULL;
    }
    return pf;
}

int _pf_enqueue_new_segments_for_test(hls_prefetch_t *pf,
                                      const manifest_t *m) {
    if (!pf || !pf->queue || !m) return 0;
    return seg_queue_enqueue(pf->queue, m);
}

size_t _pf_segment_count_for_test(const hls_prefetch_t *pf) {
    if (!pf || !pf->queue) return 0;
    return seg_queue_count(pf->queue);
}

int _pf_get_segment_url_for_test(const hls_prefetch_t *pf,
                                 size_t idx, char *buf,
                                 size_t buflen) {
    if (!pf || !pf->queue || !buf || buflen == 0) return -1;
    const char *url = seg_queue_get_url(pf->queue, idx);
    if (!url) return -1;
    size_t url_len = strlen(url);
    if (url_len >= buflen) return -1;  /* Buffer too small */
    strcpy(buf, url);
    return 0;
}

void _pf_free_for_test(hls_prefetch_t *pf) {
    if (!pf) return;
    free(pf->manifest_url);
    if (pf->queue) { seg_queue_free(pf->queue); }
    if (pf->ring)  { ring_free(pf->ring); }
    /* Note: pf->avio is intentionally NOT freed here — the test is responsible
     * for calling avio_context_free() before _pf_free_for_test() when it has
     * called hls_prefetch_attach(). */
    free(pf);
}

/* ---------------------------------------------------------------------------
 * Segment fetcher test helper (Task 5)
 * --------------------------------------------------------------------------- */

/* curl write callback: push received bytes into the ring buffer.
 * Returns 0 (not nmemb) on ring_write failure so curl aborts. */
static size_t ring_write_cb(char *ptr, size_t size, size_t nmemb, void *userdata) {
    ring_buf_t *r = (ring_buf_t *)userdata;
    size_t total = size * nmemb;
    if (total == 0) return 0;
    int written = ring_write(r, (const unsigned char *)ptr, total);
    if (written < 0) return 0;   /* signal curl to abort */
    return (size_t)written;
}

/* ---------------------------------------------------------------------------
 * AVIO test helpers (Task 7) — expose the read callback and struct accessors
 * so unit tests can exercise the AVIO bridge without direct struct access.
 * --------------------------------------------------------------------------- */

int _pf_avio_read_for_test(hls_prefetch_t *pf, unsigned char *buf, int buf_size) {
    if (!pf || !buf || buf_size <= 0) return 0;
    return avio_read_packet(pf, buf, buf_size);
}

ring_buf_t *_pf_get_ring_for_test(hls_prefetch_t *pf) {
    if (!pf) return NULL;
    return pf->ring;
}

void _pf_set_ring_for_test(hls_prefetch_t *pf, ring_buf_t *r) {
    if (!pf) return;
    pf->ring = r;
}

struct AVIOContext *_pf_get_avio_for_test(const hls_prefetch_t *pf) {
    if (!pf) return NULL;
    return pf->avio;
}

/* ---------------------------------------------------------------------------
 * Segment fetcher test helper (Task 5)
 * --------------------------------------------------------------------------- */

int _pf_fetch_segment_for_test(const char *url, ring_buf_t *r) {
    if (!url || !r) return -1;

    CURL *c = curl_easy_init();
    if (!c) return -1;

    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, 5L);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, ring_write_cb);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, r);
    curl_easy_setopt(c, CURLOPT_USERAGENT, "Lavf/58.76.100");
    /* Fail fast on HTTP errors (4xx/5xx) */
    curl_easy_setopt(c, CURLOPT_FAILONERROR, 1L);

    CURLcode rc = curl_easy_perform(c);

    long status = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(c);

    if (rc != CURLE_OK) {
        fprintf(stderr, "[fetch_segment_test] curl error: %s (url=%s)\n",
                curl_easy_strerror(rc), url);
        return -1;
    }
    if (status != 200) {
        fprintf(stderr, "[fetch_segment_test] HTTP %ld (url=%s)\n", status, url);
        return -1;
    }
    return 0;
}
