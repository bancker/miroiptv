#ifndef TV_NPO_H
#define TV_NPO_H

#include <stddef.h>

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

#endif
