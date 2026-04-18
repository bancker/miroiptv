# NPO TV Player Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a C POC that plays NPO 1/2/3 live streams with an EPG overlay highlighting NOS Journaal entries.

**Architecture:** Single-process C program with three threads — main (owns SDL, renders video + overlay), decoder (owns libav, demuxes + decodes both streams), SDL audio callback (pulls from audio queue). Bounded SPSC queues connect producer and consumer threads. EPG is fetched over libcurl and parsed with cJSON from NPO's public `start-api.npo.nl` endpoints.

**Tech Stack:** C11, MSYS2/MinGW64 GCC, FFmpeg (libavformat/libavcodec/libavutil/libswscale/libswresample), SDL2, SDL2_ttf, libcurl, cJSON, GNU Make.

---

## File structure

Each file has one clear responsibility. See the design spec for full rationale.

```
tv/
├── Makefile                    # pkg-config driven build
├── .gitignore                  # build/, *.o, etc
├── README.md                   # build + run instructions
├── src/
│   ├── main.c                  # CLI, init, event loop, channel switch
│   ├── common.h                # frame/packet types, queue API
│   ├── queue.c                 # SPSC bounded queue impl (threading helpers)
│   ├── npo.h / npo.c           # libcurl + cJSON: HTTP, EPG, stream-URL resolve
│   ├── player.h / player.c     # libav: open HLS, demux + decode thread
│   ├── render.h / render.c     # SDL2 video + audio output, SDL_ttf overlay
│   └── sync.h / sync.c         # audio-master clock, pacing helpers
├── tests/
│   ├── test_npo_parse.c        # EPG JSON parsing unit test
│   └── fixtures/
│       └── epg_sample.json     # captured NPO API response
├── assets/
│   └── DejaVuSans.ttf          # overlay font (bundled)
└── build/                      # gitignored output
    └── tv.exe
```

**Responsibility boundaries:**

- `common.h` defines the data types that cross thread boundaries. Nothing else.
- `queue.c` implements a thread-safe bounded queue once. Used by video and audio queues.
- `npo.c` is pure data: HTTP in, structured data out. No SDL, no libav.
- `player.c` is pure decode: URL in, frames out onto queues. No SDL, no HTTP.
- `render.c` is pure presentation: frames in, screen/audio out. No libav, no HTTP.
- `sync.c` is a small helper module — just the clock logic.
- `main.c` is the only place that knows about *all* modules.

---

## Task 1: MSYS2 and packages

**Files:** none (system setup)

- [ ] **Step 1: Install MSYS2**

If you don't have MSYS2 yet, install via scoop (easiest):

```bash
scoop install msys2
```

Or via winget:

```powershell
winget install MSYS2.MSYS2
```

After install, launch the **"MSYS2 MinGW 64-bit"** shell (not plain MSYS2), which pre-sets the right PATH for the MinGW64 toolchain.

- [ ] **Step 2: Install build dependencies**

In the MSYS2 MinGW64 shell:

```bash
pacman -Syu  # update pacman itself; may require restart of the shell
pacman -S --needed \
  mingw-w64-x86_64-gcc \
  mingw-w64-x86_64-ffmpeg \
  mingw-w64-x86_64-SDL2 \
  mingw-w64-x86_64-SDL2_ttf \
  mingw-w64-x86_64-curl \
  mingw-w64-x86_64-cjson \
  mingw-w64-x86_64-pkgconf \
  make
```

- [ ] **Step 3: Verify the toolchain**

```bash
gcc --version
pkg-config --modversion libavformat
pkg-config --modversion sdl2
pkg-config --modversion SDL2_ttf
pkg-config --modversion libcurl
pkg-config --modversion libcjson
```

Expected: each prints a version string without error.

No commit this task.

---

## Task 2: Project skeleton and hello-world build

**Files:**
- Create: `Makefile`
- Create: `.gitignore`
- Create: `README.md`
- Create: `src/main.c`

- [ ] **Step 1: Create `Makefile`**

```makefile
# NPO TV player — Makefile
# Requires MSYS2 MinGW64 with ffmpeg, SDL2, SDL2_ttf, libcurl, cjson, pkgconf installed.

CC       = gcc
PKGS     = libavformat libavcodec libavutil libswscale libswresample sdl2 SDL2_ttf libcurl libcjson
CFLAGS   = -Wall -Wextra -Wpedantic -std=c11 $(shell pkg-config --cflags $(PKGS))
LDLIBS   = $(shell pkg-config --libs $(PKGS)) -pthread

ifdef DEBUG
CFLAGS  += -g -O0 -fsanitize=address -fno-omit-frame-pointer -DDEBUG=1
LDLIBS  += -fsanitize=address
else
CFLAGS  += -O2
endif

SRC      = $(wildcard src/*.c)
OBJ      = $(SRC:src/%.c=build/%.o)
BIN      = build/tv.exe

TEST_SRC = tests/test_npo_parse.c src/npo.c src/queue.c
TEST_BIN = build/test_npo_parse.exe

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

test: $(TEST_BIN)
	./$(TEST_BIN)

$(TEST_BIN): $(TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(TEST_SRC) -o $@ $(LDLIBS)

clean:
	rm -rf build
```

- [ ] **Step 2: Create `.gitignore`**

```
build/
*.o
*.exe
*.pdb
.vscode/
.idea/
```

- [ ] **Step 3: Create `README.md`**

````markdown
# NPO TV Player (POC)

A small C program that plays NPO 1 / 2 / 3 live via FFmpeg libraries and SDL2,
with an on-screen EPG overlay that highlights NOS Journaal.

## Build

Requires MSYS2 MinGW64 with:
```
pacman -S mingw-w64-x86_64-gcc mingw-w64-x86_64-ffmpeg \
  mingw-w64-x86_64-SDL2 mingw-w64-x86_64-SDL2_ttf \
  mingw-w64-x86_64-curl mingw-w64-x86_64-cjson \
  mingw-w64-x86_64-pkgconf make
```

Then:
```
make
./build/tv.exe
```

## Controls

- `1` / `2` / `3` — switch to NPO 1 / 2 / 3
- `e` — toggle EPG overlay
- `f` — toggle fullscreen
- `space` — pause / resume
- `q` / `Esc` — quit

## CLI flags

- `--channel NED1|NED2|NED3` — start on a specific channel
- `--stream-url <url>` — bypass NPO API and open HLS URL directly (break-glass)
- `--font <path>` — override overlay font (default: `assets/DejaVuSans.ttf`)
````

- [ ] **Step 4: Create `src/main.c` (hello world)**

```c
#include <stdio.h>

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    puts("tv: hello world");
    return 0;
}
```

- [ ] **Step 5: Build and run**

```bash
make
./build/tv.exe
```

Expected output:
```
tv: hello world
```

- [ ] **Step 6: Commit**

```bash
git add Makefile .gitignore README.md src/main.c
git commit -m "feat: project skeleton with hello-world main"
```

---

## Task 3: Shared types and the SPSC bounded queue

**Files:**
- Create: `src/common.h`
- Create: `src/queue.c`

- [ ] **Step 1: Create `src/common.h`**

```c
#ifndef TV_COMMON_H
#define TV_COMMON_H

#include <stdint.h>
#include <stddef.h>
#include <pthread.h>

/* ---- frame / audio chunk payloads ---- */

/* Raw decoded video frame (YUV420P). The decoder owns malloc, main owns free. */
typedef struct {
    int      width;
    int      height;
    int      stride_y;
    int      stride_u;
    int      stride_v;
    uint8_t *y;     /* width * height */
    uint8_t *u;     /* width/2 * height/2 */
    uint8_t *v;     /* width/2 * height/2 */
    double   pts;   /* seconds, stream time */
} video_frame_t;

/* Decoded audio chunk, s16le stereo. */
typedef struct {
    int16_t *samples;     /* interleaved L/R */
    size_t   n_samples;   /* per channel count */
    int      sample_rate;
    double   pts;         /* seconds, stream time */
} audio_chunk_t;

void video_frame_free(video_frame_t *f);
void audio_chunk_free(audio_chunk_t *c);

/* ---- SPSC bounded queue, void* payload ---- */

typedef struct {
    void           **slots;
    size_t           capacity;
    size_t           head;
    size_t           tail;
    size_t           count;
    int              closed;         /* producer said "no more" */
    pthread_mutex_t  mu;
    pthread_cond_t   not_full;
    pthread_cond_t   not_empty;
} queue_t;

int   queue_init(queue_t *q, size_t capacity);
void  queue_destroy(queue_t *q);                     /* frees remaining items with the callback in free_fn */
void  queue_destroy_with(queue_t *q, void (*free_fn)(void *));
int   queue_push(queue_t *q, void *item);            /* blocks when full; returns -1 if closed */
void *queue_pop(queue_t *q);                         /* blocks when empty; returns NULL when closed & empty */
void  queue_close(queue_t *q);                       /* wakes everyone */
void  queue_drain(queue_t *q, void (*free_fn)(void *));

#endif
```

- [ ] **Step 2: Create `src/queue.c`**

```c
#include "common.h"
#include <stdlib.h>
#include <string.h>

void video_frame_free(video_frame_t *f) {
    if (!f) return;
    free(f->y); free(f->u); free(f->v);
    free(f);
}

void audio_chunk_free(audio_chunk_t *c) {
    if (!c) return;
    free(c->samples);
    free(c);
}

int queue_init(queue_t *q, size_t capacity) {
    memset(q, 0, sizeof(*q));
    q->slots = calloc(capacity, sizeof(void *));
    if (!q->slots) return -1;
    q->capacity = capacity;
    if (pthread_mutex_init(&q->mu, NULL) != 0) return -1;
    if (pthread_cond_init(&q->not_full, NULL) != 0) return -1;
    if (pthread_cond_init(&q->not_empty, NULL) != 0) return -1;
    return 0;
}

void queue_destroy(queue_t *q) {
    queue_destroy_with(q, NULL);
}

void queue_destroy_with(queue_t *q, void (*free_fn)(void *)) {
    if (free_fn) queue_drain(q, free_fn);
    free(q->slots);
    pthread_mutex_destroy(&q->mu);
    pthread_cond_destroy(&q->not_full);
    pthread_cond_destroy(&q->not_empty);
    memset(q, 0, sizeof(*q));
}

int queue_push(queue_t *q, void *item) {
    pthread_mutex_lock(&q->mu);
    while (q->count == q->capacity && !q->closed)
        pthread_cond_wait(&q->not_full, &q->mu);
    if (q->closed) { pthread_mutex_unlock(&q->mu); return -1; }
    q->slots[q->tail] = item;
    q->tail = (q->tail + 1) % q->capacity;
    q->count++;
    pthread_cond_signal(&q->not_empty);
    pthread_mutex_unlock(&q->mu);
    return 0;
}

void *queue_pop(queue_t *q) {
    pthread_mutex_lock(&q->mu);
    while (q->count == 0 && !q->closed)
        pthread_cond_wait(&q->not_empty, &q->mu);
    if (q->count == 0) { pthread_mutex_unlock(&q->mu); return NULL; }
    void *item = q->slots[q->head];
    q->head = (q->head + 1) % q->capacity;
    q->count--;
    pthread_cond_signal(&q->not_full);
    pthread_mutex_unlock(&q->mu);
    return item;
}

void queue_close(queue_t *q) {
    pthread_mutex_lock(&q->mu);
    q->closed = 1;
    pthread_cond_broadcast(&q->not_empty);
    pthread_cond_broadcast(&q->not_full);
    pthread_mutex_unlock(&q->mu);
}

void queue_drain(queue_t *q, void (*free_fn)(void *)) {
    pthread_mutex_lock(&q->mu);
    while (q->count) {
        void *item = q->slots[q->head];
        q->head = (q->head + 1) % q->capacity;
        q->count--;
        if (free_fn) free_fn(item);
    }
    pthread_mutex_unlock(&q->mu);
}
```

- [ ] **Step 3: Build to confirm it compiles**

```bash
make
```

Expected: no errors. `main.c` doesn't use the queue yet; this task just verifies the code compiles.

- [ ] **Step 4: Commit**

```bash
git add src/common.h src/queue.c
git commit -m "feat: shared types and SPSC bounded queue"
```

---

## Task 4: HTTP fetch via libcurl

**Files:**
- Create: `src/npo.h`
- Create: `src/npo.c`

- [ ] **Step 1: Create `src/npo.h` with just the HTTP function**

```c
#ifndef TV_NPO_H
#define TV_NPO_H

#include <stddef.h>

/* Fetches `url` via HTTP GET. On success returns 0 and *out points to a
 * malloc'd NUL-terminated body; caller frees. On failure returns -1 and *out
 * is NULL. `extra_headers` may be NULL or a NULL-terminated array of header
 * strings ("Name: Value"). */
int npo_http_get(const char *url, const char *const *extra_headers,
                 char **out, size_t *out_len);

#endif
```

- [ ] **Step 2: Create `src/npo.c` with the implementation**

```c
#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct { char *data; size_t len; size_t cap; } buf_t;

static size_t on_data(void *p, size_t sz, size_t n, void *ud) {
    buf_t *b = ud;
    size_t add = sz * n;
    if (b->len + add + 1 > b->cap) {
        size_t new_cap = b->cap ? b->cap * 2 : 4096;
        while (new_cap < b->len + add + 1) new_cap *= 2;
        char *p2 = realloc(b->data, new_cap);
        if (!p2) return 0;
        b->data = p2;
        b->cap = new_cap;
    }
    memcpy(b->data + b->len, p, add);
    b->len += add;
    b->data[b->len] = '\0';
    return add;
}

int npo_http_get(const char *url, const char *const *extra_headers,
                 char **out, size_t *out_len) {
    *out = NULL;
    if (out_len) *out_len = 0;

    CURL *c = curl_easy_init();
    if (!c) return -1;

    buf_t buf = {0};
    struct curl_slist *hdrs = NULL;
    if (extra_headers) {
        for (size_t i = 0; extra_headers[i]; ++i)
            hdrs = curl_slist_append(hdrs, extra_headers[i]);
    }

    curl_easy_setopt(c, CURLOPT_URL, url);
    curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, on_data);
    curl_easy_setopt(c, CURLOPT_WRITEDATA, &buf);
    curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(c, CURLOPT_TIMEOUT, 15L);
    curl_easy_setopt(c, CURLOPT_USERAGENT, "tv/0.1 (+npo-poc)");
    if (hdrs) curl_easy_setopt(c, CURLOPT_HTTPHEADER, hdrs);

    CURLcode rc = curl_easy_perform(c);
    long code = 0;
    curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &code);
    curl_slist_free_all(hdrs);
    curl_easy_cleanup(c);

    if (rc != CURLE_OK) {
        fprintf(stderr, "http: %s: %s\n", url, curl_easy_strerror(rc));
        free(buf.data);
        return -1;
    }
    if (code < 200 || code >= 300) {
        fprintf(stderr, "http: %s: status %ld\n", url, code);
        free(buf.data);
        return -1;
    }

    *out = buf.data;
    if (out_len) *out_len = buf.len;
    return 0;
}
```

- [ ] **Step 3: Wire a quick smoke test in `src/main.c`**

Temporarily replace `main.c` with:

```c
#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    curl_global_init(CURL_GLOBAL_DEFAULT);
    char *body = NULL; size_t len = 0;
    int rc = npo_http_get("https://httpbin.org/get", NULL, &body, &len);
    if (rc == 0) {
        printf("got %zu bytes, first 200:\n%.200s\n", len, body);
    }
    free(body);
    curl_global_cleanup();
    return rc;
}
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: prints `got XXXX bytes, first 200:` followed by the start of a JSON response containing `"url": "https://httpbin.org/get"`.

- [ ] **Step 5: Commit**

```bash
git add src/npo.h src/npo.c src/main.c
git commit -m "feat: libcurl HTTP GET wrapper with smoke test"
```

---

## Task 5: EPG parsing with a TDD unit test

**Files:**
- Create: `tests/fixtures/epg_sample.json`
- Create: `tests/test_npo_parse.c`
- Modify: `src/npo.h` — add EPG types + parser
- Modify: `src/npo.c` — implement parser

- [ ] **Step 1: Capture an EPG fixture**

Fetch a real EPG response and save it:

```bash
mkdir -p tests/fixtures
curl -sSL 'https://start-api.npo.nl/epg/ned1' -o tests/fixtures/epg_sample.json || \
curl -sSL 'https://start-api.npo.nl/v2/schedule/channel/NED1' -o tests/fixtures/epg_sample.json
```

If both fail (API moved), craft a minimal fixture by hand matching the *expected* shape — the parser will be written to match the fixture. Example minimal fixture you can paste instead:

```json
{
  "schedule": [
    {
      "id": "a1",
      "title": "Tijd voor MAX",
      "start": "2026-04-18T17:00:00+02:00",
      "end":   "2026-04-18T17:30:00+02:00"
    },
    {
      "id": "a2",
      "title": "NOS Journaal",
      "start": "2026-04-18T18:00:00+02:00",
      "end":   "2026-04-18T18:15:00+02:00"
    },
    {
      "id": "a3",
      "title": "EenVandaag",
      "start": "2026-04-18T18:15:00+02:00",
      "end":   "2026-04-18T18:45:00+02:00"
    }
  ]
}
```

If you had to craft the fixture by hand, note it in `docs/superpowers/specs/2026-04-18-npo-tv-player-design.md` under "Open questions" — it means the real API shape needs to be confirmed at runtime and the parser may need adjustment.

- [ ] **Step 2: Extend `src/npo.h` with EPG types**

Add to `src/npo.h`:

```c
#include <time.h>

typedef struct {
    char   *id;
    char   *title;
    time_t  start;   /* unix time */
    time_t  end;
    int     is_news; /* 1 when title starts with "NOS Journaal" (case-insensitive) */
} epg_entry_t;

typedef struct {
    epg_entry_t *entries;
    size_t       count;
} epg_t;

int  npo_parse_epg(const char *json, size_t json_len, epg_t *out);
void npo_epg_free(epg_t *e);
```

- [ ] **Step 3: Write the failing test `tests/test_npo_parse.c`**

```c
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
```

- [ ] **Step 4: Run the test — expect it to fail at compile time**

```bash
make test
```

Expected: link error on `npo_parse_epg` / `npo_epg_free` (undefined reference). That's our failing test.

- [ ] **Step 5: Implement `npo_parse_epg` and `npo_epg_free` in `src/npo.c`**

Add at the top of `src/npo.c`:

```c
#include <cjson/cJSON.h>
#include <strings.h>  /* strcasecmp; on MinGW64 available via <string.h>, but be explicit */
#include <time.h>
```

On MinGW64, use `_stricmp` if `strcasecmp` is missing:

```c
#ifndef HAVE_STRCASECMP
#  ifdef _WIN32
#    define strcasecmp _stricmp
#    define strncasecmp _strnicmp
#  endif
#endif
```

Then implement:

```c
/* Parses "2026-04-18T18:00:00+02:00" → time_t (UTC). Returns 0 on success. */
static int parse_iso8601(const char *s, time_t *out) {
    if (!s) return -1;
    int Y, M, D, h, m, sec, tzh = 0, tzm = 0;
    char tzsign = '+';
    int n = sscanf(s, "%d-%d-%dT%d:%d:%d%c%d:%d",
                   &Y, &M, &D, &h, &m, &sec, &tzsign, &tzh, &tzm);
    if (n < 6) return -1;
    struct tm t = {0};
    t.tm_year = Y - 1900;
    t.tm_mon  = M - 1;
    t.tm_mday = D;
    t.tm_hour = h;
    t.tm_min  = m;
    t.tm_sec  = sec;
    /* timegm isn't portable on MinGW; compute from _mkgmtime, then subtract tz. */
#ifdef _WIN32
    time_t utc = _mkgmtime(&t);
#else
    time_t utc = timegm(&t);
#endif
    if (n >= 9) {
        int off = (tzh * 60 + tzm) * 60;
        if (tzsign == '-') off = -off;
        utc -= off;
    }
    *out = utc;
    return 0;
}

static int title_is_news(const char *t) {
    return t && strncasecmp(t, "NOS Journaal", 12) == 0;
}

/* Walks a JSON tree that may have its entries under various keys: "schedule",
 * "items", "data", or the root being an array. Returns an array of entries. */
static const cJSON *find_entries_array(const cJSON *root) {
    if (cJSON_IsArray(root)) return root;
    const char *keys[] = { "schedule", "items", "data", "epg", "broadcasts", NULL };
    for (size_t i = 0; keys[i]; ++i) {
        cJSON *a = cJSON_GetObjectItemCaseSensitive(root, keys[i]);
        if (cJSON_IsArray(a)) return a;
    }
    return NULL;
}

static char *jstrdup(const cJSON *o, const char *key) {
    cJSON *s = cJSON_GetObjectItemCaseSensitive(o, key);
    if (!cJSON_IsString(s) || !s->valuestring) return NULL;
    return strdup(s->valuestring);
}

int npo_parse_epg(const char *json, size_t json_len, epg_t *out) {
    (void)json_len;
    memset(out, 0, sizeof(*out));
    cJSON *root = cJSON_Parse(json);
    if (!root) return -1;
    const cJSON *arr = find_entries_array(root);
    if (!arr) { cJSON_Delete(root); return -1; }

    size_t n = cJSON_GetArraySize(arr);
    out->entries = calloc(n, sizeof(epg_entry_t));
    if (!out->entries) { cJSON_Delete(root); return -1; }

    size_t written = 0;
    cJSON *item = NULL;
    cJSON_ArrayForEach(item, arr) {
        const char *title_keys[] = { "title", "programTitle", "name", NULL };
        const char *start_keys[] = { "start", "startDate", "from", "starttime", NULL };
        const char *end_keys[]   = { "end", "endDate", "until", "endtime", NULL };
        const char *id_keys[]    = { "id", "guid", "mid", NULL };

        char *title = NULL, *start_s = NULL, *end_s = NULL, *id = NULL;
        for (size_t i = 0; title_keys[i] && !title; ++i) title   = jstrdup(item, title_keys[i]);
        for (size_t i = 0; start_keys[i] && !start_s; ++i) start_s = jstrdup(item, start_keys[i]);
        for (size_t i = 0; end_keys[i]   && !end_s;   ++i) end_s   = jstrdup(item, end_keys[i]);
        for (size_t i = 0; id_keys[i]    && !id;      ++i) id      = jstrdup(item, id_keys[i]);

        if (!title || !start_s || !end_s) {
            free(title); free(start_s); free(end_s); free(id);
            continue;
        }

        time_t start, end;
        if (parse_iso8601(start_s, &start) != 0 || parse_iso8601(end_s, &end) != 0) {
            free(title); free(start_s); free(end_s); free(id);
            continue;
        }

        out->entries[written].id      = id ? id : strdup("");
        out->entries[written].title   = title;
        out->entries[written].start   = start;
        out->entries[written].end     = end;
        out->entries[written].is_news = title_is_news(title);
        written++;
        free(start_s); free(end_s);
    }

    out->count = written;
    cJSON_Delete(root);
    return 0;
}

void npo_epg_free(epg_t *e) {
    if (!e) return;
    for (size_t i = 0; i < e->count; ++i) {
        free(e->entries[i].id);
        free(e->entries[i].title);
    }
    free(e->entries);
    memset(e, 0, sizeof(*e));
}
```

- [ ] **Step 6: Run the test — expect it to pass**

```bash
make test
```

Expected output:
```
OK test_parses_expected_count
OK test_flags_nos_journaal
OK test_times_are_parsed
all tests passed
```

If it fails because the real API shape doesn't match (e.g., different key names), inspect `tests/fixtures/epg_sample.json` and add the correct key to the `*_keys[]` arrays in `npo_parse_epg`.

- [ ] **Step 7: Commit**

```bash
git add tests/ src/npo.h src/npo.c
git commit -m "feat: EPG JSON parser with NOS Journaal flagging + unit test"
```

---

## Task 6: NPO stream-URL resolver

**Files:**
- Modify: `src/npo.h` — add channel types + resolver
- Modify: `src/npo.c` — implement resolver
- Modify: `src/main.c` — smoke-test resolver

- [ ] **Step 1: Extend `src/npo.h`**

Add:

```c
typedef struct {
    const char *display;    /* "NPO 1" */
    const char *code;       /* "NED1" — used for EPG */
    const char *product_id; /* "LI_NL1_4188102" — used for stream-URL resolve */
} npo_channel_t;

extern const npo_channel_t NPO_CHANNELS[];
extern const size_t        NPO_CHANNELS_COUNT;

/* Resolves channel to an HLS manifest URL. On success returns 0 and *out_url
 * points to a malloc'd URL (caller frees). On failure returns -1 with logging. */
int npo_resolve_stream(const npo_channel_t *ch, char **out_url);

/* Fetches EPG for channel. 0 on success, -1 on failure. */
int npo_fetch_epg(const npo_channel_t *ch, epg_t *out);
```

- [ ] **Step 2: Implement in `src/npo.c`**

Add:

```c
const npo_channel_t NPO_CHANNELS[] = {
    { "NPO 1", "NED1", "LI_NL1_4188102" },
    { "NPO 2", "NED2", "LI_NL1_4188103" },
    { "NPO 3", "NED3", "LI_NL1_4188105" },
};
const size_t NPO_CHANNELS_COUNT = sizeof(NPO_CHANNELS) / sizeof(NPO_CHANNELS[0]);

/* --- stream URL resolver ---
 * NPO's public player works in two steps:
 *   1. GET  https://npo.nl/start/api/domain/player-token?productId=<PID>
 *      Response JSON contains { "jwt": "...", "token": "..." } (keys vary; scan).
 *   2. POST https://prod.npoplayer.nl/stream-link  (or similar)
 *      Body: { "profileName": "dash", "drmType": "widevine" ... }
 *      Headers: Authorization: Bearer <jwt>
 *      Response: JSON with { "stream": { "streamURL": "..." } }
 *
 * Because this API is undocumented, the exact shape may shift. The strategy:
 *   - do step 1; scan for any string that looks like a JWT (three '.'-separated b64 parts).
 *   - do step 2 with Authorization: Bearer <jwt>; scan response for any
 *     "streamURL" / "url" / "manifest" key with a value ending in ".m3u8".
 *   - if step 2 gives a DASH URL (.mpd) instead, retry with a profile hint ("hls").
 *   - if resolution fails, return -1 and let the caller fall back to --stream-url.
 */

static char *find_json_string(const cJSON *node, const char *const *keys) {
    if (!node || !cJSON_IsObject(node)) return NULL;
    for (size_t i = 0; keys[i]; ++i) {
        cJSON *v = cJSON_GetObjectItemCaseSensitive(node, keys[i]);
        if (cJSON_IsString(v) && v->valuestring) return strdup(v->valuestring);
    }
    /* recurse */
    cJSON *child = node->child;
    while (child) {
        char *r = find_json_string(child, keys);
        if (r) return r;
        child = child->next;
    }
    return NULL;
}

static char *find_hls_url(const cJSON *node) {
    if (!node) return NULL;
    if (cJSON_IsString(node) && node->valuestring) {
        if (strstr(node->valuestring, ".m3u8")) return strdup(node->valuestring);
    }
    cJSON *child = node ? node->child : NULL;
    while (child) {
        char *r = find_hls_url(child);
        if (r) return r;
        child = child->next;
    }
    return NULL;
}

int npo_resolve_stream(const npo_channel_t *ch, char **out_url) {
    *out_url = NULL;

    /* Step 1: player-token */
    char token_url[512];
    snprintf(token_url, sizeof(token_url),
        "https://npo.nl/start/api/domain/player-token?productId=%s",
        ch->product_id);

    char *token_body = NULL; size_t token_len = 0;
    if (npo_http_get(token_url, NULL, &token_body, &token_len) != 0) {
        fprintf(stderr, "resolve: player-token fetch failed for %s\n", ch->display);
        return -1;
    }

    cJSON *root = cJSON_Parse(token_body);
    if (!root) {
        fprintf(stderr, "resolve: player-token JSON parse failed\n");
        free(token_body);
        return -1;
    }
    const char *jwt_keys[] = { "jwt", "token", "playerToken", NULL };
    char *jwt = find_json_string(root, jwt_keys);
    cJSON_Delete(root);
    free(token_body);
    if (!jwt) {
        fprintf(stderr, "resolve: no JWT/token found in player-token response\n");
        return -1;
    }

    /* Step 2: stream-link */
    char auth[1024];
    snprintf(auth, sizeof(auth), "Authorization: Bearer %s", jwt);
    const char *headers[] = {
        auth,
        "Content-Type: application/json",
        NULL
    };

    /* Use the known stream-link endpoint; if this changes, consult the Kodi NPO
     * plugin's current source as reference: https://github.com/add-ons/plugin.video.npo */
    char *body = NULL; size_t blen = 0;
    const char *stream_url_endpoint = "https://prod.npoplayer.nl/stream-link";
    int rc = npo_http_get(stream_url_endpoint, headers, &body, &blen);
    if (rc != 0) {
        fprintf(stderr, "resolve: stream-link fetch failed\n");
        free(jwt);
        return -1;
    }

    cJSON *sroot = cJSON_Parse(body);
    if (!sroot) {
        fprintf(stderr, "resolve: stream-link JSON parse failed\n");
        free(body); free(jwt);
        return -1;
    }
    char *hls = find_hls_url(sroot);
    cJSON_Delete(sroot);
    free(body);
    free(jwt);

    if (!hls) {
        fprintf(stderr, "resolve: no HLS URL in stream-link response\n");
        return -1;
    }
    *out_url = hls;
    return 0;
}

int npo_fetch_epg(const npo_channel_t *ch, epg_t *out) {
    char url[256];
    snprintf(url, sizeof(url),
             "https://start-api.npo.nl/v2/schedule/channel/%s", ch->code);
    char *body = NULL; size_t len = 0;
    if (npo_http_get(url, NULL, &body, &len) != 0) return -1;
    int rc = npo_parse_epg(body, len, out);
    free(body);
    return rc;
}
```

- [ ] **Step 3: Smoke test in `src/main.c`**

Temporarily replace `main.c` with:

```c
#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    curl_global_init(CURL_GLOBAL_DEFAULT);
    const npo_channel_t *ch = &NPO_CHANNELS[0];

    puts("resolving stream URL...");
    char *url = NULL;
    if (npo_resolve_stream(ch, &url) == 0) {
        printf("stream: %s\n", url);
        free(url);
    } else {
        puts("resolve failed — use --stream-url override when implementing CLI");
    }

    puts("fetching EPG...");
    epg_t e = {0};
    if (npo_fetch_epg(ch, &e) == 0) {
        for (size_t i = 0; i < e.count; ++i) {
            printf("  %s  %s %s\n",
                   e.entries[i].is_news ? "[NEWS]" : "      ",
                   e.entries[i].title,
                   e.entries[i].is_news ? "<- journaal" : "");
        }
    } else {
        puts("epg fetch failed");
    }
    npo_epg_free(&e);

    curl_global_cleanup();
    return 0;
}
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: prints either a `.m3u8` URL (success) or an error message locating the exact step. Also prints EPG entries with `[NEWS]` prefix on NOS Journaal lines.

If the stream-URL resolution fails, that's **risk R1 from the spec materialising**. Write down the failing step in your notes and move on — Task 13's `--stream-url` flag is your escape hatch.

- [ ] **Step 5: Sanity-check with ffplay**

If you got a URL, confirm it plays:

```bash
ffplay "<url from step 4>"
```

A window should open and show NPO 1 within a few seconds.

- [ ] **Step 6: Commit**

```bash
git add src/npo.h src/npo.c src/main.c
git commit -m "feat: NPO stream-URL resolver and EPG fetcher with smoke test"
```

---

## Task 7: SDL window + event loop

**Files:**
- Create: `src/render.h`
- Create: `src/render.c`
- Modify: `src/main.c`

- [ ] **Step 1: Create `src/render.h`**

```c
#ifndef TV_RENDER_H
#define TV_RENDER_H

#include <SDL2/SDL.h>
#include <stdbool.h>

typedef struct {
    SDL_Window   *window;
    SDL_Renderer *renderer;
    int           width;
    int           height;
    bool          fullscreen;
} render_t;

int  render_init(render_t *r, int w, int h, const char *title);
void render_shutdown(render_t *r);
void render_toggle_fullscreen(render_t *r);

#endif
```

- [ ] **Step 2: Create `src/render.c`**

```c
#include "render.h"
#include <stdio.h>

int render_init(render_t *r, int w, int h, const char *title) {
    if (SDL_Init(SDL_INIT_VIDEO | SDL_INIT_AUDIO | SDL_INIT_EVENTS) != 0) {
        fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
        return -1;
    }
    r->width = w; r->height = h; r->fullscreen = false;
    r->window = SDL_CreateWindow(title,
        SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED,
        w, h, SDL_WINDOW_SHOWN | SDL_WINDOW_RESIZABLE);
    if (!r->window) { fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError()); return -1; }
    r->renderer = SDL_CreateRenderer(r->window, -1,
        SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
    if (!r->renderer) { fprintf(stderr, "SDL_CreateRenderer: %s\n", SDL_GetError()); return -1; }
    return 0;
}

void render_shutdown(render_t *r) {
    if (r->renderer) SDL_DestroyRenderer(r->renderer);
    if (r->window)   SDL_DestroyWindow(r->window);
    SDL_Quit();
}

void render_toggle_fullscreen(render_t *r) {
    r->fullscreen = !r->fullscreen;
    SDL_SetWindowFullscreen(r->window,
        r->fullscreen ? SDL_WINDOW_FULLSCREEN_DESKTOP : 0);
}
```

- [ ] **Step 3: Replace `src/main.c` with an SDL-loop**

```c
#include "render.h"
#include <SDL2/SDL.h>
#include <stdio.h>

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    render_t r = {0};
    if (render_init(&r, 960, 540, "tv — NPO") != 0) return 1;

    bool running = true;
    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            switch (ev.type) {
            case SDL_QUIT: running = false; break;
            case SDL_KEYDOWN:
                if (ev.key.keysym.sym == SDLK_q || ev.key.keysym.sym == SDLK_ESCAPE)
                    running = false;
                else if (ev.key.keysym.sym == SDLK_f)
                    render_toggle_fullscreen(&r);
                break;
            }
        }
        SDL_SetRenderDrawColor(r.renderer, 20, 20, 30, 255);
        SDL_RenderClear(r.renderer);
        SDL_RenderPresent(r.renderer);
        SDL_Delay(16);
    }

    render_shutdown(&r);
    return 0;
}
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: a dark-blue-grey 960×540 window opens titled "tv — NPO". `f` toggles fullscreen. `q` or `Esc` closes.

- [ ] **Step 5: Commit**

```bash
git add src/render.h src/render.c src/main.c
git commit -m "feat: SDL window + event loop (quit, fullscreen)"
```

---

## Task 8: libav — open HLS and probe streams

**Files:**
- Create: `src/player.h`
- Create: `src/player.c`
- Modify: `src/main.c` — temporary probe

- [ ] **Step 1: Create `src/player.h`**

```c
#ifndef TV_PLAYER_H
#define TV_PLAYER_H

#include "common.h"
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libswscale/swscale.h>
#include <libswresample/swresample.h>
#include <pthread.h>

typedef struct {
    AVFormatContext  *fmt;
    int               video_idx;
    int               audio_idx;
    AVCodecContext   *vctx;
    AVCodecContext   *actx;
    struct SwsContext *sws;         /* to YUV420P if needed */
    struct SwrContext *swr;         /* to s16 stereo 48k */

    queue_t           video_q;      /* holds video_frame_t* */
    queue_t           audio_q;      /* holds audio_chunk_t* */

    pthread_t         thread;
    volatile int      stop;
    int               audio_sample_rate_out;
} player_t;

int  player_open(player_t *p, const char *url);
void player_close(player_t *p);
int  player_start(player_t *p);            /* spawns decoder thread */
void player_stop(player_t *p);             /* signals stop, joins */

#endif
```

- [ ] **Step 2: Create `src/player.c` — init/close only (no decode thread yet)**

```c
#include "player.h"
#include <stdio.h>
#include <string.h>

int player_open(player_t *p, const char *url) {
    memset(p, 0, sizeof(*p));
    p->video_idx = -1;
    p->audio_idx = -1;
    p->audio_sample_rate_out = 48000;

    if (avformat_open_input(&p->fmt, url, NULL, NULL) != 0) {
        fprintf(stderr, "avformat_open_input failed for %s\n", url);
        return -1;
    }
    if (avformat_find_stream_info(p->fmt, NULL) < 0) {
        fprintf(stderr, "avformat_find_stream_info failed\n");
        return -1;
    }

    for (unsigned i = 0; i < p->fmt->nb_streams; ++i) {
        AVStream *s = p->fmt->streams[i];
        if (s->codecpar->codec_type == AVMEDIA_TYPE_VIDEO && p->video_idx < 0)
            p->video_idx = (int)i;
        else if (s->codecpar->codec_type == AVMEDIA_TYPE_AUDIO && p->audio_idx < 0)
            p->audio_idx = (int)i;
    }
    if (p->video_idx < 0 || p->audio_idx < 0) {
        fprintf(stderr, "need both video and audio streams (v=%d a=%d)\n",
                p->video_idx, p->audio_idx);
        return -1;
    }

    /* Open video decoder */
    AVCodecParameters *vpar = p->fmt->streams[p->video_idx]->codecpar;
    const AVCodec *vc = avcodec_find_decoder(vpar->codec_id);
    if (!vc) { fprintf(stderr, "no decoder for video codec id %d\n", vpar->codec_id); return -1; }
    p->vctx = avcodec_alloc_context3(vc);
    avcodec_parameters_to_context(p->vctx, vpar);
    if (avcodec_open2(p->vctx, vc, NULL) < 0) return -1;

    /* Open audio decoder */
    AVCodecParameters *apar = p->fmt->streams[p->audio_idx]->codecpar;
    const AVCodec *ac = avcodec_find_decoder(apar->codec_id);
    if (!ac) { fprintf(stderr, "no decoder for audio codec id %d\n", apar->codec_id); return -1; }
    p->actx = avcodec_alloc_context3(ac);
    avcodec_parameters_to_context(p->actx, apar);
    if (avcodec_open2(p->actx, ac, NULL) < 0) return -1;

    if (queue_init(&p->video_q, 16) != 0) return -1;
    if (queue_init(&p->audio_q, 32) != 0) return -1;
    return 0;
}

void player_close(player_t *p) {
    if (p->sws) sws_freeContext(p->sws);
    if (p->swr) swr_free(&p->swr);
    if (p->vctx) avcodec_free_context(&p->vctx);
    if (p->actx) avcodec_free_context(&p->actx);
    if (p->fmt)  avformat_close_input(&p->fmt);
    queue_destroy_with(&p->video_q, (void(*)(void*))video_frame_free);
    queue_destroy_with(&p->audio_q, (void(*)(void*))audio_chunk_free);
    memset(p, 0, sizeof(*p));
}

/* player_start / player_stop are stubs in this task; next task fills them. */
int  player_start(player_t *p) { (void)p; return 0; }
void player_stop(player_t *p)  { (void)p; }
```

- [ ] **Step 3: Probe in `main.c`**

Replace `main.c` with a probe variant (we'll get back to SDL in task 9):

```c
#include "player.h"
#include "npo.h"
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    const char *url = (argc > 1) ? argv[1] : NULL;
    char *resolved = NULL;

    curl_global_init(CURL_GLOBAL_DEFAULT);
    if (!url) {
        if (npo_resolve_stream(&NPO_CHANNELS[0], &resolved) != 0) return 2;
        url = resolved;
    }
    printf("url: %s\n", url);

    player_t p;
    if (player_open(&p, url) != 0) { free(resolved); return 3; }

    printf("video: %dx%d, codec=%s\n", p.vctx->width, p.vctx->height,
           avcodec_get_name(p.vctx->codec_id));
    printf("audio: %d Hz, %d ch, codec=%s\n", p.actx->sample_rate,
           p.actx->ch_layout.nb_channels,
           avcodec_get_name(p.actx->codec_id));

    player_close(&p);
    free(resolved);
    curl_global_cleanup();
    return 0;
}
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: prints the URL, then something like:
```
video: 1280x720, codec=h264
audio: 48000 Hz, 2 ch, codec=aac
```

- [ ] **Step 5: Commit**

```bash
git add src/player.h src/player.c src/main.c
git commit -m "feat: libav stream open + probe (video/audio indices + decoders ready)"
```

---

## Task 9: Decoder thread + video rendering

**Files:**
- Modify: `src/player.c` — implement decoder thread
- Modify: `src/render.h` — add video-render helpers
- Modify: `src/render.c` — YV12 texture upload
- Modify: `src/main.c` — wire SDL + player together (video only first)

- [ ] **Step 1: Implement the decoder thread in `src/player.c`**

Replace the stub `player_start` / `player_stop` with:

```c
static void alloc_frame_copy_yuv(const AVFrame *in, video_frame_t *out) {
    out->width  = in->width;
    out->height = in->height;
    out->stride_y = in->width;
    out->stride_u = in->width / 2;
    out->stride_v = in->width / 2;
    out->y = malloc((size_t)out->stride_y * in->height);
    out->u = malloc((size_t)out->stride_u * in->height / 2);
    out->v = malloc((size_t)out->stride_v * in->height / 2);
    for (int r = 0; r < in->height; ++r)
        memcpy(out->y + r * out->stride_y, in->data[0] + r * in->linesize[0], out->stride_y);
    for (int r = 0; r < in->height / 2; ++r)
        memcpy(out->u + r * out->stride_u, in->data[1] + r * in->linesize[1], out->stride_u);
    for (int r = 0; r < in->height / 2; ++r)
        memcpy(out->v + r * out->stride_v, in->data[2] + r * in->linesize[2], out->stride_v);
}

static double ts_to_seconds(int64_t pts, AVRational tb) {
    if (pts == AV_NOPTS_VALUE) return 0.0;
    return (double)pts * tb.num / tb.den;
}

static void push_video_frame(player_t *p, AVFrame *frame) {
    /* For POC we expect YUV420P. If not, the stream decoder produced something
     * exotic — swscale on the fly to YUV420P. */
    if (frame->format != AV_PIX_FMT_YUV420P) {
        if (!p->sws) {
            p->sws = sws_getContext(frame->width, frame->height, frame->format,
                                    frame->width, frame->height, AV_PIX_FMT_YUV420P,
                                    SWS_BILINEAR, NULL, NULL, NULL);
        }
        AVFrame *conv = av_frame_alloc();
        conv->format = AV_PIX_FMT_YUV420P;
        conv->width  = frame->width;
        conv->height = frame->height;
        av_frame_get_buffer(conv, 32);
        sws_scale(p->sws, (const uint8_t *const *)frame->data, frame->linesize,
                  0, frame->height, conv->data, conv->linesize);
        conv->pts = frame->pts;
        video_frame_t *vf = calloc(1, sizeof(*vf));
        alloc_frame_copy_yuv(conv, vf);
        vf->pts = ts_to_seconds(conv->pts, p->fmt->streams[p->video_idx]->time_base);
        av_frame_free(&conv);
        queue_push(&p->video_q, vf);
        return;
    }
    video_frame_t *vf = calloc(1, sizeof(*vf));
    alloc_frame_copy_yuv(frame, vf);
    vf->pts = ts_to_seconds(frame->pts, p->fmt->streams[p->video_idx]->time_base);
    queue_push(&p->video_q, vf);
}

static void push_audio_frame(player_t *p, AVFrame *frame) {
    if (!p->swr) {
        AVChannelLayout out_layout = AV_CHANNEL_LAYOUT_STEREO;
        swr_alloc_set_opts2(&p->swr,
            &out_layout, AV_SAMPLE_FMT_S16, p->audio_sample_rate_out,
            &p->actx->ch_layout, p->actx->sample_fmt, p->actx->sample_rate,
            0, NULL);
        swr_init(p->swr);
    }
    int max_out_samples = (int)av_rescale_rnd(
        swr_get_delay(p->swr, p->actx->sample_rate) + frame->nb_samples,
        p->audio_sample_rate_out, p->actx->sample_rate, AV_ROUND_UP);
    int16_t *out_buf = malloc((size_t)max_out_samples * 2 * sizeof(int16_t));
    uint8_t *out_ptrs[1] = { (uint8_t *)out_buf };
    int written = swr_convert(p->swr, out_ptrs, max_out_samples,
                              (const uint8_t **)frame->extended_data, frame->nb_samples);
    if (written <= 0) { free(out_buf); return; }
    audio_chunk_t *ac = calloc(1, sizeof(*ac));
    ac->samples     = out_buf;
    ac->n_samples   = written;
    ac->sample_rate = p->audio_sample_rate_out;
    ac->pts         = ts_to_seconds(frame->pts, p->fmt->streams[p->audio_idx]->time_base);
    queue_push(&p->audio_q, ac);
}

static void *decoder_loop(void *ud) {
    player_t *p = ud;
    AVPacket *pkt = av_packet_alloc();
    AVFrame  *fr  = av_frame_alloc();

    while (!p->stop) {
        int rc = av_read_frame(p->fmt, pkt);
        if (rc < 0) {
            if (rc == AVERROR_EOF) break;
            fprintf(stderr, "av_read_frame: %d\n", rc);
            break;
        }
        AVCodecContext *ctx = NULL;
        int is_video = 0;
        if (pkt->stream_index == p->video_idx) { ctx = p->vctx; is_video = 1; }
        else if (pkt->stream_index == p->audio_idx) { ctx = p->actx; }
        if (!ctx) { av_packet_unref(pkt); continue; }

        if (avcodec_send_packet(ctx, pkt) == 0) {
            while (avcodec_receive_frame(ctx, fr) == 0) {
                if (is_video) push_video_frame(p, fr);
                else          push_audio_frame(p, fr);
                av_frame_unref(fr);
            }
        }
        av_packet_unref(pkt);
    }

    queue_close(&p->video_q);
    queue_close(&p->audio_q);
    av_frame_free(&fr);
    av_packet_free(&pkt);
    return NULL;
}

int player_start(player_t *p) {
    p->stop = 0;
    return pthread_create(&p->thread, NULL, decoder_loop, p);
}

void player_stop(player_t *p) {
    p->stop = 1;
    queue_close(&p->video_q);
    queue_close(&p->audio_q);
    pthread_join(p->thread, NULL);
}
```

- [ ] **Step 2: Add a video-texture helper to `render.h` / `render.c`**

In `render.h` add:

```c
typedef struct {
    SDL_Texture *texture;
    int          width;
    int          height;
} video_tex_t;

void video_tex_init(video_tex_t *t);
int  video_tex_upload(video_tex_t *t, SDL_Renderer *r, const video_frame_t *f);
void video_tex_destroy(video_tex_t *t);
```

Include `common.h` at the top. In `render.c` add:

```c
#include "common.h"

void video_tex_init(video_tex_t *t) {
    t->texture = NULL; t->width = t->height = 0;
}

static int ensure_texture(video_tex_t *t, SDL_Renderer *r, int w, int h) {
    if (t->texture && t->width == w && t->height == h) return 0;
    if (t->texture) SDL_DestroyTexture(t->texture);
    t->texture = SDL_CreateTexture(r, SDL_PIXELFORMAT_IYUV,
                                   SDL_TEXTUREACCESS_STREAMING, w, h);
    if (!t->texture) { fprintf(stderr, "SDL_CreateTexture: %s\n", SDL_GetError()); return -1; }
    t->width = w; t->height = h;
    return 0;
}

int video_tex_upload(video_tex_t *t, SDL_Renderer *r, const video_frame_t *f) {
    if (ensure_texture(t, r, f->width, f->height) != 0) return -1;
    return SDL_UpdateYUVTexture(t->texture, NULL,
        f->y, f->stride_y, f->u, f->stride_u, f->v, f->stride_v);
}

void video_tex_destroy(video_tex_t *t) {
    if (t->texture) SDL_DestroyTexture(t->texture);
    t->texture = NULL;
}
```

- [ ] **Step 3: Wire it all up in `src/main.c` (video only, no audio yet)**

```c
#include "render.h"
#include "player.h"
#include "npo.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    curl_global_init(CURL_GLOBAL_DEFAULT);

    char *url = NULL;
    if (npo_resolve_stream(&NPO_CHANNELS[0], &url) != 0) {
        fprintf(stderr, "resolve failed\n");
        return 2;
    }
    printf("stream: %s\n", url);

    player_t pl;
    if (player_open(&pl, url) != 0) { free(url); return 3; }
    if (player_start(&pl) != 0)     { free(url); player_close(&pl); return 3; }

    render_t r;
    if (render_init(&r, pl.vctx->width, pl.vctx->height, "tv — NPO 1") != 0) return 4;
    video_tex_t tex; video_tex_init(&tex);

    bool running = true;
    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) running = false;
            else if (ev.type == SDL_KEYDOWN) {
                if (ev.key.keysym.sym == SDLK_q || ev.key.keysym.sym == SDLK_ESCAPE) running = false;
                else if (ev.key.keysym.sym == SDLK_f) render_toggle_fullscreen(&r);
            }
        }

        /* non-blocking pop: drain up to N frames per iteration to catch up if lagging */
        video_frame_t *vf = queue_pop(&pl.video_q);
        if (!vf) { running = false; break; }

        video_tex_upload(&tex, r.renderer, vf);
        SDL_RenderClear(r.renderer);
        SDL_RenderCopy(r.renderer, tex.texture, NULL, NULL);
        SDL_RenderPresent(r.renderer);
        video_frame_free(vf);

        /* naive pacing (no sync yet): 40ms/frame; audio clock replaces this in task 11 */
        SDL_Delay(40);
    }

    player_stop(&pl);
    video_tex_destroy(&tex);
    render_shutdown(&r);
    player_close(&pl);
    free(url);
    curl_global_cleanup();
    return 0;
}
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: a window opens sized to the stream resolution and shows NPO 1 video. **No audio yet.** Video may run a little fast or slow (no sync yet). `q` / `Esc` quits.

- [ ] **Step 5: Commit**

```bash
git add src/player.c src/render.h src/render.c src/main.c
git commit -m "feat: decoder thread + SDL YV12 rendering (video only, no sync)"
```

---

## Task 10: Audio playback

**Files:**
- Modify: `src/render.h` — add audio opener
- Modify: `src/render.c` — SDL audio init + callback
- Modify: `src/main.c` — plug audio in

- [ ] **Step 1: Extend `src/render.h`**

```c
typedef struct {
    SDL_AudioDeviceID device;
    int               sample_rate;
    queue_t          *q;              /* audio_chunk_t* queue we pull from */

    /* partial-chunk carry-over */
    const int16_t    *cur_samples;    /* points into cur->samples */
    size_t            cur_remaining;  /* per channel */
    audio_chunk_t    *cur;            /* currently consumed chunk */
    volatile int64_t  samples_played; /* monotonically increasing, guarded by device lock */
} audio_out_t;

int  audio_open(audio_out_t *ao, queue_t *q, int sample_rate);
void audio_close(audio_out_t *ao);
```

- [ ] **Step 2: Implement in `src/render.c`**

```c
static void audio_callback(void *ud, uint8_t *stream, int len) {
    audio_out_t *ao = ud;
    int16_t *out = (int16_t *)stream;
    int need_samples = len / (int)sizeof(int16_t) / 2; /* per channel */

    while (need_samples > 0) {
        if (ao->cur_remaining == 0) {
            if (ao->cur) { audio_chunk_free(ao->cur); ao->cur = NULL; }
            audio_chunk_t *c = queue_pop(ao->q);
            if (!c) {
                memset(out, 0, (size_t)need_samples * 2 * sizeof(int16_t));
                return;
            }
            ao->cur = c;
            ao->cur_samples   = c->samples;
            ao->cur_remaining = c->n_samples;
        }
        int take = need_samples < (int)ao->cur_remaining ? need_samples : (int)ao->cur_remaining;
        memcpy(out, ao->cur_samples, (size_t)take * 2 * sizeof(int16_t));
        out               += take * 2;
        ao->cur_samples   += take * 2;
        ao->cur_remaining -= take;
        need_samples      -= take;
        ao->samples_played += take;
    }
}

int audio_open(audio_out_t *ao, queue_t *q, int sample_rate) {
    memset(ao, 0, sizeof(*ao));
    ao->q = q; ao->sample_rate = sample_rate;

    SDL_AudioSpec want = {0}, have = {0};
    want.freq     = sample_rate;
    want.format   = AUDIO_S16SYS;
    want.channels = 2;
    want.samples  = 1024;
    want.callback = audio_callback;
    want.userdata = ao;

    ao->device = SDL_OpenAudioDevice(NULL, 0, &want, &have, 0);
    if (ao->device == 0) {
        fprintf(stderr, "SDL_OpenAudioDevice: %s\n", SDL_GetError());
        return -1;
    }
    SDL_PauseAudioDevice(ao->device, 0);
    return 0;
}

void audio_close(audio_out_t *ao) {
    if (ao->device) SDL_CloseAudioDevice(ao->device);
    if (ao->cur)    audio_chunk_free(ao->cur);
    memset(ao, 0, sizeof(*ao));
}
```

- [ ] **Step 3: Wire audio in `src/main.c`**

Add after `render_init` and before the event loop:

```c
audio_out_t ao;
if (audio_open(&ao, &pl.audio_q, pl.audio_sample_rate_out) != 0) return 4;
```

Add before `render_shutdown`:

```c
audio_close(&ao);
```

- [ ] **Step 4: Build and run**

```bash
make && ./build/tv.exe
```

Expected: window with video AND audio. Audio may still be slightly out of sync (fixed next task). `q` quits cleanly.

- [ ] **Step 5: Commit**

```bash
git add src/render.h src/render.c src/main.c
git commit -m "feat: SDL audio playback via callback pulling from audio queue"
```

---

## Task 11: A/V sync (audio-master clock)

**Files:**
- Create: `src/sync.h`
- Create: `src/sync.c`
- Modify: `src/main.c` — replace naive 40ms delay with clock-driven pacing

- [ ] **Step 1: Create `src/sync.h`**

```c
#ifndef TV_SYNC_H
#define TV_SYNC_H

#include <stdint.h>

/* The audio-master clock: "what is the audio's current playback time in seconds?".
 * Implemented as samples_played / sample_rate, taken from the audio_out_t state. */
typedef struct {
    const volatile int64_t *samples_played;
    int                     sample_rate;
    double                  first_pts;   /* first audio pts we saw; baseline offset */
    int                     have_first;
} av_clock_t;

void   av_clock_init(av_clock_t *c, const volatile int64_t *samples_played, int sample_rate);
void   av_clock_mark_first_pts(av_clock_t *c, double pts);
double av_clock_now(const av_clock_t *c);

#endif
```

- [ ] **Step 2: Create `src/sync.c`**

```c
#include "sync.h"

void av_clock_init(av_clock_t *c, const volatile int64_t *samples_played, int sample_rate) {
    c->samples_played = samples_played;
    c->sample_rate    = sample_rate;
    c->first_pts      = 0.0;
    c->have_first     = 0;
}

void av_clock_mark_first_pts(av_clock_t *c, double pts) {
    if (c->have_first) return;
    c->first_pts  = pts;
    c->have_first = 1;
}

double av_clock_now(const av_clock_t *c) {
    return c->first_pts + (double)(*c->samples_played) / (double)c->sample_rate;
}
```

- [ ] **Step 3: Replace the naive delay in `src/main.c`**

Add include: `#include "sync.h"`.

Before the event loop, after `audio_open`:

```c
av_clock_t clk;
av_clock_init(&clk, &ao.samples_played, ao.sample_rate);

/* Prime first-pts from the first audio chunk we see arriving on the queue —
 * but peeking a queue isn't supported, so instead consume the first audio chunk,
 * store its pts, and re-queue it. Simpler: set first_pts in the audio callback
 * on its first packet. Simplest still for POC: take first-pts from the first
 * video frame popped and hope audio aligns (usually within 40ms at stream start). */
```

Replace the frame-render block with:

```c
video_frame_t *vf = queue_pop(&pl.video_q);
if (!vf) { running = false; break; }

if (!clk.have_first) av_clock_mark_first_pts(&clk, vf->pts);

/* wait until audio clock reaches video pts, or drop if too late */
const double DROP_THRESHOLD = 0.040;    /* 40 ms */
const double MAX_SLEEP      = 0.100;    /* cap sleep to stay responsive to events */

double now = av_clock_now(&clk);
double diff = vf->pts - now;
if (diff > 0) {
    if (diff > MAX_SLEEP) diff = MAX_SLEEP;
    SDL_Delay((Uint32)(diff * 1000));
}
now = av_clock_now(&clk);
if (now - vf->pts > DROP_THRESHOLD) {
    /* too late, drop */
    video_frame_free(vf);
    continue;
}

video_tex_upload(&tex, r.renderer, vf);
SDL_RenderClear(r.renderer);
SDL_RenderCopy(r.renderer, tex.texture, NULL, NULL);
SDL_RenderPresent(r.renderer);
video_frame_free(vf);
```

Remove the `SDL_Delay(40)` from the previous task.

- [ ] **Step 4: Build and run, verify stability**

```bash
make && ./build/tv.exe
```

Expected: video and audio in sync, no drift. Let it run for ~2 minutes and confirm lip-sync stays aligned.

- [ ] **Step 5: Commit**

```bash
git add src/sync.h src/sync.c src/main.c
git commit -m "feat: audio-master A/V clock with sleep + drop video pacing"
```

---

## Task 12: Channel switching (keys 1/2/3)

**Files:**
- Modify: `src/main.c`

- [ ] **Step 1: Refactor `main.c` so that "player lifecycle" is a function you can call per channel**

Replace `main.c` with:

```c
#include "render.h"
#include "player.h"
#include "npo.h"
#include "sync.h"
#include <SDL2/SDL.h>
#include <curl/curl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    player_t     player;
    audio_out_t  audio;
    video_tex_t  tex;
    av_clock_t   clk;
    char        *url;
    const npo_channel_t *channel;
    int          running;
} playback_t;

static int playback_open(playback_t *pb, render_t *r, const npo_channel_t *ch) {
    memset(pb, 0, sizeof(*pb));
    pb->channel = ch;
    if (npo_resolve_stream(ch, &pb->url) != 0) {
        fprintf(stderr, "resolve failed for %s\n", ch->display);
        return -1;
    }
    if (player_open(&pb->player, pb->url) != 0) return -1;
    if (player_start(&pb->player) != 0)         return -1;
    if (audio_open(&pb->audio, &pb->player.audio_q, pb->player.audio_sample_rate_out) != 0) return -1;
    av_clock_init(&pb->clk, &pb->audio.samples_played, pb->audio.sample_rate);
    video_tex_init(&pb->tex);

    char title[128];
    snprintf(title, sizeof(title), "tv — %s", ch->display);
    SDL_SetWindowTitle(r->window, title);
    return 0;
}

static void playback_close(playback_t *pb) {
    player_stop(&pb->player);
    audio_close(&pb->audio);
    video_tex_destroy(&pb->tex);
    player_close(&pb->player);
    free(pb->url);
    memset(pb, 0, sizeof(*pb));
}

static const npo_channel_t *key_to_channel(SDL_Keycode k) {
    switch (k) {
        case SDLK_1: return &NPO_CHANNELS[0];
        case SDLK_2: return &NPO_CHANNELS[1];
        case SDLK_3: return &NPO_CHANNELS[2];
        default: return NULL;
    }
}

int main(int argc, char **argv) {
    (void)argc; (void)argv;
    curl_global_init(CURL_GLOBAL_DEFAULT);

    render_t r;
    if (render_init(&r, 960, 540, "tv — booting") != 0) return 1;

    playback_t pb;
    if (playback_open(&pb, &r, &NPO_CHANNELS[0]) != 0) return 2;

    int running = 1;
    int paused  = 0;

    while (running) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            if (ev.type == SDL_QUIT) running = 0;
            else if (ev.type == SDL_KEYDOWN) {
                SDL_Keycode k = ev.key.keysym.sym;
                if (k == SDLK_q || k == SDLK_ESCAPE) running = 0;
                else if (k == SDLK_f) render_toggle_fullscreen(&r);
                else if (k == SDLK_SPACE) {
                    paused = !paused;
                    SDL_PauseAudioDevice(pb.audio.device, paused);
                }
                else {
                    const npo_channel_t *nch = key_to_channel(k);
                    if (nch && nch != pb.channel) {
                        playback_close(&pb);
                        if (playback_open(&pb, &r, nch) != 0) { running = 0; break; }
                    }
                }
            }
        }

        video_frame_t *vf = queue_pop(&pb.player.video_q);
        if (!vf) {
            /* decoder ended (EOF or error). Let main loop idle briefly. */
            SDL_Delay(100);
            continue;
        }
        if (!pb.clk.have_first) av_clock_mark_first_pts(&pb.clk, vf->pts);

        double diff = vf->pts - av_clock_now(&pb.clk);
        if (diff > 0) SDL_Delay((Uint32)((diff > 0.1 ? 0.1 : diff) * 1000));
        if (av_clock_now(&pb.clk) - vf->pts > 0.040) {
            video_frame_free(vf);
            continue;
        }

        video_tex_upload(&pb.tex, r.renderer, vf);
        SDL_RenderClear(r.renderer);
        SDL_RenderCopy(r.renderer, pb.tex.texture, NULL, NULL);
        SDL_RenderPresent(r.renderer);
        video_frame_free(vf);
    }

    playback_close(&pb);
    render_shutdown(&r);
    curl_global_cleanup();
    return 0;
}
```

- [ ] **Step 2: Build and run, test switching**

```bash
make && ./build/tv.exe
```

Expected: NPO 1 plays. Press `2` — title changes to "tv — NPO 2", new stream loads, plays. `3` same for NPO 3. `1` back to NPO 1. `space` pauses + resumes audio. Multiple rapid switches don't crash (may flicker/black briefly while resolving).

- [ ] **Step 3: Commit**

```bash
git add src/main.c
git commit -m "feat: channel switching on keys 1/2/3 with title + pause toggle"
```

---

## Task 13: EPG overlay with SDL_ttf and NOS highlighting

**Files:**
- Create: `assets/DejaVuSans.ttf` (download)
- Modify: `src/render.h` — add overlay API
- Modify: `src/render.c` — SDL_ttf rendering
- Modify: `src/main.c` — load EPG, draw overlay on `e` toggle

- [ ] **Step 1: Download DejaVu Sans**

```bash
mkdir -p assets
curl -sSL -o assets/DejaVuSans.ttf \
  https://github.com/dejavu-fonts/dejavu-fonts/raw/version_2_37/ttf/DejaVuSans.ttf
```

- [ ] **Step 2: Add overlay API in `src/render.h`**

```c
#include <SDL2/SDL_ttf.h>
#include "npo.h"

typedef struct {
    TTF_Font    *font_regular;
    TTF_Font    *font_bold;
    SDL_Texture *cached;     /* last-rendered overlay texture */
    int          cached_w;
    int          cached_h;
    int          dirty;
} overlay_t;

int  overlay_init(overlay_t *o, const char *font_path);
void overlay_shutdown(overlay_t *o);
void overlay_mark_dirty(overlay_t *o);
int  overlay_render(overlay_t *o, SDL_Renderer *r,
                    const epg_t *epg, int window_w, int window_h);
```

- [ ] **Step 3: Implement in `src/render.c`**

```c
int overlay_init(overlay_t *o, const char *font_path) {
    memset(o, 0, sizeof(*o));
    if (TTF_Init() != 0) { fprintf(stderr, "TTF_Init: %s\n", TTF_GetError()); return -1; }
    o->font_regular = TTF_OpenFont(font_path, 18);
    o->font_bold    = TTF_OpenFont(font_path, 20);
    if (!o->font_regular || !o->font_bold) {
        fprintf(stderr, "TTF_OpenFont(%s): %s\n", font_path, TTF_GetError());
        return -1;
    }
    TTF_SetFontStyle(o->font_bold, TTF_STYLE_BOLD);
    o->dirty = 1;
    return 0;
}

void overlay_shutdown(overlay_t *o) {
    if (o->cached) SDL_DestroyTexture(o->cached);
    if (o->font_regular) TTF_CloseFont(o->font_regular);
    if (o->font_bold)    TTF_CloseFont(o->font_bold);
    TTF_Quit();
    memset(o, 0, sizeof(*o));
}

void overlay_mark_dirty(overlay_t *o) { o->dirty = 1; }

static SDL_Surface *render_line(TTF_Font *font, const char *text, SDL_Color col) {
    return TTF_RenderUTF8_Blended(font, text, col);
}

static const epg_entry_t *find_current(const epg_t *epg, time_t now) {
    for (size_t i = 0; i < epg->count; ++i)
        if (epg->entries[i].start <= now && now < epg->entries[i].end)
            return &epg->entries[i];
    return NULL;
}

int overlay_render(overlay_t *o, SDL_Renderer *r, const epg_t *epg, int ww, int wh) {
    const int pad = 12, line_h = 26, max_lines = 4, overlay_h = pad * 2 + line_h * max_lines;
    const int overlay_w = ww;

    if (o->dirty || !o->cached || o->cached_w != overlay_w || o->cached_h != overlay_h) {
        if (o->cached) SDL_DestroyTexture(o->cached);
        o->cached = SDL_CreateTexture(r, SDL_PIXELFORMAT_RGBA8888,
                                      SDL_TEXTUREACCESS_TARGET, overlay_w, overlay_h);
        if (!o->cached) return -1;
        SDL_SetTextureBlendMode(o->cached, SDL_BLENDMODE_BLEND);

        SDL_SetRenderTarget(r, o->cached);
        SDL_SetRenderDrawBlendMode(r, SDL_BLENDMODE_BLEND);
        SDL_SetRenderDrawColor(r, 0, 0, 0, 180);
        SDL_RenderClear(r);

        time_t now = time(NULL);
        const epg_entry_t *cur = find_current(epg, now);

        SDL_Color white = { 235, 235, 235, 255 };
        SDL_Color red   = { 255,  90,  90, 255 };

        int y = pad;
        char buf[256];
        /* line 0: "Nu: <title> (HH:MM - HH:MM)" */
        if (cur) {
            struct tm lt_s = *localtime(&cur->start);
            struct tm lt_e = *localtime(&cur->end);
            snprintf(buf, sizeof(buf), "Nu: %s  (%02d:%02d - %02d:%02d)",
                     cur->title, lt_s.tm_hour, lt_s.tm_min, lt_e.tm_hour, lt_e.tm_min);
        } else {
            snprintf(buf, sizeof(buf), "Nu: (geen programma gevonden in EPG)");
        }
        SDL_Surface *surf = render_line(o->font_bold, buf,
                                        (cur && cur->is_news) ? red : white);
        if (surf) {
            SDL_Texture *tx = SDL_CreateTextureFromSurface(r, surf);
            SDL_Rect dst = { pad, y, surf->w, surf->h };
            SDL_RenderCopy(r, tx, NULL, &dst);
            SDL_DestroyTexture(tx);
            SDL_FreeSurface(surf);
        }

        /* next N upcoming */
        int shown = 0;
        size_t start_idx = 0;
        for (size_t i = 0; i < epg->count; ++i) {
            if (epg->entries[i].start > now) { start_idx = i; break; }
        }
        for (size_t i = start_idx; i < epg->count && shown < max_lines - 1; ++i) {
            y += line_h;
            const epg_entry_t *e = &epg->entries[i];
            struct tm lt_s = *localtime(&e->start);
            snprintf(buf, sizeof(buf), "   %02d:%02d  %s",
                     lt_s.tm_hour, lt_s.tm_min, e->title);
            SDL_Surface *s2 = render_line(o->font_regular, buf, e->is_news ? red : white);
            if (s2) {
                SDL_Texture *tx2 = SDL_CreateTextureFromSurface(r, s2);
                SDL_Rect dst = { pad, y, s2->w, s2->h };
                SDL_RenderCopy(r, tx2, NULL, &dst);
                SDL_DestroyTexture(tx2);
                SDL_FreeSurface(s2);
            }
            shown++;
        }

        SDL_SetRenderTarget(r, NULL);
        o->cached_w = overlay_w;
        o->cached_h = overlay_h;
        o->dirty = 0;
    }

    SDL_Rect dst = { 0, wh - o->cached_h, o->cached_w, o->cached_h };
    SDL_RenderCopy(r, o->cached, NULL, &dst);
    return 0;
}
```

- [ ] **Step 4: Wire overlay and EPG into `main.c`**

Add includes:
```c
#include <time.h>
```

Extend `playback_t`:
```c
typedef struct {
    player_t     player;
    audio_out_t  audio;
    video_tex_t  tex;
    av_clock_t   clk;
    epg_t        epg;
    char        *url;
    const npo_channel_t *channel;
} playback_t;
```

In `playback_open`, after `player_open`:
```c
if (npo_fetch_epg(ch, &pb->epg) != 0) {
    fprintf(stderr, "warn: EPG fetch failed for %s\n", ch->display);
}
```

In `playback_close`:
```c
npo_epg_free(&pb->epg);
```

Before the event loop in `main`:
```c
overlay_t ov;
if (overlay_init(&ov, "assets/DejaVuSans.ttf") != 0) return 5;
int show_overlay = 1;
```

In the key handler, add:
```c
else if (k == SDLK_e) show_overlay = !show_overlay;
```

Also call `overlay_mark_dirty(&ov)` right after a channel switch.

Before `SDL_RenderPresent`, add:
```c
if (show_overlay) {
    int ww, wh;
    SDL_GetRendererOutputSize(r.renderer, &ww, &wh);
    overlay_render(&ov, r.renderer, &pb.epg, ww, wh);
}
```

Before `render_shutdown`:
```c
overlay_shutdown(&ov);
```

- [ ] **Step 5: Build and run**

```bash
make && ./build/tv.exe
```

Expected: video plays; semi-transparent strip at the bottom shows "Nu: <programma> (start-end)" + next 3 upcoming programmes. NOS Journaal entries are rendered in red. `e` toggles overlay. Switching channels reloads EPG.

- [ ] **Step 6: Commit**

```bash
git add assets/DejaVuSans.ttf src/render.h src/render.c src/main.c
git commit -m "feat: SDL_ttf EPG overlay with NOS Journaal highlighting"
```

---

## Task 14: CLI flags + polish

**Files:**
- Modify: `src/main.c`
- Modify: `README.md` if anything changed materially

- [ ] **Step 1: Add CLI parsing at the start of `main`**

Add after `curl_global_init`:

```c
    const char *start_code     = "NED1";
    const char *stream_override = NULL;
    const char *font_path      = "assets/DejaVuSans.ttf";

    for (int i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--help") == 0) {
            puts("tv: NPO live player\n"
                 "  --channel NED1|NED2|NED3\n"
                 "  --stream-url <url>     bypass NPO API, open this HLS directly\n"
                 "  --font <path>          override overlay font\n");
            return 0;
        }
        if (strcmp(argv[i], "--channel") == 0 && i + 1 < argc) start_code = argv[++i];
        else if (strcmp(argv[i], "--stream-url") == 0 && i + 1 < argc) stream_override = argv[++i];
        else if (strcmp(argv[i], "--font") == 0 && i + 1 < argc) font_path = argv[++i];
        else { fprintf(stderr, "unknown arg: %s (try --help)\n", argv[i]); return 2; }
    }

    const npo_channel_t *start_ch = &NPO_CHANNELS[0];
    for (size_t i = 0; i < NPO_CHANNELS_COUNT; ++i)
        if (strcmp(NPO_CHANNELS[i].code, start_code) == 0) start_ch = &NPO_CHANNELS[i];
```

Modify `playback_open` to accept a stream-override URL (signature change):

```c
static int playback_open(playback_t *pb, render_t *r, const npo_channel_t *ch,
                         const char *stream_override);
```

In its body, replace the `npo_resolve_stream` call with:

```c
    if (stream_override) {
        pb->url = strdup(stream_override);
    } else if (npo_resolve_stream(ch, &pb->url) != 0) {
        fprintf(stderr, "resolve failed for %s (use --stream-url to override)\n", ch->display);
        return -1;
    }
```

Update all callers and change `overlay_init` call to use `font_path`:

```c
if (overlay_init(&ov, font_path) != 0) return 5;
```

Note: `stream_override` applies only on the initial channel. Channel switching always uses the resolver.

- [ ] **Step 2: Print the channel list at startup**

Right after the CLI parse, add:

```c
    puts("Available channels:");
    for (size_t i = 0; i < NPO_CHANNELS_COUNT; ++i) {
        const npo_channel_t *c = &NPO_CHANNELS[i];
        printf("  [%zu] %-8s code=%-6s product=%s\n",
               i + 1, c->display, c->code, c->product_id);
    }
    puts("");
```

- [ ] **Step 3: Build and run — full acceptance run**

```bash
make && ./build/tv.exe
```

Verify, in order:
1. Channel list prints at startup.
2. Window opens, NPO 1 plays video + audio.
3. EPG overlay shows at the bottom. NOS Journaal is in red.
4. Press `e` — overlay hides. Press again — shows.
5. Press `2` — switches to NPO 2, new EPG loads.
6. Press `3` — switches to NPO 3.
7. Press `1` — back to NPO 1.
8. Press `f` — fullscreen. Press again — windowed.
9. Press `space` — audio pauses (SDL_PauseAudio). Press again — resumes.
10. Press `q` — quits cleanly, no crash on teardown.

Run with a fake URL to test the override path:
```bash
./build/tv.exe --stream-url "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8"
```
Expected: plays the Mux test stream. EPG calls will still hit NPO but the overlay can cope with an empty EPG by printing "(geen programma gevonden in EPG)".

- [ ] **Step 4: Run the unit test one more time**

```bash
make test
```

Expected: all tests still pass.

- [ ] **Step 5: Update README's CLI section if anything drifted**

Make sure `README.md`'s flags section matches what `--help` prints.

- [ ] **Step 6: Commit**

```bash
git add src/main.c README.md
git commit -m "feat: CLI flags (--channel, --stream-url, --font) + startup channel list"
```

---

## Acceptance checklist

Final POC acceptance (all must be true):

- [ ] `make` builds cleanly with zero warnings at `-Wall -Wextra -Wpedantic`.
- [ ] `make test` passes three EPG-parse tests.
- [ ] `./build/tv.exe` opens a window playing NPO 1 with in-sync audio within ~10 s.
- [ ] EPG overlay (`e`) shows current + next 3 entries; NOS Journaal in red.
- [ ] `1`/`2`/`3` switches between NPO 1/2/3 without crash.
- [ ] `f`, `space`, `q`/Esc behave correctly.
- [ ] `--stream-url` override works with an arbitrary HLS URL.
- [ ] Spec's risks R1–R5 still flagged in `docs/superpowers/specs/...design.md` with any discovered specifics appended.
