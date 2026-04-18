#include "../src/npo.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char *slurp(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (!f) { perror(path); exit(2); }
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = malloc(sz + 1);
    fread(buf, 1, sz, f);
    buf[sz] = 0;
    fclose(f);
    if (len) *len = (size_t)sz;
    return buf;
}

static void test_parses_expected_count(void) {
    size_t n;
    char *j = slurp("tests/fixtures/epg_sample.json", &n);
    epg_t e = {0};
    assert(npo_parse_epg(j, n, &e) == 0);
    assert(e.count >= 1);
    npo_epg_free(&e);
    free(j);
    puts("OK test_parses_expected_count");
}

static void test_flags_nos_journaal(void) {
    size_t n;
    char *j = slurp("tests/fixtures/epg_sample.json", &n);
    epg_t e = {0};
    assert(npo_parse_epg(j, n, &e) == 0);

    int found_news = 0, found_non_news = 0;
    for (size_t i = 0; i < e.count; ++i) {
        if (e.entries[i].is_news) found_news = 1;
        else found_non_news = 1;
    }
    assert(found_news);
    assert(found_non_news);
    npo_epg_free(&e);
    free(j);
    puts("OK test_flags_nos_journaal");
}

static void test_times_are_parsed(void) {
    size_t n;
    char *j = slurp("tests/fixtures/epg_sample.json", &n);
    epg_t e = {0};
    assert(npo_parse_epg(j, n, &e) == 0);
    for (size_t i = 0; i < e.count; ++i) {
        assert(e.entries[i].start > 0);
        assert(e.entries[i].end   > e.entries[i].start);
    }
    npo_epg_free(&e);
    free(j);
    puts("OK test_times_are_parsed");
}

int main(void) {
    test_parses_expected_count();
    test_flags_nos_journaal();
    test_times_are_parsed();
    puts("all tests passed");
    return 0;
}
