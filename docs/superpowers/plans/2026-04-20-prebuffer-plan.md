# HLS Pre-roll Buffer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pre-roll buffer + custom HLS refresh subsystem per `docs/superpowers/specs/2026-04-20-prebuffer-design.md` (commit `6d407dd`). Every portal-induced drop ≤15 s becomes invisible to the user.

**Architecture:** See §2 of the spec. New module `src/hls_prefetch.{c,h}`, one prefetcher pthread per HLS session, bounded byte ring buffer, custom `AVIOContext` bridge into libav.

**Tech stack:** C11, pthread, libcurl (already linked), libavformat (for AVIOContext). No new deps.

**Spec sections referenced throughout:**
- §3 decision tree
- §4 `hls_prefetch` module interface + internals
- §5 player.c integration
- §6 debug HUD
- §9 testing strategy

---

## File structure

| File | Purpose |
|---|---|
| `src/hls_prefetch.h` (NEW) | Public interface (§4.1) |
| `src/hls_prefetch_internal.h` (NEW) | Struct layout + helpers exposed to tests |
| `src/hls_prefetch.c` (NEW) | Implementation |
| `tests/test_hls_prefetch.c` (NEW) | Unit tests (§9.1) — parser, ring buffer, segment queue, thread lifecycle |
| `tests/test_integration_prefetch.c` (NEW) | Integration tests with mock HTTP server (§9.2) |
| `tests/fixtures/manifest_valid.m3u8` (NEW) | Valid HLS manifest |
| `tests/fixtures/manifest_empty.m3u8` (NEW) | `#EXTM3U` + `#EXT-X-ENDLIST`, no segments |
| `tests/fixtures/manifest_malformed.m3u8` (NEW) | Not `#EXTM3U`-prefixed |
| `src/player.h` | Add `hls_prefetch_t *prefetch` field (§5.3) |
| `src/player.c` | Add URL detect + prefetch open/attach/close (§5.1, 5.2) |
| `src/main.c` | Debug HUD extension (§6) |
| `Makefile` | Two new test binaries + integration target |

---

## Task 1: Scaffold module + Makefile wiring (no behavior)

**Files:**
- Create: `src/hls_prefetch.h`, `src/hls_prefetch_internal.h`, `src/hls_prefetch.c`
- Create: `tests/test_hls_prefetch.c` (stub)
- Modify: `Makefile`

- [ ] **Step 1: Create `src/hls_prefetch.h`** — copy §4.1 from the spec verbatim. Public API only.

- [ ] **Step 2: Create `src/hls_prefetch_internal.h`** — declare the `manifest_t` / `hls_segment_t` / ring-buffer internal helpers the tests will drive directly (manifest_parse, ring_write, ring_read, ring_close). Shape:

```c
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
```

- [ ] **Step 3: Create `src/hls_prefetch.c`** with stub implementations. Every public function returns a safe default: `hls_prefetch_open` returns NULL, `hls_prefetch_attach` returns -1, `hls_prefetch_close` is a no-op, `hls_prefetch_get_stats` zeros the struct. Internal helpers (`manifest_parse`, `ring_*`) also stub-return: parser returns -1, ring_new returns NULL, etc.

- [ ] **Step 4: Create `tests/test_hls_prefetch.c`** stub:

```c
#include "../src/hls_prefetch_internal.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    puts("OK stub_links");
    return 0;
}
```

- [ ] **Step 5: Extend Makefile.** Add targets analogous to the existing test binaries:

```make
PREFETCH_TEST_SRC = tests/test_hls_prefetch.c src/hls_prefetch.c src/queue.c
PREFETCH_TEST_BIN = build/test_hls_prefetch.exe

$(PREFETCH_TEST_BIN): $(PREFETCH_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(PREFETCH_TEST_SRC) -o $@ $(LDLIBS)
```

And extend `test` target to run `./$(PREFETCH_TEST_BIN)` after the existing binaries.

- [ ] **Step 6: Build + run.** `./mk clean && ./mk test` must pass with `OK stub_links` among the output.

- [ ] **Step 7: Commit.**

```
hls_prefetch: scaffold module + test binary, Makefile wiring

No behavior yet — all public functions are safe-default stubs and
internal helpers return sentinel values. Establishes the integration
surface for Tasks 2-12.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 2: Ring buffer — tests + implementation

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`

- [ ] **Step 1: Write 10 ring-buffer tests.** All pure-memory, no pthread for most (pthread only for blocking tests).

1. `test_ring_new_has_capacity_zero_count` — `ring_new(1024)` → cap=1024, count=0.
2. `test_ring_write_read_round_trip` — write 4 bytes, read 4 bytes, assert equality.
3. `test_ring_write_beyond_capacity_blocks` — cap=16, spawn thread that writes 32 bytes; parent reads 16 bytes, thread unblocks, finishes.
4. `test_ring_read_empty_returns_zero_after_timeout` — empty ring, read with timeout_ms=50, expect 0 (timeout).
5. `test_ring_close_wakes_reader_with_zero` — reader blocked in `ring_read`; close; reader returns 0.
6. `test_ring_close_wakes_writer_returns_error` — writer blocked on full ring; close; writer's `ring_write` returns -1.
7. `test_ring_wraps_around` — cap=16, write 12, read 10, write 10 — no corruption, final read gets correct bytes.
8. `test_ring_count_after_partial_read` — write 100, read 50, `ring_count()` returns 50.
9. `test_ring_concurrent_stress` — producer thread writes 100 KB in 1 KB chunks; consumer reads 100 KB in 900-byte chunks; assert byte-accurate (e.g., fill with `(uint8_t)i`, check on read).
10. `test_ring_free_on_null_is_safe` — `ring_free(NULL)` doesn't crash.

All tests use `#include <pthread.h>` where needed; MinGW ships with pthread.h.

- [ ] **Step 2: Run tests — expect failures.**

- [ ] **Step 3: Implement `ring_new`, `ring_free`, `ring_write`, `ring_read`, `ring_close`, `ring_count`, `ring_capacity`.** Internal struct:

```c
struct ring_buf {
    unsigned char  *data;
    size_t          cap;
    size_t          head;   /* read position */
    size_t          tail;   /* write position */
    size_t          count;
    int             closed;
    pthread_mutex_t mu;
    pthread_cond_t  not_full;
    pthread_cond_t  not_empty;
};
```

Design:
- `ring_write`: returns -1 if closed mid-write; returns total bytes written (always == n if not closed).
- `ring_read`: returns total bytes read (up to `want`). On timeout with no data, returns 0. On closed-and-empty, returns 0.
- `ring_close`: sets `closed=1`, broadcasts both conds.
- Timeout: `pthread_cond_timedwait` with the timeout value added to `CLOCK_REALTIME` now. MinGW supports this.

- [ ] **Step 4: All tests pass.**

- [ ] **Step 5: Commit.**

```
hls_prefetch: ring buffer + 10 tests

Classic bounded byte ring with producer-consumer discipline: blocking
ring_write waits on not_full, ring_read waits on not_empty with
caller-provided timeout. close() wakes everyone. All 10 tests cover
round-trip / wrap / concurrent stress / close-during-wait.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 3: Manifest parser — tests + implementation

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`
Create: `tests/fixtures/manifest_valid.m3u8`, `manifest_empty.m3u8`, `manifest_malformed.m3u8`

- [ ] **Step 1: Create fixtures.**

`manifest_valid.m3u8`:
```
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-MEDIA-SEQUENCE:448
#EXT-X-TARGETDURATION:12
#EXTINF:11.520000,
/hls/e1462bab13d1d09d852eae4a45fec5f0/755880_448.ts
#EXTINF:7.680000,
/hls/84a6f78c3b089cb121f980478c49ab64/755880_449.ts
```

`manifest_empty.m3u8`:
```
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-ENDLIST
```

`manifest_malformed.m3u8`:
```
<html>404 Not Found</html>
```

- [ ] **Step 2: Write 6 parser tests.**

1. `test_manifest_parse_valid` — load fixture, assert `n_segments=2`, `media_sequence=448`, `target_duration_ms=12000`, first segment URL resolves against `http://example.com/live/foo.m3u8` to `http://example.com/hls/.../755880_448.ts`.
2. `test_manifest_parse_empty` — empty-segments fixture → `n_segments=0`, success rc.
3. `test_manifest_parse_rejects_non_m3u` — malformed fixture → rc=-1.
4. `test_manifest_parse_rejects_null_text` — `manifest_parse(NULL, 0, NULL, &m)` → rc=-1.
5. `test_manifest_parse_absolute_url_preserved` — synthetic manifest with `http://cdn.example/foo.ts` as segment → URL preserved verbatim.
6. `test_manifest_parse_sequence_without_duration_defaults` — manifest missing `#EXT-X-TARGETDURATION` → `target_duration_ms` defaults to 10000 (10 s) per HLS spec.

- [ ] **Step 3: Implement `manifest_parse` and `manifest_free`.** Line-oriented parser:

```c
int manifest_parse(const char *text, size_t len, const char *base_url,
                   manifest_t *out) {
    if (!text || !out) return -1;
    memset(out, 0, sizeof(*out));
    out->target_duration_ms = 10000;   /* HLS default */

    /* First non-blank line must start with #EXTM3U */
    /* Iterate: split on \n. For each line:
     *   starts with '#EXT-X-MEDIA-SEQUENCE:' -> atoi tail into media_sequence
     *   starts with '#EXT-X-TARGETDURATION:' -> atoi tail, *1000 into target_duration_ms
     *   starts with '#EXTINF:' -> set "expect URL next" flag
     *   non-#, non-empty, after EXTINF -> push segment with resolved URL
     *   otherwise ignored
     */
    /* ... grow segments array with realloc as needed ... */
    return 0;
}
```

URL resolver:
- starts with `http://` or `https://` → use as-is.
- starts with `/` → extract scheme+host from base_url, concatenate.
- otherwise → replace last path component of base_url.

- [ ] **Step 4: All tests pass.**

- [ ] **Step 5: Commit.**

```
hls_prefetch: manifest parser + 6 tests

Line-oriented HLS parser. Extracts EXT-X-MEDIA-SEQUENCE,
EXT-X-TARGETDURATION, and segment URLs (resolved against base URL).
Rejects non-M3U input, survives missing tags with documented defaults.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 4: Segment queue — tests + implementation

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`

Internal-only — segment queue is part of `hls_prefetch_t` but we expose thin test helpers: `hls_prefetch_t *_pf_new_for_test(void)` and `int _pf_enqueue_new_segments_for_test(hls_prefetch_t *, const manifest_t *)`. Declared in `hls_prefetch_internal.h`.

- [ ] **Step 1: Write 4 tests.**

1. `test_segments_enqueued_in_sequence_order` — enqueue manifest with seq 448, 449, 450; queue has 3 entries in that order.
2. `test_segments_dedup_on_subsequent_manifest` — first manifest seq 448-450, second seq 449-451; queue should only gain 451 (448,449,450 already there).
3. `test_segments_skip_stale` — queue current is at seq 500; manifest arrives with seq 448-450 (all stale); queue unchanged.
4. `test_segments_queue_cap` — push 20 segments; queue saturates at its fixed size (16 per spec); oldest drops.

- [ ] **Step 2: Implement `_pf_enqueue_new_segments_for_test`** + underlying logic. Key invariant: track the highest sequence ever seen; reject manifests where all sequences are ≤ that.

- [ ] **Step 3: Tests pass.**

- [ ] **Step 4: Commit.**

```
hls_prefetch: segment queue — sequence-ordered, deduped, bounded

Enqueue only segments with sequence > highest-seen. Drop stale
(below current live). Cap at 16 pending segments — if the fetcher
can't keep up, drop oldest (prioritize live-edge over completeness).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 5: Segment fetcher — test against local HTTP server

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`

Introduces a tiny test-only HTTP server. Used by Tasks 5, 10, 11.

- [ ] **Step 1: Add a minimal HTTP server helper to the test file.** Listens on `127.0.0.1:0` (OS-assigned port), serves from a registered route table. Spawned as a pthread. ~80 lines of C; use `socket`, `bind`, `listen`, `accept`, `recv`, `send`.

- [ ] **Step 2: Write 3 tests.**

1. `test_fetch_segment_basic` — register `/seg.ts` → 200 + 4 KB body; call `_pf_fetch_segment_for_test` with that URL into a ring; ring now contains the 4 KB.
2. `test_fetch_segment_404_fails` — register `/seg.ts` → 404; fetch returns -1.
3. `test_fetch_segment_drops_connection_fails` — server closes mid-response; fetch returns -1 (curl reports incomplete transfer).

- [ ] **Step 3: Implement `_pf_fetch_segment_for_test`.** Uses curl as in §4.5 of the spec; write callback pushes into the ring.

- [ ] **Step 4: Tests pass. Clean up test server cleanly (join pthread, close socket).**

- [ ] **Step 5: Commit.**

```
hls_prefetch: segment fetcher + test HTTP server

Curl-based single-segment fetcher that streams bytes into a ring
buffer. Tests drive it against a localhost HTTP server (shared
harness for Tasks 5/10/11).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 6: Prefetcher thread lifecycle

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`

Wires `hls_prefetch_open` + `hls_prefetch_close` to actually spawn and cleanly stop the thread.

- [ ] **Step 1: Write 3 tests.**

1. `test_prefetch_open_close` — open against local test server (valid manifest, 2 segments); close after 200 ms; join succeeds, no leaks.
2. `test_prefetch_open_invalid_url_returns_null` — URL that fails to resolve (e.g., `http://nonexistent.invalid/foo.m3u8`): `hls_prefetch_open` may return non-NULL but the first manifest fetch fails and the thread retries; close still works.
3. `test_prefetch_stats_populated` — after a successful fetch, `get_stats` returns `manifest_refreshes >= 1`, `segments_fetched >= 1`, `last_refresh_ms != 0`.

- [ ] **Step 2: Implement `hls_prefetch_open`** — malloc the struct, create ring, start thread per §4.3. Implement `hls_prefetch_close` — `pf->stop = 1`, `ring_close(pf->ring)`, `pthread_join`, free everything.

- [ ] **Step 3: Implement the thread loop per spec §4.3.** Backoff on error, re-fetch fresh manifest URL each cycle, enqueue new segments, fetch them (via Task 5 code), sleep half-target-duration.

- [ ] **Step 4: Tests pass.**

- [ ] **Step 5: Commit.**

```
hls_prefetch: thread lifecycle — open spawns worker, close joins

Worker loop: fetch manifest (with exponential backoff on error),
enqueue new segments, fetch them into the ring, sleep
target_duration_ms / 2 (clamped 2-6s). Re-fetches the *original*
manifest URL each cycle so we get fresh CDN tokens — avoiding the
libav-cached-redirect-509 bug this whole module exists to fix.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 7: AVIO bridge — tests + implementation

**Files:** Modify `src/hls_prefetch.c`, `tests/test_hls_prefetch.c`

- [ ] **Step 1: Write 3 tests.**

1. `test_avio_read_returns_ring_bytes` — manually push 4 KB into pf's ring; call the AVIO read callback (via a helper `_pf_avio_read_for_test`); assert the 4 KB comes out.
2. `test_avio_read_on_empty_ring_times_out` — empty ring, no prefetcher (or stop it); read returns AVERROR(EAGAIN)? No — our spec says return 0 → libav converts to AVERROR_EOF. Test: `_pf_avio_read_for_test` on empty ring returns 0.
3. `test_avio_attach_sets_fmt_pb` — allocate AVFormatContext; call `hls_prefetch_attach`; assert `fmt->pb == pf->avio` and `fmt->flags & AVFMT_FLAG_CUSTOM_IO`.

- [ ] **Step 2: Implement `hls_prefetch_attach` per §4.7 and the read callback per §4.7.** Allocate `avio_read_buf`, `avio_alloc_context`, store on pf.

- [ ] **Step 3: Tests pass.**

- [ ] **Step 4: Commit.**

```
hls_prefetch: AVIOContext bridge so libav reads from the ring

Custom read_packet callback that drains from pf's ring buffer.
hls_prefetch_attach() slots the AVIOContext into an
AVFormatContext so the caller can then avformat_open_input(NULL).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 8: player.c integration

**Files:** Modify `src/player.h`, `src/player.c`

- [ ] **Step 1: Add `hls_prefetch_t *prefetch` field to `player_t`** at the top of the struct (before threading state):

```c
#include "hls_prefetch.h"
...
typedef struct {
    /* ... */
    hls_prefetch_t *prefetch;   /* HLS live prefetcher; NULL for non-HLS */
    /* ... */
} player_t;
```

- [ ] **Step 2: Add the URL-sniff branch in `player_open`** (spec §5.1). Before `avformat_open_input`, check if URL ends with `.m3u8` AND starts with `http`. If so, open prefetch and attach. Env escape hatch: `TV_DISABLE_PREFETCH=1` forces fallback.

```c
int is_live_hls = !getenv("TV_DISABLE_PREFETCH") &&
                  url && strstr(url, ".m3u8") &&
                  (strncmp(url, "http://", 7) == 0 ||
                   strncmp(url, "https://", 8) == 0);
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
    if (avformat_open_input(&p->fmt, NULL, NULL, &opts) != 0) {
        fprintf(stderr, "avformat_open_input (custom IO) failed\n");
        goto fail;
    }
} else {
    if (avformat_open_input(&p->fmt, url, NULL, &opts) != 0) {
        fprintf(stderr, "avformat_open_input failed for %s\n", url);
        goto fail;
    }
}
```

- [ ] **Step 3: Extend `player_close`.** Per spec §5.2: stop the prefetcher BEFORE `avformat_close_input` so libav's final flush doesn't block on a stale AVIOContext. Ordering:

```c
void player_close(player_t *p) {
    if (p->prefetch) {
        hls_prefetch_close(p->prefetch);
        p->prefetch = NULL;
    }
    if (p->fmt) avformat_close_input(&p->fmt);
    /* ... existing cleanup ... */
}
```

Also extend Makefile's main binary target: `SRC` is already `$(wildcard src/*.c)` so `src/hls_prefetch.c` is picked up automatically. Verify.

- [ ] **Step 4: Build the full app.**

```
./mk clean && ./mk
```

Zero warnings, zero errors. Binary produced.

- [ ] **Step 5: Run existing 74 unit tests — must still pass.**

```
./mk test
```

74 OK lines (plus any new prefetch-binary lines from Tasks 2-7 — that's fine, just verify the pre-existing ones still pass).

- [ ] **Step 6: Commit.**

```
player: route HLS live URLs through hls_prefetch

New player_t.prefetch field. player_open sniffs for .m3u8 URLs and
routes them through the prefetcher + custom AVIOContext instead of
direct libav open. Non-HLS paths (VOD, timeshift, direct URLs)
unchanged. Escape hatch: TV_DISABLE_PREFETCH=1 reverts to the
direct path for field debugging.

player_close tears down the prefetcher before avformat_close_input
so the AVIOContext's backing ring is still valid during libav's
final packet flush.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 9: Debug HUD extension for prefetch stats

**Files:** Modify `src/main.c`

- [ ] **Step 1:** Find the debug-mode snprintf block in main.c (introduced by commit `7e0831c` — look for `DEBUG  ch=`). Extend to include prefetch buffer depth + segments fetched + manifest refreshes when `pb->player.prefetch` is non-NULL.

Final format string (one long line; overlay auto-handles width):

```c
if (pb->player.prefetch) {
    hls_prefetch_stats_t ps;
    hls_prefetch_get_stats(pb->player.prefetch, &ps);
    snprintf(debug_buf, sizeof(debug_buf),
             "DEBUG  ch=%s  vq=%zu/%zu aq=%zu/%zu  dec_age=%ums  "
             "dec_done=%d  clk=%.1fs  buf=%.1fMB/%.1fMB  segs=%zu  "
             "manif=%zu/%zu",
             ch_name,
             pb->player.video_q.count, pb->player.video_q.capacity,
             pb->player.audio_q.count, pb->player.audio_q.capacity,
             dec_age, pb->player.decoder_done,
             av_clock_ready(&pb->clk) ? av_clock_now(&pb->clk) : 0.0,
             ps.bytes_buffered / 1048576.0, ps.bytes_capacity / 1048576.0,
             ps.segments_fetched,
             ps.manifest_refreshes - ps.manifest_errors,
             ps.manifest_refreshes);
} else {
    /* existing format without prefetch fields */
}
```

- [ ] **Step 2:** Build + smoke test (manual — `mk` + launch + press `d` on a live channel). Should show `buf=0.0MB/20.0MB  segs=0  manif=0/0` at first, climbing as the prefetcher does its work.

- [ ] **Step 3: Commit.**

```
main: extend debug HUD with prefetch stats

Adds buf=X.XMB/Y.YMB, segs=N, manif=OK/TOTAL when the current
playback is going through hls_prefetch. Buffer depth is the
single most important runtime health number — when it goes to
zero while manif errors climb, the portal is rate-limiting us
and we're living on borrowed time.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 10: Integration test harness

**Files:**
- Create: `tests/test_integration_prefetch.c`
- Modify: `Makefile`

- [ ] **Step 1: Build out a reusable mock HTTP server harness in `tests/test_integration_prefetch.c`.** It builds on the tiny server from Task 5 but adds scripting:

```c
typedef struct {
    int     serve_fail_after_n_manifests;   /* -1 = never */
    int     manifest_fail_code;              /* 509 by default */
    int     drop_connection_after_n_segs;    /* -1 = never */
    int     rotate_tokens;                   /* if 1, segments get per-request tokens
                                              * and stale tokens return 403 */
    /* Counters populated as the server serves */
    volatile int n_manifest_served;
    volatile int n_segments_served;
} mock_server_t;

mock_server_t *mock_server_start(void);        /* returns OS-assigned port */
int            mock_server_port(mock_server_t *);
void           mock_server_stop(mock_server_t *);
```

- [ ] **Step 2: Add Makefile target** for `test_integration_prefetch.exe`, linking the full set (`src/hls_prefetch.c`, `src/queue.c`, plus whatever libs the tests need).

```make
INTEG_TEST_SRC = tests/test_integration_prefetch.c src/hls_prefetch.c src/queue.c
INTEG_TEST_BIN = build/test_integration_prefetch.exe

$(INTEG_TEST_BIN): $(INTEG_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(INTEG_TEST_SRC) -o $@ $(LDLIBS)
```

Append `./$(INTEG_TEST_BIN)` to the `test` target.

- [ ] **Step 3: Commit (server only, no tests yet).**

```
tests: mock HTTP server harness for prefetch integration tests

Localhost HTTP server with scriptable failure injection: 509-after-N,
drop-after-N-segments, rotating-token support. Used by Task 11 tests
to verify prefetch survives portal-class failures without ever
returning EOF to libav.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 11: Integration test scenarios

**Files:** Modify `tests/test_integration_prefetch.c`

- [ ] **Step 1: Write 4 integration tests.**

1. **`test_steady_state_playback`** — server serves clean manifests + segments. Open prefetch, wait 5 s, verify `get_stats` shows bytes_buffered > 500 KB and no manifest errors.

2. **`test_survives_509_storm`** — server returns 509 on manifests #3 through #8, then recovers. Prefetch runs for 15 s. Verify:
   - Prefetcher retried with backoff (from `manifest_errors` count vs wall time).
   - Ring buffer never went to 0 during the storm (because segments were already in it).
   - After recovery, fetching resumes normally.

3. **`test_survives_dropped_segment_connection`** — server drops connection mid-segment once. Prefetch continues; next segment fetch succeeds. Log shows the drop, no cascade.

4. **`test_token_rotation`** — `rotate_tokens=1`: each manifest contains segment URLs with tokens that only work for that manifest's timestamp (server rejects old tokens with 403). Prefetch opens, runs for 20 s. Verify `segments_fetched >= 1`, `manifest_refreshes >= 3`, and no segment failures are logged (each token is used before its manifest expires).

- [ ] **Step 2: All tests pass.** Each test lasts 5-20 s wall time — `./mk test` total runtime will climb from ~1 s to ~30 s. Acceptable.

- [ ] **Step 3: Commit.**

```
tests: 4 integration scenarios against mock HTTP server

Covers the exact failure modes the real portal inflicts: 509 storm,
dropped mid-segment connection, rotating per-manifest tokens. Each
runs 5-20s against a localhost mock server with scripted failures.

Confirms the prefetcher keeps the ring buffer non-empty through all
of these, which is the core value proposition: the decoder doesn't
see any of the network chaos.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 12: Final review + merge + tag

- [ ] **Step 1: Run full test suite.** `./mk clean && ./mk test`. Every test binary passes. Count OK lines — should be 74 (existing) + 10 (ring) + 6 (parser) + 4 (segment queue) + 3 (segment fetcher) + 3 (thread lifecycle) + 3 (AVIO) + 4 (integration) = **107 tests**.

- [ ] **Step 2: Dispatch code-reviewer subagent** on the full `feat/prebuffer` diff against `main` (stable2 = cd04294). Must return READY-TO-MERGE or a list of fixes.

- [ ] **Step 3: Fix any review findings. Re-run tests.**

- [ ] **Step 4: Manual smoke test** (user's side, after merge): launch on the real portal, watch NPO 1 for 10 min, press `d` to see buffer depth stay above zero. Expected: no stutter, ever.

- [ ] **Step 5: Merge to main + tag stable3.**

```
git checkout main
git merge --no-ff feat/prebuffer
git tag -a stable3 -m "stable3 — pre-roll buffer + custom HLS refresh"
```

- [ ] **Step 6: Delete feat/prebuffer branch after confirming merge.**

---

## Coverage check

| Spec section | Task |
|---|---|
| §2 architecture | Task 8 ties it together |
| §3 decision tree | Task 8 |
| §4.1 public API | Task 1 (header), Tasks 6-7 (implementation) |
| §4.2 internal state | Task 1 (scaffold) + 2, 4, 6 (filled in) |
| §4.3 thread loop | Task 6 |
| §4.4 manifest parser | Task 3 |
| §4.5 segment fetcher | Task 5 |
| §4.6 ring buffer | Task 2 |
| §4.7 AVIO bridge | Task 7 |
| §5 player.c integration | Task 8 |
| §6 debug HUD | Task 9 |
| §7 watchdog interaction | Passive — watchdog unchanged |
| §8 failure modes table | Task 11 (integration tests cover them) |
| §9.1 unit tests | Tasks 2-7 |
| §9.2 integration tests | Tasks 10-11 |
| §10 rollback (env escape hatch) | Task 8 |
| §11 perf budget | Implicit — ring buffer + curl are inherently cheap |
