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
