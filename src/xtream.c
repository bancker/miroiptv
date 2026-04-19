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

/* Catch-up streams from category "NL | TERUGKIJKEN" (category_id 1550),
 * with tv_archive=1, tv_archive_duration=2 (days). These are the live-NPO
 * streams mirrored with timeshift support. */
const int XTREAM_NPO_ARCHIVE_STREAM_IDS[3] = { 1124362, 1124363, 1124364 };

/* RTL catch-up: RTL4 HD, RTL5 HD, RTL7 HD, RTL8 HD, RTLZ HD. All have
 * tv_archive=1 and live in category_id 1550 like NPO. Discovered via
 * get_live_streams&category_id=1550 on m.hnlol.com. */
const int XTREAM_RTL_ARCHIVE_STREAM_IDS[5] = {
    1124365, 1124366, 1124369, 1124372, 1124375
};
const char * const XTREAM_RTL_CHANNEL_NAMES[5] = {
    "RTL 4", "RTL 5", "RTL 7", "RTL 8", "RTL Z"
};

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

/* Shared parsing logic. `action` is either "get_short_epg" (fast, 4 upcoming)
 * or "get_simple_data_table" (slower, entire 2-day window including past). */
static int xtream_fetch_epg_internal(const xtream_t *x, int stream_id,
                                     const char *action, epg_t *out) {
    memset(out, 0, sizeof(*out));

    char url[512];
    snprintf(url, sizeof(url),
        "http://%s:%d/player_api.php?username=%s&password=%s&action=%s&stream_id=%d",
        x->host, x->port, x->user, x->pass, action, stream_id);

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

int xtream_fetch_epg(const xtream_t *x, int stream_id, epg_t *out) {
    return xtream_fetch_epg_internal(x, stream_id, "get_short_epg", out);
}

int xtream_fetch_epg_full(const xtream_t *x, int stream_id, epg_t *out) {
    return xtream_fetch_epg_internal(x, stream_id, "get_simple_data_table", out);
}

int xtream_fetch_live_list(const xtream_t *x, const char *category_id,
                           xtream_live_list_t *out) {
    memset(out, 0, sizeof(*out));
    char url[512];
    if (category_id) {
        snprintf(url, sizeof(url),
            "http://%s:%d/player_api.php?username=%s&password=%s&action=get_live_streams&category_id=%s",
            x->host, x->port, x->user, x->pass, category_id);
    } else {
        snprintf(url, sizeof(url),
            "http://%s:%d/player_api.php?username=%s&password=%s&action=get_live_streams",
            x->host, x->port, x->user, x->pass);
    }

    char *body = NULL; size_t len = 0;
    if (npo_http_get(url, NULL, &body, &len) != 0) return -1;

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!cJSON_IsArray(root)) { if (root) cJSON_Delete(root); return -1; }

    size_t n = cJSON_GetArraySize(root);
    out->entries = calloc(n, sizeof(xtream_live_entry_t));
    if (!out->entries) { cJSON_Delete(root); return -1; }

    size_t w = 0;
    cJSON *item = NULL;
    cJSON_ArrayForEach(item, root) {
        cJSON *id_j   = cJSON_GetObjectItemCaseSensitive(item, "stream_id");
        cJSON *name_j = cJSON_GetObjectItemCaseSensitive(item, "name");
        cJSON *num_j  = cJSON_GetObjectItemCaseSensitive(item, "num");
        if (!cJSON_IsNumber(id_j)) continue;
        if (!cJSON_IsString(name_j) || !name_j->valuestring) continue;

        out->entries[w].stream_id = id_j->valueint;
        out->entries[w].num       = cJSON_IsNumber(num_j) ? num_j->valueint : (int)(w + 1);
        out->entries[w].name      = strdup(name_j->valuestring);
        if (!out->entries[w].name) continue;
        w++;
    }
    out->count = w;
    cJSON_Delete(root);
    return 0;
}

void xtream_live_list_free(xtream_live_list_t *list) {
    if (!list) return;
    for (size_t i = 0; i < list->count; ++i) free(list->entries[i].name);
    free(list->entries);
    memset(list, 0, sizeof(*list));
}

char *xtream_timeshift_url(const xtream_t *x, int archive_stream_id,
                           time_t start_time, int duration_min) {
    if (duration_min <= 0) duration_min = 30;  /* sensible default */
    /* The portal interprets the URL time as LOCAL (portal timezone =
     * Europe/Amsterdam). Empirically confirmed: requesting .../2026-04-18:18-00/...
     * served the 18:00 Amsterdam edition, not the 20:00 (=18:00 UTC) edition.
     * So format in localtime, assuming the user's machine matches the portal's
     * timezone. */
    struct tm local_tm;
#ifdef _WIN32
    if (localtime_s(&local_tm, &start_time) != 0) return NULL;
#else
    if (!localtime_r(&start_time, &local_tm)) return NULL;
#endif
    /* "YYYY-MM-DD:HH-MM" = 16 chars + NUL. Bumped to 40 so gcc stops worrying
     * about worst-case tm_year formatting (%04d can produce 5+ chars for
     * years > 9999 — irrelevant for us but the static analyzer can't tell). */
    char when[40];
    snprintf(when, sizeof(when), "%04d-%02d-%02d:%02d-%02d",
             local_tm.tm_year + 1900, local_tm.tm_mon + 1, local_tm.tm_mday,
             local_tm.tm_hour, local_tm.tm_min);

    /* Use .m3u8, not .ts. The .ts variant is served as one long finite HTTP
     * response that the portal drops mid-transfer every 30-60 s, forcing libav
     * to reconnect from byte 0 (the server doesn't honour Range). The .m3u8
     * variant serves the same content as ~30 short segments so each TCP
     * connection only lives for one 60 s segment; libav's HLS demuxer handles
     * the stitching seamlessly. */
    size_t cap = strlen(x->host) + strlen(x->user) + strlen(x->pass) + 128;
    char *out = malloc(cap);
    if (!out) return NULL;
    snprintf(out, cap, "http://%s:%d/timeshift/%s/%s/%d/%s/%d.m3u8",
             x->host, x->port, x->user, x->pass,
             duration_min, when, archive_stream_id);
    return out;
}

char *xtream_stream_url(const xtream_t *x, int stream_id) {
    /* http://HOST:PORT/live/USER/PASS/ID.ts — raw MPEG-TS via a single long-
     * lived TCP connection. We previously used .m3u8 (HLS manifest) but this
     * portal rate-limits playlist refreshes with HTTP 509 every ~30-60s,
     * causing libav's HLS demuxer to fail the reload and exit. The .ts
     * endpoint avoids playlist refreshes entirely — one redirect to the
     * origin, then pure byte stream. Verified: 15.8 MB pulled in 12s, no
     * stalls, sync byte alignment clean. */
    size_t cap = strlen(x->host) + strlen(x->user) + strlen(x->pass) + 64;
    char *out = malloc(cap);
    if (!out) return NULL;
    snprintf(out, cap, "http://%s:%d/live/%s/%s/%d.ts",
             x->host, x->port, x->user, x->pass, stream_id);
    return out;
}
