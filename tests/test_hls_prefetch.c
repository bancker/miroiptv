/*
 * Unit tests for hls_prefetch module — Task 2: ring buffer (10 tests).
 * Build: see Makefile target PREFETCH_TEST_BIN.
 */

#include "../src/hls_prefetch_internal.h"
#include <assert.h>
#include <stdio.h>
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
    return 0;
}
