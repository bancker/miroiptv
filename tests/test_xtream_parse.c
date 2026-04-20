/* tests/test_xtream_parse.c — 10 tests for xtream_parse() from src/xtream.c */
#include "../src/xtream.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void test_parse_basic(void) {
    xtream_t x;
    assert(xtream_parse("u:p@h", &x) == 0);
    assert(strcmp(x.user, "u") == 0);
    assert(strcmp(x.pass, "p") == 0);
    assert(strcmp(x.host, "h") == 0);
    assert(x.port == 8080);
    xtream_free(&x);
    puts("OK test_parse_basic");
}

static void test_parse_with_port(void) {
    xtream_t x;
    assert(xtream_parse("u:p@h:1234", &x) == 0);
    assert(strcmp(x.user, "u") == 0);
    assert(strcmp(x.pass, "p") == 0);
    assert(strcmp(x.host, "h") == 0);
    assert(x.port == 1234);
    xtream_free(&x);
    puts("OK test_parse_with_port");
}

static void test_parse_rejects_null(void) {
    xtream_t x;
    assert(xtream_parse(NULL, &x) == -1);
    puts("OK test_parse_rejects_null");
}

static void test_parse_rejects_empty(void) {
    xtream_t x;
    assert(xtream_parse("", &x) == -1);
    puts("OK test_parse_rejects_empty");
}

static void test_parse_rejects_no_at(void) {
    xtream_t x;
    assert(xtream_parse("userpass", &x) == -1);
    puts("OK test_parse_rejects_no_at");
}

static void test_parse_rejects_no_colon_in_userpass(void) {
    xtream_t x;
    assert(xtream_parse("userpass@host", &x) == -1);
    puts("OK test_parse_rejects_no_colon_in_userpass");
}

static void test_parse_rejects_empty_user(void) {
    xtream_t x;
    /* ':' is at position 0, which equals 'spec', so colon==spec -> -1 */
    assert(xtream_parse(":pass@host", &x) == -1);
    puts("OK test_parse_rejects_empty_user");
}

static void test_parse_rejects_empty_pass(void) {
    xtream_t x;
    /* colon+1 == at, so pass_len == 0 -> -1 */
    assert(xtream_parse("user:@host", &x) == -1);
    puts("OK test_parse_rejects_empty_pass");
}

static void test_parse_port_out_of_range_falls_back(void) {
    xtream_t x;
    /* 99999 > 65535 -> falls back to 8080 (see xtream.c:73) */
    assert(xtream_parse("u:p@h:99999", &x) == 0);
    assert(x.port == 8080);
    xtream_free(&x);
    puts("OK test_parse_port_out_of_range_falls_back");
}

static void test_parse_special_chars(void) {
    /* "user.name:p@ss@host.example.com:8080"
     *
     * strchr(spec, '@') finds the FIRST '@', which is between 'p' and 'ss'.
     * So the userpass segment is "user.name:p" -> user="user.name", pass="p".
     * host_start = "ss@host.example.com:8080"
     * strchr(host_start, ':') finds ':' before "8080"
     * host = "ss@host.example.com", port = 8080.
     *
     * This documents the actual parser behaviour for inputs that contain
     * '@' inside the password. */
    xtream_t x;
    assert(xtream_parse("user.name:p@ss@host.example.com:8080", &x) == 0);
    assert(strcmp(x.user, "user.name") == 0);
    assert(strcmp(x.pass, "p") == 0);
    /* host contains the @ because host_start begins right after the first @ */
    assert(strcmp(x.host, "ss@host.example.com") == 0);
    assert(x.port == 8080);
    xtream_free(&x);
    puts("OK test_parse_special_chars");
}

int main(void) {
    test_parse_basic();
    test_parse_with_port();
    test_parse_rejects_null();
    test_parse_rejects_empty();
    test_parse_rejects_no_at();
    test_parse_rejects_no_colon_in_userpass();
    test_parse_rejects_empty_user();
    test_parse_rejects_empty_pass();
    test_parse_port_out_of_range_falls_back();
    test_parse_special_chars();
    return 0;
}
