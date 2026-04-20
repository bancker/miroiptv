# Design: HLS Pre-roll Buffer + Custom Manifest Refresh

**Date:** 2026-04-20
**Status:** Draft
**Branch:** `feat/prebuffer`
**Parent tag:** `stable2` (`cd04294`)

## 1. Problem and goal

### What the user sees today

On `m.hnlol.com` live streams, playback dies every 15–30 seconds with a
visible gap of 1–3 seconds while the watchdog detects and reopens the
stream. Four distinct failure modes drive this, all upstream:

1. **Raw `.ts` endpoint** keeps one TCP connection open to the CDN
   (`ce.zova.pro`) and closes it after 15–30 s (per-connection TTL /
   token expiry).
2. **`.m3u8` endpoint** works until libav's HLS demuxer tries to
   refresh the manifest on the CDN URL it cached after the initial
   302 redirect — the original token expires and the CDN returns
   HTTP 509.
3. **libav caches the redirect target** and has no option to force
   re-resolution from the original URL per refresh cycle.
4. **No pre-roll buffer** — the moment the network has a hiccup,
   playback starves instantly because we only hold ~500 ms of data
   in the queues.

### What commercial players do

- Fetch HLS segments well ahead of the live edge (10–30 s of buffer).
- Poll the manifest fresh each cycle with token-aware retry / backoff.
- Keep the decoder fed from the local buffer; network stalls are
  invisible as long as the buffer hasn't drained.
- Surface issues only if the buffer exhausts (genuine sustained outage).

### Goal

Deliver a module that does all four, wraps libav's demuxer with a
custom `AVIOContext`, and replaces libav's HLS demuxer entirely for
this portal class. The watchdog stays as the safety net underneath
— but with this layer in place, it should essentially never need to
fire on live streams.

### Non-goals
- VOD / timeshift playback — those stay on libav's current path (they
  don't have the token-churn problem; they're finite resources).
- Adaptive bitrate — single-quality HLS is sufficient; this portal
  serves one quality anyway.
- Parallel segment fetches — start with a single-connection fetcher;
  future optimization if throughput becomes the bottleneck.
- Seeking within live streams — not meaningful for live; if
  implemented, it's a separate feature.
- Caching segments to disk — memory-only buffer; simpler, bounded.

## 2. Architecture overview

Three new threads per `player_t` (only for HLS live sources):

```
+-----------------+                +-------------------+
| main thread     |                | prefetcher thread |
| (SDL event)     |                | (new)             |
+-----------------+                +-------------------+
                                            |
                                   fetches manifest + segments,
                                   pushes MPEG-TS bytes into
                                            |
                                            v
                                   +-------------------+
                                   | segment ring buf  |
                                   | (bounded, ~20 MB) |
                                   +-------------------+
                                            |
                                            v
                                   +-------------------+
                                   | AVIOContext       |
                                   | (custom read_pkt) |
                                   +-------------------+
                                            |
                                            v
                                   +-------------------+
                                   | libav decoder     |
                                   | (existing thread) |
                                   +-------------------+
```

**Three separate concerns, three separate chunks of state.**

- **`hls_prefetch_t`** (new) — owns the network side. Thread,
  manifest cache, segment queue, fetched-bytes ring buffer, HTTP
  handles.
- **`avio_bridge_t`** (new, small) — owns the libav side. An
  `AVIOContext*` whose `read_packet` callback reads from the ring
  buffer (or blocks briefly if empty).
- **`player_t`** (existing) — unchanged inner loop; just opens via a
  different entry point for HLS URLs.

## 3. Decision tree at `player_open`

```
player_open(url)
├── url ends with ".m3u8" and starts with "http"?
│     ├── yes → hls_prefetch_open(url)
│     │         ├── spawn prefetcher thread
│     │         ├── return AVIOContext* backed by ring buffer
│     │         └── pass to avformat_open_input with a custom AVFormatContext
│     └── no  → existing path (avformat_open_input on raw URL)
```

Everything VOD / timeshift / episodes / user-supplied direct URLs
bypasses the new path and behaves exactly as today. We isolate the
risk to one URL pattern.

## 4. hls_prefetch module

### 4.1 Public interface (`src/hls_prefetch.h`)

```c
#ifndef HLS_PREFETCH_H
#define HLS_PREFETCH_H

#include <libavformat/avformat.h>
#include <stddef.h>
#include <pthread.h>

/* Forward. Internals are opaque to callers. */
typedef struct hls_prefetch hls_prefetch_t;

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
```

### 4.2 Internal state (`src/hls_prefetch.c`)

```c
typedef struct {
    char     *url;           /* e.g. /hls/abc/755880_448.ts */
    int       sequence;      /* MEDIA-SEQUENCE number, monotonic */
    int       fetched;       /* 1 once bytes are in the ring */
} hls_segment_t;

struct hls_prefetch {
    /* Immutable after open */
    char          *manifest_url;      /* ORIGINAL URL (m.hnlol.com/...). Re-fetched
                                         fresh every cycle so we get new CDN
                                         token. Never cached/reused. */
    /* Thread control */
    pthread_t      thread;
    volatile int   stop;              /* set by close, read by thread */

    /* Segment queue — recently-observed segments from the manifest,
     * in sequence order. Prefetcher pops from the head, fetcher
     * pushes to the tail. Bounded; older-than-live segments are
     * dropped silently (we only keep ~4 ahead). */
    pthread_mutex_t seg_mu;
    pthread_cond_t  seg_cv;
    hls_segment_t   segments[16];
    size_t          seg_head, seg_tail, seg_count;

    /* Ring buffer — raw MPEG-TS bytes, fed by the prefetcher,
     * drained by libav's read_packet callback. Two condition vars:
     *   not_full: fetcher waits when ring is full
     *   not_empty: read_packet waits when ring is empty
     */
    pthread_mutex_t buf_mu;
    pthread_cond_t  buf_not_full;
    pthread_cond_t  buf_not_empty;
    unsigned char  *buf;              /* malloc'd, capacity fixed at open */
    size_t          buf_cap;          /* ~20 MB */
    size_t          buf_head;         /* read position  */
    size_t          buf_tail;         /* write position */
    size_t          buf_count;        /* bytes currently held */

    /* Stats (atomic-ish — single writer per field, point reads) */
    volatile size_t segments_fetched;
    volatile size_t manifest_refreshes;
    volatile size_t manifest_errors;
    volatile unsigned int last_refresh_ms;

    /* AVIOContext glue */
    AVIOContext    *avio;             /* owned; freed in close */
    unsigned char  *avio_read_buf;    /* small scratch libav requires */
    size_t          avio_read_buf_sz;
};
```

### 4.3 Prefetcher thread loop

```c
static void *hls_prefetch_thread(void *arg) {
    hls_prefetch_t *pf = arg;
    int backoff_ms = 500;   /* starts small, doubles on error, caps at 5000 */
    while (!pf->stop) {
        manifest_t m = {0};
        int rc = fetch_manifest(pf->manifest_url, &m);
        pf->manifest_refreshes++;
        if (rc != 0) {
            pf->manifest_errors++;
            fprintf(stderr, "[prefetch] manifest fetch rc=%d — retry in %dms\n",
                    rc, backoff_ms);
            msleep(backoff_ms);
            backoff_ms = backoff_ms * 2 > 5000 ? 5000 : backoff_ms * 2;
            continue;
        }
        backoff_ms = 500;   /* reset on success */
        pf->last_refresh_ms = (unsigned int)SDL_GetTicks();

        /* Enqueue any new segments we haven't seen (by sequence). */
        enqueue_new_segments(pf, &m);
        manifest_free(&m);

        /* Drain the segment queue — fetch each and push bytes into
         * the ring. This blocks on buf_not_full when the ring is
         * full, which gives us natural back-pressure from the
         * decoder's read rate. */
        fetch_and_stream_segments(pf);

        /* Sleep until next refresh cycle. HLS target duration is
         * ~TARGETDURATION from the manifest (stored in m.target_duration).
         * Refresh at half that for minimal lag — typically 3-6s.
         * This is still ~10x slower than libav's default keep-alive
         * pattern, staying well under the portal's rate limit. */
        unsigned int refresh_interval_ms = m.target_duration_ms / 2;
        if (refresh_interval_ms < 2000) refresh_interval_ms = 2000;
        if (refresh_interval_ms > 6000) refresh_interval_ms = 6000;
        msleep(refresh_interval_ms);
    }
    return NULL;
}
```

### 4.4 Manifest parsing (stateless)

```c
typedef struct {
    int       target_duration_ms;
    int       media_sequence;
    hls_segment_t *segments;     /* malloc'd; free via manifest_free */
    size_t         n_segments;
} manifest_t;

/* Minimal line-oriented HLS parser. Recognises:
 *   #EXT-X-TARGETDURATION:N          -> target_duration_ms = N * 1000
 *   #EXT-X-MEDIA-SEQUENCE:N          -> media_sequence = N
 *   #EXTINF:F,                       -> next non-# line is a segment URL
 *   (URL)                            -> resolved against base, pushed to list
 *
 * Everything else silently ignored. Bad lines don't fail the parse.
 *
 * Returns 0 on success, -1 on empty / non-EXTM3U / malformed file. */
int manifest_parse(const char *text, size_t len,
                   const char *base_url, manifest_t *out);

void manifest_free(manifest_t *m);
```

HLS segment URLs can be relative (`/hls/abc/755880_448.ts`) or absolute.
Resolver: if the segment URL starts with `http://` or `https://`, use
as-is; if it starts with `/`, concatenate with the scheme+host of
`base_url`; otherwise, replace the last path component of `base_url`.

### 4.5 Segment fetcher

Single-connection curl-based. One segment at a time. Each segment gets a
fresh curl handle (no keep-alive — that was the 509 cause).

```c
static int fetch_segment(hls_prefetch_t *pf, hls_segment_t *seg) {
    CURL *c = curl_easy_init();
    if (!c) return -1;
    curl_easy_setopt(c, CURLOPT_URL, seg->url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, 5L);
    /* Write callback: push bytes directly into the ring buffer,
     * blocking on buf_not_full if ring is full. */
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, ring_write_cb);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, pf);
    /* UA: match what we've verified works with the portal. */
    curl_easy_setopt(c, CURLOPT_USERAGENT, "Lavf/58.76.100");
    CURLcode rc = curl_easy_perform(c);
    long status = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
    curl_easy_cleanup(c);
    if (rc != CURLE_OK || status != 200) {
        fprintf(stderr, "[prefetch] segment fetch failed: url=%s rc=%d status=%ld\n",
                seg->url, rc, status);
        return -1;
    }
    pf->segments_fetched++;
    return 0;
}
```

If a segment fetch fails, the prefetcher logs and moves on — the ring
still has whatever was buffered, decoder keeps draining, one missed
segment becomes a tiny gap that libav's error-resilience tolerates
(we already set `+discardcorrupt+genpts` in player_open opts).

### 4.6 Ring buffer

Classic producer-consumer. Uses existing `queue.c`-style discipline
but with byte-granularity rather than item-granularity.

Capacity: **20 MiB** default (empirically: 15 s of HD video at
~10 Mbps comes out to ~19 MB; 20 MiB gives 15 s + headroom). Settable
via env `TV_PREBUFFER_BYTES=N` for testing.

Blocking semantics:
- `ring_write(pf, buf, n)` — called by fetcher on curl write callback.
  If ring has `n` bytes free, write + signal `buf_not_empty`. Else,
  write as much as fits, wait on `buf_not_full`, continue until all
  `n` bytes written or `pf->stop`.
- `ring_read(pf, buf, want)` — called by AVIO callback. Returns bytes
  actually read (up to `want`). If empty and not stopping, wait on
  `buf_not_empty` (with a 200 ms timeout so libav doesn't deadlock
  on shutdown). On timeout, return `AVERROR(EAGAIN)` — libav retries.
  On `pf->stop`, return `AVERROR_EOF`.

### 4.7 AVIOContext bridge

```c
static int avio_read_packet(void *opaque, uint8_t *buf, int buf_size) {
    hls_prefetch_t *pf = opaque;
    int got = ring_read(pf, buf, buf_size);
    if (got == 0) return AVERROR_EOF;
    return got;  /* libav accepts short reads fine */
}

int hls_prefetch_attach(hls_prefetch_t *pf, AVFormatContext *fmt) {
    if (!pf || !fmt || fmt->pb) return -1;
    pf->avio_read_buf_sz = 32 * 1024;
    pf->avio_read_buf = av_malloc(pf->avio_read_buf_sz);
    if (!pf->avio_read_buf) return -1;
    pf->avio = avio_alloc_context(pf->avio_read_buf,
                                  (int)pf->avio_read_buf_sz,
                                  0,              /* write_flag: 0 (read-only) */
                                  pf,             /* opaque */
                                  avio_read_packet,
                                  NULL, NULL);    /* write, seek: NULL */
    if (!pf->avio) return -1;
    pf->avio->seekable = 0;   /* live stream, not seekable */
    fmt->pb = pf->avio;
    fmt->flags |= AVFMT_FLAG_CUSTOM_IO;
    return 0;
}
```

## 5. Integration into `player.c`

### 5.1 Decision point

```c
int player_open(player_t *p, const char *url) {
    memset(p, 0, sizeof(*p));
    p->video_idx = -1;
    p->audio_idx = -1;
    p->audio_sample_rate_out = 48000;

    p->fmt = avformat_alloc_context();
    if (!p->fmt) return -1;
    p->fmt->interrupt_callback.callback = io_interrupt_cb;
    p->fmt->interrupt_callback.opaque   = p;

    /* Detect HLS URL. Portal-live only — timeshift/VOD/episodes go
     * direct. */
    int is_live_hls = url && strstr(url, ".m3u8") &&
                      (strncmp(url, "http://", 7) == 0 ||
                       strncmp(url, "https://", 8) == 0);

    AVDictionary *opts = NULL;
    /* ... existing opts ... */

    if (is_live_hls) {
        p->prefetch = hls_prefetch_open(url);
        if (!p->prefetch) {
            fprintf(stderr, "hls_prefetch_open failed for %s\n", url);
            goto fail;
        }
        if (hls_prefetch_attach(p->prefetch, p->fmt) != 0) {
            fprintf(stderr, "hls_prefetch_attach failed\n");
            goto fail;
        }
        /* With custom IO we pass NULL for URL. libav will use the
         * AVIOContext we attached. */
        if (avformat_open_input(&p->fmt, NULL, NULL, &opts) != 0) {
            fprintf(stderr, "avformat_open_input (custom IO) failed\n");
            goto fail;
        }
    } else {
        /* Existing direct path. */
        if (avformat_open_input(&p->fmt, url, NULL, &opts) != 0) {
            fprintf(stderr, "avformat_open_input failed for %s\n", url);
            goto fail;
        }
    }
    av_dict_free(&opts);
    /* ... rest unchanged ... */
}
```

### 5.2 Cleanup

`player_close` teardown order:
1. `p->stop = 1` (signals decoder thread to exit its loop)
2. If prefetch: `hls_prefetch_close(p->prefetch)` — this also signals
   the ring to wake up any pending read.
3. `avformat_close_input(&p->fmt)` — this releases the AVIOContext,
   which calls free on `pf->avio_read_buf` (we pass it in alloc;
   libav's av_free handles).

Wait: `avio_alloc_context` takes ownership of the buffer. When
`avformat_close_input` frees `fmt->pb`, it calls `av_free(pb->buffer)`.
So we must not double-free `pf->avio_read_buf` in `hls_prefetch_close`.
Comment this clearly in the teardown code.

### 5.3 `player_t` field additions

```c
typedef struct {
    /* ... existing fields ... */
    hls_prefetch_t *prefetch;   /* non-NULL when this stream is HLS
                                   live with our custom fetcher. NULL
                                   for VOD / timeshift / direct URLs. */
} player_t;
```

## 6. Debug HUD extension

When `d` mode is on, add prefetch stats to the HUD line:

```
DEBUG  ch=NPO 1  vq=14/16 aq=32/32  dec_age=45ms  dec_done=0  clk=12.4s
       samples=589824  buf=19.2MB/20.0MB  segs=7  manif=4/4
```

Layout: one long string, the overlay wraps naturally (or we split into
two lines). Showing `buf=X/Y` is the single most useful number — it's
the "if this goes to 0, playback will die" indicator.

When not on HLS: stats are all 0, HUD shows `(no prefetch)` where the
buffer number would be, or omits the line.

## 7. Watchdog interaction

The existing watchdog in `main.c` stays as-is. Expected changes in
observed behavior:

- `decoder_done` should be set far less often on live HLS —
  libav no longer hits AVERROR_EOF because our ring buffer
  (a) never actually EOFs while the prefetcher is running and
  (b) returns AVERROR(EAGAIN) on transient empty.
- `decoder_wedged` (no-read-for-5s) could still fire if the
  prefetcher falls behind for more than 5 s. That's a genuine
  problem state and the restart is appropriate.

No code change to main.c. The watchdog becomes belt-and-suspenders;
the prefetcher is the primary defense.

## 8. Failure modes and their handling

| Scenario | Current behaviour | New behaviour |
|---|---|---|
| Portal drops connection after 20 s | Decoder EOF, 3-s black gap, reopen | Invisible — buffer carries the 20 s, prefetcher reconnects transparently |
| Portal 509 on manifest refresh | Decoder EOF, 3-s black gap, reopen | Prefetcher logs, backs off 500 ms, retries. Decoder keeps reading from buffer. User sees nothing. |
| Segment fetch fails (404 / 5xx) | N/A — libav demuxed | Prefetcher logs, skips, advances to next. Decoder sees a small time gap which `+discardcorrupt` handles. |
| Network fully down for >15 s | Decoder EOF, fail loop | Buffer exhausts, `ring_read` returns EAGAIN with timeout, decoder gets no data. Watchdog fires after 5 s (decoder_wedged). Restart. When network returns, prefetcher succeeds again — no data loss. |
| Manifest URL becomes permanently 404 | Initial open fails | `hls_prefetch_open` returns NULL. player_open fails. User toast. |
| User changes channel mid-playback | playback_close → player_close → unchanged | hls_prefetch_close cleanly stops thread + frees. |

## 9. Testing strategy

### 9.1 Unit tests (`tests/test_hls_prefetch.c`)

New test binary. Mock HTTP via a local tiny server (or a function
pointer injection for curl — simpler). 15–20 tests:

1. Parse minimal manifest.
2. Parse manifest with multiple segments, absolute URLs.
3. Parse manifest with relative URLs resolves correctly.
4. Parse rejects non-`#EXTM3U` input.
5. Parse handles empty segments list.
6. Ring write/read one byte round-trip.
7. Ring write fills to capacity, blocks next write.
8. Ring read drains to empty, returns EAGAIN on timeout.
9. Ring close wakes pending writer.
10. Ring close wakes pending reader with EOF.
11. Segment queue enqueues in sequence order, dedups.
12. Segment queue drops stale (lower than current live) segments.
13. Prefetcher thread starts + stops cleanly.
14. Stats increment on fetch.
15. AVIO read callback returns bytes from ring.
16. AVIO read callback returns EOF after close.

### 9.2 Integration test harness (`tests/integration_prefetch.c`)

Tiny HTTP server on `localhost:<random_port>` (via pthread), serves a
scripted manifest + segments. Injectable failure modes:

- `serve_drop_after_n_requests(N)` — close connection mid-response.
- `serve_509_after_n_manifests(N)` — return 509 on the Nth manifest GET.
- `serve_rotating_tokens()` — manifests contain segments with tokens
  that expire after one use; old tokens return 403.

Test scenarios:
- `test_playback_survives_509_storm` — server 509s manifest 10x in a
  row; prefetcher retries; decoder reads from buffer the whole time;
  assert: no AVIO EOF during the storm.
- `test_playback_survives_dropped_connection` — server drops
  mid-segment; prefetcher re-fetches; assert: small data gap, decoder
  continues.
- `test_token_rotation` — each manifest has fresh tokens; assert:
  segments fetched correctly across rotation.

### 9.3 Existing 74 tests

Must still pass. No changes to their semantics.

## 10. Rollback plan

The entire prefetcher lives behind a URL-pattern check. If problems
arise on specific channels:

- Env var `TV_DISABLE_PREFETCH=1` skips the HLS path and falls back
  to direct libav open. For field testing and user-side escape hatch.
- Roll forward a revert commit that inverts the `is_live_hls` check.

## 11. Performance budget (lightweight mandate)

- Prefetcher thread: one syscall (`curl_easy_perform`) per segment,
  one malloc/free per manifest. Steady-state CPU: < 1% on a modern
  box (ffmpeg benchmarks confirm).
- Ring buffer: one memcpy per byte pushed, one per byte pulled. At
  10 Mbps = 1.25 MB/s both directions = 2.5 MB/s aggregate memcpy =
  negligible (<0.1% CPU).
- Main thread: zero added work when debug mode off; one extra
  snprintf per 250 ms when debug mode on.
- Memory: 20 MiB for the ring buffer per active HLS session (single
  active at a time in this app; no inflation).

Fits the "no feature may slow it down" constraint: this is a
background thread doing network I/O; the existing audio/video threads
see net benefit (fewer stalls, fewer restarts).

## 12. Open questions (at spec time)

1. **AVIO buffer size.** libav calls `read_packet` with buf_size that
   it picks (typically 32 KB). Smaller values mean more function
   calls but smoother latency; bigger means fewer calls but coarser.
   Starting with whatever libav asks for (caller-driven) and
   instrumenting; tune if needed. Not blocking ship.
2. **HLS target_duration clamp.** Portal serves 10–12 s target
   durations. Clamping to 2–6 s refresh interval seems safe; will
   validate in integration tests.
3. **Multiple concurrent HLS sessions** (not a current use case —
   app plays one stream at a time — but VOD+live overlaps might
   happen eventually). Spec assumes one; if we ever want two, the
   ring buffer sizing needs per-session budget. Document but don't
   solve.

## 13. Commit plan summary

~12 commits on `feat/prebuffer`, in TDD order:

1. Scaffold `hls_prefetch.{h,c}` with empty skeletons + Makefile wiring.
2. Ring buffer + its 10 unit tests.
3. Manifest parser + its 6 unit tests.
4. Segment queue (in-memory, no network) + its 5 unit tests.
5. Curl segment fetcher + unit tests (using a local toy HTTP server).
6. Prefetch thread + its teardown tests.
7. AVIO bridge + libav integration test (no network, stub data).
8. `player.c` integration — URL sniff, attach/detach lifecycle.
9. Debug HUD extension to show prefetch stats.
10. Integration harness (mock HTTP server, scripted failures).
11. 4–6 integration scenarios against the harness.
12. Final code review pass + merge.

## 14. Out of scope (deferred)

- **Audio device continuity across restart.** Significant SDL
  refactor; separate design spec. After this lands we can see
  whether the watchdog even fires enough to matter.
- **Skip-bad-segment recovery.** libav's `+discardcorrupt` handles
  this at the decoder level already. If integration tests show it's
  not enough, a subsequent spec covers it.
- **Parallel connections for throughput.** Not needed for single-
  quality HLS at the user's bandwidth.
- **CI pipeline (GitHub Actions).** Separate commit after this
  merges. Trivial YAML once the Makefile's all-test target is solid.
