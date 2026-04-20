/*
 * Unit tests for hls_prefetch module — Task 2: ring buffer (10 tests).
 * Build: see Makefile target PREFETCH_TEST_BIN.
 */

#include "../src/hls_prefetch_internal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdint.h>

/* ---- helpers ------------------------------------------------------------ */

#define OK(name)  puts("OK " name)

/* ---- test 1 ------------------------------------------------------------- */

static void test_ring_new_has_capacity_zero_count(void) {
    ring_buf_t *r = ring_new(1024);
    assert(r != NULL);
    assert(ring_capacity(r) == 1024);
    assert(ring_count(r)    == 0);
    ring_free(r);
    OK("test_ring_new_has_capacity_zero_count");
}

/* ---- test 2 ------------------------------------------------------------- */

static void test_ring_write_read_round_trip(void) {
    ring_buf_t *r = ring_new(64);
    assert(r != NULL);

    const unsigned char src[4] = {0xDE, 0xAD, 0xBE, 0xEF};
    unsigned char       dst[4] = {0};

    int wrc = ring_write(r, src, 4);
    assert(wrc == 4);
    assert(ring_count(r) == 4);

    int rrc = ring_read(r, dst, 4, 1000);
    assert(rrc == 4);
    assert(memcmp(src, dst, 4) == 0);
    assert(ring_count(r) == 0);

    ring_free(r);
    OK("test_ring_write_read_round_trip");
}

/* ---- test 3 ------------------------------------------------------------- */

typedef struct {
    ring_buf_t    *r;
    int            result;   /* return value of ring_write */
} write_arg_t;

static void *writer_32(void *arg) {
    write_arg_t *a = arg;
    unsigned char buf[32];
    memset(buf, 0xAA, 32);
    a->result = ring_write(a->r, buf, 32);
    return NULL;
}

static void test_ring_write_beyond_capacity_blocks(void) {
    /* cap=16, write 32 → writer blocks; parent reads 16 → writer unblocks */
    ring_buf_t *r = ring_new(16);
    assert(r != NULL);

    write_arg_t arg = { r, 0 };
    pthread_t tid;
    pthread_create(&tid, NULL, writer_32, &arg);

    /* Give the writer a moment to block */
    struct timespec ts = {0, 20000000}; /* 20 ms */
    nanosleep(&ts, NULL);

    /* Read all 16 bytes — unblocks the writer to write the remaining 16 */
    unsigned char buf[32];
    int got = 0;
    while (got < 32) {
        int n = ring_read(r, buf + got, 32 - got, 2000);
        assert(n > 0);
        got += n;
    }

    pthread_join(tid, NULL);
    assert(arg.result == 32);
    ring_free(r);
    OK("test_ring_write_beyond_capacity_blocks");
}

/* ---- test 4 ------------------------------------------------------------- */

static void test_ring_read_empty_returns_zero_after_timeout(void) {
    ring_buf_t *r = ring_new(64);
    assert(r != NULL);

    unsigned char buf[8];
    int rc = ring_read(r, buf, 8, 50);   /* 50 ms timeout */
    assert(rc == 0);

    ring_free(r);
    OK("test_ring_read_empty_returns_zero_after_timeout");
}

/* ---- test 5 ------------------------------------------------------------- */

typedef struct {
    ring_buf_t *r;
    int         result;
} reader_arg_t;

static void *blocking_reader(void *arg) {
    reader_arg_t *a = arg;
    unsigned char buf[4];
    /* Use a long timeout; we expect ring_close to wake us */
    a->result = ring_read(a->r, buf, 4, 5000);
    return NULL;
}

static void test_ring_close_wakes_reader_with_zero(void) {
    ring_buf_t *r = ring_new(64);
    assert(r != NULL);

    reader_arg_t arg = { r, -99 };
    pthread_t tid;
    pthread_create(&tid, NULL, blocking_reader, &arg);

    /* Let the reader block */
    struct timespec ts = {0, 20000000};
    nanosleep(&ts, NULL);

    ring_close(r);
    pthread_join(tid, NULL);

    /* reader must have returned 0 (closed-empty) */
    assert(arg.result == 0);
    ring_free(r);
    OK("test_ring_close_wakes_reader_with_zero");
}

/* ---- test 6 ------------------------------------------------------------- */

typedef struct {
    ring_buf_t *r;
    int         result;
} writer_close_arg_t;

static void *blocking_writer(void *arg) {
    writer_close_arg_t *a = arg;
    /* Ring is full: write one more byte — should block then return -1 */
    unsigned char byte = 0x42;
    a->result = ring_write(a->r, &byte, 1);
    return NULL;
}

static void test_ring_close_wakes_writer_returns_error(void) {
    /* Fill the ring completely first */
    ring_buf_t *r = ring_new(8);
    assert(r != NULL);

    unsigned char fill[8];
    memset(fill, 0xFF, 8);
    int wrc = ring_write(r, fill, 8);
    assert(wrc == 8);

    /* Spawn a writer that will block (ring is full) */
    writer_close_arg_t arg = { r, 0 };
    pthread_t tid;
    pthread_create(&tid, NULL, blocking_writer, &arg);

    struct timespec ts = {0, 20000000};
    nanosleep(&ts, NULL);

    ring_close(r);
    pthread_join(tid, NULL);

    assert(arg.result == -1);
    ring_free(r);
    OK("test_ring_close_wakes_writer_returns_error");
}

/* ---- test 7 ------------------------------------------------------------- */

static void test_ring_wraps_around(void) {
    /* cap=16; write 12, read 10, write 10 — wrap guaranteed */
    ring_buf_t *r = ring_new(16);
    assert(r != NULL);

    unsigned char src[12], mid[10], ext[10], out[10];
    for (int i = 0; i < 12; i++) src[i] = (unsigned char)(i + 1);
    for (int i = 0; i < 10; i++) ext[i] = (unsigned char)(100 + i);

    int wrc = ring_write(r, src, 12);
    assert(wrc == 12);

    int rrc = ring_read(r, mid, 10, 1000);
    assert(rrc == 10);
    /* Verify first 10 bytes */
    for (int i = 0; i < 10; i++) assert(mid[i] == (unsigned char)(i + 1));

    /* Now write 10 more — tail will wrap */
    wrc = ring_write(r, ext, 10);
    assert(wrc == 10);

    /* Drain: should get remaining 2 from first write + 10 from second */
    unsigned char drain[12];
    int got = 0;
    while (got < 12) {
        int n = ring_read(r, drain + got, 12 - got, 1000);
        assert(n > 0);
        got += n;
    }
    /* bytes 11 and 12 from original write */
    assert(drain[0] == 11);
    assert(drain[1] == 12);
    /* then the 10 ext bytes */
    for (int i = 0; i < 10; i++) assert(drain[2 + i] == ext[i]);
    (void)out;

    ring_free(r);
    OK("test_ring_wraps_around");
}

/* ---- test 8 ------------------------------------------------------------- */

static void test_ring_count_after_partial_read(void) {
    ring_buf_t *r = ring_new(256);
    assert(r != NULL);

    unsigned char buf[100];
    memset(buf, 0x5A, 100);

    int wrc = ring_write(r, buf, 100);
    assert(wrc == 100);
    assert(ring_count(r) == 100);

    unsigned char out[50];
    int rrc = ring_read(r, out, 50, 1000);
    assert(rrc == 50);
    assert(ring_count(r) == 50);

    ring_free(r);
    OK("test_ring_count_after_partial_read");
}

/* ---- test 9 ------------------------------------------------------------- */

#define STRESS_TOTAL   (100 * 1024)   /* 100 KB */
#define WRITE_CHUNK    1024           /* 1 KB chunks */
#define READ_CHUNK     900            /* 900-byte chunks */

typedef struct {
    ring_buf_t *r;
    int         ok;
} stress_arg_t;

static void *stress_producer(void *arg) {
    stress_arg_t *a = arg;
    unsigned char buf[WRITE_CHUNK];
    for (int i = 0; i < STRESS_TOTAL; i += WRITE_CHUNK) {
        for (int j = 0; j < WRITE_CHUNK; j++)
            buf[j] = (unsigned char)((i + j) & 0xFF);
        int rc = ring_write(a->r, buf, WRITE_CHUNK);
        if (rc != WRITE_CHUNK) { a->ok = 0; return NULL; }
    }
    a->ok = 1;
    return NULL;
}

static void *stress_consumer(void *arg) {
    stress_arg_t *a = arg;
    unsigned char buf[READ_CHUNK];
    int total = 0;
    int expected_byte = 0;
    while (total < STRESS_TOTAL) {
        int want = READ_CHUNK;
        if (total + want > STRESS_TOTAL) want = STRESS_TOTAL - total;
        int rc = ring_read(a->r, buf, want, 5000);
        if (rc <= 0) { a->ok = 0; return NULL; }
        for (int i = 0; i < rc; i++) {
            if (buf[i] != (unsigned char)(expected_byte & 0xFF)) {
                a->ok = 0;
                return NULL;
            }
            expected_byte++;
        }
        total += rc;
    }
    a->ok = 1;
    return NULL;
}

static void test_ring_concurrent_stress(void) {
    ring_buf_t *r = ring_new(4096);
    assert(r != NULL);

    stress_arg_t pa = { r, -1 };
    stress_arg_t ca = { r, -1 };
    pthread_t pt, ct;
    pthread_create(&pt, NULL, stress_producer, &pa);
    pthread_create(&ct, NULL, stress_consumer, &ca);
    pthread_join(pt, NULL);
    pthread_join(ct, NULL);

    assert(pa.ok == 1);
    assert(ca.ok == 1);
    ring_free(r);
    OK("test_ring_concurrent_stress");
}

/* ---- test 10 ------------------------------------------------------------ */

static void test_ring_free_on_null_is_safe(void) {
    ring_free(NULL);   /* must not crash */
    OK("test_ring_free_on_null_is_safe");
}

/* ---- Task 3: manifest parser tests ---------------------------------------- */

/* Helper to load a fixture file from disk */
static char *load_fixture(const char *path, size_t *out_len) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (sz < 0) { fclose(f); return NULL; }
    char *buf = malloc(sz);
    if (!buf) { fclose(f); return NULL; }
    size_t nread = fread(buf, 1, sz, f);
    fclose(f);
    *out_len = nread;
    return buf;
}

/* ---- test 11 ------------------------------------------------------------ */

static void test_manifest_parse_valid(void) {
    size_t len = 0;
    char *text = load_fixture("tests/fixtures/manifest_valid.m3u8", &len);
    assert(text != NULL);

    manifest_t m;
    int rc = manifest_parse(text, len, "http://example.com/live/foo.m3u8", &m);
    assert(rc == 0);
    assert(m.n_segments == 2);
    assert(m.media_sequence == 448);
    assert(m.target_duration_ms == 12000);

    /* Check first segment URL resolves correctly:
     * Original: /hls/e1462bab13d1d09d852eae4a45fec5f0/755880_448.ts
     * Base: http://example.com/live/foo.m3u8
     * Expected: http://example.com/hls/.../755880_448.ts
     */
    assert(m.segments[0].url != NULL);
    assert(strstr(m.segments[0].url, "http://example.com") != NULL);
    assert(strstr(m.segments[0].url, "/hls/") != NULL);
    assert(strstr(m.segments[0].url, "755880_448.ts") != NULL);

    manifest_free(&m);
    free(text);
    OK("test_manifest_parse_valid");
}

/* ---- test 12 ------------------------------------------------------------ */

static void test_manifest_parse_empty(void) {
    size_t len = 0;
    char *text = load_fixture("tests/fixtures/manifest_empty.m3u8", &len);
    assert(text != NULL);

    manifest_t m;
    int rc = manifest_parse(text, len, "http://example.com/live/foo.m3u8", &m);
    assert(rc == 0);
    assert(m.n_segments == 0);
    assert(m.target_duration_ms == 10000);

    manifest_free(&m);
    free(text);
    OK("test_manifest_parse_empty");
}

/* ---- test 13 ------------------------------------------------------------ */

static void test_manifest_parse_rejects_non_m3u(void) {
    size_t len = 0;
    char *text = load_fixture("tests/fixtures/manifest_malformed.m3u8", &len);
    assert(text != NULL);

    manifest_t m;
    int rc = manifest_parse(text, len, "http://example.com/live/foo.m3u8", &m);
    assert(rc == -1);  /* must reject */

    manifest_free(&m);
    free(text);
    OK("test_manifest_parse_rejects_non_m3u");
}

/* ---- test 14 ------------------------------------------------------------ */

static void test_manifest_parse_rejects_null_text(void) {
    manifest_t m;
    int rc = manifest_parse(NULL, 0, NULL, &m);
    assert(rc == -1);

    rc = manifest_parse("test", 4, NULL, NULL);
    assert(rc == -1);

    OK("test_manifest_parse_rejects_null_text");
}

/* ---- test 15 ------------------------------------------------------------ */

static void test_manifest_parse_absolute_url_preserved(void) {
    /* Manifest with absolute segment URL */
    const char *text =
        "#EXTM3U\n"
        "#EXT-X-MEDIA-SEQUENCE:1\n"
        "#EXT-X-TARGETDURATION:10\n"
        "#EXTINF:10.0,\n"
        "http://cdn.example.com/segments/seg-001.ts\n";

    manifest_t m;
    int rc = manifest_parse(text, strlen(text),
                           "http://example.com/live/foo.m3u8", &m);
    assert(rc == 0);
    assert(m.n_segments == 1);
    assert(m.segments[0].url != NULL);
    assert(strcmp(m.segments[0].url, "http://cdn.example.com/segments/seg-001.ts") == 0);

    manifest_free(&m);
    OK("test_manifest_parse_absolute_url_preserved");
}

/* ---- test 16 ------------------------------------------------------------ */

static void test_manifest_parse_sequence_without_duration_defaults(void) {
    /* Manifest without #EXT-X-TARGETDURATION */
    const char *text =
        "#EXTM3U\n"
        "#EXT-X-MEDIA-SEQUENCE:0\n"
        "#EXTINF:5.0,\n"
        "/seg1.ts\n";

    manifest_t m;
    int rc = manifest_parse(text, strlen(text),
                           "http://example.com/live/foo.m3u8", &m);
    assert(rc == 0);
    assert(m.target_duration_ms == 10000);  /* HLS spec default */
    assert(m.n_segments == 1);

    manifest_free(&m);
    OK("test_manifest_parse_sequence_without_duration_defaults");
}

/* ---- Task 4: segment queue tests ----------------------------------- */

/* ---- test 17 ------------------------------------------------------------ */

static void test_segments_enqueued_in_sequence_order(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    /* Create a manifest with segments at sequence 448, 449, 450 */
    manifest_t m;
    memset(&m, 0, sizeof(m));
    m.n_segments = 3;
    m.media_sequence = 448;
    m.target_duration_ms = 12000;
    m.segments = malloc(3 * sizeof(hls_segment_t));
    assert(m.segments != NULL);

    for (int i = 0; i < 3; i++) {
        char url_buf[64];
        snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", 448 + i);
        m.segments[i].url = malloc(strlen(url_buf) + 1);
        strcpy(m.segments[i].url, url_buf);
        m.segments[i].sequence = 448 + i;
        m.segments[i].fetched = 0;
    }

    int enqueued = _pf_enqueue_new_segments_for_test(pf, &m);
    assert(enqueued == 3);
    assert(_pf_segment_count_for_test(pf) == 3);

    /* Verify order: seq 448, 449, 450 */
    char buf[64];
    for (int i = 0; i < 3; i++) {
        int rc = _pf_get_segment_url_for_test(pf, i, buf, sizeof(buf));
        assert(rc == 0);
        int expected_seq = 448 + i;
        char expected_suffix[16];
        snprintf(expected_suffix, sizeof(expected_suffix), "seg_%d.ts", expected_seq);
        assert(strstr(buf, expected_suffix) != NULL);
    }

    manifest_free(&m);
    _pf_free_for_test(pf);
    OK("test_segments_enqueued_in_sequence_order");
}

/* ---- test 18 ------------------------------------------------------------ */

static void test_segments_dedup_on_subsequent_manifest(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    /* First manifest: seq 448, 449, 450 */
    manifest_t m1;
    memset(&m1, 0, sizeof(m1));
    m1.n_segments = 3;
    m1.media_sequence = 448;
    m1.target_duration_ms = 12000;
    m1.segments = malloc(3 * sizeof(hls_segment_t));
    for (int i = 0; i < 3; i++) {
        char url_buf[64];
        snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", 448 + i);
        m1.segments[i].url = malloc(strlen(url_buf) + 1);
        strcpy(m1.segments[i].url, url_buf);
        m1.segments[i].sequence = 448 + i;
        m1.segments[i].fetched = 0;
    }

    int enqueued1 = _pf_enqueue_new_segments_for_test(pf, &m1);
    assert(enqueued1 == 3);
    assert(_pf_segment_count_for_test(pf) == 3);

    /* Second manifest: seq 449, 450, 451 (so 451 is new) */
    manifest_t m2;
    memset(&m2, 0, sizeof(m2));
    m2.n_segments = 3;
    m2.media_sequence = 449;
    m2.target_duration_ms = 12000;
    m2.segments = malloc(3 * sizeof(hls_segment_t));
    for (int i = 0; i < 3; i++) {
        char url_buf[64];
        snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", 449 + i);
        m2.segments[i].url = malloc(strlen(url_buf) + 1);
        strcpy(m2.segments[i].url, url_buf);
        m2.segments[i].sequence = 449 + i;
        m2.segments[i].fetched = 0;
    }

    int enqueued2 = _pf_enqueue_new_segments_for_test(pf, &m2);
    /* Should only enqueue 451 (449, 450 already seen) */
    assert(enqueued2 == 1);
    assert(_pf_segment_count_for_test(pf) == 4);  /* 448, 449, 450, 451 */

    /* Verify the new segment is 451 */
    char buf[64];
    int rc = _pf_get_segment_url_for_test(pf, 3, buf, sizeof(buf));
    assert(rc == 0);
    assert(strstr(buf, "seg_451.ts") != NULL);

    manifest_free(&m1);
    manifest_free(&m2);
    _pf_free_for_test(pf);
    OK("test_segments_dedup_on_subsequent_manifest");
}

/* ---- test 19 ------------------------------------------------------------ */

static void test_segments_skip_stale(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    /* First manifest: seq 500-502 (establish highest-seen = 502) */
    manifest_t m1;
    memset(&m1, 0, sizeof(m1));
    m1.n_segments = 3;
    m1.media_sequence = 500;
    m1.target_duration_ms = 12000;
    m1.segments = malloc(3 * sizeof(hls_segment_t));
    for (int i = 0; i < 3; i++) {
        char url_buf[64];
        snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", 500 + i);
        m1.segments[i].url = malloc(strlen(url_buf) + 1);
        strcpy(m1.segments[i].url, url_buf);
        m1.segments[i].sequence = 500 + i;
        m1.segments[i].fetched = 0;
    }

    int enqueued1 = _pf_enqueue_new_segments_for_test(pf, &m1);
    assert(enqueued1 == 3);

    /* Second manifest: seq 448-450 (all stale, below current highest) */
    manifest_t m2;
    memset(&m2, 0, sizeof(m2));
    m2.n_segments = 3;
    m2.media_sequence = 448;
    m2.target_duration_ms = 12000;
    m2.segments = malloc(3 * sizeof(hls_segment_t));
    for (int i = 0; i < 3; i++) {
        char url_buf[64];
        snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", 448 + i);
        m2.segments[i].url = malloc(strlen(url_buf) + 1);
        strcpy(m2.segments[i].url, url_buf);
        m2.segments[i].sequence = 448 + i;
        m2.segments[i].fetched = 0;
    }

    int enqueued2 = _pf_enqueue_new_segments_for_test(pf, &m2);
    /* Should enqueue nothing (all below 502) */
    assert(enqueued2 == 0);
    assert(_pf_segment_count_for_test(pf) == 3);  /* unchanged */

    manifest_free(&m1);
    manifest_free(&m2);
    _pf_free_for_test(pf);
    OK("test_segments_skip_stale");
}

/* ---- test 20 ------------------------------------------------------------ */

static void test_segments_queue_cap(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    /* Push 20 segments (4 batches of 5) — queue should cap at 16 */
    for (int batch = 0; batch < 4; batch++) {
        manifest_t m;
        memset(&m, 0, sizeof(m));
        m.n_segments = 5;
        m.media_sequence = 1 + batch * 5;
        m.target_duration_ms = 12000;
        m.segments = malloc(5 * sizeof(hls_segment_t));

        for (int i = 0; i < 5; i++) {
            int seq = (1 + batch * 5) + i;
            char url_buf[64];
            snprintf(url_buf, sizeof(url_buf), "http://example.com/seg_%d.ts", seq);
            m.segments[i].url = malloc(strlen(url_buf) + 1);
            strcpy(m.segments[i].url, url_buf);
            m.segments[i].sequence = seq;
            m.segments[i].fetched = 0;
        }

        int enqueued = _pf_enqueue_new_segments_for_test(pf, &m);
        manifest_free(&m);

        size_t count = _pf_segment_count_for_test(pf);
        /* After batch 0: 5 segs. After batch 1: 10. After batch 2: 15.
         * After batch 3: cap at 16 (oldest drop) */
        if (batch < 3) {
            assert(count == (batch + 1) * 5);
        } else {
            assert(count == 16);  /* Capped */
        }
    }

    _pf_free_for_test(pf);
    OK("test_segments_queue_cap");
}

/* ---- main --------------------------------------------------------------- */

int main(void) {
    /* Task 1 scaffold sentinel — confirms the test binary links correctly */
    puts("OK stub_links");

    /* Task 2: ring buffer */
    test_ring_new_has_capacity_zero_count();
    test_ring_write_read_round_trip();
    test_ring_write_beyond_capacity_blocks();
    test_ring_read_empty_returns_zero_after_timeout();
    test_ring_close_wakes_reader_with_zero();
    test_ring_close_wakes_writer_returns_error();
    test_ring_wraps_around();
    test_ring_count_after_partial_read();
    test_ring_concurrent_stress();
    test_ring_free_on_null_is_safe();

    /* Task 3: manifest parser */
    test_manifest_parse_valid();
    test_manifest_parse_empty();
    test_manifest_parse_rejects_non_m3u();
    test_manifest_parse_rejects_null_text();
    test_manifest_parse_absolute_url_preserved();
    test_manifest_parse_sequence_without_duration_defaults();

    /* Task 4: segment queue */
    test_segments_enqueued_in_sequence_order();
    test_segments_dedup_on_subsequent_manifest();
    test_segments_skip_stale();
    test_segments_queue_cap();

    return 0;
}
