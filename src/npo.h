#ifndef TV_NPO_H
#define TV_NPO_H

#include <stddef.h>

/* Fetches `url` via HTTP GET. On success returns 0 and *out points to a
 * malloc'd NUL-terminated body; caller frees. On failure returns -1 and *out
 * is NULL. `extra_headers` may be NULL or a NULL-terminated array of header
 * strings ("Name: Value"). */
int npo_http_get(const char *url, const char *const *extra_headers,
                 char **out, size_t *out_len);

#endif
