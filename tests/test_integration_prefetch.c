/*
 * Integration test harness for hls_prefetch — Tasks 10 and 11.
 *
 * Provides a mock HTTP server with scripted failure injection:
 *   - 509-after-N-manifests (permanent threshold)
 *   - remaining_manifest_failures (windowed: next N requests fail)
 *   - drop-connection on the Nth segment request
 *   - remaining_segment_drops (windowed: next N segment drops)
 *   - rotating per-manifest tokens (stale tokens → 403)
 *
 * Task 10 main() just verifies the server starts and stops cleanly.
 * Task 11 adds 4 scenario tests.
 */

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>

/* Winsock2 for POSIX-style sockets on MinGW / Windows. */
#include <winsock2.h>
#include <ws2tcpip.h>

#include "../src/hls_prefetch.h"

/* =========================================================================
 * Mock HTTP server
 * ========================================================================= */

/*
 * Token-rotation note:
 *   When rotate_tokens is non-zero, each manifest response includes
 *   segment URLs of the form  /seg/<N>?t=<TOKEN>  where TOKEN equals
 *   the manifest-served count at the time that manifest was issued.
 *   The segment handler compares the token in the URL against
 *   `current_token` (the latest issued token) and returns 403 if they
 *   differ.  In practice the prefetcher fetches segments with the URLs
 *   it got from the latest manifest, so tokens should always match
 *   unless the prefetcher uses stale URLs from an earlier manifest.
 */

typedef struct mock_server {
    /* ---- configuration (set before / during the run) ---- */
    int  serve_fail_after_n_manifests;  /* -1 = never (permanent threshold) */
    int  manifest_fail_code;            /* e.g. 509 */
    int  drop_connection_after_n_segs;  /* -1 = never (permanent threshold) */
    int  rotate_tokens;                 /* 0 = off, 1 = on */

    /* ---- windowed failure injection (set mid-test via inject API) ---- */
    volatile int remaining_manifest_failures;  /* countdown: next N manifest
                                                * requests get fail_code */
    volatile int remaining_segment_drops;      /* countdown: next N segment
                                                * requests get connection drop */

    /* ---- stats (read after scenario) ---- */
    volatile int n_manifests_served;
    volatile int n_segments_served;

    /* ---- internals ---- */
    SOCKET      listen_sock;
    int         port;
    volatile int stop;
    pthread_t   thread;

    /* Current token: equals n_manifests_served at time of last manifest
     * response.  Segments must present this token when rotate_tokens=1. */
    volatile int current_token;
} mock_server_t;

/* --------------------------------------------------------------------------
 * Internal helpers
 * -------------------------------------------------------------------------- */

static void ms_send_raw(SOCKET s, const char *buf, int len) {
    int sent = 0;
    while (sent < len) {
        int n = (int)send(s, buf + sent, len - sent, 0);
        if (n <= 0) break;
        sent += n;
    }
}

static void ms_send_response(SOCKET s, int status, const char *status_text,
                              const unsigned char *body, size_t body_len) {
    char hdr[256];
    int hlen = snprintf(hdr, sizeof(hdr),
                        "HTTP/1.0 %d %s\r\n"
                        "Content-Length: %zu\r\n"
                        "Connection: close\r\n"
                        "\r\n",
                        status, status_text, body_len);
    ms_send_raw(s, hdr, hlen);
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

/* Build a manifest with 2 segments.  When rotate_tokens is set the segment
 * URLs include ?t=TOKEN where TOKEN is the value of current_token *after*
 * incrementing it (i.e. the token the client must present for these segs). */
static void ms_handle_manifest(SOCKET s, mock_server_t *ms) {
    int n = ms->n_manifests_served; /* read before deciding */

    /* Windowed failure injection: remaining_manifest_failures takes precedence
     * over the permanent serve_fail_after_n_manifests threshold. */
    if (ms->remaining_manifest_failures > 0) {
        ms->remaining_manifest_failures--;
        ms_send_response(s, ms->manifest_fail_code, "Limit Exceeded", NULL, 0);
        return;
    }

    /* Permanent failure injection: return manifest_fail_code after N serves */
    if (ms->serve_fail_after_n_manifests >= 0 &&
        n >= ms->serve_fail_after_n_manifests) {
        ms_send_response(s, ms->manifest_fail_code, "Limit Exceeded", NULL, 0);
        return;
    }

    /* Update token *before* building the manifest so the URLs in the
     * manifest carry the token that is current when this manifest is issued. */
    ms->current_token = n + 1;  /* token = 1-based manifest count */
    ms->n_manifests_served++;

    char body[512];
    int blen;
    if (ms->rotate_tokens) {
        blen = snprintf(body, sizeof(body),
            "#EXTM3U\r\n"
            "#EXT-X-VERSION:3\r\n"
            "#EXT-X-MEDIA-SEQUENCE:%d\r\n"
            "#EXT-X-TARGETDURATION:4\r\n"
            "#EXTINF:4.0,\r\n"
            "/seg/0?t=%d\r\n"
            "#EXTINF:4.0,\r\n"
            "/seg/1?t=%d\r\n",
            ms->n_manifests_served,
            ms->current_token,
            ms->current_token);
    } else {
        blen = snprintf(body, sizeof(body),
            "#EXTM3U\r\n"
            "#EXT-X-VERSION:3\r\n"
            "#EXT-X-MEDIA-SEQUENCE:%d\r\n"
            "#EXT-X-TARGETDURATION:4\r\n"
            "#EXTINF:4.0,\r\n"
            "/seg/0\r\n"
            "#EXTINF:4.0,\r\n"
            "/seg/1\r\n",
            ms->n_manifests_served);
    }

    ms_send_response(s, 200, "OK", (const unsigned char *)body, (size_t)blen);
}

/* Parse the token query param from a URL path like /seg/0?t=3.
 * Returns the token value, or -1 if not present. */
static int ms_parse_token(const char *path) {
    const char *q = strchr(path, '?');
    if (!q) return -1;
    const char *t = strstr(q, "t=");
    if (!t) return -1;
    return atoi(t + 2);
}

static void ms_handle_segment(SOCKET s, mock_server_t *ms, const char *path) {
    int n = ms->n_segments_served; /* read before deciding */

    /* Token check */
    if (ms->rotate_tokens) {
        int tok = ms_parse_token(path);
        if (tok != ms->current_token) {
            ms_send_response(s, 403, "Forbidden", NULL, 0);
            return;
        }
    }

    /* Windowed drop injection: remaining_segment_drops counts down. */
    if (ms->remaining_segment_drops > 0) {
        ms->remaining_segment_drops--;
        /* Drop by closing without sending — curl sees incomplete transfer. */
        (void)s;
        return;
    }

    /* Permanent drop-connection injection: close socket on the Nth request */
    if (ms->drop_connection_after_n_segs >= 0 &&
        n == ms->drop_connection_after_n_segs) {
        (void)s;
        return;
    }

    /* Normal response: 512 KB of 0x47 (MPEG-TS sync byte) */
#define SEG_BODY_LEN (512 * 1024)
    static unsigned char seg_body[SEG_BODY_LEN];
    static int seg_body_inited = 0;
    if (!seg_body_inited) {
        memset(seg_body, 0x47, SEG_BODY_LEN);
        seg_body_inited = 1;
    }

    ms->n_segments_served++;
    ms_send_response(s, 200, "OK", seg_body, SEG_BODY_LEN);
}

/* Accept loop — runs in a pthread. */
static void *ms_thread(void *arg) {
    mock_server_t *ms = (mock_server_t *)arg;

    while (!ms->stop) {
        fd_set rset;
        FD_ZERO(&rset);
        FD_SET(ms->listen_sock, &rset);
        struct timeval tv = { 0, 100000 }; /* 100 ms */
        int sel = select((int)ms->listen_sock + 1, &rset, NULL, NULL, &tv);
        if (sel <= 0) continue;

        struct sockaddr_in caddr;
        int caddrlen = sizeof(caddr);
        SOCKET client = accept(ms->listen_sock,
                               (struct sockaddr *)&caddr, &caddrlen);
        if (client == INVALID_SOCKET) continue;

        /* Read the request line (up to 2 KB) */
        char req[2048];
        int nread = recv(client, req, (int)sizeof(req) - 1, 0);
        if (nread <= 0) { closesocket(client); continue; }
        req[nread] = '\0';

        /* Extract path from "GET /path HTTP/1.x\r\n..." */
        char path[1024] = "/";
        sscanf(req, "%*s %1023s", path);

        if (strcmp(path, "/stream.m3u8") == 0) {
            ms_handle_manifest(client, ms);
        } else if (strncmp(path, "/seg/", 5) == 0) {
            ms_handle_segment(client, ms, path);
        } else {
            ms_send_response(client, 404, "Not Found", NULL, 0);
        }

        closesocket(client);
    }
    return NULL;
}

/* --------------------------------------------------------------------------
 * Public API
 * -------------------------------------------------------------------------- */

/* Start the mock server on a random localhost port. Returns an opaque
 * handle (malloc'd). Spawns a pthread. Returns NULL on error. */
mock_server_t *mock_server_start(void) {
    /* WSAStartup — idempotent, safe to call multiple times */
    WSADATA wsa;
    WSAStartup(MAKEWORD(2, 2), &wsa);

    mock_server_t *ms = calloc(1, sizeof(*ms));
    if (!ms) return NULL;

    /* Sensible defaults */
    ms->serve_fail_after_n_manifests  = -1;
    ms->manifest_fail_code            = 509;
    ms->drop_connection_after_n_segs  = -1;
    ms->rotate_tokens                 = 0;
    ms->remaining_manifest_failures   = 0;
    ms->remaining_segment_drops       = 0;

    ms->listen_sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (ms->listen_sock == INVALID_SOCKET) { free(ms); return NULL; }

    int yes = 1;
    setsockopt(ms->listen_sock, SOL_SOCKET, SO_REUSEADDR,
               (const char *)&yes, sizeof(yes));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family      = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port        = 0;  /* OS assigns */

    if (bind(ms->listen_sock, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        closesocket(ms->listen_sock); free(ms); return NULL;
    }
    if (listen(ms->listen_sock, 16) != 0) {
        closesocket(ms->listen_sock); free(ms); return NULL;
    }

    /* Retrieve the OS-assigned port */
    int alen = sizeof(addr);
    getsockname(ms->listen_sock, (struct sockaddr *)&addr, &alen);
    ms->port = ntohs(addr.sin_port);

    if (pthread_create(&ms->thread, NULL, ms_thread, ms) != 0) {
        closesocket(ms->listen_sock); free(ms); return NULL;
    }

    return ms;
}

/* Return the port the mock is listening on. */
int mock_server_port(mock_server_t *ms) {
    if (!ms) return -1;
    return ms->port;
}

/* Return a heap-allocated base URL "http://127.0.0.1:PORT". Caller frees. */
char *mock_server_base_url(mock_server_t *ms) {
    if (!ms) return NULL;
    char *url = malloc(64);
    if (!url) return NULL;
    snprintf(url, 64, "http://127.0.0.1:%d", ms->port);
    return url;
}

/* Configure failure injection.  Call before the scenario starts. */
void mock_server_config(mock_server_t *ms,
                        int serve_fail_after_n_manifests,
                        int manifest_fail_code,
                        int drop_connection_after_n_segs,
                        int rotate_tokens) {
    if (!ms) return;
    ms->serve_fail_after_n_manifests = serve_fail_after_n_manifests;
    ms->manifest_fail_code           = manifest_fail_code;
    ms->drop_connection_after_n_segs = drop_connection_after_n_segs;
    ms->rotate_tokens                = rotate_tokens;
}

/* Inject a windowed manifest failure burst: the next `n` manifest requests
 * will return `code` (regardless of serve_fail_after_n_manifests setting).
 * Thread-safe write — int assignment is atomic on x86. */
void mock_server_inject_manifest_failures(mock_server_t *ms, int n, int code) {
    if (!ms) return;
    ms->manifest_fail_code          = code;
    ms->remaining_manifest_failures = n;
}

/* Inject a windowed segment drop burst: the next `n` segment requests will
 * have their connection dropped mid-response (curl sees incomplete transfer). */
void mock_server_inject_segment_drops(mock_server_t *ms, int n) {
    if (!ms) return;
    ms->remaining_segment_drops = n;
}

/* Stats — read after scenario completes. */
int mock_server_manifests_served(mock_server_t *ms) {
    if (!ms) return 0;
    return ms->n_manifests_served;
}

int mock_server_segments_served(mock_server_t *ms) {
    if (!ms) return 0;
    return ms->n_segments_served;
}

/* Stop + join + free. */
void mock_server_stop(mock_server_t *ms) {
    if (!ms) return;
    ms->stop = 1;
    pthread_join(ms->thread, NULL);
    closesocket(ms->listen_sock);
    free(ms);
}

/* =========================================================================
 * Helpers shared across integration tests
 * ========================================================================= */

static void msleep(int ms) {
    if (ms <= 0) return;
    struct timespec ts;
    ts.tv_sec  = ms / 1000;
    ts.tv_nsec = (long)(ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

/* Build a full manifest URL from a mock server. Caller frees. */
static char *make_manifest_url(mock_server_t *ms) {
    char *url = malloc(128);
    if (!url) return NULL;
    snprintf(url, 128, "http://127.0.0.1:%d/stream.m3u8", ms->port);
    return url;
}

/* =========================================================================
 * Task 11 — Integration scenario tests
 * ========================================================================= */

/* ---------------------------------------------------------------------------
 * test_steady_state_playback
 *
 * Clean server (no failures).  Open prefetch, sleep 5 s.
 * Assert bytes_buffered > 500 KB, manifest_errors == 0, segments_fetched >= 2.
 * --------------------------------------------------------------------------- */
static void test_steady_state_playback(void) {
    mock_server_t *ms = mock_server_start();
    assert(ms != NULL);

    char *manifest_url = make_manifest_url(ms);
    assert(manifest_url != NULL);

    hls_prefetch_t *pf = hls_prefetch_open(manifest_url);
    assert(pf != NULL);
    free(manifest_url);

    /* Give the prefetcher time to fetch several manifests and segments */
    msleep(5000);

    hls_prefetch_stats_t stats;
    hls_prefetch_get_stats(pf, &stats);

    assert(stats.bytes_buffered > 500 * 1024);
    assert(stats.manifest_errors == 0);
    assert(stats.segments_fetched >= 2);

    hls_prefetch_close(pf);
    mock_server_stop(ms);

    puts("OK test_steady_state_playback");
}

/* ---------------------------------------------------------------------------
 * test_survives_509_storm
 *
 * Start clean, let prefetcher warm up for 3 s (ring fills with a few
 * segments).  Then inject 5 consecutive 509s on manifest requests.
 * Sleep 15 s (prefetcher backs off exponentially but eventually recovers).
 * Assert manifest_errors >= 5 AND < manifest_refreshes (recovery happened).
 * Assert the ring was never closed (prefetch thread still alive at the end).
 * --------------------------------------------------------------------------- */
static void test_survives_509_storm(void) {
    mock_server_t *ms = mock_server_start();
    assert(ms != NULL);

    char *manifest_url = make_manifest_url(ms);
    assert(manifest_url != NULL);

    hls_prefetch_t *pf = hls_prefetch_open(manifest_url);
    assert(pf != NULL);
    free(manifest_url);

    /* Phase 1: warm up — prefetcher fetches a few manifests + fills ring */
    msleep(3000);

    /* Verify we have some data before the storm */
    hls_prefetch_stats_t pre_storm;
    hls_prefetch_get_stats(pf, &pre_storm);
    assert(pre_storm.manifest_errors == 0);
    assert(pre_storm.bytes_buffered > 0);   /* buffer has data before storm */

    /* Phase 2: inject 5 consecutive 509s */
    mock_server_inject_manifest_failures(ms, 5, 509);

    /* Phase 3: wait out the storm + recovery.
     * Backoff schedule (ms): 500 → 1000 → 2000 → 4000 → 5000 = ~12.5 s
     * with 2 s sleep between successful polls → full 15 s covers it. */
    msleep(15000);

    hls_prefetch_stats_t post_storm;
    hls_prefetch_get_stats(pf, &post_storm);

    /* At least 5 manifest errors recorded */
    assert(post_storm.manifest_errors >= 5);

    /* Recovery happened: successful refreshes outnumber errors */
    assert(post_storm.manifest_errors < post_storm.manifest_refreshes);

    /* Thread is still alive: manifest_refreshes keeps growing after recovery.
     * We know it recovered because manifest_errors < manifest_refreshes. */
    assert(post_storm.manifest_refreshes > pre_storm.manifest_refreshes);

    hls_prefetch_close(pf);
    mock_server_stop(ms);

    puts("OK test_survives_509_storm");
}

/* ---------------------------------------------------------------------------
 * test_survives_dropped_segment_connection
 *
 * Inject 1 dropped segment connection (curl sees incomplete transfer).
 * After 10 s, assert segments_fetched >= 3 (the prefetcher kept going).
 * No crash / assert fire from the prefetch thread is the implicit assertion.
 * --------------------------------------------------------------------------- */
static void test_survives_dropped_segment_connection(void) {
    mock_server_t *ms = mock_server_start();
    assert(ms != NULL);

    char *manifest_url = make_manifest_url(ms);
    assert(manifest_url != NULL);

    hls_prefetch_t *pf = hls_prefetch_open(manifest_url);
    assert(pf != NULL);
    free(manifest_url);

    /* Let it start up, then inject a single segment connection drop */
    msleep(1000);
    mock_server_inject_segment_drops(ms, 1);

    /* Wait for recovery and several more successful fetches */
    msleep(9000);

    hls_prefetch_stats_t stats;
    hls_prefetch_get_stats(pf, &stats);

    /* At least 3 segments successfully fetched despite the drop */
    assert(stats.segments_fetched >= 3);

    assert(mock_server_segments_served(ms) >= 2);  /* server saw requests */

    hls_prefetch_close(pf);
    mock_server_stop(ms);

    puts("OK test_survives_dropped_segment_connection");
}

/* ---------------------------------------------------------------------------
 * test_token_rotation
 *
 * rotate_tokens=1: each manifest issues segment URLs with a per-manifest
 * token (?t=N).  Server rejects stale tokens with 403.  The prefetcher
 * must fetch segments promptly (before the next manifest refresh rotates
 * the token away).
 *
 * Run for 20 s.  Assert:
 *   - segments_fetched >= 4  (prefetcher kept up through multiple rotations)
 *   - manifest_errors == 0   (manifests themselves always 200)
 * --------------------------------------------------------------------------- */
static void test_token_rotation(void) {
    mock_server_t *ms = mock_server_start();
    assert(ms != NULL);

    /* Enable token rotation */
    mock_server_config(ms,
                       -1,    /* serve_fail_after_n_manifests: never */
                       509,   /* manifest_fail_code: unused */
                       -1,    /* drop_connection_after_n_segs: never */
                       1);    /* rotate_tokens: on */

    char *manifest_url = make_manifest_url(ms);
    assert(manifest_url != NULL);

    hls_prefetch_t *pf = hls_prefetch_open(manifest_url);
    assert(pf != NULL);
    free(manifest_url);

    /* Run long enough for multiple manifest rotations.
     * With target_duration=4s, sleep=2s per cycle: ~10 cycles in 20 s. */
    msleep(20000);

    hls_prefetch_stats_t stats;
    hls_prefetch_get_stats(pf, &stats);

    /* Manifests always succeeded */
    assert(stats.manifest_errors == 0);

    /* Several segments fetched with fresh tokens */
    assert(stats.segments_fetched >= 4);

    hls_prefetch_close(pf);
    mock_server_stop(ms);

    puts("OK test_token_rotation");
}

/* =========================================================================
 * main — Task 10 smoke test + Task 11 integration scenarios
 * ========================================================================= */

int main(void) {
    /* Task 10: verify the server starts and stops cleanly */
    mock_server_t *ms = mock_server_start();
    assert(ms != NULL);
    assert(mock_server_port(ms) > 0);
    mock_server_stop(ms);
    puts("OK mock_server_starts_and_stops");

    /* Task 11: integration scenarios */
    test_steady_state_playback();
    test_survives_509_storm();
    test_survives_dropped_segment_connection();
    test_token_rotation();

    return 0;
}
