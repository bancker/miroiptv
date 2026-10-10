/* favorites_internal.h — test-only internal hooks. Not part of the public
 * API. Ships in the repo so tests can link against implementation details
 * without going through favorites_init (which requires a catalog). */
#ifndef FAVORITES_INTERNAL_H
#define FAVORITES_INTERNAL_H

#include "favorites.h"

/* Load from a specific path (skips favorites_path resolution and catalog
 * reconciliation). Same degrade-to-empty semantics as favorites_init. */
int favorites_load_from_path(favorites_t *fv, const char *path);

/* Write current in-memory state to a specific path atomically. Returns 0
 * on success, -1 on failure. */
int favorites_save_to_path(const favorites_t *fv, const char *path);

#endif
