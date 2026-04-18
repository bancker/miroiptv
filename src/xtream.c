#include "xtream.h"
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

/* Decodes base64 in-place. Output length is always <= input length, so we can
 * reuse the buffer. Returns decoded byte count on success, -1 on malformed
 * input. NUL-termination left to caller. */
static int b64_decode_inplace(char *s) {
    static signed char t[256];
    static int initialized = 0;
    if (!initialized) {
        memset(t, -1, sizeof(t));
        const char *alphabet =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for (int i = 0; i < 64; ++i) t[(unsigned char)alphabet[i]] = (signed char)i;
        initialized = 1;
    }
    char *out = s;
    const char *in = s;
    int buf = 0, bits = 0;
    while (*in && *in != '=') {
        unsigned char c = (unsigned char)*in++;
        if (c == '\n' || c == '\r' || c == ' ' || c == '\t') continue;
        int v = t[c];
        if (v < 0) return -1;
        buf = (buf << 6) | v;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            *out++ = (char)((buf >> bits) & 0xFF);
        }
    }
    return (int)(out - s);
}

int xtream_fetch_epg(const xtream_t *x, int stream_id, epg_t *out) {
    memset(out, 0, sizeof(*out));

    char url[512];
    snprintf(url, sizeof(url),
        "http://%s:%d/player_api.php?username=%s&password=%s&action=get_short_epg&stream_id=%d",
        x->host, x->port, x->user, x->pass, stream_id);

    char *body = NULL;
    size_t len = 0;
    if (npo_http_get(url, NULL, &body, &len) != 0) return -1;

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!root) return -1;

    cJSON *listings = cJSON_GetObjectItemCaseSensitive(root, "epg_listings");
    if (!cJSON_IsArray(listings)) { cJSON_Delete(root); return -1; }

    size_t n = cJSON_GetArraySize(listings);
    if (n == 0) { cJSON_Delete(root); return 0; }

    out->entries = calloc(n, sizeof(epg_entry_t));
    if (!out->entries) { cJSON_Delete(root); return -1; }

    size_t written = 0;
    cJSON *item = NULL;
    cJSON_ArrayForEach(item, listings) {
        cJSON *title_j = cJSON_GetObjectItemCaseSensitive(item, "title");
        cJSON *start_j = cJSON_GetObjectItemCaseSensitive(item, "start_timestamp");
        cJSON *stop_j  = cJSON_GetObjectItemCaseSensitive(item, "stop_timestamp");
        cJSON *id_j    = cJSON_GetObjectItemCaseSensitive(item, "id");

        if (!cJSON_IsString(title_j) || !title_j->valuestring) continue;
        if (!cJSON_IsString(start_j) || !start_j->valuestring) continue;
        if (!cJSON_IsString(stop_j)  || !stop_j->valuestring)  continue;

        char *title = strdup(title_j->valuestring);
        if (!title) continue;
        int tlen = b64_decode_inplace(title);
        if (tlen < 0) { free(title); continue; }
        title[tlen] = '\0';

        time_t start = (time_t)atoll(start_j->valuestring);
        time_t end   = (time_t)atoll(stop_j->valuestring);
        if (start <= 0 || end <= start) { free(title); continue; }

        char *id = NULL;
        if (cJSON_IsString(id_j) && id_j->valuestring) id = strdup(id_j->valuestring);
        if (!id) id = strdup("");

        out->entries[written].id      = id;
        out->entries[written].title   = title;
        out->entries[written].start   = start;
        out->entries[written].end     = end;
        out->entries[written].is_news = (strncasecmp(title, "NOS Journaal", 12) == 0);
        written++;
    }

    out->count = written;
    cJSON_Delete(root);
    return 0;
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
