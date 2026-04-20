#include "../src/favorites.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#  include <windows.h>
#  include <direct.h>
#else
#  include <unistd.h>
#  include <sys/stat.h>
#  include <dirent.h>
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

static int file_exists(const char *p) {
    FILE *f = fopen(p, "rb");
    if (!f) return 0;
    fclose(f);
    return 1;
}

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

int main(void) {
    test_path_env_override();
    test_path_defaults_nonnull();
    test_missing_file_is_empty();
    test_empty_file_is_empty();
    test_empty_array_is_empty();
    test_round_trip_read_valid();
    test_edge_cases();
    test_malformed_backs_up_and_resets();
    test_write_empty_produces_empty_array();
    test_round_trip_write_read();
    test_write_escapes_json_specials();
    test_write_creates_parent_dir();
    test_write_utf8_no_bom();
    test_write_fail_keeps_memory_state();
    test_write_is_atomic_on_existing_file();
    test_is_favorite_lookup();
    test_toggle_add_remove();
    test_capacity_growth();
    test_remove_noop();
    return 0;
}
