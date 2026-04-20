/* favorites.h — live-channel favorites module.
 *
 * Owns the user's personal shortlist of live channels. Persists to
 * $TV_FAVORITES_PATH (or %APPDATA%\miroiptv\favorites.json on Windows,
 * $XDG_CONFIG_HOME/miroiptv/favorites.json on Linux). Reconciles stored
 * ids against the current portal catalog on init so renamed/moved channels
 * don't silently break.
 *
 * Single instance per process, owned by main.c. Not thread-safe — all calls
 * must be from the main (SDL event) thread. */
#ifndef FAVORITES_H
#define FAVORITES_H

#include "xtream.h"   /* xtream_live_list_t */
#include <stddef.h>

typedef struct {
    int   stream_id;   /* portal's live stream id; primary key */
    int   num;         /* portal's display order; cached for sorting */
    char *name;        /* malloc'd; cached so overlay renders offline and
                          so reconcile can fall back to name match */
    int   hidden;      /* 1 if reconciliation couldn't match this entry
                          to any catalog channel; kept in file, excluded
                          from the visible iterator. 0 otherwise. */
} favorite_t;

typedef struct {
    favorite_t *entries;    /* sorted by num ascending after reconcile */
    size_t      count;
    size_t      cap;
    char       *path;       /* resolved favorites.json path, malloc'd */
} favorites_t;

/* Resolves the favorites.json path. Caller frees. Honors $TV_FAVORITES_PATH
 * first (for tests), then platform defaults. Never returns NULL: falls back
 * to "favorites.json" in CWD if everything else fails. */
char *favorites_path(void);

/* One-shot startup: resolves path, loads file, reconciles against the
 * current live catalog. After this call, favorites are ready to use.
 * Returns 0 always; load failures degrade to empty state (logged). */
int  favorites_init(favorites_t *fv, const xtream_live_list_t *catalog);

/* Free all owned memory. Safe to call on a zeroed struct. */
void favorites_free(favorites_t *fv);

/* Pure lookup. O(N) scan. */
int  favorites_is_favorite(const favorites_t *fv, int stream_id);

/* Idempotent toggle. On add, requires name + num (from the channel the
 * caller has in hand). On remove, looks up by id. Writes file.
 * Returns 0 on success, -1 on disk-write failure (in-memory state is
 * still mutated — caller should surface a toast). */
int  favorites_toggle(favorites_t *fv, int stream_id, int num,
                      const char *name);

/* Explicit remove (used by Del key in the favorites overlay). Writes file.
 * Returns 0 if the id was present, -1 if it wasn't, -2 on disk-write fail. */
int  favorites_remove(favorites_t *fv, int stream_id);

/* Iterator over visible (non-hidden) entries. Order is ascending by num.
 * Pointer is invalidated by any mutating call. */
size_t favorites_visible_count(const favorites_t *fv);
const favorite_t *favorites_visible_at(const favorites_t *fv, size_t idx);

#endif /* FAVORITES_H */
