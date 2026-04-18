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
