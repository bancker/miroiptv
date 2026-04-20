#include "favorites.h"
#include "favorites_internal.h"
#include <cjson/cJSON.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#  include <windows.h>
#  include <direct.h>
#  define PATH_SEP '\\'
#else
#  include <sys/stat.h>
#  include <sys/types.h>
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

static int fav_push(favorites_t *fv, int stream_id, int num, const char *name) {
    /* Dedup: skip duplicates. */
    for (size_t i = 0; i < fv->count; ++i)
        if (fv->entries[i].stream_id == stream_id) return 0;

    if (fv->count == fv->cap) {
        size_t ncap = fv->cap ? fv->cap * 2 : 8;
        favorite_t *grown = realloc(fv->entries, ncap * sizeof(*grown));
        if (!grown) return -1;
        fv->entries = grown;
        fv->cap     = ncap;
    }
    favorite_t *f = &fv->entries[fv->count];
    f->stream_id = stream_id;
    f->num       = num;
    f->name      = name ? strdup(name) : strdup("");
    f->hidden    = 0;
    if (!f->name) return -1;
    ++fv->count;
    return 1;
}

int favorites_load_from_path(favorites_t *fv, const char *path) {
    /* Don't memset here — caller owns fv->path and may have pre-populated
     * other fields. Reset the collection fields only. */
    for (size_t i = 0; i < fv->count; ++i) free(fv->entries[i].name);
    free(fv->entries);
    fv->entries = NULL;
    fv->count   = 0;
    fv->cap     = 0;

    size_t len = 0;
    char *body = slurp(path, &len);
    if (!body) return 0;
    if (len == 0) { free(body); return 0; }

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!root) {
        time_t tnow = time(NULL);
        struct tm lt;
#ifdef _WIN32
        localtime_s(&lt, &tnow);
#else
        localtime_r(&tnow, &lt);
#endif
        char backup[768];
        snprintf(backup, sizeof(backup),
                 "%s.corrupt-%04d%02d%02d-%02d%02d%02d",
                 path,
                 lt.tm_year + 1900, lt.tm_mon + 1, lt.tm_mday,
                 lt.tm_hour, lt.tm_min, lt.tm_sec);
        if (rename(path, backup) != 0) {
            fprintf(stderr, "favorites: malformed %s and rename to %s failed: %s\n",
                    path, backup, strerror(errno));
        } else {
            fprintf(stderr, "favorites: malformed %s — backed up to %s\n",
                    path, backup);
        }
        return 0;
    }
    if (!cJSON_IsArray(root)) { cJSON_Delete(root); return 0; }

    cJSON *item;
    cJSON_ArrayForEach(item, root) {
        cJSON *sid_j  = cJSON_GetObjectItemCaseSensitive(item, "stream_id");
        cJSON *num_j  = cJSON_GetObjectItemCaseSensitive(item, "num");
        cJSON *name_j = cJSON_GetObjectItemCaseSensitive(item, "name");

        if (!cJSON_IsNumber(sid_j)) {
            fprintf(stderr, "favorites: skipping entry without stream_id\n");
            continue;
        }
        int sid = sid_j->valueint;
        if (sid <= 0) {
            fprintf(stderr, "favorites: skipping non-positive stream_id %d\n", sid);
            continue;
        }
        int num = cJSON_IsNumber(num_j) ? num_j->valueint : 0;
        const char *name = cJSON_IsString(name_j) ? name_j->valuestring : "";
        fav_push(fv, sid, num, name);
    }
    cJSON_Delete(root);
    return 0;
}

static int mkdir_p(const char *path) {
    char tmp[1024];
    snprintf(tmp, sizeof(tmp), "%s", path);
    size_t len = strlen(tmp);
    if (len == 0) return 0;
    if (tmp[len - 1] == PATH_SEP) tmp[len - 1] = '\0';

    for (char *p = tmp + 1; *p; ++p) {
#ifdef _WIN32
        if (*p == PATH_SEP || *p == '/') {
#else
        if (*p == PATH_SEP) {
#endif
            *p = '\0';
#ifdef _WIN32
            _mkdir(tmp);
#else
            mkdir(tmp, 0755);
#endif
            *p = PATH_SEP;
        }
    }
#ifdef _WIN32
    _mkdir(tmp);
#else
    mkdir(tmp, 0755);
#endif
    return 0;
}

static void ensure_parent_dir(const char *path) {
    /* Copy up to the last PATH_SEP and mkdir_p on it. */
    char dir[1024];
    snprintf(dir, sizeof(dir), "%s", path);
    char *last = strrchr(dir, PATH_SEP);
#ifdef _WIN32
    /* Also handle '/' which MinGW accepts but strrchr won't match against
     * PATH_SEP == '\\'. */
    char *last2 = strrchr(dir, '/');
    if (!last || (last2 && last2 > last)) last = last2;
#endif
    if (!last) return;
    *last = '\0';
    if (*dir) mkdir_p(dir);
}

int favorites_save_to_path(const favorites_t *fv, const char *path) {
    ensure_parent_dir(path);

    cJSON *root = cJSON_CreateArray();
    if (!root) return -1;
    for (size_t i = 0; i < fv->count; ++i) {
        cJSON *o = cJSON_CreateObject();
        if (!o) { cJSON_Delete(root); return -1; }
        cJSON_AddNumberToObject(o, "stream_id", fv->entries[i].stream_id);
        cJSON_AddNumberToObject(o, "num",       fv->entries[i].num);
        cJSON_AddStringToObject(o, "name",
                                fv->entries[i].name ? fv->entries[i].name : "");
        cJSON_AddItemToArray(root, o);
    }
    char *text = cJSON_Print(root);   /* pretty-printed; fine at ~50 entries */
    cJSON_Delete(root);
    if (!text) return -1;

    char tmp[1024];
    snprintf(tmp, sizeof(tmp), "%s.tmp", path);

    FILE *f = fopen(tmp, "wb");
    if (!f) { free(text); return -1; }
    size_t n = strlen(text);
    if (fwrite(text, 1, n, f) != n) { fclose(f); free(text); remove(tmp); return -1; }
    fclose(f);
    free(text);

    /* Atomic replace. POSIX rename overwrites; Windows rename doesn't —
     * remove target first, then rename. Fine because a window of a few us
     * is acceptable: if the process crashes in that window, favorites.json
     * is gone but favorites.json.tmp is intact. User's data survives. */
#ifdef _WIN32
    remove(path);
#endif
    if (rename(tmp, path) != 0) {
        fprintf(stderr, "favorites: rename %s -> %s failed: %s\n",
                tmp, path, strerror(errno));
        remove(tmp);
        return -1;
    }
    return 0;
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

static int catalog_find_by_id(const xtream_live_list_t *cat, int id) {
    if (!cat) return -1;
    for (size_t i = 0; i < cat->count; ++i)
        if (cat->entries[i].stream_id == id) return (int)i;
    return -1;
}

static int catalog_find_by_name(const xtream_live_list_t *cat, const char *name) {
    if (!cat || !name) return -1;
    for (size_t i = 0; i < cat->count; ++i)
        if (cat->entries[i].name && strcmp(cat->entries[i].name, name) == 0)
            return (int)i;
    return -1;
}

static int cmp_by_num(const void *a, const void *b) {
    int na = ((const favorite_t *)a)->num;
    int nb = ((const favorite_t *)b)->num;
    return (na > nb) - (na < nb);
}

static void favorites_reconcile(favorites_t *fv, const xtream_live_list_t *cat) {
    if (!cat || cat->count == 0) {
        /* Catalog unavailable — keep all entries visible as-cached, do NOT
         * rewrite file. Zapping still works (URL only needs stream_id). */
        for (size_t i = 0; i < fv->count; ++i) fv->entries[i].hidden = 0;
        return;
    }

    int any_rewrite = 0;
    for (size_t i = 0; i < fv->count; ++i) {
        favorite_t *e = &fv->entries[i];
        int ci = catalog_find_by_id(cat, e->stream_id);
        if (ci >= 0) {
            /* Match. Refresh cached num + name (portal is source of truth). */
            e->hidden = 0;
            e->num    = cat->entries[ci].num;
            if (cat->entries[ci].name &&
                (!e->name || strcmp(e->name, cat->entries[ci].name) != 0)) {
                free(e->name);
                e->name = strdup(cat->entries[ci].name);
                any_rewrite = 1;
            }
            continue;
        }
        ci = catalog_find_by_name(cat, e->name);
        if (ci >= 0) {
            fprintf(stderr, "favorites: '%s' id %d -> %d (name match)\n",
                    e->name, e->stream_id, cat->entries[ci].stream_id);
            e->stream_id = cat->entries[ci].stream_id;
            e->num       = cat->entries[ci].num;
            e->hidden    = 0;
            any_rewrite  = 1;
            continue;
        }
        fprintf(stderr, "favorites: '%s' (id=%d) not found in catalog — hiding\n",
                e->name ? e->name : "", e->stream_id);
        e->hidden = 1;
    }

    qsort(fv->entries, fv->count, sizeof(favorite_t), cmp_by_num);

    if (any_rewrite) favorites_save_to_path(fv, fv->path);
}

int favorites_init(favorites_t *fv, const xtream_live_list_t *catalog) {
    memset(fv, 0, sizeof(*fv));
    fv->path = favorites_path();
    if (fv->path) favorites_load_from_path(fv, fv->path);
    favorites_reconcile(fv, catalog);
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
    /* Present? -> remove. */
    for (size_t i = 0; i < fv->count; ++i) {
        if (fv->entries[i].stream_id == stream_id) {
            free(fv->entries[i].name);
            memmove(&fv->entries[i], &fv->entries[i + 1],
                    (fv->count - i - 1) * sizeof(favorite_t));
            --fv->count;
            return favorites_save_to_path(fv, fv->path);
        }
    }
    /* Absent -> add at end. Order is restored by reconcile (sorted by num);
     * within a single session insertion order is fine. */
    if (fav_push(fv, stream_id, num, name) < 0) return -1;
    return favorites_save_to_path(fv, fv->path);
}

int favorites_remove(favorites_t *fv, int stream_id) {
    for (size_t i = 0; i < fv->count; ++i) {
        if (fv->entries[i].stream_id == stream_id) {
            free(fv->entries[i].name);
            memmove(&fv->entries[i], &fv->entries[i + 1],
                    (fv->count - i - 1) * sizeof(favorite_t));
            --fv->count;
            return favorites_save_to_path(fv, fv->path) == 0 ? 0 : -2;
        }
    }
    return -1;
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
