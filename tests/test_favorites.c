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

int main(void) {
    test_path_env_override();
    test_path_defaults_nonnull();
    test_missing_file_is_empty();
    test_empty_file_is_empty();
    test_empty_array_is_empty();
    test_round_trip_read_valid();
    test_edge_cases();
    test_malformed_backs_up_and_resets();
    return 0;
}
