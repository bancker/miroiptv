#ifndef TV_NPO_H
#define TV_NPO_H

#include <stddef.h>
#include <time.h>

/* Thread-safety note:
 * npo_http_get uses a per-call curl_easy handle and is safe to invoke from
 * multiple threads concurrently. HOWEVER: the caller must invoke
 * curl_global_init(CURL_GLOBAL_DEFAULT) exactly once before ANY thread calls
 * this function. Typically this is done once at program startup from main(). */

/* Fetches `url` via HTTP GET. On success returns 0 and *out points to a
 * malloc'd NUL-terminated body; caller frees. On failure returns -1 and *out
 * is NULL. `extra_headers` may be NULL or a NULL-terminated array of header
 * strings ("Name: Value"). */
int npo_http_get(const char *url, const char *const *extra_headers,
                 char **out, size_t *out_len);

/* Liveness probe: HEAD (then GET-fallback) on `url` with a short timeout.
 * Returns 0 if the server responded with a 2xx/3xx status (stream is
 * reachable and the portal hasn't returned 403/502/5xx), -1 otherwise.
 * Used before committing a zap so a dead channel surfaces as a toast
 * instead of a silent black window + stall restart cycle.
 *
 * `timeout_s` is the hard deadline — keep short (2-3s) so fast-scroll
 * zap doesn't pile up; probe runs on the zap worker thread so it doesn't
 * block main but it still steals that worker's wall time. */
int npo_http_probe(const char *url, int timeout_s);

/* ---- EPG parsing ---- */

typedef struct {
    char   *id;
    char   *title;
    time_t  start;   /* unix time */
    time_t  end;
    int     is_news; /* 1 when title starts with "NOS Journaal" (case-insensitive) */
} epg_entry_t;

typedef struct {
    epg_entry_t *entries;
    size_t       count;
} epg_t;

/* Parses an NPO EPG JSON payload into `out`. Tries several known shapes: a
 * top-level array, or an object with "schedule" / "items" / "data" / "epg" /
 * "broadcasts". Per-item keys are also probed across common aliases. Returns 0
 * on success (even if count == 0 when no recognisable entries were found),
 * -1 on parse error. Caller must `npo_epg_free(out)` either way on success. */
int  npo_parse_epg(const char *json, size_t json_len, epg_t *out);
void npo_epg_free(epg_t *e);

typedef struct {
    const char *display;    /* "NPO 1" */
    const char *code;       /* "NED1" — used for EPG */
    const char *product_id; /* "LI_NL1_4188102" — used for stream-URL resolve */
} npo_channel_t;

extern const npo_channel_t NPO_CHANNELS[];
extern const size_t        NPO_CHANNELS_COUNT;

/* Resolves channel to an HLS manifest URL. On success returns 0 and *out_url
 * points to a malloc'd URL (caller frees). On failure returns -1 with logging. */
int npo_resolve_stream(const npo_channel_t *ch, char **out_url);

/* Fetches EPG for channel. 0 on success, -1 on failure. */
int npo_fetch_epg(const npo_channel_t *ch, epg_t *out);

#endif
