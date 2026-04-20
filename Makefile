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

# Version string + upstream-repo slug, both baked in so the in-app update
# check can compare itself against github.com/<repo>/releases/latest.
# build-dist.sh passes TV_VERSION / TV_REPO via the environment; local
# builds fall back to "dev" + placeholder so the check no-ops.
TV_VERSION ?= dev
TV_REPO    ?= bancker/miroiptv
CFLAGS     += -DTV_VERSION=\"$(TV_VERSION)\" -DTV_UPDATE_REPO=\"$(TV_REPO)\"

SRC      = $(wildcard src/*.c)
OBJ      = $(SRC:src/%.c=build/%.o)
BIN      = build/miroiptv.exe

TEST_SRC = tests/test_npo_parse.c src/npo.c src/queue.c
TEST_BIN = build/test_npo_parse.exe

FAV_TEST_SRC = tests/test_favorites.c src/favorites.c
FAV_TEST_BIN = build/test_favorites.exe

QUEUE_TEST_SRC = tests/test_queue.c src/queue.c
QUEUE_TEST_BIN = build/test_queue.exe

XTREAM_TEST_SRC = tests/test_xtream_parse.c src/xtream.c src/npo.c src/queue.c
XTREAM_TEST_BIN = build/test_xtream_parse.exe

PREFETCH_TEST_SRC = tests/test_hls_prefetch.c src/hls_prefetch.c src/queue.c
PREFETCH_TEST_BIN = build/test_hls_prefetch.exe

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

test: $(TEST_BIN) $(FAV_TEST_BIN) $(QUEUE_TEST_BIN) $(XTREAM_TEST_BIN) $(PREFETCH_TEST_BIN)
	./$(TEST_BIN)
	./$(FAV_TEST_BIN)
	./$(QUEUE_TEST_BIN)
	./$(XTREAM_TEST_BIN)
	./$(PREFETCH_TEST_BIN)

$(TEST_BIN): $(TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(TEST_SRC) -o $@ $(LDLIBS)

$(FAV_TEST_BIN): $(FAV_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(FAV_TEST_SRC) -o $@ $(LDLIBS)

$(QUEUE_TEST_BIN): $(QUEUE_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(QUEUE_TEST_SRC) -o $@ $(LDLIBS)

$(XTREAM_TEST_BIN): $(XTREAM_TEST_SRC) | build
	$(CC) $(CFLAGS) -Isrc $(XTREAM_TEST_SRC) -o $@ $(LDLIBS)

$(PREFETCH_TEST_BIN): $(PREFETCH_TEST_SRC) | build
	$(CC) -Wall -Wextra -Wpedantic -std=c11 -Isrc $(PREFETCH_TEST_SRC) -o $@ -pthread

clean:
	rm -rf build
