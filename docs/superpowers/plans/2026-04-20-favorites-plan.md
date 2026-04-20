# Favorites Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a favorites feature for live channels: toggle with `*` while watching or from `f`-search, persist to disk, show a `★` marker in the OSD toast and search rows, and browse via a new `Shift+F` overlay that zaps on Enter.

**Architecture:** New self-contained `src/favorites.{c,h}` module owning JSON load/save + in-memory set + catalog reconciliation. `main.c` holds one `favorites_t`, wires it into startup / shutdown, adds key handlers, and extends two existing toast sites and `search_hit_label()`. The new `Shift+F` overlay reuses `overlay_render_search()` — a thin label array + selected-index pattern the search prompt already established.

**Tech Stack:** C11, MSYS2 / MinGW64, SDL2, SDL2_ttf, `libcjson` (already a project dep for Xtream/NPO JSON), `pkg-config`. Tests use plain C `assert`, same pattern as `tests/test_npo_parse.c`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-04-20-favorites-design.md` (commit 6d8dee5).

---

## File structure

| File | Purpose |
|---|---|
| `src/favorites.h` (NEW) | Public API: types, init/free, is_favorite, toggle, remove, visible iterator |
| `src/favorites.c` (NEW) | JSON load/save, atomic write, reconcile, set ops, path resolution |
| `tests/test_favorites.c` (NEW) | ~30 assertion tests covering parser, writer, set ops, reconcile, journeys |
| `tests/fixtures/favorites_valid.json` (NEW) | 3-entry valid fixture |
| `tests/fixtures/favorites_malformed.json` (NEW) | Truncated JSON fixture |
| `tests/fixtures/favorites_edge.json` (NEW) | Unicode name + unknown fields + missing required field |
| `tests/fixtures/favorites_10k.json` (NEW, generated) | 10 000 valid entries for stress test |
| `Makefile` | Add `test_favorites` target; extend `test` to run both binaries |
| `src/main.c` | Add `favorites_t fv`; wire startup, shutdown, toggle keys, star in toast, `Shift+F` overlay |
| `src/render.c` | Append two lines to help `lines[]` (render.c:261) |

---

## Task 1: Scaffold favorites module (no behavior yet)

**Files:**
- Create: `src/favorites.h`
- Create: `src/favorites.c`
- Modify: `Makefile` (add `test_favorites` target + append to `test`)

- [ ] **Step 1: Create `src/favorites.h` with the module's public API, types, and documentation.**

```c
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
```

- [ ] **Step 2: Create `src/favorites.c` with empty skeletons so the header links.**

```c
#include "favorites.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

char *favorites_path(void) {
    return strdup("favorites.json");   /* placeholder — real impl in Task 2 */
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
```

- [ ] **Step 3: Extend `Makefile` with a `test_favorites` binary and make `test` run both.**

Modify the `TEST_SRC`/`TEST_BIN` block and the `test` rule:

```make
TEST_SRC = tests/test_npo_parse.c src/npo.c src/queue.c
TEST_BIN = build/test_npo_parse.exe

FAV_TEST_SRC = tests/test_favorites.c src/favorites.c
FAV_TEST_BIN = build/test_favorites.exe

.PHONY: all run test clean

all: $(BIN)

$(BIN): $(OBJ) | build
	$(CC) $(OBJ) -o $@ $(LDLIBS)

build/%.o: src/%.c | build
	$(CC) $(CFLAGS) -Isrc -c $< -o $@

build:
	mkdir -p build

run: $(BIN)
	./$(BIN)

test: $(TEST_BIN) $(FAV_TEST_BIN)
	./$(TEST_BIN)
	./$(FAV_TEST_BIN)

$(TEST_BIN): $(TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(TEST_SRC) -o $@ $(LDLIBS)

$(FAV_TEST_BIN): $(FAV_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(FAV_TEST_SRC) -o $@ $(LDLIBS)
```

- [ ] **Step 4: Create a stub `tests/test_favorites.c` so the target builds now.**

```c
#include "../src/favorites.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
    favorites_t fv = {0};
    favorites_init(&fv, NULL);
    favorites_free(&fv);
    puts("OK stub_links");
    return 0;
}
```

- [ ] **Step 5: Verify the scaffold compiles and links.**

Run: `make clean && make test`
Expected: builds both binaries, prints `OK stub_links` (and whatever `test_npo_parse` already prints).

- [ ] **Step 6: Commit.**

```bash
git add src/favorites.h src/favorites.c tests/test_favorites.c Makefile
git commit -m "favorites: scaffold module, test binary, Makefile wiring

No behavior yet — init/free/is_favorite are stubs or minimal;
toggle/remove return -1. Establishes the integration surface and a
green-build baseline for subsequent TDD tasks.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Path resolution

**Files:**
- Modify: `src/favorites.c` (replace placeholder `favorites_path()`)
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Write the failing tests.**

Replace `tests/test_favorites.c` with:

```c
#include "../src/favorites.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#  include <windows.h>
#else
#  include <unistd.h>
#endif

static void setenv_portable(const char *k, const char *v) {
#ifdef _WIN32
    char buf[1024];
    snprintf(buf, sizeof(buf), "%s=%s", k, v ? v : "");
    _putenv(buf);
#else
    if (v) setenv(k, v, 1);
    else   unsetenv(k);
#endif
}

static void test_path_env_override(void) {
    setenv_portable("TV_FAVORITES_PATH", "/tmp/fav-override.json");
    char *p = favorites_path();
    assert(p);
    assert(strcmp(p, "/tmp/fav-override.json") == 0);
    free(p);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    puts("OK test_path_env_override");
}

static void test_path_defaults_nonnull(void) {
    setenv_portable("TV_FAVORITES_PATH", NULL);
    char *p = favorites_path();
    assert(p);
    assert(*p);   /* non-empty */
    /* Contains "favorites.json" somewhere in the tail. */
    assert(strstr(p, "favorites.json") != NULL);
    free(p);
    puts("OK test_path_defaults_nonnull");
}

#ifdef _WIN32
static void test_path_windows_appdata(void) {
    setenv_portable("TV_FAVORITES_PATH", NULL);
    setenv_portable("APPDATA", "C:\\TESTAPPDATA");
    char *p = favorites_path();
    assert(p);
    assert(strstr(p, "C:\\TESTAPPDATA") == p);
    assert(strstr(p, "miroiptv") != NULL);
    assert(strstr(p, "favorites.json") != NULL);
    free(p);
    puts("OK test_path_windows_appdata");
}
#else
static void test_path_linux_xdg(void) {
    setenv_portable("TV_FAVORITES_PATH", NULL);
    setenv_portable("XDG_CONFIG_HOME", "/tmp/xdg-test");
    char *p = favorites_path();
    assert(p);
    assert(strncmp(p, "/tmp/xdg-test/miroiptv/favorites.json",
                   strlen("/tmp/xdg-test/miroiptv/favorites.json")) == 0);
    free(p);
    setenv_portable("XDG_CONFIG_HOME", NULL);
    puts("OK test_path_linux_xdg");
}

static void test_path_linux_xdg_fallback(void) {
    setenv_portable("TV_FAVORITES_PATH", NULL);
    setenv_portable("XDG_CONFIG_HOME", NULL);
    setenv_portable("HOME", "/tmp/home-test");
    char *p = favorites_path();
    assert(p);
    assert(strncmp(p, "/tmp/home-test/.config/miroiptv/favorites.json",
                   strlen("/tmp/home-test/.config/miroiptv/favorites.json")) == 0);
    free(p);
    puts("OK test_path_linux_xdg_fallback");
}
#endif

int main(void) {
    test_path_env_override();
    test_path_defaults_nonnull();
#ifdef _WIN32
    test_path_windows_appdata();
#else
    test_path_linux_xdg();
    test_path_linux_xdg_fallback();
#endif
    return 0;
}
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: first test fails — the stub returns literal `"favorites.json"`, not the env override.

- [ ] **Step 3: Implement `favorites_path()`.**

Replace the placeholder in `src/favorites.c`:

```c
#include "favorites.h"
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
```

(Keep the existing `favorites_init`, `favorites_free`, `favorites_is_favorite`,
`favorites_visible_count`, `favorites_visible_at` from Task 1.)

- [ ] **Step 4: Run tests to confirm they pass.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: `OK test_path_env_override`, `OK test_path_defaults_nonnull`.

- [ ] **Step 5: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c
git commit -m "favorites: resolve path via env override + platform defaults

Priority: TV_FAVORITES_PATH -> %APPDATA%\miroiptv\favorites.json
(Windows) -> \$XDG_CONFIG_HOME/miroiptv/favorites.json ->
\$HOME/.config/miroiptv/favorites.json -> ./favorites.json as fallback.

Tests run without touching real user data.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: JSON parser — empty cases

**Files:**
- Modify: `src/favorites.c`, `src/favorites.h`
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Add a test helper + write the empty-case tests.**

At the top of `tests/test_favorites.c`, after the `setenv_portable` helper, add:

```c
/* Create a scratch tempdir and return its path (malloc'd). Caller frees. */
static char *make_tempdir(void) {
    static int ctr = 0;
    char *p = malloc(256);
#ifdef _WIN32
    char tmp[256];
    DWORD n = GetTempPathA(sizeof(tmp), tmp);
    assert(n > 0 && n < sizeof(tmp));
    snprintf(p, 256, "%sfavtest-%d-%d", tmp, (int)GetCurrentProcessId(), ctr++);
    _mkdir(p);
#else
    snprintf(p, 256, "/tmp/favtest-%d-%d", (int)getpid(), ctr++);
    mkdir(p, 0700);
#endif
    return p;
}

static void write_file(const char *path, const char *data) {
    FILE *f = fopen(path, "wb");
    assert(f);
    fwrite(data, 1, strlen(data), f);
    fclose(f);
}
```

Then add:

```c
/* favorites_load_from_path is an internal test-only helper — declared in a
 * separate "test-only" header we also add. Lets tests drive parsing without
 * going through favorites_init (which wants a catalog). */
#include "../src/favorites_internal.h"

static void test_missing_file_is_empty(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/does-not-exist.json", dir);
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, path);
    assert(rc == 0);
    assert(fv.count == 0);
    favorites_free(&fv);
    free(dir);
    puts("OK test_missing_file_is_empty");
}

static void test_empty_file_is_empty(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/empty.json", dir);
    write_file(path, "");
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, path);
    assert(rc == 0);
    assert(fv.count == 0);
    favorites_free(&fv);
    free(dir);
    puts("OK test_empty_file_is_empty");
}

static void test_empty_array_is_empty(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/arr.json", dir);
    write_file(path, "[]");
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, path);
    assert(rc == 0);
    assert(fv.count == 0);
    favorites_free(&fv);
    free(dir);
    puts("OK test_empty_array_is_empty");
}
```

Update `main()`:

```c
int main(void) {
    test_path_env_override();
    test_path_defaults_nonnull();
    test_missing_file_is_empty();
    test_empty_file_is_empty();
    test_empty_array_is_empty();
    return 0;
}
```

- [ ] **Step 2: Create `src/favorites_internal.h` for test-only hooks.**

```c
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
```

- [ ] **Step 3: Run tests, confirm they fail (unresolved symbol).**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: link error `favorites_load_from_path` not defined.

- [ ] **Step 4: Implement `favorites_load_from_path` for empty cases in `src/favorites.c`.**

Add at the top of `src/favorites.c`:

```c
#include "favorites_internal.h"
#include <cjson/cJSON.h>
#include <errno.h>
#include <sys/stat.h>
#include <time.h>
```

Then add the function. It handles missing file, empty file, and empty array
— the non-empty cases are implemented in Task 4.

```c
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
```

- [ ] **Step 5: Run tests, confirm pass.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: `OK test_missing_file_is_empty`, `OK test_empty_file_is_empty`,
`OK test_empty_array_is_empty`.

- [ ] **Step 6: Commit.**

```bash
git add src/favorites.c src/favorites_internal.h tests/test_favorites.c
git commit -m "favorites: JSON load — empty cases (missing/empty/[]) return empty

Exposes favorites_load_from_path / favorites_save_to_path via an
internal header so tests can exercise parsing directly. Non-empty
arrays parse in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: JSON parser — happy path + edge cases (non-empty arrays)

**Files:**
- Modify: `src/favorites.c`
- Modify: `tests/test_favorites.c`
- Create: `tests/fixtures/favorites_valid.json`
- Create: `tests/fixtures/favorites_edge.json`

- [ ] **Step 1: Create fixtures.**

`tests/fixtures/favorites_valid.json`:

```json
[
  {"stream_id": 1234, "num": 101, "name": "NPO 1 HD"},
  {"stream_id": 5678, "num": 201, "name": "Eurosport 1"},
  {"stream_id": 9012, "num": 301, "name": "Discovery"}
]
```

`tests/fixtures/favorites_edge.json`:

```json
[
  {"stream_id": 1111, "num": 1, "name": "Unicode ★ 🎬 HD", "color": "red"},
  {"num": 2, "name": "Missing stream_id"},
  {"stream_id": 0, "num": 3, "name": "Zero id"},
  {"stream_id": -5, "num": 4, "name": "Negative id"},
  {"stream_id": 2222, "num": 5, "name": "Keep me"},
  {"stream_id": 2222, "num": 6, "name": "Duplicate id"}
]
```

- [ ] **Step 2: Add the happy-path and edge-case tests.**

```c
static void test_round_trip_read_valid(void) {
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, "tests/fixtures/favorites_valid.json");
    assert(rc == 0);
    assert(fv.count == 3);

    /* Expect sort-by-num after reconcile, but Task 4 doesn't reconcile —
     * order is insertion order from the file. Assert by scan. */
    int found_npo = 0, found_euro = 0, found_disco = 0;
    for (size_t i = 0; i < fv.count; ++i) {
        if (fv.entries[i].stream_id == 1234) {
            assert(strcmp(fv.entries[i].name, "NPO 1 HD") == 0);
            assert(fv.entries[i].num == 101);
            found_npo = 1;
        } else if (fv.entries[i].stream_id == 5678) {
            assert(fv.entries[i].num == 201);
            found_euro = 1;
        } else if (fv.entries[i].stream_id == 9012) {
            found_disco = 1;
        }
    }
    assert(found_npo && found_euro && found_disco);
    favorites_free(&fv);
    puts("OK test_round_trip_read_valid");
}

static void test_edge_cases(void) {
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, "tests/fixtures/favorites_edge.json");
    assert(rc == 0);
    /* Accepted: id=1111 (with unknown field), id=2222 (first occurrence).
     * Rejected: missing stream_id, stream_id==0, stream_id==-5, duplicate. */
    assert(fv.count == 2);

    int found_1111 = 0, found_2222 = 0;
    for (size_t i = 0; i < fv.count; ++i) {
        if (fv.entries[i].stream_id == 1111) {
            /* Unicode must round-trip byte-for-byte. */
            assert(strcmp(fv.entries[i].name, "Unicode \xe2\x98\x85 \xf0\x9f\x8e\xac HD") == 0);
            found_1111 = 1;
        }
        if (fv.entries[i].stream_id == 2222) {
            assert(strcmp(fv.entries[i].name, "Keep me") == 0);
            found_2222 = 1;
        }
    }
    assert(found_1111 && found_2222);
    favorites_free(&fv);
    puts("OK test_edge_cases");
}
```

Register in `main()`:

```c
test_round_trip_read_valid();
test_edge_cases();
```

- [ ] **Step 3: Run, confirm failure (parser still returns empty for non-empty arrays).**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: `test_round_trip_read_valid` fails — `fv.count == 0`.

- [ ] **Step 4: Implement the non-empty parser.**

Replace the non-empty branch in `favorites_load_from_path`. Full function:

```c
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
    memset(fv, 0, sizeof(*fv));

    size_t len = 0;
    char *body = slurp(path, &len);
    if (!body) return 0;
    if (len == 0) { free(body); return 0; }

    cJSON *root = cJSON_Parse(body);
    free(body);
    if (!root) {
        fprintf(stderr, "favorites: malformed %s — ignoring for now\n", path);
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
```

- [ ] **Step 5: Run, confirm pass.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: all tests through `OK test_edge_cases` pass.

- [ ] **Step 6: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c tests/fixtures/favorites_valid.json tests/fixtures/favorites_edge.json
git commit -m "favorites: parse non-empty arrays, skip invalid entries, dedup

Per-entry rules: stream_id required + must be positive; name optional
(defaults to \"\"); unknown fields ignored; duplicate stream_id keeps
the first occurrence. All rejections log to stderr.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Malformed-file backup

**Files:**
- Modify: `src/favorites.c`
- Modify: `tests/test_favorites.c`
- Create: `tests/fixtures/favorites_malformed.json`

- [ ] **Step 1: Create malformed fixture.**

`tests/fixtures/favorites_malformed.json`:

```
[{"stream_id": 1234, "name": "half
```

(Truncated mid-string — invalid JSON.)

- [ ] **Step 2: Write the backup test.**

```c
static int file_exists(const char *p) {
    FILE *f = fopen(p, "rb");
    if (!f) return 0;
    fclose(f);
    return 1;
}

static void test_malformed_backs_up_and_resets(void) {
    char *dir = make_tempdir();
    char src[512], orig[512];
    snprintf(src,  sizeof(src),  "%s/malformed_src.json", dir);
    snprintf(orig, sizeof(orig), "%s/favorites.json",     dir);

    /* Copy the malformed fixture into the scratch dir so the test can
     * mutate it without touching the fixture file. */
    size_t len = 0;
    char *body = NULL;
    FILE *f = fopen("tests/fixtures/favorites_malformed.json", "rb");
    assert(f);
    fseek(f, 0, SEEK_END); len = ftell(f); fseek(f, 0, SEEK_SET);
    body = malloc(len + 1);
    fread(body, 1, len, f);
    body[len] = 0;
    fclose(f);
    write_file(orig, body);
    free(body);

    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, orig);
    assert(rc == 0);
    assert(fv.count == 0);

    /* Original has been renamed to favorites.json.corrupt-<timestamp>.
     * Scan the directory for any file starting with "favorites.json.corrupt-". */
    int found_backup = 0;
#ifdef _WIN32
    char pattern[512];
    snprintf(pattern, sizeof(pattern), "%s\\favorites.json.corrupt-*", dir);
    WIN32_FIND_DATAA fd;
    HANDLE h = FindFirstFileA(pattern, &fd);
    if (h != INVALID_HANDLE_VALUE) { found_backup = 1; FindClose(h); }
#else
    DIR *d = opendir(dir);
    assert(d);
    struct dirent *ent;
    while ((ent = readdir(d))) {
        if (strncmp(ent->d_name, "favorites.json.corrupt-", 23) == 0) {
            found_backup = 1; break;
        }
    }
    closedir(d);
#endif
    assert(found_backup);
    assert(!file_exists(orig));   /* original was renamed, not copied */

    favorites_free(&fv);
    free(dir);
    (void)src;
    puts("OK test_malformed_backs_up_and_resets");
}
```

Note: Add `#include <dirent.h>` (non-Windows) at the top of the test file (under the existing `#else` block).

Register: `test_malformed_backs_up_and_resets();` in `main()`.

- [ ] **Step 3: Run, confirm failure.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: backup not found (current impl only prints a warning).

- [ ] **Step 4: Implement the backup-and-rename logic.**

Replace the malformed branch in `favorites_load_from_path`:

```c
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
```

- [ ] **Step 5: Run, confirm pass.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: all tests so far pass, including `OK test_malformed_backs_up_and_resets`.

- [ ] **Step 6: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c tests/fixtures/favorites_malformed.json
git commit -m "favorites: rename corrupt file with timestamp suffix

Malformed JSON -> rename to favorites.json.corrupt-YYYYMMDD-HHMMSS
then treat memory as empty. Never delete user data.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: JSON writer + atomic rename

**Files:**
- Modify: `src/favorites.c`
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Write the writer tests.**

```c
static void test_write_empty_produces_empty_array(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/empty.json", dir);

    favorites_t fv = {0};
    int rc = favorites_save_to_path(&fv, path);
    assert(rc == 0);

    size_t n;
    char *body = slurp_for_test(path, &n);
    assert(body);
    /* Allow whitespace variations: strip leading/trailing whitespace. */
    char *s = body;
    while (*s && (*s == ' ' || *s == '\n' || *s == '\t' || *s == '\r')) ++s;
    assert(strncmp(s, "[]", 2) == 0);
    free(body);
    favorites_free(&fv);
    free(dir);
    puts("OK test_write_empty_produces_empty_array");
}

static void test_round_trip_write_read(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/rt.json", dir);

    favorites_t fv = {0};
    fav_push_for_test(&fv, 1234, 101, "NPO 1 HD");
    fav_push_for_test(&fv, 5678, 201, "Eurosport 1");
    fav_push_for_test(&fv, 9012, 301, "Discovery");
    assert(favorites_save_to_path(&fv, path) == 0);

    favorites_t fv2 = {0};
    assert(favorites_load_from_path(&fv2, path) == 0);
    assert(fv2.count == 3);

    favorites_free(&fv);
    favorites_free(&fv2);
    free(dir);
    puts("OK test_round_trip_write_read");
}

static void test_write_escapes_json_specials(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/esc.json", dir);

    favorites_t fv = {0};
    const char *weird = "A \"quoted\" name\nwith newline and \\ backslash";
    fav_push_for_test(&fv, 7777, 1, weird);
    assert(favorites_save_to_path(&fv, path) == 0);

    favorites_t fv2 = {0};
    assert(favorites_load_from_path(&fv2, path) == 0);
    assert(fv2.count == 1);
    assert(strcmp(fv2.entries[0].name, weird) == 0);

    favorites_free(&fv);
    favorites_free(&fv2);
    free(dir);
    puts("OK test_write_escapes_json_specials");
}

static void test_write_creates_parent_dir(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/nested/subdir/fav.json", dir);

    favorites_t fv = {0};
    fav_push_for_test(&fv, 4242, 1, "Nested");
    assert(favorites_save_to_path(&fv, path) == 0);
    assert(file_exists(path));
    favorites_free(&fv);
    free(dir);
    puts("OK test_write_creates_parent_dir");
}

static void test_write_utf8_no_bom(void) {
    char *dir = make_tempdir();
    char path[512];
    snprintf(path, sizeof(path), "%s/utf.json", dir);

    favorites_t fv = {0};
    fav_push_for_test(&fv, 1, 1, "\xe2\x98\x85 Star");   /* ★ Star */
    assert(favorites_save_to_path(&fv, path) == 0);

    size_t n;
    char *body = slurp_for_test(path, &n);
    assert(body);
    assert(n >= 3);
    /* Reject UTF-8 BOM (EF BB BF) at file start. */
    assert(!(body[0] == (char)0xEF && body[1] == (char)0xBB && body[2] == (char)0xBF));
    /* Raw UTF-8 bytes for the star must appear somewhere in the file. */
    assert(strstr(body, "\xe2\x98\x85") != NULL);
    free(body);
    favorites_free(&fv);
    free(dir);
    puts("OK test_write_utf8_no_bom");
}

static void test_write_fail_keeps_memory_state(void) {
    favorites_t fv = {0};
    fav_push_for_test(&fv, 4242, 1, "InMem");

    /* Path we can't write to: a directory doesn't exist AND we can't mkdir
     * it (root-only paths on Linux, reserved names on Windows).
     * On Linux: /proc is read-only — writing under it fails.
     * On Windows: NUL is a reserved device name — can't create subpaths. */
#ifdef _WIN32
    const char *bad = "NUL\\cannot\\write\\here.json";
#else
    const char *bad = "/proc/cannot/write/here.json";
#endif
    int rc = favorites_save_to_path(&fv, bad);
    assert(rc == -1);
    /* In-memory state is unchanged. */
    assert(fv.count == 1);
    assert(strcmp(fv.entries[0].name, "InMem") == 0);

    favorites_free(&fv);
    puts("OK test_write_fail_keeps_memory_state");
}

static void test_write_is_atomic_on_existing_file(void) {
    /* We don't truly fault-inject between tmp-write and rename (would need
     * to mock fopen/rename or intercept syscalls — too invasive for a C
     * test harness). Verify the weaker but still-meaningful property:
     * after a successful write over an existing file, the previous content
     * is gone AND no .tmp artifact is left behind. */
    char *dir = make_tempdir();
    char path[512], tmp[560];
    snprintf(path, sizeof(path), "%s/atomic.json", dir);
    snprintf(tmp,  sizeof(tmp),  "%s.tmp", path);

    /* Existing file with old content. */
    write_file(path, "[{\"stream_id\":1,\"num\":1,\"name\":\"old\"}]");

    favorites_t fv = {0};
    fav_push_for_test(&fv, 99, 99, "new");
    assert(favorites_save_to_path(&fv, path) == 0);

    /* .tmp must be gone. */
    assert(!file_exists(tmp));

    /* Real file has the new content. */
    favorites_t fv2 = {0};
    assert(favorites_load_from_path(&fv2, path) == 0);
    assert(fv2.count == 1);
    assert(fv2.entries[0].stream_id == 99);

    favorites_free(&fv);
    favorites_free(&fv2);
    free(dir);
    puts("OK test_write_is_atomic_on_existing_file");
}
```

Two test helpers are required:

```c
static char *slurp_for_test(const char *path, size_t *out) {
    FILE *f = fopen(path, "rb");
    if (!f) { *out = 0; return NULL; }
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    char *buf = malloc((size_t)n + 1);
    fread(buf, 1, (size_t)n, f); buf[n] = 0; fclose(f);
    *out = (size_t)n;
    return buf;
}

static void fav_push_for_test(favorites_t *fv, int sid, int num, const char *name) {
    if (fv->count == fv->cap) {
        fv->cap = fv->cap ? fv->cap * 2 : 4;
        fv->entries = realloc(fv->entries, fv->cap * sizeof(*fv->entries));
    }
    fv->entries[fv->count].stream_id = sid;
    fv->entries[fv->count].num       = num;
    fv->entries[fv->count].name      = strdup(name);
    fv->entries[fv->count].hidden    = 0;
    ++fv->count;
}
```

Register in `main()`:

```c
test_write_empty_produces_empty_array();
test_round_trip_write_read();
test_write_escapes_json_specials();
test_write_creates_parent_dir();
test_write_utf8_no_bom();
test_write_fail_keeps_memory_state();
test_write_is_atomic_on_existing_file();
```

- [ ] **Step 2: Run, confirm failure (save returns -1).**

Run: `make test_favorites && ./build/test_favorites.exe`

- [ ] **Step 3: Implement the writer.**

Add to `src/favorites.c`:

```c
static int mkdir_p(const char *path) {
    char tmp[1024];
    snprintf(tmp, sizeof(tmp), "%s", path);
    size_t len = strlen(tmp);
    if (len == 0) return 0;
    if (tmp[len - 1] == PATH_SEP) tmp[len - 1] = '\0';

    for (char *p = tmp + 1; *p; ++p) {
        if (*p == PATH_SEP) {
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
```

- [ ] **Step 4: Run, confirm pass.**

Run: `make test_favorites && ./build/test_favorites.exe`
Expected: all 4 writer tests pass.

- [ ] **Step 5: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c
git commit -m "favorites: JSON writer with atomic replace + parent mkdir

Writes to .tmp, then renames onto the real path. Remove-then-rename on
Windows (rename can't overwrite). Parent dir is mkdir -p'd so first-run
into a fresh %APPDATA%\miroiptv folder just works.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Set operations — toggle, remove, capacity growth

**Files:**
- Modify: `src/favorites.c`
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Write the set-op tests.**

```c
static void test_is_favorite_lookup(void) {
    favorites_t fv = {0};
    fav_push_for_test(&fv, 100, 1, "A");
    fav_push_for_test(&fv, 200, 2, "B");
    assert(favorites_is_favorite(&fv, 100) == 1);
    assert(favorites_is_favorite(&fv, 200) == 1);
    assert(favorites_is_favorite(&fv, 999) == 0);
    favorites_free(&fv);
    puts("OK test_is_favorite_lookup");
}

static void test_toggle_add_remove(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t fv = {0};
    assert(favorites_init(&fv, NULL) == 0);
    assert(fv.count == 0);

    assert(favorites_toggle(&fv, 42, 5, "Channel 42") == 0);
    assert(favorites_is_favorite(&fv, 42) == 1);
    assert(fv.count == 1);

    assert(favorites_toggle(&fv, 42, 5, "Channel 42") == 0);
    assert(favorites_is_favorite(&fv, 42) == 0);
    assert(fv.count == 0);

    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_toggle_add_remove");
}

static void test_capacity_growth(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/big.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t fv = {0};
    favorites_init(&fv, NULL);
    for (int i = 1; i <= 200; ++i) {
        char name[32]; snprintf(name, sizeof(name), "Ch %d", i);
        assert(favorites_toggle(&fv, i, i, name) == 0);
    }
    assert(fv.count == 200);
    for (int i = 1; i <= 200; ++i) assert(favorites_is_favorite(&fv, i) == 1);

    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_capacity_growth");
}

static void test_remove_noop(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/r.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t fv = {0};
    favorites_init(&fv, NULL);
    assert(favorites_remove(&fv, 999) == -1);
    assert(fv.count == 0);

    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_remove_noop");
}
```

Register in `main()`:

```c
test_is_favorite_lookup();
test_toggle_add_remove();
test_capacity_growth();
test_remove_noop();
```

- [ ] **Step 2: Run, confirm failure (toggle/remove still return -1).**

- [ ] **Step 3: Implement toggle + remove.**

Replace the placeholder implementations in `src/favorites.c`:

```c
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
```

Update `favorites_init` to load from the resolved path:

```c
int favorites_init(favorites_t *fv, const xtream_live_list_t *catalog) {
    (void)catalog;   /* reconcile wired in Task 8 */
    memset(fv, 0, sizeof(*fv));
    fv->path = favorites_path();
    if (fv->path) favorites_load_from_path(fv, fv->path);
    /* load_from_path overwrites path because it memsets. Restore it. */
    if (!fv->path) fv->path = favorites_path();
    return 0;
}
```

Wait — `favorites_load_from_path` does `memset(fv, 0, sizeof(*fv))` which wipes `path`. Fix by not memsetting inside the loader when called from init. Cleanest is to pull the memset up into the callers. Replace the first line of `favorites_load_from_path` with:

```c
    /* Don't memset here — caller owns fv->path and may have pre-populated
     * other fields. Reset the collection fields only. */
    for (size_t i = 0; i < fv->count; ++i) free(fv->entries[i].name);
    free(fv->entries);
    fv->entries = NULL;
    fv->count   = 0;
    fv->cap     = 0;
```

And update the edge/round-trip tests to start with `favorites_t fv = {0};` (already do).

Then `favorites_init` becomes:

```c
int favorites_init(favorites_t *fv, const xtream_live_list_t *catalog) {
    (void)catalog;
    memset(fv, 0, sizeof(*fv));
    fv->path = favorites_path();
    if (fv->path) favorites_load_from_path(fv, fv->path);
    return 0;
}
```

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c
git commit -m "favorites: toggle/remove with immediate save, capacity growth

Toggle is idempotent: add when absent, remove when present. Each
mutation writes the file synchronously (a few KB — trivial I/O).
load_from_path no longer wipes fv->path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Reconciliation against the catalog

**Files:**
- Modify: `src/favorites.c`
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Write the reconcile tests.**

```c
/* Build a synthetic catalog for tests. */
static void mk_catalog(xtream_live_list_t *out,
                      const int *ids, const int *nums, const char **names, size_t n) {
    out->entries = calloc(n, sizeof(xtream_live_entry_t));
    out->count   = n;
    for (size_t i = 0; i < n; ++i) {
        out->entries[i].stream_id = ids[i];
        out->entries[i].num       = nums[i];
        out->entries[i].name      = strdup(names[i]);
    }
}
static void free_catalog(xtream_live_list_t *cat) {
    for (size_t i = 0; i < cat->count; ++i) free(cat->entries[i].name);
    free(cat->entries); cat->entries = NULL; cat->count = 0;
}

static void test_reconcile_id_match(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "NPO 1 HD");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    int ids[] = {1234}; int nums[] = {101}; const char *names[] = {"NPO 1 HD"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 1);

    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 0);
    assert(fv.entries[0].stream_id == 1234);

    free_catalog(&cat);
    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_id_match");
}

static void test_reconcile_name_fallback_rewrites_id(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "NPO 1 HD");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    /* Catalog has a channel with the same NAME but different ID. */
    int ids[] = {9999}; int nums[] = {101}; const char *names[] = {"NPO 1 HD"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 1);

    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 0);
    assert(fv.entries[0].stream_id == 9999);   /* rewritten! */

    /* File should have been rewritten with new id. */
    favorites_t fv2 = {0};
    favorites_load_from_path(&fv2, path);
    assert(fv2.count == 1);
    assert(fv2.entries[0].stream_id == 9999);
    favorites_free(&fv2);

    free_catalog(&cat);
    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_name_fallback_rewrites_id");
}

static void test_reconcile_both_miss_hides_entry(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "DeadChannel");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    int ids[] = {5555}; int nums[] = {1}; const char *names[] = {"Something else"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 1);

    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 1);
    assert(favorites_visible_count(&fv) == 0);

    free_catalog(&cat);
    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_both_miss_hides_entry");
}

static void test_reconcile_catalog_empty_keeps_all_visible(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "Whatever");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    xtream_live_list_t cat = {0};   /* catalog load failed */
    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 0);
    assert(fv.entries[0].stream_id == 1234);
    assert(favorites_visible_count(&fv) == 1);

    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_catalog_empty_keeps_all_visible");
}

static void test_reconcile_case_sensitive(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    /* Seed with name "NPO 1 HD" under a dead id. */
    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "NPO 1 HD");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    /* Catalog has case-only difference: "npo 1 hd". */
    int ids[] = {9999}; int nums[] = {101}; const char *names[] = {"npo 1 hd"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 1);

    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    /* Case differs -> no name match -> hidden. */
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 1);
    assert(fv.entries[0].stream_id == 1234);   /* id NOT rewritten */

    free_catalog(&cat);
    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_case_sensitive");
}

static void test_reconcile_determinism_on_name_collision(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "Shared Name");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    /* Two catalog channels share the name. First one (lower num) wins. */
    int ids[]           = {5001, 5002};
    int nums[]          = {10, 20};
    const char *names[] = {"Shared Name", "Shared Name"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 2);

    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    assert(fv.count == 1);
    assert(fv.entries[0].hidden == 0);
    assert(fv.entries[0].stream_id == 5001);   /* first occurrence wins */

    free_catalog(&cat);
    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_determinism_on_name_collision");
}

static void test_reconcile_idempotent(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/f.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t seed = {0};
    fav_push_for_test(&seed, 1234, 101, "NPO 1 HD");
    favorites_save_to_path(&seed, path);
    favorites_free(&seed);

    int ids[] = {1234}; int nums[] = {101}; const char *names[] = {"NPO 1 HD"};
    xtream_live_list_t cat = {0};
    mk_catalog(&cat, ids, nums, names, 1);

    /* First init — no rewrite expected (ids already match). Capture mtime. */
    favorites_t fv = {0};
    favorites_init(&fv, &cat);
    struct stat s1; assert(stat(path, &s1) == 0);
    favorites_free(&fv);

    /* Wait a full second so mtime can tick (POSIX mtime is 1s granularity). */
#ifdef _WIN32
    Sleep(1100);
#else
    struct timespec ts = {1, 100 * 1000 * 1000}; nanosleep(&ts, NULL);
#endif

    favorites_t fv2 = {0};
    favorites_init(&fv2, &cat);
    struct stat s2; assert(stat(path, &s2) == 0);
    assert(s1.st_mtime == s2.st_mtime);   /* no rewrite on second init */

    favorites_free(&fv2);
    free_catalog(&cat);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_reconcile_idempotent");
}
```

Register all seven reconcile tests in `main()`:

```c
test_reconcile_id_match();
test_reconcile_name_fallback_rewrites_id();
test_reconcile_both_miss_hides_entry();
test_reconcile_catalog_empty_keeps_all_visible();
test_reconcile_case_sensitive();
test_reconcile_determinism_on_name_collision();
test_reconcile_idempotent();
```

- [ ] **Step 2: Run, confirm failure.**

- [ ] **Step 3: Implement reconcile.**

Add to `src/favorites.c`:

```c
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
```

Call it from `favorites_init`:

```c
int favorites_init(favorites_t *fv, const xtream_live_list_t *catalog) {
    memset(fv, 0, sizeof(*fv));
    fv->path = favorites_path();
    if (fv->path) favorites_load_from_path(fv, fv->path);
    favorites_reconcile(fv, catalog);
    return 0;
}
```

Add `#include <sys/stat.h>` to the test file (for `stat`). Add `#include <time.h>`
if not already present.

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit.**

```bash
git add src/favorites.c tests/test_favorites.c
git commit -m "favorites: reconcile stored entries against the live catalog

Primary key is stream_id. If a stored id is missing from the catalog
but its name matches a channel there, rewrite the id and persist.
If neither matches, set hidden=1 (kept in file, excluded from the
visible iterator). If the catalog itself is empty (fetch failed),
leave everything visible and don't rewrite. Entries sorted by num
afterwards so the overlay renders in catalog order.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: End-to-end journey + disk-error tests

**Files:**
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Write journey tests.**

```c
static void test_journey_save_reload(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/j.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    {
        favorites_t fv = {0};
        favorites_init(&fv, NULL);
        favorites_toggle(&fv, 100, 1, "A");
        favorites_toggle(&fv, 200, 2, "B");
        favorites_toggle(&fv, 300, 3, "C");
        favorites_free(&fv);
    }

    favorites_t fv2 = {0};
    favorites_init(&fv2, NULL);
    assert(fv2.count == 3);
    assert(favorites_is_favorite(&fv2, 100));
    assert(favorites_is_favorite(&fv2, 200));
    assert(favorites_is_favorite(&fv2, 300));
    favorites_free(&fv2);

    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_journey_save_reload");
}

static void test_journey_add_remove_reload(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/j2.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    {
        favorites_t fv = {0};
        favorites_init(&fv, NULL);
        favorites_toggle(&fv, 100, 1, "A");
        favorites_toggle(&fv, 200, 2, "B");
        favorites_toggle(&fv, 300, 3, "C");
        favorites_toggle(&fv, 200, 2, "B");  /* remove */
        favorites_free(&fv);
    }

    favorites_t fv2 = {0};
    favorites_init(&fv2, NULL);
    assert(fv2.count == 2);
    assert(favorites_is_favorite(&fv2, 100));
    assert(!favorites_is_favorite(&fv2, 200));
    assert(favorites_is_favorite(&fv2, 300));
    favorites_free(&fv2);

    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_journey_add_remove_reload");
}

static void test_journey_random_ops_oracle(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/oracle.json", dir);
    setenv_portable("TV_FAVORITES_PATH", path);

    favorites_t fv = {0};
    favorites_init(&fv, NULL);

    unsigned int seed = 0xdeadbeef;
    char oracle[200] = {0};
    for (int i = 0; i < 400; ++i) {
        /* Custom rand so the test is reproducible cross-platform. */
        seed = seed * 1103515245u + 12345u;
        int id = (int)((seed >> 8) % 200) + 1;
        favorites_toggle(&fv, id, id, "X");
        oracle[id - 1] ^= 1;
    }
    for (int i = 1; i <= 200; ++i) {
        assert(favorites_is_favorite(&fv, i) == (int)oracle[i - 1]);
    }

    favorites_free(&fv);
    setenv_portable("TV_FAVORITES_PATH", NULL);
    free(dir);
    puts("OK test_journey_random_ops_oracle");
}
```

Register in `main()`. Run, all pass (no implementation change needed — these
just exercise the existing surface).

- [ ] **Step 2: Run to confirm pass.**

Run: `make test_favorites && ./build/test_favorites.exe`

- [ ] **Step 3: Commit.**

```bash
git add tests/test_favorites.c
git commit -m "favorites: end-to-end journey tests (save/reload, random ops)

Random-op test uses a deterministic custom rand against a bool[200]
oracle; 400 toggles, final state must match. Catches ordering,
write, read, and dedup bugs as a single failure mode.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Wire module into main.c startup + shutdown

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Add the favorites include + local variable.**

In `src/main.c` at the top:

```c
#include "favorites.h"
```

In `main()`, near the other live/vod/series lists (around line 800), declare:

```c
favorites_t favorites = {0};
```

- [ ] **Step 2: Call favorites_init after the live catalog loads.**

Directly after the `xtream_fetch_live_list` block (around line 826–836), add:

```c
if (portal.host) {
    favorites_init(&favorites, &live_list);
    fprintf(stderr, "[favorites] %zu loaded (%zu visible)\n",
            favorites.count, favorites_visible_count(&favorites));
}
```

(Kept inside the `portal.host` guard so no-portal mode doesn't touch the file.)

- [ ] **Step 3: Free on shutdown.**

In the cleanup section near the bottom (before `xtream_live_list_free(&live_list)`
at line 2252), add:

```c
favorites_free(&favorites);
```

- [ ] **Step 4: Build the full app.**

Run: `make clean && make`
Expected: clean build, no warnings.

- [ ] **Step 5: Smoke-run.**

Run the app briefly (Ctrl+C after a few seconds once a channel is playing):
`./build/miroiptv.exe`
Expected: stderr shows `[favorites] 0 loaded (0 visible)` on first run.

- [ ] **Step 6: Commit.**

```bash
git add src/main.c
git commit -m "main: init/free favorites at startup/shutdown

Loads favorites.json after the live catalog is fetched (so reconcile
has data to match against). Free is sequenced before the catalog
so reconcile's cached name pointers are still valid if anything
were to touch them mid-teardown.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Toggle `*` from playback context + star in toast

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Add the key handler.**

In the event loop `else if` chain (after the `SDLK_s` block at ~line 1272 and
before the `SDLK_UP|SDLK_DOWN` block at ~line 1274), add:

```c
else if (k == SDLK_ASTERISK ||
         (k == SDLK_8 && (ev.key.keysym.mod & KMOD_SHIFT))) {
    /* '*' on US keyboards is Shift+8. Some layouts send SDLK_ASTERISK
     * directly (numpad *, other layouts). Accept both. */
    if (current_live_idx < 0 || current_live_idx >= (int)live_list.count) {
        snprintf(toast_text, sizeof(toast_text),
                 "Favorites only work on live portal channels");
        toast_until_ms = SDL_GetTicks() + 2500;
    } else {
        xtream_live_entry_t *e = &live_list.entries[current_live_idx];
        int was_fav = favorites_is_favorite(&favorites, e->stream_id);
        int rc = favorites_toggle(&favorites, e->stream_id, e->num, e->name);
        if (rc == 0) {
            snprintf(toast_text, sizeof(toast_text),
                     "%s %s favorites",
                     was_fav ? "\xe2\x98\x86" : "\xe2\x98\x85",  /* ☆ / ★ */
                     was_fav ? "Removed from" : "Added to");
        } else {
            snprintf(toast_text, sizeof(toast_text),
                     "Couldn't save favorites (still toggled in memory)");
        }
        toast_until_ms = SDL_GetTicks() + 2500;
    }
}
```

- [ ] **Step 2: Add the star prefix to the zap-complete toasts.**

Replace `main.c:1741`:

```c
/* Phase-0 toast */
int is_fav = favorites_is_favorite(&favorites, e->stream_id);
snprintf(toast_text, sizeof(toast_text),
         "%s%s",
         is_fav ? "\xe2\x98\x85 " : "",
         e->name);
toast_until_ms = SDL_GetTicks() + 8000;
```

Replace `main.c:1758` (phase-2 enrich):

```c
if (now_title && *now_title) {
    int is_fav = favorites_is_favorite(&favorites, zap_prep->epg_stream_id);
    snprintf(toast_text, sizeof(toast_text),
             "%s%s  |  %s",
             is_fav ? "\xe2\x98\x85 " : "",
             zap_prep->label, now_title);
```

- [ ] **Step 3: Build + smoke test.**

Run: `make`
Expected: clean build.

Manual:
1. Launch `./build/miroiptv.exe`.
2. Wait for a channel to come up.
3. Press `*` → toast shows `★ Added to favorites`.
4. Zap to another channel (wheel or up/down) → that toast has no star.
5. Zap back → toast has `★` prefix.
6. Press `*` again → toast shows `☆ Removed from favorites`.
7. Quit with `q`. Relaunch. Zap to the channel you favorited → star is back (persisted).

- [ ] **Step 4: Commit.**

```bash
git add src/main.c
git commit -m "main: '*' toggles favorite on current live channel + star in toast

Accepts both SDLK_ASTERISK and Shift+8 as '*' so any keyboard layout
works. Zap toasts prepend the ★ glyph (U+2605, UTF-8) when the
channel is a favorite. No-portal mode shows a 'requires live portal'
toast instead of silently doing nothing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Toggle `*` from f-search + star in search rows

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Add the search-mode branch.**

Inside the `if (search_active)` block at ~line 1023, add a new `else if` after
the existing `SDLK_DOWN` case (around line 1067):

```c
} else if (sk == SDLK_ASTERISK ||
           (sk == SDLK_8 && (ev.key.keysym.mod & KMOD_SHIFT))) {
    /* Only meaningful on [LIVE] rows — leave VOD/series alone. */
    if (search_hits_count > 0 && search_sel < search_hits_count) {
        search_hit_t h = search_hits[search_sel];
        if (h.kind == SEARCH_HIT_LIVE && h.idx < (int)live_list.count) {
            xtream_live_entry_t *e = &live_list.entries[h.idx];
            favorites_toggle(&favorites, e->stream_id, e->num, e->name);
            /* Swallow the TEXTINPUT that follows. */
            search_swallow_next_text = 1;
        }
    }
    continue;
}
```

Also: the Shift+8 path fires TEXTINPUT for `*` which the search prompt would
then append to the query. The `search_swallow_next_text = 1` line above
handles that cleanly — it piggybacks on the same swallow flag already used
for the initial `f` keypress.

- [ ] **Step 2: Update `search_hit_label()` to show the star for favorited lives.**

The current signature is:

```c
static const char *search_hit_label(const search_hit_t *h,
                                    const xtream_live_list_t *live,
                                    const xtream_vod_list_t  *vods,
                                    const xtream_series_list_t *series,
                                    char *buf, size_t buflen);
```

Add a `favorites_t *fav` parameter:

```c
static const char *search_hit_label(const search_hit_t *h,
                                    const xtream_live_list_t *live,
                                    const xtream_vod_list_t  *vods,
                                    const xtream_series_list_t *series,
                                    const favorites_t *fav,
                                    char *buf, size_t buflen) {
    const char *name = "";
    const char *tag  = "";
    int starred = 0;
    switch (h->kind) {
    case SEARCH_HIT_LIVE:
        tag = "[LIVE]  ";
        name = live->entries[h->idx].name;
        starred = fav && favorites_is_favorite(fav, live->entries[h->idx].stream_id);
        break;
    case SEARCH_HIT_VOD:    tag = "[MOVIE] "; name = vods->entries[h->idx].name;   break;
    case SEARCH_HIT_SERIES: tag = "[SERIES] "; name = series->entries[h->idx].name; break;
    }
    if (starred) {
        /* Insert the star between the tag and the name. */
        snprintf(buf, buflen, "%s\xe2\x98\x85 %s", tag, name);
    } else {
        snprintf(buf, buflen, "%s%s", tag, name);
    }
    return buf;
}
```

- [ ] **Step 2b: Update the one caller (main.c:2200).**

```c
for (int i = 0; i < nshow; ++i) {
    search_hit_label(&search_hits[i], &live_list, &vod_list, &series_list,
                     &favorites, labels[i], sizeof(labels[i]));
    names[i] = labels[i];
}
```

- [ ] **Step 3: Build.**

Run: `make`
Expected: clean build.

- [ ] **Step 4: Smoke test.**

1. Launch, `f`, type a channel name that matches a live row you haven't favorited.
2. Arrow-down to it, press `*` → no toast, but you can cancel search and re-open: the row now shows `[LIVE]★  CHANNEL NAME`.
3. Repeat `*` → star goes away.
4. Esc out of search, press `f` again — change persisted.

(Star isn't drawn mid-session without reopening? It is — `search_hit_label` re-runs every frame the overlay is visible. Confirm: the star appears immediately after `*`, no reopen needed.)

- [ ] **Step 5: Commit.**

```bash
git add src/main.c
git commit -m "main: '*' in f-search toggles favorite on highlighted [LIVE] row

search_hit_label gains a favorites_t param and inserts ★ between
the [LIVE] tag and the channel name when the stream_id is a favorite.
Shift+8 path swallows the following TEXTINPUT so '*' doesn't end up
in the query buffer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: Shift+F favorites overlay — state + open/close

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Declare overlay state near the other overlay locals (~line 793).**

```c
int  fav_overlay_active = 0;
int  fav_overlay_sel    = 0;
```

- [ ] **Step 2: Split the `SDLK_f` handler: plain f opens search, Shift+F opens overlay.**

Replace the block at main.c:1165:

```c
else if (k == SDLK_f) {
    if (ev.key.keysym.mod & KMOD_SHIFT) {
        /* Shift+F: favorites overlay. No async work — just flip the
         * state bit and let the render branch pick up the list. */
        fav_overlay_active   = 1;
        fav_overlay_sel      = 0;
        hint_until_ms        = 0;
    } else {
        /* Plain f: search prompt. Fullscreen moved to F11 in 2026-04-18. */
        search_active = 1;
        search_query[0] = '\0';
        search_query_len = 0;
        search_hits_count = 0;
        search_sel = 0;
        search_swallow_next_text = 1;
        SDL_StartTextInput();
    }
}
```

- [ ] **Step 3: Add the overlay input handler (mirrors epg_full pattern).**

Directly before the `if (epg_full_active)` block at main.c:897, add:

```c
/* Favorites overlay (Shift+F). Intercepts keys before search /
 * episode / EPG modes so arrows + Enter route to favorites. */
if (fav_overlay_active) {
    if (ev.type == SDL_KEYDOWN) {
        SDL_Keycode sk = ev.key.keysym.sym;
        int n = (int)favorites_visible_count(&favorites);
        if (sk == SDLK_ESCAPE) {
            fav_overlay_active = 0;
        } else if (sk == SDLK_f && (ev.key.keysym.mod & KMOD_SHIFT)) {
            fav_overlay_active = 0;   /* toggle off on Shift+F */
        } else if (sk == SDLK_UP && n > 0) {
            fav_overlay_sel = (fav_overlay_sel - 1 + n) % n;
        } else if (sk == SDLK_DOWN && n > 0) {
            fav_overlay_sel = (fav_overlay_sel + 1) % n;
        } else if ((sk == SDLK_PAGEUP || sk == SDLK_PAGEDOWN) && n > 0) {
            int delta = (sk == SDLK_PAGEUP) ? -10 : +10;
            fav_overlay_sel += delta;
            if (fav_overlay_sel < 0) fav_overlay_sel = 0;
            if (fav_overlay_sel >= n) fav_overlay_sel = n - 1;
        } else if ((sk == SDLK_RETURN || sk == SDLK_KP_ENTER) &&
                   n > 0 && fav_overlay_sel < n) {
            const favorite_t *fe = favorites_visible_at(&favorites,
                                                        (size_t)fav_overlay_sel);
            if (fe && current_live_idx >= 0) {
                /* Find the catalog index by stream_id; reuse wheel pipeline. */
                int target_idx = -1;
                for (size_t i = 0; i < live_list.count; ++i) {
                    if (live_list.entries[i].stream_id == fe->stream_id) {
                        target_idx = (int)i; break;
                    }
                }
                if (target_idx >= 0) {
                    pending_wheel_delta = target_idx - current_live_idx;
                    last_wheel_ts       = SDL_GetTicks() - 400;  /* fire immediately */
                }
            }
            fav_overlay_active = 0;
        } else if (sk == SDLK_DELETE && n > 0 && fav_overlay_sel < n) {
            const favorite_t *fe = favorites_visible_at(&favorites,
                                                        (size_t)fav_overlay_sel);
            if (fe) {
                int removed_id = fe->stream_id;
                favorites_remove(&favorites, removed_id);
                /* Clamp selection after removal. */
                int n2 = (int)favorites_visible_count(&favorites);
                if (fav_overlay_sel >= n2) fav_overlay_sel = n2 > 0 ? n2 - 1 : 0;
            }
        }
    }
    continue;
}
```

Note: `continue` skips the rest of the event branches while the overlay is up.

- [ ] **Step 4: Build + test the state layer.**

Run: `make`

Manual: launch, ensure no favorites yet → press Shift+F → nothing visible
yet (rendering comes in Task 14) but no crash. Arrow keys shouldn't
do anything visible either. Esc closes. `q` still quits (the overlay
handler doesn't swallow `q`).

- [ ] **Step 5: Commit.**

```bash
git add src/main.c
git commit -m "main: Shift+F state machine — open, close, arrows, Del, Enter

Intercepts keys before search/episode/EPG overlays so navigation
inside the overlay routes correctly. Enter uses the existing
zap_prep wheel pipeline by computing a delta from current_live_idx.
Rendering is still a no-op — added in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: Shift+F favorites overlay — rendering

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Add render branch.**

In the overlay render block around main.c:2200–2215, insert a case for the
favorites overlay. Reuses `overlay_render_search()` directly — both the
populated and empty states go through the same call, just with different
`n`. The stock "(no matches…)" hint that `overlay_render_search` shows for
`n == 0` is acceptable as a secondary line; the primary header teaches the
`*` binding.

```c
} else if (episode_picker_active) {
    /* ... existing episode picker render (keep unchanged) ... */
} else if (fav_overlay_active) {
    size_t n = favorites_visible_count(&favorites);
    if (n == 0) {
        const char *hdr = "Favorites — press * while watching a channel to add it";
        overlay_render_search(&ov, r.renderer, hdr, NULL, 0, 0, ww, wh);
    } else {
        static char fav_labels[64][128];
        const char *names[64];
        int show = (int)(n < 64 ? n : 64);
        int sel = fav_overlay_sel;
        if (sel >= show) sel = show - 1;
        for (int i = 0; i < show; ++i) {
            const favorite_t *fe = favorites_visible_at(&favorites, (size_t)i);
            if (fe) {
                snprintf(fav_labels[i], sizeof(fav_labels[i]),
                         "%4d  %s", fe->num, fe->name ? fe->name : "");
            } else {
                fav_labels[i][0] = '\0';
            }
            names[i] = fav_labels[i];
        }
        const char *hdr = "Favorites — Enter zaps, Del removes, Esc closes";
        overlay_render_search(&ov, r.renderer, hdr, names, show, sel, ww, wh);
    }
} else if (search_active) {
    /* ... existing search render ... */
}
```

(Preserve the existing `else if` order; only insert the `fav_overlay_active`
branch.)

- [ ] **Step 2: Build + manual test.**

Run: `make && ./build/miroiptv.exe`

Manual (requires at least one live channel):
1. Press Shift+F with no favorites → overlay appears, header says "Favorites — press * while watching…", no rows.
2. Esc closes.
3. Zap to a channel, press `*` → toast confirms.
4. Zap to another, press `*`.
5. Press Shift+F → two rows, sorted by portal num, first row highlighted.
6. Arrow-down to second, Enter → zaps to that channel (toast shows name).
7. Shift+F again → overlay opens, highlighted row is the first one.
8. Del on the first row → list shrinks to one row, selection clamps.
9. Esc.
10. Quit, relaunch → Shift+F: the remaining favorite is still there.

- [ ] **Step 3: Commit.**

```bash
git add src/main.c
git commit -m "main: render the Shift+F favorites overlay via overlay_render_search

Rows: '{num:>4}  {name}'. Empty-state repurposes the header line to
teach the '*' binding. Reuses the existing overlay_render_search
function with n=0 for empty and the visible favorites iterator for
non-empty — no new render code in render.c.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: Help sheet update

**Files:**
- Modify: `src/render.c`

- [ ] **Step 1: Append two lines to the help `lines[]` array at render.c:261.**

Insert after the existing `"  f             Search channels…"` line:

```c
    "  *             Toggle favorite on current channel (also works in 'f' search)",
    "  Shift+f       Favorites list (Enter zaps, Del removes)",
```

Full modified section:

```c
"  f             Search channels + movies + series (type, up/down, Enter)",
"  *             Toggle favorite on current channel (also works in 'f' search)",
"  Shift+f       Favorites list (Enter zaps, Del removes)",
"  F11           Toggle fullscreen",
```

- [ ] **Step 2: Build + verify.**

Run: `make && ./build/miroiptv.exe`

Manual:
1. Press `?` → help overlay shows the two new lines in the Keyboard section.
2. Close with `?`.

- [ ] **Step 3: Commit.**

```bash
git add src/render.c
git commit -m "render: advertise '*' and Shift+F in the help sheet

Keeps the comment at render.c:260 ('Keep in sync with the actual
bindings in main.c') true.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: Stress fixtures + long-name / unicode + 10k stress

**Files:**
- Create: `tests/fixtures/favorites_10k.json` (generated)
- Modify: `tests/test_favorites.c`

- [ ] **Step 1: Add a generator snippet at the top of `main()` in the test file** so CI reliably has the fixture.

Actually cleaner: add a test helper that generates the fixture into a tempdir
rather than committing a 10k-line JSON file. Tests stay hermetic.

```c
static char *generate_10k_fixture(void) {
    char *dir = make_tempdir();
    char *path = malloc(512);
    snprintf(path, 512, "%s/10k.json", dir);
    FILE *f = fopen(path, "wb");
    assert(f);
    fprintf(f, "[\n");
    for (int i = 0; i < 10000; ++i) {
        fprintf(f, "  {\"stream_id\": %d, \"num\": %d, \"name\": \"Ch %d\"}%s\n",
                i + 1, i + 1, i + 1, i == 9999 ? "" : ",");
    }
    fprintf(f, "]\n");
    fclose(f);
    free(dir);
    return path;
}

static void test_stress_10k_entries(void) {
    char *path = generate_10k_fixture();
    favorites_t fv = {0};
    int rc = favorites_load_from_path(&fv, path);
    assert(rc == 0);
    assert(fv.count == 10000);
    favorites_free(&fv);
    free(path);
    puts("OK test_stress_10k_entries");
}

static void test_unicode_and_long_name(void) {
    char *dir = make_tempdir();
    char path[512]; snprintf(path, sizeof(path), "%s/edge.json", dir);

    /* 2 KB name + unicode glyph. */
    char big[2100];
    memset(big, 'X', 2000);
    big[2000] = 0;
    strcat(big, "\xe2\x98\x85");  /* star U+2605 */

    FILE *f = fopen(path, "wb"); assert(f);
    fprintf(f, "[{\"stream_id\": 1, \"num\": 1, \"name\": \"%s\"}]", big);
    fclose(f);

    favorites_t fv = {0};
    assert(favorites_load_from_path(&fv, path) == 0);
    assert(fv.count == 1);
    assert(strcmp(fv.entries[0].name, big) == 0);
    favorites_free(&fv);
    free(dir);
    puts("OK test_unicode_and_long_name");
}
```

Register both in `main()`.

- [ ] **Step 2: Run, confirm pass.**

- [ ] **Step 3: Commit.**

```bash
git add tests/test_favorites.c
git commit -m "favorites: stress (10k entries) + unicode + 2KB name tests

Generates fixtures on the fly rather than committing a 10k-line file.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 17: Final manual-smoke checklist + README line

**Files:**
- Modify: `README.md` (one line under the key list, if the readme has one)

- [ ] **Step 1: Check README for a keybinding section.**

Run: `grep -n -i "shortcut\|keybind\|f11\|Shift+" README.md || echo "no section — skip"`

If there's a section, append:

```
- `*` — toggle favorite on the current live channel (or on any `[LIVE]` row in `f`-search).
- `Shift+F` — open favorites list; Enter zaps, Del removes.
```

If there's no section, skip this step.

- [ ] **Step 2: Manual smoke checklist (do not commit — record results in the PR body).**

Run `./build/miroiptv.exe` on a working portal and tick these off:

```
[ ] First run: stderr shows "[favorites] 0 loaded (0 visible)".
[ ] Zap to a channel, press '*'. Toast: "★ Added to favorites".
[ ] Toast while on that channel begins with "★ ".
[ ] Zap to a different channel. Toast has no leading star.
[ ] Press 'f', type partial name of favorited channel. Arrow-down to the [LIVE] row.
    The row reads "[LIVE]★  NAME".
[ ] Press '*' in search — '*' does not appear in the query buffer. Esc, re-open f.
    The row now has no star (toggle worked).
[ ] Re-favorite it ('*' once more).
[ ] Press Shift+F. Overlay appears, row visible.
[ ] Press Enter. Overlay closes, zap fires, toast shows the channel name.
[ ] Re-open Shift+F. Press Del on the row. Row disappears.
    Overlay now says "Favorites — press * while watching…".
[ ] Add 3 favorites across different portal categories (sport, news, etc.).
[ ] Quit with 'q'. Relaunch. Shift+F shows all 3, sorted by portal num.
[ ] Try '?' — help shows the two new lines.
[ ] Rename favorites.json to favorites.json.broken manually, edit it to be invalid
    JSON, rename back, relaunch: stderr shows "malformed … — backed up to
    favorites.json.corrupt-…", the app starts with empty favorites, the
    backup file exists.
[ ] `make test` still green.
```

- [ ] **Step 3: Commit (only if README was modified).**

```bash
git add README.md
git commit -m "docs: note favorites bindings in README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Coverage check against the spec

| Spec section | Implementing task(s) |
|---|---|
| §1 Context & goals | All tasks together |
| §2 Data model (`favorite_t`, `favorites_t`, hidden flag) | Task 1 (types), Task 7 (set ops), Task 8 (hidden via reconcile) |
| §3 Persistence (path resolution, JSON, atomic, malformed backup) | Tasks 2, 3, 4, 5, 6 |
| §4 Public API | Tasks 1, 6, 7, 8 |
| §5 Key bindings (`*`, `Shift+F`, `Del`, close keys) | Tasks 11, 12, 13 |
| §6 Visualization (star in OSD + f-search rows) | Tasks 11, 12 |
| §7 Favorites overlay (Shift+F) | Tasks 13, 14 |
| §8 Help sheet update | Task 15 |
| §9 Edge cases & error handling | Task 5 (malformed), Task 6 (atomic + disk-write-fail test), Task 7 (dedup on push), Task 8 (reconcile), Task 11 (no-portal toast), Task 11/12 (toggle returns -1 on write fail — surfaced as toast) |
| §10.3 Parser tests (11 items) | Tasks 3 (3 empty), 4 (6 edge), 5 (malformed), 16 (stress 10k + long-name) |
| §10.4 Writer tests (6 items) | Task 6 (empty, round-trip, escapes, parent dir, utf8-no-bom, atomic, fail-keeps-memory) |
| §10.5 Set semantics (6 items) | Task 7 |
| §10.6 Reconcile tests (7 items) | Task 8 (id match, name fallback, both miss, catalog empty, case sensitive, determinism, idempotent) |
| §10.7 Path resolution (4 items) | Task 2 (env + platform-conditional) |
| §10.8 End-to-end (3 items) | Task 9 |
| §10.9 Disk-error | Task 6 (test_write_fail_keeps_memory_state) |
| §10.10 Not-unit-testable items | Manual checklist in Task 17 |
| §11 Instrumentation (optional) | Deferred — spec marks as not-v1 |
| §12 Out of scope — live guide | Deferred (separate brainstorm cycle, tracked as task #9 in the session task list) |
| §13 Open questions | None at plan time |

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-20-favorites-plan.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
