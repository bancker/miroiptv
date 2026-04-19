#include "npo.h"
#include <curl/curl.h>
#include <cjson/cJSON.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#  define strcasecmp _stricmp
#  define strncasecmp _strnicmp
#else
#  include <strings.h>
#endif

typedef struct { char *data; size_t len; size_t cap; } buf_t;

static size_t on_data(void *p, size_t sz, size_t n, void *ud) {
    buf_t *b = ud;
    size_t add = sz * n;
    /* Cap at 64 MiB — NPO EPG and stream-link JSON are well under 1 MB. A larger
     * response either means we're being redirected somewhere surprising, or
     * something is very wrong. Better to fail fast than blow memory. */
    static const size_t MAX_BODY = 64u * 1024u * 1024u;
    if (add > MAX_BODY - b->len - 1) return 0;  /* overflow / over cap */

    if (b->len + add + 1 > b->cap) {
        size_t new_cap = b->cap ? b->cap * 2 : 4096;
        while (new_cap < b->len + add + 1) {
            if (new_cap > MAX_BODY / 2) { new_cap = b->len + add + 1; break; }
            new_cap *= 2;
        }
        char *p2 = realloc(b->data, new_cap);
        if (!p2) return 0;
        b->data = p2;
        b->cap = new_cap;
    }
    memcpy(b->data + b->len, p, add);
    b->len += add;
    b->data[b->len] = '\0';
    return add;
}

int npo_http_get(const char *url, const char *const *extra_headers,
                 char **out, size_t *out_len) {
    *out = NULL;
    if (out_len) *out_len = 0;

    CURL *c = curl_easy_init();
    if (!c) return -1;

    buf_t buf = {0};
    struct curl_slist *hdrs = NULL;
    if (extra_headers) {
        for (size_t i = 0; extra_headers[i]; ++i)
            hdrs = curl_slist_append(hdrs, extra_headers[i]);
    }

    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, on_data);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, &buf);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_USERAGENT, "tv/0.1 (+npo-poc)");
    if (hdrs) curl_easy_setopt(c, CURLOPT_HTTPHEADER, hdrs);

    CURLcode rc = curl_easy_perform(c);
    long code = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &code);
    curl_slist_free_all(hdrs);
    curl_easy_cleanup(c);

    if (rc != CURLE_OK) {
        fprintf(stderr, "http: %s: %s\n", url, curl_easy_strerror(rc));
        free(buf.data);
        return -1;
    }
    if (code < 200 || code >= 300) {
        fprintf(stderr, "http: %s: status %ld\n", url, code);
        if (buf.data && buf.len > 0) {
            fprintf(stderr, "http: body (first 200 bytes): %.200s\n", buf.data);
        }
        free(buf.data);
        return -1;
    }

    *out = buf.data;
    if (out_len) *out_len = buf.len;
    return 0;
}

/* Discard callback for the probe. We only want the response code, not the
 * actual bytes — curl needs a write-function when NOBODY is off though. */
static size_t probe_discard(char *p, size_t sz, size_t n, void *ud) {
    (void)p; (void)ud; return sz * n;
}

int npo_http_probe(const char *url, int timeout_s) {
    CURL *c = curl_easy_init();
    if (!c) return -1;

    /* Plain GET with a tiny Range, NOT HEAD. Early versions used HEAD +
     * fallback-to-GET-on-405/501, but the m.hnlol.com portal returns 502
     * on HEAD for /live/ URLs (nginx rejects HEAD on its stream proxy),
     * and 502 isn't in the fallback set. Result: probe rejected every
     * working channel. A plain GET with CURLOPT_FOLLOWLOCATION chases the
     * portal's 302-to-origin redirect and the origin answers 200 OK —
     * that's our success signal. We discard the response body via a
     * throwaway write callback; the kernel is still faster than opening
     * a full libav probe + connection. */
    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, (long)timeout_s);
    curl_easy_setopt(c, CURLOPT_CONNECTTIMEOUT, (long)(timeout_s > 1 ? timeout_s - 1 : 1));
    curl_easy_setopt(c, CURLOPT_USERAGENT, "tv/0.1 (+probe)");
    curl_easy_setopt(c, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(c, CURLOPT_RANGE, "0-0");           /* at most 1 byte */
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, probe_discard);

    CURLcode rc = curl_easy_perform(c);
    long code = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &code);
    curl_easy_cleanup(c);

    /* Accept 2xx and 3xx (some servers don't honour Range, returning 200
     * — that's fine, stream is reachable). Also accept 206 Partial
     * Content. 4xx/5xx means the channel is actually dead. */
    int ok = (rc == CURLE_OK && code >= 200 && code < 400);
    if (!ok)
        fprintf(stderr, "[probe] %s → rc=%d code=%ld\n", url, rc, code);
    return ok ? 0 : -1;
}

/* ---- EPG parsing ---- */

/* Parses "2026-04-18T18:00:00+02:00" into a UTC time_t. Returns 0 on success.
 * Accepts a trailing "Z" or "+HH:MM" / "-HH:MM"; when absent, treats the input
 * as already UTC. */
static int parse_iso8601(const char *s, time_t *out) {
    if (!s) return -1;
    int Y, M, D, h, m, sec, tzh = 0, tzm = 0;
    char tzsign = '+';
    int n = sscanf(s, "%d-%d-%dT%d:%d:%d%c%d:%d",
                   &Y, &M, &D, &h, &m, &sec, &tzsign, &tzh, &tzm);
    if (n < 6) return -1;
    struct tm t = {0};
    t.tm_year = Y - 1900;
    t.tm_mon  = M - 1;
    t.tm_mday = D;
    t.tm_hour = h;
    t.tm_min  = m;
    t.tm_sec  = sec;
#ifdef _WIN32
    time_t utc = _mkgmtime(&t);
#else
    time_t utc = timegm(&t);
#endif
    if (utc == (time_t)-1) return -1;
    if (n >= 9 && (tzsign == '+' || tzsign == '-')) {
        int off = (tzh * 60 + tzm) * 60;
        if (tzsign == '-') off = -off;
        utc -= off;
    }
    *out = utc;
    return 0;
}

static int title_is_news(const char *t) {
    return t && strncasecmp(t, "NOS Journaal", 12) == 0;
}

/* Walks a JSON tree that may have its entries under various keys: "schedule",
 * "items", "data", "epg", "broadcasts", or the root being an array. Returns
 * the entries array or NULL if none recognised. */
static const cJSON *find_entries_array(const cJSON *root) {
    if (cJSON_IsArray(root)) return root;
    const char *keys[] = { "schedule", "items", "data", "epg", "broadcasts", NULL };
    for (size_t i = 0; keys[i]; ++i) {
        cJSON *a = cJSON_GetObjectItemCaseSensitive(root, keys[i]);
        if (cJSON_IsArray(a)) return a;
    }
    return NULL;
}

static char *jstrdup(const cJSON *o, const char *key) {
    cJSON *s = cJSON_GetObjectItemCaseSensitive(o, key);
    if (!cJSON_IsString(s) || !s->valuestring) return NULL;
    return strdup(s->valuestring);
}

int npo_parse_epg(const char *json, size_t json_len, epg_t *out) {
    (void)json_len;
    memset(out, 0, sizeof(*out));
    cJSON *root = cJSON_Parse(json);
    if (!root) return -1;
    const cJSON *arr = find_entries_array(root);
    if (!arr) { cJSON_Delete(root); return -1; }

    size_t n = (size_t)cJSON_GetArraySize(arr);
    if (n == 0) { cJSON_Delete(root); return 0; }

    out->entries = calloc(n, sizeof(epg_entry_t));
    if (!out->entries) { cJSON_Delete(root); return -1; }

    size_t written = 0;
    cJSON *item = NULL;
    cJSON_ArrayForEach(item, arr) {
        const char *title_keys[] = { "title", "programTitle", "name", NULL };
        const char *start_keys[] = { "start", "startDate", "from", "starttime", NULL };
        const char *end_keys[]   = { "end", "endDate", "until", "endtime", NULL };
        const char *id_keys[]    = { "id", "guid", "mid", NULL };

        char *title = NULL, *start_s = NULL, *end_s = NULL, *id = NULL;
        for (size_t i = 0; title_keys[i] && !title; ++i) title   = jstrdup(item, title_keys[i]);
        for (size_t i = 0; start_keys[i] && !start_s; ++i) start_s = jstrdup(item, start_keys[i]);
        for (size_t i = 0; end_keys[i]   && !end_s;   ++i) end_s   = jstrdup(item, end_keys[i]);
        for (size_t i = 0; id_keys[i]    && !id;      ++i) id      = jstrdup(item, id_keys[i]);

        if (!title || !start_s || !end_s) {
            free(title); free(start_s); free(end_s); free(id);
            continue;
        }

        time_t start, end;
        if (parse_iso8601(start_s, &start) != 0 || parse_iso8601(end_s, &end) != 0) {
            free(title); free(start_s); free(end_s); free(id);
            continue;
        }

        out->entries[written].id      = id ? id : strdup("");
        out->entries[written].title   = title;
        out->entries[written].start   = start;
        out->entries[written].end     = end;
        out->entries[written].is_news = title_is_news(title);
        written++;
        free(start_s); free(end_s);
    }

    out->count = written;
    cJSON_Delete(root);
    return 0;
}

void npo_epg_free(epg_t *e) {
    if (!e) return;
    for (size_t i = 0; i < e->count; ++i) {
        free(e->entries[i].id);
        free(e->entries[i].title);
    }
    free(e->entries);
    memset(e, 0, sizeof(*e));
}

const npo_channel_t NPO_CHANNELS[] = {
    { "NPO 1", "NED1", "LI_NL1_4188102" },
    { "NPO 2", "NED2", "LI_NL1_4188103" },
    { "NPO 3", "NED3", "LI_NL1_4188105" },
};
const size_t NPO_CHANNELS_COUNT = sizeof(NPO_CHANNELS) / sizeof(NPO_CHANNELS[0]);

/* --- stream URL resolver ---
 * NPO's public player flow (two steps):
 *   1. GET  https://npo.nl/start/api/domain/player-token?productId=<PID>
 *      Response JSON contains { "jwt": "...", ... } or { "token": "...", ... }.
 *   2. POST https://prod.npoplayer.nl/stream-link
 *      Header: Authorization: Bearer <jwt>
 *      Response: JSON with an HLS URL somewhere (may be nested).
 *
 * Because this API is undocumented and changes, we:
 *   - scan the token response for any string value under any of several likely keys.
 *   - scan the stream-link response for any string value ending in ".m3u8".
 * If either step fails, we return -1 and the caller falls back to --stream-url.
 * Logging is per-step so R1 failures are easy to diagnose.
 */

static char *find_json_string(const cJSON *node, const char *const *keys) {
    if (!node || !cJSON_IsObject(node)) return NULL;
    for (size_t i = 0; keys[i]; ++i) {
        cJSON *v = cJSON_GetObjectItemCaseSensitive(node, keys[i]);
        if (cJSON_IsString(v) && v->valuestring) return strdup(v->valuestring);
    }
    cJSON *child = node->child;
    while (child) {
        char *r = find_json_string(child, keys);
        if (r) return r;
        child = child->next;
    }
    return NULL;
}

static char *find_hls_url(const cJSON *node) {
    if (!node) return NULL;
    if (cJSON_IsString(node) && node->valuestring) {
        if (strstr(node->valuestring, ".m3u8")) return strdup(node->valuestring);
    }
    cJSON *child = node ? node->child : NULL;
    while (child) {
        char *r = find_hls_url(child);
        if (r) return r;
        child = child->next;
    }
    return NULL;
}

int npo_resolve_stream(const npo_channel_t *ch, char **out_url) {
    *out_url = NULL;

    /* Step 1: player-token */
    char token_url[512];
    snprintf(token_url, sizeof(token_url),
        "https://npo.nl/start/api/domain/player-token?productId=%s",
        ch->product_id);

    fprintf(stderr, "resolve: step 1/2 GET %s\n", token_url);

    char *token_body = NULL; size_t token_len = 0;
    if (npo_http_get(token_url, NULL, &token_body, &token_len) != 0) {
        fprintf(stderr, "resolve: step 1 failed (player-token fetch) for %s\n", ch->display);
        return -1;
    }

    cJSON *root = cJSON_Parse(token_body);
    if (!root) {
        fprintf(stderr, "resolve: step 1 JSON parse failed\n");
        free(token_body);
        return -1;
    }
    const char *jwt_keys[] = { "jwt", "token", "playerToken", NULL };
    char *jwt = find_json_string(root, jwt_keys);
    cJSON_Delete(root);
    free(token_body);
    if (!jwt) {
        fprintf(stderr, "resolve: step 1 — no JWT/token key in player-token response\n");
        return -1;
    }

    /* Step 2: stream-link */
    char auth[1024];
    snprintf(auth, sizeof(auth), "Authorization: Bearer %s", jwt);
    const char *headers[] = {
        auth,
        "Content-Type: application/json",
        NULL
    };

    const char *stream_url_endpoint = "https://prod.npoplayer.nl/stream-link";
    fprintf(stderr, "resolve: step 2/2 GET %s\n", stream_url_endpoint);

    char *body = NULL; size_t blen = 0;
    int rc = npo_http_get(stream_url_endpoint, headers, &body, &blen);
    if (rc != 0) {
        fprintf(stderr, "resolve: step 2 failed (stream-link fetch)\n");
        free(jwt);
        return -1;
    }

    cJSON *sroot = cJSON_Parse(body);
    if (!sroot) {
        fprintf(stderr, "resolve: step 2 JSON parse failed\n");
        free(body); free(jwt);
        return -1;
    }
    char *hls = find_hls_url(sroot);
    cJSON_Delete(sroot);
    free(body);
    free(jwt);

    if (!hls) {
        fprintf(stderr, "resolve: step 2 — no HLS URL (.m3u8) found in stream-link response\n");
        return -1;
    }
    *out_url = hls;
    return 0;
}

int npo_fetch_epg(const npo_channel_t *ch, epg_t *out) {
    char url[256];
    snprintf(url, sizeof(url),
             "https://start-api.npo.nl/v2/schedule/channel/%s", ch->code);
    char *body = NULL; size_t len = 0;
    if (npo_http_get(url, NULL, &body, &len) != 0) return -1;
    int rc = npo_parse_epg(body, len, out);
    free(body);
    return rc;
}
