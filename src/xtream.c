#include "xtream.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* NPO 1 HD = 755880, NPO 2 HD = 755881, NPO 3 HD = 755882.
 * Discovered via GET player_api.php?action=get_live_streams on m.hnlol.com.
 * If the portal's stream IDs change, update this table. */
const int XTREAM_NPO_STREAM_IDS[3] = { 755880, 755881, 755882 };

int xtream_parse(const char *spec, xtream_t *out) {
    memset(out, 0, sizeof(*out));
    if (!spec || !*spec) return -1;

    const char *at = strchr(spec, '@');
    if (!at || at == spec) return -1;

    /* user:pass */
    const char *colon = memchr(spec, ':', (size_t)(at - spec));
    if (!colon || colon == spec || colon + 1 == at) return -1;

    size_t user_len = (size_t)(colon - spec);
    size_t pass_len = (size_t)(at - colon - 1);
    out->user = malloc(user_len + 1);
    out->pass = malloc(pass_len + 1);
    if (!out->user || !out->pass) { xtream_free(out); return -1; }
    memcpy(out->user, spec, user_len);         out->user[user_len] = '\0';
    memcpy(out->pass, colon + 1, pass_len);    out->pass[pass_len] = '\0';

    /* host[:port] */
    const char *host_start = at + 1;
    const char *host_colon = strchr(host_start, ':');
    if (host_colon) {
        size_t host_len = (size_t)(host_colon - host_start);
        out->host = malloc(host_len + 1);
        if (!out->host) { xtream_free(out); return -1; }
        memcpy(out->host, host_start, host_len);
        out->host[host_len] = '\0';
        out->port = atoi(host_colon + 1);
        if (out->port <= 0 || out->port > 65535) out->port = 8080;
    } else {
        out->host = strdup(host_start);
        if (!out->host) { xtream_free(out); return -1; }
        out->port = 8080;
    }
    return 0;
}

void xtream_free(xtream_t *x) {
    if (!x) return;
    free(x->host);
    free(x->user);
    free(x->pass);
    memset(x, 0, sizeof(*x));
}

char *xtream_stream_url(const xtream_t *x, int stream_id) {
    /* http://HOST:PORT/live/USER/PASS/ID.m3u8 — the canonical Xtream Codes
     * live endpoint. Format switches between .m3u8 (HLS manifest, playable
     * via libavformat) and .ts (raw MPEG-TS). We prefer .m3u8. */
    size_t cap = strlen(x->host) + strlen(x->user) + strlen(x->pass) + 64;
    char *out = malloc(cap);
    if (!out) return NULL;
    snprintf(out, cap, "http://%s:%d/live/%s/%s/%d.m3u8",
             x->host, x->port, x->user, x->pass, stream_id);
    return out;
}
