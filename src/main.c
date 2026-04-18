#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    curl_global_init(CURL_GLOBAL_DEFAULT);
    char *body = NULL; size_t len = 0;
    int rc = npo_http_get("https://httpbin.org/get", NULL, &body, &len);
    if (rc == 0) {
        printf("got %zu bytes, first 200:\n%.200s\n", len, body);
    }
    free(body);
    curl_global_cleanup();
    return rc;
}
