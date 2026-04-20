#include "update_check.h"
#include "npo.h"          /* npo_http_get */

#include <cjson/cJSON.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* TV_VERSION and TV_UPDATE_REPO are baked in via the Makefile (-DTV_VERSION=...,
 * -DTV_UPDATE_REPO=...). If either is missing we compile a no-op stub so
 * developer builds without the defines don't phone home.
 *
 * Thread safety: update_check_start spawns ONE detached worker thread.
 * The worker writes g_latest + g_available once at completion. Main reads
 * them from update_check_take_notice. Cross-thread visibility is via the
 * volatile int g_available release-flag — once it transitions 0→1 the
 * accompanying string is fully written (x86 TSO would make this work
 * without volatile too, but it's worth keeping explicit on a multi-
 * platform codepath). */

#if !defined(TV_VERSION) || !defined(TV_UPDATE_REPO)
void update_check_start(void) { /* no-op in dev builds */ }
int  update_check_take_notice(char *out, size_t cap) {
    (void)out; (void)cap; return 0;
}
#else

static volatile int g_available = 0;      /* 1 once a newer tag is detected */
static volatile int g_reported  = 0;      /* 1 after main has seen the notice */
static char         g_latest[64] = {0};   /* latest tag, e.g. "v26.4.20.1" */

/* Parse "v26.4.20.1" / "26.04.20" / similar into up to 4 ints. Missing
 * components default to 0, so "v27" compares greater than "v26.99.99.99"
 * which matches the usual "major bump dominates" intuition. Returns the
 * number of components parsed (useful for debugging, callers ignore). */
static int parse_version(const char *s, int out[4]) {
    memset(out, 0, sizeof(int) * 4);
    if (!s) return 0;
    if (*s == 'v' || *s == 'V') s++;
    int parts = 0;
    while (*s && parts < 4) {
        char *end;
        long n = strtol(s, &end, 10);
        if (s == end) break;
        out[parts++] = (int)n;
        s = end;
        while (*s == '.' || *s == '-' || *s == '_') s++;
    }
    return parts;
}

/* -1 when a < b, 0 when equal, +1 when a > b. Ignores any "-dirty" /
 * commit-hash suffix that git-describe tacks on local builds: those
 * parse as 0-and-garbage past the fourth component, which compares
 * equal to the release tag they're based on — so a dev build doesn't
 * falsely flag itself as older than the release it derives from. */
static int cmp_version(const char *a, const char *b) {
    int va[4], vb[4];
    parse_version(a, va);
    parse_version(b, vb);
    for (int i = 0; i < 4; ++i) {
        if (va[i] < vb[i]) return -1;
        if (va[i] > vb[i]) return +1;
    }
    return 0;
}

static void *update_worker(void *ud) {
    (void)ud;
    char url[256];
    snprintf(url, sizeof(url),
             "https://api.github.com/repos/%s/releases/latest",
             TV_UPDATE_REPO);

    /* GitHub's API requires a non-empty User-Agent and responds better
     * when we ask for the v3 accept header. Unauthenticated hits are
     * rate-limited to 60/hour per IP which is ample for a once-per-launch
     * check; if that ever bites we can stash the last check time under
     * %APPDATA% and skip when it's recent. */
    const char *headers[] = {
        "Accept: application/vnd.github.v3+json",
        "User-Agent: miroiptv (github-update-check)",
        NULL,
    };
    char *body = NULL;
    size_t len = 0;
    if (npo_http_get(url, headers, &body, &len) != 0) return NULL;

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!cJSON_IsObject(root)) { if (root) cJSON_Delete(root); return NULL; }

    cJSON *tag = cJSON_GetObjectItemCaseSensitive(root, "tag_name");
    if (cJSON_IsString(tag) && tag->valuestring &&
        cmp_version(TV_VERSION, tag->valuestring) < 0) {
        snprintf(g_latest, sizeof(g_latest), "%s", tag->valuestring);
        g_available = 1;   /* published AFTER g_latest is written */
        fprintf(stderr, "[update] newer version available: %s (running %s)\n",
                tag->valuestring, TV_VERSION);
    }
    cJSON_Delete(root);
    return NULL;
}

void update_check_start(void) {
    pthread_t t;
    if (pthread_create(&t, NULL, update_worker, NULL) == 0)
        pthread_detach(t);
}

int update_check_take_notice(char *out, size_t cap) {
    if (!g_available || g_reported) return 0;
    g_reported = 1;    /* one-shot: we don't want to spam the toast */
    snprintf(out, cap,
             "New version %s available — see github.com/%s/releases",
             g_latest, TV_UPDATE_REPO);
    return 1;
}
#endif
