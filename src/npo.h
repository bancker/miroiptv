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

#endif
