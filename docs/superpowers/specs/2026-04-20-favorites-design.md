# Design: Favorite live channels

**Date:** 2026-04-20
**Status:** Approved (brainstorming), pending user review of spec
**Target platform:** Windows 11 + Linux (same code paths as the rest of miroiptv)

## 1. Context and goals

### Problem
miroiptv has ~15 000 live channels loaded from the Xtream portal. Finding the
handful the user actually watches means scrolling, typing into `f`-search, or
remembering portal numbers. There is no concept of a personal shortlist.

Separately, the user wants a live-channel guide that shows EPG across channels
(not just the currently-tuned one). A guide over 15 000 channels is expensive
to populate (~15 k HTTP round-trips). Restricting the guide to a curated
shortlist collapses that cost by two orders of magnitude.

This spec designs the shortlist — *favorites* — as a standalone feature. The
guide is the next brainstorming cycle and will extend the UI this spec builds.

### Goals
1. Mark any live channel as a favorite from two contexts: while watching it,
   and while it is the highlighted `[LIVE]` row in `f`-search.
2. Unmark from the same two contexts plus a `Del` key in the favorites
   overlay.
3. Persist favorites across app restarts.
4. Surface favorite status with a `★` marker in the OSD toast and in
   `f`-search rows.
5. Provide a `Shift+F` overlay that lists favorites (portal-order) and zaps
   on Enter.
6. Design the overlay so the future live guide is a pure data-layer
   extension (EPG column per row) rather than a new overlay.

### Non-goals
- Favorites for VOD or series (live only this cycle).
- A favorites-management UI beyond `Del` in the overlay (no reorder,
  no rename).
- Hot-reload of `favorites.json` on external edits.
- Multi-portal favorites (stream_ids are portal-scoped; behaviour with a
  different portal is undefined and documented).
- EPG rendering in the `Shift+F` overlay — that is the next cycle.

## 2. Data model

Core types in a new `src/favorites.h` / `src/favorites.c` module.

```c
typedef struct {
    int   stream_id;   /* portal's live stream id (unique key) */
    int   num;         /* portal's display order; cached for sorting */
    char *name;        /* malloc'd; cached for two reasons:
                          (1) enables the name-fallback reconcile in §3;
                          (2) lets the overlay render if the catalog
                              fetch failed this session (offline start). */
    int   hidden;      /* 1 if reconciliation could not match this entry
                          to any current catalog channel; kept in file,
                          excluded from overlay. 0 otherwise. */
} favorite_t;

typedef struct {
    favorite_t *entries;    /* sorted by num ascending after reconcile */
    size_t      count;
    size_t      cap;
    char       *path;       /* resolved favorites.json path, malloc'd */
} favorites_t;
```

Expected working size: user estimates ~50 entries. Linear scan on
`is_favorite(id)` is ~50 int comparisons — not worth a hash set.

## 3. Persistence

### 3.1 File location

Resolved by `favorites_path()` in this priority order:

1. `$TV_FAVORITES_PATH` env var (used by tests; not documented for end
   users).
2. Windows: `%APPDATA%\miroiptv\favorites.json`.
3. Linux: `$XDG_CONFIG_HOME/miroiptv/favorites.json`, falling back to
   `$HOME/.config/miroiptv/favorites.json` if `XDG_CONFIG_HOME` unset.

Parent directory is created on write if missing.

### 3.2 File format

UTF-8 encoded JSON, no BOM. One array of objects:

```json
[
  {"stream_id": 1234, "num": 101, "name": "NPO 1 HD"},
  {"stream_id": 5678, "num": 201, "name": "Eurosport 1"}
]
```

Unknown fields are ignored on read. Entries missing `stream_id` are skipped
with a stderr warning. Entries with `stream_id <= 0` are rejected (portal ids
are always positive).

### 3.3 Atomic write

Every mutation (add, remove, id-rewrite during reconcile) writes the full
file. Two-step to survive a crash mid-write:

1. Write full contents to `favorites.json.tmp`.
2. `rename()` over `favorites.json`.

If the rename fails, the original file is untouched and the in-memory state
is kept; caller is expected to surface a toast.

### 3.4 Malformed file recovery

If parse fails on read, rename the file to
`favorites.json.corrupt-YYYYMMDD-HHMMSS` and treat memory as empty. Never
delete user data.

## 4. Public API (module interface)

```c
/* One-shot startup: resolves path, loads file, reconciles against the
 * current live catalog. After this call, favorites are ready to use.
 * Returns 0 always; failures are logged and degrade to empty state. */
int  favorites_init(favorites_t *fv, const xtream_live_list_t *catalog);

/* Pure lookup. O(N) scan over fv->entries. */
int  favorites_is_favorite(const favorites_t *fv, int stream_id);

/* Idempotent toggle. On add, requires name + num (from the channel the
 * caller has in hand). On remove, looks up by id. Writes file.
 * Returns 0 on success, -1 on disk write failure (in-memory state is
 * still mutated — see §9). */
int  favorites_toggle(favorites_t *fv, int stream_id, int num,
                      const char *name);

/* Explicit remove (used by Del key in the overlay). */
int  favorites_remove(favorites_t *fv, int stream_id);

/* Iterator over visible (non-hidden) entries, sorted by num. */
size_t favorites_visible_count(const favorites_t *fv);
const favorite_t *favorites_visible_at(const favorites_t *fv, size_t idx);

void favorites_free(favorites_t *fv);
```

No other module mutates `favorites_t`. `main.c` owns one instance.

## 5. Key bindings and dispatch

### 5.1 New bindings

| Key | Context | Action |
|---|---|---|
| `*` (Shift+8) | Watching a live channel (no overlay active) | Toggle favorite status of current channel; OSD toast `★ Added to favorites` / `☆ Removed from favorites` (~1.5 s) |
| `*` | `f`-search with a `[LIVE]` row highlighted | Toggle that channel's favorite status; row re-renders with/without star; no zap |
| `Shift+F` | Any non-modal context | Open favorites overlay |
| `Del` | Inside the favorites overlay | Remove highlighted row (rewrites file) |
| `Esc` / `Shift+F` | Inside the favorites overlay | Close overlay (`q` remains global quit, per the rest of the app) |
| `↑` / `↓` | Inside the favorites overlay | Move cursor |
| `PgUp` / `PgDn` | Inside the favorites overlay | Jump 10 rows |
| `Enter` | Inside the favorites overlay | Zap to highlighted channel via existing `zap_prep` worker (same path `f`-search uses) |

### 5.2 What `*` does *not* do
- Not bound inside the `Shift+F` overlay (curation there uses `Del`; a
  different verb for a different context).
- Not bound when any full-screen overlay is active (help, full-EPG, episode
  picker). Existing mutual-exclusion rules apply.

### 5.3 Collisions
Verified against current bindings (`main.c` event loop + `render.c:261`
help sheet): `f`, `F11`, `t`, `q`, `s`, `a`, `e`, `Shift+E`, `n`, `r`,
digits, arrows, space, `?`. No collision with `*` or `Shift+F`.

## 6. Visualization

### 6.1 OSD toast
When the currently-tuned live channel is a favorite, any toast that renders
its display name prepends `★ `. Implementation: a single
`favorites_is_favorite(&fv, stream_id)` check at the toast string-build
sites (`main.c` has ~3 such sites for the zap-completed toast). Format
string gains the star conditionally.

### 6.2 `f`-search rows
`search_hit_label()` in `main.c` currently emits labels like
`[LIVE] NPO 1 HD`. Extend it: if the hit is a live channel and
`favorites_is_favorite()` returns 1, emit `[LIVE]★ NPO 1 HD` instead.

### 6.3 Glyph choice
`★` = U+2605. The TTF font already in the project renders `[LIVE]` and
other BMP glyphs without issue; U+2605 is BMP. Implementation verifies the
glyph renders (fallback to ASCII `*` if `TTF_GlyphIsProvided` returns
false — safety net, not expected to trigger).

## 7. Favorites overlay (Shift+F)

### 7.1 Layout
Mirrors the `epg_full_active` overlay in `main.c:784+`. Scrollable vertical
list, centred over the video surface, fixed width (~60 chars), height
bounded by window. Rows:

```
  {num:>4}  {name}
```

One highlighted row via cursor index. Standard overlay chrome
(padding, background alpha) from `render.c` overlay helpers.

### 7.2 Empty state
Single centered line:

```
  No favorites yet. Press * while watching a channel to add it.
```

The key teaches itself.

### 7.3 State machine
Reuses the pattern from `epg_full_active`: a handful of locals in the
main loop (`fav_overlay_active`, `fav_sel`), keys are intercepted at the
top of the event branch before search / episode / help modes so the overlay
has priority while open.

### 7.4 Zap path
`Enter` calls the same `zap_prep` worker `f`-search uses for `[LIVE]`
hits — the code path is proven. No new async infra.

### 7.5 Designed for extension
The guide cycle will add EPG to this overlay by:
- Adding an `epg_now` char buffer to each visible row's render state.
- Fetching `get_short_epg` for each favorite on overlay open (and caching
  with a TTL), filling those buffers asynchronously.
- Widening the row to `{num}  {name}  │  {epg_now.title}  {epg_now.time}`.

No structural change to this overlay's key handling, state machine, or
zap path is expected.

## 8. Help sheet update

`render.c:261` has a static `lines[]` array explicitly marked
"Keep in sync with the actual bindings in main.c." Append these two lines
(exact text):

```
  *             Toggle favorite on current channel (also works in 'f' search)
  Shift+f       Favorites list (Enter zaps, Del removes)
```

(`?` to toggle help is already listed.)

## 9. Edge cases and error handling

| Case | Behavior |
|---|---|
| `favorites.json` missing | Start empty. No log noise on first run. |
| File empty (0 bytes) | Start empty, no error. |
| Malformed JSON | Rename to `favorites.json.corrupt-<timestamp>`; start empty; stderr warning. Original bytes preserved. |
| Disk write fails on toggle | Keep the in-memory mutation (so the visible state matches what the user just did), show toast `Could not save favorites` (~2 s), stderr log. Retry is implicit: the next successful write overwrites. |
| Reconcile: id in file not in catalog, name in catalog | Rewrite id to new value. Save. Warn on stderr. |
| Reconcile: neither id nor name match catalog | Set `hidden=1`. Keep in file. Excluded from visible iterator. |
| Reconcile: catalog load failed (size 0) | Skip reconcile entirely: keep all entries visible using cached `num` and `name`. Do NOT rewrite file. Zapping still works because stream URLs only need `stream_id`. Entries with dead ids will surface as normal playback errors, same as any other stream failure. |
| Duplicate `stream_id` on file | On load, keep the first occurrence, skip the rest, stderr warning. |
| Toggle during in-flight `zap_prep` for same id | No conflict — `is_favorite` is a pure lookup; `zap_prep` does not read favorites. |
| User edits file mid-run | Not reloaded. Documented limitation. |
| User runs against a different portal | Stream ids do not transfer. Reconcile's name-fallback may or may not succeed depending on channel names. Documented as "undefined but best-effort". |
| Toggle from f-search on a non-LIVE row | No-op (only `[LIVE]` rows accept the toggle). |

## 10. Testing

### 10.1 Structure
- `tests/test_favorites.c` — plain C assertions, same pattern as
  `tests/test_npo_parse.c`.
- `tests/fixtures/favorites_*.json` — input fixtures.
- New Makefile target `test_favorites` + binary `build/test_favorites.exe`.
- `make test` builds and runs all test binaries.

### 10.2 Test-hygiene requirement
`favorites_path()` honors `$TV_FAVORITES_PATH` if set. Every persistence
test sets it to a scratch temp dir via `mkdtemp()` (or Win32 equivalent)
and cleans up on exit. Non-negotiable — guards real user data.

### 10.3 Parser robustness
- `test_missing_file_is_empty`
- `test_empty_file_is_empty`
- `test_empty_array_is_empty`
- `test_malformed_backs_up_and_resets`
- `test_unknown_fields_ignored`
- `test_missing_required_field_skipped`
- `test_unicode_in_name_preserved` (e.g. `"NPO 1 ★ HD"` round-trip)
- `test_long_name_accepted` (2 KB name)
- `test_duplicate_stream_id_dedup`
- `test_nonpositive_stream_id_rejected`
- `test_stress_10k_entries` (<100 ms)

### 10.4 Writer correctness
- `test_write_empty_produces_empty_array` — file contains `[]`, not missing, not `null`.
- `test_round_trip` — write 3, read 3, all fields identical.
- `test_write_escapes_json_specials` — `"`, `\`, `\n`, `\t` preserved round-trip.
- `test_write_is_atomic` — fault-injection between tmp write and rename leaves original untouched.
- `test_write_creates_parent_dir`.
- `test_write_utf8_encoded` — no BOM.

### 10.5 Set semantics
- `test_add_to_empty`
- `test_toggle_removes`
- `test_toggle_twice_net_zero`
- `test_is_favorite_after_add` / `test_is_favorite_after_remove`
- `test_capacity_growth` — 200 distinct ids from a 4-entry initial cap.
- `test_remove_nonpresent_is_noop`

### 10.6 Reconciliation with catalog
- `test_reconcile_id_match` — no rewrite, no disk write.
- `test_reconcile_name_fallback_rewrites_id` — id rewritten + persisted.
- `test_reconcile_both_miss_hides_entry` — `hidden=1`, still in file.
- `test_reconcile_catalog_empty_keeps_all_visible` — no id rewrite, no file write, all entries remain visible using cached fields.
- `test_reconcile_name_match_case_sensitive` — case-only difference does NOT rewrite (documented).
- `test_reconcile_determinism` — two catalog channels share a name → pick first-by-portal-num.
- `test_reconcile_idempotent` — second reconcile on unchanged data triggers no disk write.

### 10.7 Path resolution
- `test_path_env_override` — `TV_FAVORITES_PATH` used verbatim.
- `test_path_windows_appdata` (`#ifdef _WIN32`).
- `test_path_linux_xdg` (`#ifndef _WIN32`).
- `test_path_linux_xdg_fallback` — unset var → `$HOME/.config/...`.

### 10.8 End-to-end journeys
- `test_journey_save_reload` — add 3 → write → fresh read → same 3.
- `test_journey_add_remove_reload` — add 3 → remove 1 → write → fresh read → 2.
- `test_journey_random_ops_oracle` — 100 random ops against a `bool[65536]` oracle → final states match.

### 10.9 Disk-error handling
- `test_write_fail_keeps_memory_state` — bad-permissions path → toggle returns -1, but in-memory state is mutated.

### 10.10 Not unit-testable (explicit)
- SDL overlay rendering (`Shift+F` list visuals).
- Keypress dispatch in `main.c`.
- Toast display timing.

Manual smoke test covers these; matches the existing stance for
`f`-search and `epg_full` overlays.

## 11. Instrumentation hooks (optional, not v1)

Precedent: `main.c:615` already defines `TV_AUTO_PRESS_1_AT_S` and related
envs for scripted behaviour. If later we want an e2e smoke test over the
real event loop, add `TV_AUTO_TOGGLE_FAV_AT_S=3.0` to press `*` at a
scripted time. Not required for v1.

## 12. Out of scope — the live-channel guide

The guide (EPG across all favorites) is deferred to its own brainstorming
cycle. The `Shift+F` overlay in §7 is designed to accept EPG data without
structural change: add a per-row `epg_now` buffer, fetch
`xtream_fetch_epg` per favorite on overlay open with a TTL cache, widen
the row format. No new key bindings. No new overlay.

## 13. Open questions

None at spec time. If any surface during implementation, they will be
raised in the plan or the PR.
