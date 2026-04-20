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
