/*
 * Unit tests for hls_prefetch module — Tasks 2-7.
 * Build: see Makefile target PREFETCH_TEST_BIN.
 */

#include "../src/hls_prefetch_internal.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdint.h>
#include <libavformat/avformat.h>
#include <libavformat/avio.h>

/* Winsock2 for the test HTTP server (MinGW / Windows). */
#include <winsock2.h>
#include <ws2tcpip.h>

/* ---- helpers ------------------------------------------------------------ */

#define OK(name)  puts("OK " name)

/* Sleep for ms milliseconds (matches hls_prefetch.c's msleep; used in tests). */
static void msleep(int ms) {
    if (ms <= 0) return;
    struct timespec ts;
    ts.tv_sec  = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

/* =========================================================================
 * Minimal test HTTP server (Task 5)
 *
 * Listens on 127.0.0.1:0 (OS-assigned port) in a background pthread.
 * Supports a small route table: register a path → response callback.
 * Used by Tasks 5, 10, 11.
 * ========================================================================= */

#define TEST_SERVER_MAX_ROUTES 16

/* A route: path (e.g. "/seg.ts") → handler that writes the full HTTP
 * response into the accepted socket and returns. */
typedef void (*route_handler_t)(SOCKET client_sock, void *userdata);

typedef struct {
    const char      *path;
    route_handler_t  handler;
    void            *userdata;
} route_t;

typedef struct {
    SOCKET    listen_sock;
    int       port;          /* OS-assigned, set after bind */
    volatile int stop;       /* set 1 to signal shutdown */

    route_t   routes[TEST_SERVER_MAX_ROUTES];
    int       n_routes;

    pthread_t thread;
} test_server_t;

/* Send a minimal HTTP/1.0 response. */
static void server_send_response(SOCKET s, int status,
                                 const char *status_text,
                                 const unsigned char *body, size_t body_len) {
    char hdr[256];
    int hlen = snprintf(hdr, sizeof(hdr),
                        "HTTP/1.0 %d %s\r\n"
                        "Content-Length: %zu\r\n"
                        "Connection: close\r\n"
                        "\r\n",
                        status, status_text, body_len);
    send(s, hdr, hlen, 0);
    if (body && body_len > 0) {
        size_t sent = 0;
        while (sent < body_len) {
            int n = (int)send(s, (const char *)body + sent,
                              (int)(body_len - sent), 0);
            if (n <= 0) break;
            sent += (size_t)n;
        }
    }
}

static void *server_thread(void *arg) {
    test_server_t *srv = (test_server_t *)arg;

    while (!srv->stop) {
        /* Use select with a short timeout so we can check srv->stop */
        fd_set rset;
        FD_ZERO(&rset);
        FD_SET(srv->listen_sock, &rset);
        struct timeval tv = { 0, 100000 }; /* 100 ms */
        int sel = select((int)srv->listen_sock + 1, &rset, NULL, NULL, &tv);
        if (sel <= 0) continue;

        struct sockaddr_in caddr;
        int caddrlen = sizeof(caddr);
        SOCKET client = accept(srv->listen_sock,
                               (struct sockaddr *)&caddr, &caddrlen);
        if (client == INVALID_SOCKET) continue;

        /* Read request line (up to 1 KB) */
        char req[1024];
        int nread = recv(client, req, sizeof(req) - 1, 0);
        if (nread <= 0) { closesocket(client); continue; }
        req[nread] = '\0';

        /* Extract the path from "GET /path HTTP/1.x" */
        char path[512] = "/";
        sscanf(req, "%*s %511s", path);

        /* Find matching route */
        int matched = 0;
        for (int i = 0; i < srv->n_routes; i++) {
            if (strcmp(srv->routes[i].path, path) == 0) {
                srv->routes[i].handler(client, srv->routes[i].userdata);
                matched = 1;
                break;
            }
        }
        if (!matched) {
            server_send_response(client, 404, "Not Found", NULL, 0);
        }

        closesocket(client);
    }
    return NULL;
}

/* Start the server. Returns a heap-allocated test_server_t with port set.
 * Caller must call test_server_stop() when done. */
static test_server_t *test_server_start(void) {
    /* WSAStartup — idempotent, safe to call multiple times */
    WSADATA wsa;
    WSAStartup(MAKEWORD(2, 2), &wsa);

    test_server_t *srv = calloc(1, sizeof(*srv));
    if (!srv) return NULL;

    srv->listen_sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (srv->listen_sock == INVALID_SOCKET) { free(srv); return NULL; }

    /* Allow rapid reuse in tests */
    int yes = 1;
    setsockopt(srv->listen_sock, SOL_SOCKET, SO_REUSEADDR,
               (const char *)&yes, sizeof(yes));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family      = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port        = 0;  /* OS assigns */

    if (bind(srv->listen_sock, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        closesocket(srv->listen_sock); free(srv); return NULL;
    }
    if (listen(srv->listen_sock, 8) != 0) {
        closesocket(srv->listen_sock); free(srv); return NULL;
    }

    /* Retrieve the OS-assigned port */
    int alen = sizeof(addr);
    getsockname(srv->listen_sock, (struct sockaddr *)&addr, &alen);
    srv->port = ntohs(addr.sin_port);

    pthread_create(&srv->thread, NULL, server_thread, srv);
    return srv;
}

/* Register a route. Must be called before any request arrives (single-threaded
 * setup phase). */
static void test_server_add_route(test_server_t *srv, const char *path,
                                  route_handler_t handler, void *userdata) {
    assert(srv->n_routes < TEST_SERVER_MAX_ROUTES);
    srv->routes[srv->n_routes].path     = path;
    srv->routes[srv->n_routes].handler  = handler;
    srv->routes[srv->n_routes].userdata = userdata;
    srv->n_routes++;
}

/* Stop the server pthread cleanly and free. */
static void test_server_stop(test_server_t *srv) {
    if (!srv) return;
    srv->stop = 1;
    pthread_join(srv->thread, NULL);
    closesocket(srv->listen_sock);
    free(srv);
}

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

        (void)_pf_enqueue_new_segments_for_test(pf, &m);
        manifest_free(&m);

        size_t count = _pf_segment_count_for_test(pf);
        /* After batch 0: 5 segs. After batch 1: 10. After batch 2: 15.
         * After batch 3: cap at 16 (oldest drop) */
        if (batch < 3) {
            assert(count == (size_t)(batch + 1) * 5);
        } else {
            assert(count == 16);  /* Capped */
        }
    }

    _pf_free_for_test(pf);
    OK("test_segments_queue_cap");
}

/* =========================================================================
 * Task 5: segment fetcher tests
 * ========================================================================= */

/* Route handler: respond 200 + a 4096-byte body */
#define SEG_BODY_SIZE 4096
static unsigned char g_seg_body[SEG_BODY_SIZE];

static void handler_200_4k(SOCKET s, void *userdata) {
    (void)userdata;
    server_send_response(s, 200, "OK", g_seg_body, SEG_BODY_SIZE);
}

/* Route handler: respond 404 */
static void handler_404(SOCKET s, void *userdata) {
    (void)userdata;
    server_send_response(s, 404, "Not Found", NULL, 0);
}

/* Route handler: close socket immediately without sending anything */
static void handler_drop_connection(SOCKET s, void *userdata) {
    (void)userdata;
    /* Just return — the caller will closesocket(client) immediately,
     * so curl sees a connection reset before any data arrives. */
    (void)s;
}

/* ---- test 21 ------------------------------------------------------------ */

static void test_fetch_segment_basic(void) {
    /* Fill body with a recognisable pattern */
    for (int i = 0; i < SEG_BODY_SIZE; i++)
        g_seg_body[i] = (unsigned char)(i & 0xFF);

    test_server_t *srv = test_server_start();
    assert(srv != NULL);
    test_server_add_route(srv, "/seg.ts", handler_200_4k, NULL);

    /* Build URL */
    char url[64];
    snprintf(url, sizeof(url), "http://127.0.0.1:%d/seg.ts", srv->port);

    ring_buf_t *r = ring_new(SEG_BODY_SIZE + 1024);
    assert(r != NULL);

    int rc = _pf_fetch_segment_for_test(url, r);
    assert(rc == 0);
    assert(ring_count(r) == SEG_BODY_SIZE);

    test_server_stop(srv);
    ring_free(r);
    OK("test_fetch_segment_basic");
}

/* ---- test 22 ------------------------------------------------------------ */

static void test_fetch_segment_404_fails(void) {
    test_server_t *srv = test_server_start();
    assert(srv != NULL);
    test_server_add_route(srv, "/seg.ts", handler_404, NULL);

    char url[64];
    snprintf(url, sizeof(url), "http://127.0.0.1:%d/seg.ts", srv->port);

    ring_buf_t *r = ring_new(8192);
    assert(r != NULL);

    int rc = _pf_fetch_segment_for_test(url, r);
    assert(rc == -1);

    test_server_stop(srv);
    ring_free(r);
    OK("test_fetch_segment_404_fails");
}

/* ---- test 23 ------------------------------------------------------------ */

static void test_fetch_segment_drops_connection_fails(void) {
    test_server_t *srv = test_server_start();
    assert(srv != NULL);
    test_server_add_route(srv, "/seg.ts", handler_drop_connection, NULL);

    char url[64];
    snprintf(url, sizeof(url), "http://127.0.0.1:%d/seg.ts", srv->port);

    ring_buf_t *r = ring_new(8192);
    assert(r != NULL);

    int rc = _pf_fetch_segment_for_test(url, r);
    assert(rc == -1);

    test_server_stop(srv);
    ring_free(r);
    OK("test_fetch_segment_drops_connection_fails");
}

/* =========================================================================
 * Task 6: prefetcher thread lifecycle tests
 *
 * Note: plan listed 3 tests but test 3 (stats_populated) is identical to
 * test 1 (open_close already asserts all the same stats fields).  Collapsed
 * to 2 distinct tests per plan's own "collapse if #3 duplicates #1" note.
 * ========================================================================= */

/* Manifest served by the test server for Task 6.
 * Two segments at sequence 1 and 2.  URLs are absolute so the parser
 * doesn't need a meaningful base URL from the manifest URL itself. */
#define T6_MANIFEST_TEMPLATE \
    "#EXTM3U\r\n" \
    "#EXT-X-VERSION:3\r\n" \
    "#EXT-X-MEDIA-SEQUENCE:1\r\n" \
    "#EXT-X-TARGETDURATION:4\r\n" \
    "#EXTINF:4.0,\r\n" \
    "http://127.0.0.1:%d/seg1.ts\r\n" \
    "#EXTINF:4.0,\r\n" \
    "http://127.0.0.1:%d/seg2.ts\r\n"

/* Small segment body — 512 bytes is enough */
#define T6_SEG_SIZE 512

static unsigned char g_t6_seg_body[T6_SEG_SIZE];

/* Route handler: serve the manifest (port substituted at test startup) */
typedef struct { int port; } t6_manifest_userdata_t;

static void handler_t6_manifest(SOCKET s, void *userdata) {
    t6_manifest_userdata_t *u = (t6_manifest_userdata_t *)userdata;
    char body[512];
    int body_len = snprintf(body, sizeof(body), T6_MANIFEST_TEMPLATE,
                            u->port, u->port);
    server_send_response(s, 200, "OK",
                         (const unsigned char *)body, (size_t)body_len);
}

static void handler_t6_seg(SOCKET s, void *userdata) {
    (void)userdata;
    server_send_response(s, 200, "OK", g_t6_seg_body, T6_SEG_SIZE);
}

/* ---- test 24 (task 6, test 1+3 combined) -------------------------------- */

static void test_prefetch_open_close(void) {
    /* Fill segment bodies with a recognisable pattern */
    for (int i = 0; i < T6_SEG_SIZE; i++)
        g_t6_seg_body[i] = (unsigned char)(i & 0xFF);

    test_server_t *srv = test_server_start();
    assert(srv != NULL);

    /* Userdata shares the server port for URL substitution in the manifest */
    t6_manifest_userdata_t udata = { srv->port };
    test_server_add_route(srv, "/live.m3u8", handler_t6_manifest, &udata);
    test_server_add_route(srv, "/seg1.ts",   handler_t6_seg,       NULL);
    test_server_add_route(srv, "/seg2.ts",   handler_t6_seg,       NULL);

    /* Use a small ring so we don't malloc 20 MB in a unit test */
    char ring_env[32];
    snprintf(ring_env, sizeof(ring_env), "%d", 256 * 1024);  /* 256 KB */
#ifdef _WIN32
    _putenv_s("TV_PREBUFFER_BYTES", ring_env);
#else
    setenv("TV_PREBUFFER_BYTES", ring_env, 1);
#endif

    char manifest_url[128];
    snprintf(manifest_url, sizeof(manifest_url),
             "http://127.0.0.1:%d/live.m3u8", srv->port);

    hls_prefetch_t *pf = hls_prefetch_open(manifest_url);
    assert(pf != NULL);

    /* Let the prefetcher run for 500 ms — enough for at least one manifest
     * refresh and one segment fetch against the localhost test server */
    msleep(500);

    /* Snapshot stats before closing */
    hls_prefetch_stats_t st;
    hls_prefetch_get_stats(pf, &st);

    hls_prefetch_close(pf);
    test_server_stop(srv);

    /* The stats assertions (covers plan's test 3 as well) */
    assert(st.manifest_refreshes >= 1);
    assert(st.segments_fetched   >= 1);
    assert(st.last_refresh_ms    != 0);
    assert(st.bytes_capacity     == 256 * 1024);

    /* Reset env for subsequent tests */
#ifdef _WIN32
    _putenv_s("TV_PREBUFFER_BYTES", "");
#else
    unsetenv("TV_PREBUFFER_BYTES");
#endif

    OK("test_prefetch_open_close");
}

/* ---- test 25 (task 6, test 2) ------------------------------------------ */

static void test_prefetch_open_invalid_url(void) {
    /* Port 1 on loopback — reserved; no OS process will ever listen there.
     * With pre-flight enabled, hls_prefetch_open now returns NULL fast
     * (within the 1-second connect timeout) rather than spawning a thread
     * that retries forever. */

    /* Small ring again */
    char ring_env[32];
    snprintf(ring_env, sizeof(ring_env), "%d", 64 * 1024);
#ifdef _WIN32
    _putenv_s("TV_PREBUFFER_BYTES", ring_env);
#else
    setenv("TV_PREBUFFER_BYTES", ring_env, 1);
#endif

    hls_prefetch_t *pf = hls_prefetch_open("http://127.0.0.1:1/fake.m3u8");
    assert(pf == NULL);   /* pre-flight fails fast on unreachable port */

#ifdef _WIN32
    _putenv_s("TV_PREBUFFER_BYTES", "");
#else
    unsetenv("TV_PREBUFFER_BYTES");
#endif

    OK("test_prefetch_open_invalid_url");
}

/* =========================================================================
 * Task 7: AVIO bridge tests
 * ========================================================================= */

/* ---- test 26: read callback returns ring bytes --------------------------- */

static void test_avio_read_returns_ring_bytes(void) {
    /* Build a minimal pf with a ring — no background thread needed. */
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    /* Give it a ring large enough to hold 4096 bytes */
    ring_buf_t *ring = ring_new(8192);
    assert(ring != NULL);
    _pf_set_ring_for_test(pf, ring);

    /* Push exactly 4096 known bytes */
    unsigned char src[4096];
    for (int i = 0; i < 4096; i++) src[i] = (unsigned char)(i & 0xFF);
    int wrc = ring_write(ring, src, 4096);
    assert(wrc == 4096);

    /* Read via the AVIO callback helper */
    unsigned char dst[4096];
    int got = _pf_avio_read_for_test(pf, dst, 4096);
    assert(got == 4096);
    assert(memcmp(src, dst, 4096) == 0);

    /* _pf_free_for_test frees the ring we attached above */
    _pf_free_for_test(pf);
    OK("test_avio_read_returns_ring_bytes");
}

/* ---- test 27: read on empty closed ring returns 0 (-> EOF) -------------- */

static void test_avio_read_on_empty_ring_eof(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    ring_buf_t *ring = ring_new(4096);
    assert(ring != NULL);
    _pf_set_ring_for_test(pf, ring);

    /* Close the ring without writing anything */
    ring_close(ring);

    unsigned char buf[256];
    int got = _pf_avio_read_for_test(pf, buf, 256);
    /* ring_read on a closed-empty ring returns 0;
     * returning 0 from read_packet causes libav to treat it as EOF. */
    assert(got == 0);

    /* _pf_free_for_test frees the ring */
    _pf_free_for_test(pf);
    OK("test_avio_read_on_empty_ring_eof");
}

/* ---- test 28: attach sets fmt->pb and AVFMT_FLAG_CUSTOM_IO -------------- */

static void test_avio_attach_sets_fmt_pb(void) {
    hls_prefetch_t *pf = _pf_new_for_test();
    assert(pf != NULL);

    ring_buf_t *ring = ring_new(4096);
    assert(ring != NULL);
    _pf_set_ring_for_test(pf, ring);

    AVFormatContext *fmt = avformat_alloc_context();
    assert(fmt != NULL);
    assert(fmt->pb == NULL);  /* freshly allocated — no pb yet */

    int rc = hls_prefetch_attach(pf, fmt);
    assert(rc == 0);

    AVIOContext *avio = (AVIOContext *)_pf_get_avio_for_test(pf);
    assert(avio != NULL);
    assert(fmt->pb == avio);
    assert(fmt->flags & AVFMT_FLAG_CUSTOM_IO);
    assert(avio->seekable == 0);

    /* Teardown: null out fmt->pb first so avformat_free_context doesn't
     * double-free the AVIOContext owned by pf.
     * Then free avio explicitly (avio_context_free also frees avio_read_buf).
     * _pf_free_for_test frees the ring and the pf shell. */
    fmt->pb = NULL;
    avformat_free_context(fmt);

    avio_context_free(&avio);
    /* ring is freed by _pf_free_for_test below */
    _pf_free_for_test(pf);

    OK("test_avio_attach_sets_fmt_pb");
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

    /* Task 5: segment fetcher + test HTTP server */
    test_fetch_segment_basic();
    test_fetch_segment_404_fails();
    test_fetch_segment_drops_connection_fails();

    /* Task 6: prefetcher thread lifecycle (2 tests, plan's test 3 collapsed
     * into test 1 as it asserts the same stats fields) */
    test_prefetch_open_close();
    test_prefetch_open_invalid_url();

    /* Task 7: AVIO bridge */
    test_avio_read_returns_ring_bytes();
    test_avio_read_on_empty_ring_eof();
    test_avio_attach_sets_fmt_pb();

    return 0;
}
