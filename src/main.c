#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    curl_global_init(CURL_GLOBAL_DEFAULT);
    const npo_channel_t *ch = &NPO_CHANNELS[0];

    puts("Available channels:");
    for (size_t i = 0; i < NPO_CHANNELS_COUNT; ++i) {
        printf("  %-8s code=%-6s product=%s\n",
               NPO_CHANNELS[i].display, NPO_CHANNELS[i].code, NPO_CHANNELS[i].product_id);
    }
    puts("");

    /* Allow override: ./tv.exe <hls-url> bypasses the NPO resolver. Useful when
     * R1 (NPO API changed/broken) applies. */
    char *url = NULL;
    if (argc > 1) {
        url = strdup(argv[1]);
        printf("using --stream-url override: %s\n", url);
    } else {
        puts("resolving stream URL via NPO API...");
        if (npo_resolve_stream(ch, &url) != 0) {
            fputs("\nresolve failed. Pass an HLS URL as argv[1] to bypass:\n"
                  "  ./tv.exe https://example.com/stream.m3u8\n", stderr);
        } else {
            printf("stream URL: %s\n", url);
        }
    }

    puts("\nfetching EPG...");
    epg_t e = {0};
    if (npo_fetch_epg(ch, &e) == 0 && e.count > 0) {
        for (size_t i = 0; i < e.count && i < 8; ++i) {
            printf("  %s  %s\n",
                   e.entries[i].is_news ? "[NEWS]" : "      ",
                   e.entries[i].title);
        }
    } else {
        puts("  (EPG fetch failed or empty — expected if start-api.npo.nl is unavailable)");
    }
    npo_epg_free(&e);

    free(url);
    curl_global_cleanup();
    return 0;
}
