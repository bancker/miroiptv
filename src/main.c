#include "player.h"
#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    const char *url = (argc > 1) ? argv[1] : NULL;
    char *resolved = NULL;

    curl_global_init(CURL_GLOBAL_DEFAULT);
    if (!url) {
        if (npo_resolve_stream(&NPO_CHANNELS[0], &resolved) != 0) {
            fputs("usage: ./tv.exe <hls-url>   (resolver broken, see R1)\n", stderr);
            return 2;
        }
        url = resolved;
    }
    printf("url: %s\n", url);

    player_t p;
    if (player_open(&p, url) != 0) { free(resolved); return 3; }

    printf("video: %dx%d, codec=%s\n", p.vctx->width, p.vctx->height,
           avcodec_get_name(p.vctx->codec_id));
    printf("audio: %d Hz, %d ch, codec=%s\n", p.actx->sample_rate,
           p.actx->ch_layout.nb_channels,
           avcodec_get_name(p.actx->codec_id));

    player_close(&p);
    free(resolved);
    curl_global_cleanup();
    return 0;
}
