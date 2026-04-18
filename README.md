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
