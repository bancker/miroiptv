#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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
