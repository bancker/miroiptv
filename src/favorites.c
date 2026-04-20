#include "favorites.h"
#include "favorites_internal.h"
#include <cjson/cJSON.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#  include <windows.h>
#  include <direct.h>
#  define PATH_SEP '\\'
#else
#  include <sys/stat.h>
#  include <sys/types.h>
#  include <time.h>
#  include <unistd.h>
#  define PATH_SEP '/'
#endif

static char *join_path(const char *a, const char *b) {
    size_t la = strlen(a), lb = strlen(b);
    char *out = malloc(la + 1 + lb + 1);
    if (!out) return NULL;
    memcpy(out, a, la);
    out[la] = PATH_SEP;
    memcpy(out + la + 1, b, lb);
    out[la + 1 + lb] = '\0';
    return out;
}

static char *slurp(const char *path, size_t *len_out) {
    FILE *f = fopen(path, "rb");
    if (!f) { if (len_out) *len_out = 0; return NULL; }
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n < 0) { fclose(f); if (len_out) *len_out = 0; return NULL; }
    char *buf = malloc((size_t)n + 1);
    if (!buf) { fclose(f); if (len_out) *len_out = 0; return NULL; }
    size_t got = fread(buf, 1, (size_t)n, f);
    fclose(f);
    buf[got] = '\0';
    if (len_out) *len_out = got;
    return buf;
}

int favorites_load_from_path(favorites_t *fv, const char *path) {
    memset(fv, 0, sizeof(*fv));

    size_t len = 0;
    char *body = slurp(path, &len);
    if (!body) return 0;        /* missing file -> empty, no error */
    if (len == 0) { free(body); return 0; }  /* empty file -> empty */

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!root) {
        /* Malformed — backup + empty. Implemented in Task 5. */
        fprintf(stderr, "favorites: malformed %s — ignoring for now\n", path);
        return 0;
    }
    if (!cJSON_IsArray(root)) { cJSON_Delete(root); return 0; }

    size_t n = cJSON_GetArraySize(root);
    if (n == 0) { cJSON_Delete(root); return 0; }

    /* Non-empty array: parsing implemented in Task 4. For now return empty
     * so the empty-case tests pass. */
    cJSON_Delete(root);
    return 0;
}

int favorites_save_to_path(const favorites_t *fv, const char *path) {
    (void)fv; (void)path;
    return -1;   /* implemented in Task 6 */
}

char *favorites_path(void) {
    const char *env = getenv("TV_FAVORITES_PATH");
    if (env && *env) return strdup(env);

#ifdef _WIN32
    const char *appdata = getenv("APPDATA");
    if (appdata && *appdata) {
        char *dir = join_path(appdata, "miroiptv");
        if (dir) {
            char *full = join_path(dir, "favorites.json");
            free(dir);
            if (full) return full;
        }
    }
#else
    const char *xdg = getenv("XDG_CONFIG_HOME");
    char *base = NULL;
    if (xdg && *xdg) {
        base = strdup(xdg);
    } else {
        const char *home = getenv("HOME");
        if (home && *home) {
            base = join_path(home, ".config");
        }
    }
    if (base) {
        char *dir = join_path(base, "miroiptv");
        free(base);
        if (dir) {
            char *full = join_path(dir, "favorites.json");
            free(dir);
            if (full) return full;
        }
    }
#endif

    /* Last-resort: current working dir. Keeps the app working even if the
     * env is so hostile that we can't resolve a real home directory. */
    return strdup("favorites.json");
}

int favorites_init(favorites_t *fv, const xtream_live_list_t *catalog) {
    (void)catalog;
    memset(fv, 0, sizeof(*fv));
    fv->path = favorites_path();
    return 0;
}

void favorites_free(favorites_t *fv) {
    if (!fv) return;
    for (size_t i = 0; i < fv->count; ++i) free(fv->entries[i].name);
    free(fv->entries);
    free(fv->path);
    memset(fv, 0, sizeof(*fv));
}

int favorites_is_favorite(const favorites_t *fv, int stream_id) {
    for (size_t i = 0; i < fv->count; ++i)
        if (fv->entries[i].stream_id == stream_id && !fv->entries[i].hidden)
            return 1;
    return 0;
}

int favorites_toggle(favorites_t *fv, int stream_id, int num, const char *name) {
    (void)fv; (void)stream_id; (void)num; (void)name;
    return -1;   /* placeholder */
}

int favorites_remove(favorites_t *fv, int stream_id) {
    (void)fv; (void)stream_id;
    return -1;   /* placeholder */
}

size_t favorites_visible_count(const favorites_t *fv) {
    size_t n = 0;
    for (size_t i = 0; i < fv->count; ++i) if (!fv->entries[i].hidden) ++n;
    return n;
}

const favorite_t *favorites_visible_at(const favorites_t *fv, size_t idx) {
    size_t seen = 0;
    for (size_t i = 0; i < fv->count; ++i) {
        if (fv->entries[i].hidden) continue;
        if (seen == idx) return &fv->entries[i];
        ++seen;
    }
    return NULL;
}
