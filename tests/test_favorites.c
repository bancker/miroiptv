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

int main(void) {
    test_path_env_override();
    test_path_defaults_nonnull();
    test_missing_file_is_empty();
    test_empty_file_is_empty();
    test_empty_array_is_empty();
    return 0;
}
